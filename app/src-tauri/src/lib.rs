mod capture;
mod db;
mod engines;
mod ocr;
mod popup;
mod screen_capture;
mod screen_translation;
mod settings;

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
};

use serde::Serialize;
use tauri::{
    menu::{CheckMenuItemBuilder, MenuBuilder, MenuItem, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WebviewWindow, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{
    Code, GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState,
};
use tauri_plugin_updater::{Update, UpdaterExt};

use engines::{Engines, Translation};
use settings::Settings;

pub struct AppState {
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    engines: Engines,
    db: db::Db,
    hotkey_status: Mutex<Vec<HotkeyStatus>>,
    /// Пользовательский флаг из tray и временная пауза во время записи сочетания — разные состояния.
    hotkeys_enabled: AtomicBool,
    hotkeys_suspended: AtomicBool,
    /// Единственный capture, которому текущий popup вправе заменить исходное выделение.
    popup_capture: Mutex<PopupCaptureStore<capture::InputContext>>,
    /// Найденное обновление: сюда кладёт проверка, отсюда берёт установка — второй раз в сеть не ходим.
    update: Mutex<Option<Update>>,
    /// Пункт трея «Обновить до X.Y.Z»: создаётся выключенным, включается по находке.
    update_item: Mutex<Option<MenuItem<tauri::Wry>>>,
    /// Одноразовое подтверждение popup перед скрытием для захвата экрана.
    screen_capture_ack: screen_translation::ScreenCaptureAck,
}

/// Статус регистрации одного хоткея — отдаётся фронтенду после каждого сохранения настроек.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyStatus {
    field: String,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Popup,
    Replace,
    Window,
    Screen,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PopupShow {
    request_id: u64,
    text: String,
    target: String,
    detected: Option<String>,
    clipboard_replaced: bool,
    can_replace: bool,
    origin: &'static str,
    phase: &'static str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateResult {
    #[serde(flatten)]
    translation: Translation,
    history_id: Option<i64>,
    word_mode: bool,
    is_favorite: bool,
    request_id: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PopupError {
    request_id: u64,
    message: String,
}

fn parse_hotkey(s: &str) -> Result<Shortcut, String> {
    // Текст ошибки парсера крейта нечитаем для пользователя — заменяем понятным.
    let sc = s
        .parse::<Shortcut>()
        .map_err(|_| "Некорректное сочетание клавиш".to_string())?;
    // Хоткей без модификатора перехватил бы обычную букву во всех программах; F-клавиши — исключение.
    let is_fkey = matches!(
        sc.key,
        Code::F1
            | Code::F2
            | Code::F3
            | Code::F4
            | Code::F5
            | Code::F6
            | Code::F7
            | Code::F8
            | Code::F9
            | Code::F10
            | Code::F11
            | Code::F12
            | Code::F13
            | Code::F14
            | Code::F15
            | Code::F16
            | Code::F17
            | Code::F18
            | Code::F19
            | Code::F20
            | Code::F21
            | Code::F22
            | Code::F23
            | Code::F24
    );
    if sc.mods.is_empty() && !is_fkey {
        return Err("Нужен Ctrl, Alt, Shift или Win".to_string());
    }
    Ok(sc)
}

fn hotkeys_should_be_active(enabled: bool, suspended: bool) -> bool {
    enabled && !suspended
}

/// Пока пользователь записывает новое сочетание, старые хоткеи не должны перехватывать нажатия.
#[tauri::command]
fn hotkeys_suspend(app: AppHandle, state: State<AppState>, suspended: bool) {
    state.hotkeys_suspended.store(suspended, Ordering::SeqCst);
    let active = hotkeys_should_be_active(state.hotkeys_enabled.load(Ordering::SeqCst), suspended);
    let s = state.settings.lock().unwrap().clone();
    let status = register_hotkeys(&app, &s, active);
    *state.hotkey_status.lock().unwrap() = status;
}

#[cfg(test)]
mod hotkey_tests {
    use super::parse_hotkey;

    #[test]
    fn modifiers_required_except_fkeys() {
        assert!(parse_hotkey("Ctrl+Alt+T").is_ok());
        assert!(parse_hotkey("Super+Shift+Space").is_ok());
        assert!(parse_hotkey("F5").is_ok());
        assert!(parse_hotkey("T").is_err());
        assert!(parse_hotkey("Ctrl+Alt+").is_err());
        assert!(parse_hotkey("").is_err());
        assert!(super::hotkeys_should_be_active(true, false));
        assert!(!super::hotkeys_should_be_active(false, false));
        assert!(!super::hotkeys_should_be_active(true, true));
    }
}

/// Проверяет и регистрирует все хоткеи; каждый получает свой статус, независимо от остальных.
/// Порядок: проверка (парсинг + совпадения между полями), unregister_all, затем регистрация валидных.
fn register_hotkeys(app: &AppHandle, s: &Settings, active: bool) -> Vec<HotkeyStatus> {
    let gs = app.global_shortcut();
    let fields = [
        ("hotkeyPopup", "Перевести в попап", &s.hotkey_popup),
        ("hotkeyReplace", "Заменить выделенное", &s.hotkey_replace),
        ("hotkeyWindow", "Открыть окно", &s.hotkey_window),
        ("hotkeyScreen", "Перевести с экрана", &s.hotkey_screen),
    ];

    let mut checked: Vec<Result<Shortcut, String>> =
        fields.iter().map(|(_, _, hk)| parse_hotkey(hk)).collect();
    for i in 0..checked.len() {
        if checked[i].is_err() {
            continue;
        }
        for j in 0..i {
            if let (Ok(a), Ok(b)) = (&checked[i], &checked[j]) {
                if a == b {
                    checked[i] = Err(format!("Совпадает с «{}»", fields[j].1));
                    break;
                }
            }
        }
    }

    gs.unregister_all().ok();

    checked
        .into_iter()
        .zip(fields.iter())
        .map(|(r, (field, _, _))| {
            let error = match r {
                Err(e) => Some(e),
                Ok(sc) if active => gs
                    .register(sc)
                    .err()
                    .map(|_| "Занято другой программой".to_string()),
                Ok(_) => None,
            };
            HotkeyStatus {
                field: field.to_string(),
                error,
            }
        })
        .collect()
}

static BUSY: AtomicBool = AtomicBool::new(false);
static POPUP_REQUEST: AtomicU64 = AtomicU64::new(0);
const MAX_POPUP_REPLACEMENT_BYTES: usize = 1_000_000;

struct PopupCaptureSession<C> {
    request_id: u64,
    source_text: String,
    context: C,
}

struct PopupCaptureStore<C> {
    current: Option<PopupCaptureSession<C>>,
}

impl<C> Default for PopupCaptureStore<C> {
    fn default() -> Self {
        Self { current: None }
    }
}

impl<C> PopupCaptureStore<C> {
    fn clear(&mut self) {
        self.current = None;
    }

    fn set(&mut self, request_id: u64, source_text: String, context: C) {
        self.current = Some(PopupCaptureSession {
            request_id,
            source_text,
            context,
        });
    }

    fn take_for_replace(
        &mut self,
        current_request_id: u64,
        request_id: u64,
        source_text: &str,
        translated_text: String,
    ) -> Result<(PopupCaptureSession<C>, String), String> {
        if translated_text.is_empty() {
            return Err("Перевод пуст; замена отменена".into());
        }
        if translated_text.len() > MAX_POPUP_REPLACEMENT_BYTES {
            return Err("Перевод слишком большой для безопасной замены".into());
        }
        if request_id != current_request_id {
            return Err("Этот перевод уже устарел; выделите текст заново".into());
        }
        let Some(session) = self.current.as_ref() else {
            return Err(
                "Для этого перевода нет активного выделения или замена уже выполнена".into(),
            );
        };
        if session.request_id != request_id {
            return Err("Этот перевод уже устарел; выделите текст заново".into());
        }
        if session.source_text != source_text {
            return Err("Исходный текст изменился; замена отменена".into());
        }
        Ok((
            self.current.take().expect("popup capture checked above"),
            translated_text,
        ))
    }
}

struct BusyReset;

impl Drop for BusyReset {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

fn next_popup_request() -> u64 {
    POPUP_REQUEST.fetch_add(1, Ordering::SeqCst) + 1
}

fn request_matches(current: u64, candidate: u64) -> bool {
    current == candidate
}

fn popup_request_is_current(request_id: u64) -> bool {
    request_matches(POPUP_REQUEST.load(Ordering::SeqCst), request_id)
}

fn popup_command_origin_is_valid(label: &str, visible: bool) -> bool {
    label == "popup" && visible
}

fn on_shortcut(app: &AppHandle, sc: &Shortcut, ev: ShortcutEvent) {
    if ev.state() != ShortcutState::Pressed {
        return;
    }
    let s = app.state::<AppState>().settings.lock().unwrap().clone();
    let action = [
        (&s.hotkey_popup, Action::Popup),
        (&s.hotkey_replace, Action::Replace),
        (&s.hotkey_window, Action::Window),
        (&s.hotkey_screen, Action::Screen),
    ]
    .into_iter()
    .find(|(hk, _)| parse_hotkey(hk).map(|p| &p == sc).unwrap_or(false))
    .map(|(_, a)| a);
    if let Some(action) = action {
        if let Err(error) = dispatch_action(app, action, s) {
            eprintln!("действие хоткея: {error}");
        }
    }
}

fn dispatch_action(app: &AppHandle, action: Action, settings: Settings) -> Result<(), String> {
    // Автоповтор и параллельные native-capture операции не должны создавать второй selector.
    if BUSY
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("Другая операция захвата уже выполняется".into());
    }
    let request_id = next_popup_request();
    let state = app.state::<AppState>();
    let Ok(mut popup_capture) = state.popup_capture.lock() else {
        BUSY.store(false, Ordering::SeqCst);
        return Err("Состояние окна перевода недоступно".into());
    };
    popup_capture.clear();
    drop(popup_capture);
    let app = app.clone();
    std::thread::spawn(move || {
        if action == Action::Screen {
            screen_translation::run(app, settings, request_id);
        } else {
            let _busy_reset = BusyReset;
            run_action(app, action, settings, request_id);
        }
    });
    Ok(())
}

fn show_popup_error(app: &AppHandle, s: &Settings, request_id: u64, text: String, message: String) {
    let _ = app.emit_to(
        "popup",
        "popup:show",
        PopupShow {
            request_id,
            text,
            target: s.primary_lang.clone(),
            detected: None,
            clipboard_replaced: false,
            can_replace: false,
            origin: "selection",
            phase: "translating",
        },
    );
    std::thread::sleep(std::time::Duration::from_millis(30));
    let _ = popup::show_at_cursor(app);
    let _ = app.emit_to(
        "popup",
        "popup:error",
        PopupError {
            request_id,
            message,
        },
    );
}

fn run_action(app: AppHandle, action: Action, s: Settings, request_id: u64) {
    let cap = match capture::capture_selection(&app) {
        Ok(cap) => cap,
        Err(message) => {
            if action == Action::Window {
                show_main(&app);
            } else {
                show_popup_error(&app, &s, request_id, String::new(), message);
            }
            return;
        }
    };
    eprintln!(
        "захват: {} символов",
        cap.text.as_deref().map(|t| t.chars().count()).unwrap_or(0)
    );
    match action {
        Action::Window => {
            show_main(&app);
            if let Some(text) = cap.text {
                let _ = app.emit_to("main", "main:prefill", text);
            }
        }
        Action::Popup => {
            // Сначала фронтенд узнаёт о показе и сбрасывает состояние на пилюлю, потом окно реально появляется.
            let Some(text) = cap.text else {
                let _ = app.emit_to(
                    "popup",
                    "popup:show",
                    PopupShow {
                        request_id,
                        text: String::new(),
                        target: s.primary_lang,
                        detected: None,
                        clipboard_replaced: false,
                        can_replace: false,
                        origin: "selection",
                        phase: "translating",
                    },
                );
                std::thread::sleep(std::time::Duration::from_millis(30));
                if let Err(e) = popup::show_at_cursor(&app) {
                    eprintln!("попап: {e}");
                }
                return;
            };
            let hint = engines::guess_lang(&text);
            let target = engines::pick_target(hint, &s.primary_lang, &s.secondary_lang);
            app.state::<AppState>().popup_capture.lock().unwrap().set(
                request_id,
                text.clone(),
                cap.context.clone(),
            );
            let _ = app.emit_to(
                "popup",
                "popup:show",
                PopupShow {
                    request_id,
                    text: text.clone(),
                    target: target.clone(),
                    detected: hint.map(String::from),
                    clipboard_replaced: cap.clipboard_replaced,
                    can_replace: true,
                    origin: "selection",
                    phase: "translating",
                },
            );
            std::thread::sleep(std::time::Duration::from_millis(30));
            if let Err(e) = popup::show_at_cursor(&app) {
                eprintln!("попап: {e}");
            }
            tauri::async_runtime::spawn(async move {
                match do_translate(&app, &text, Some(target), None, "popup").await {
                    Ok(mut r) if popup_request_is_current(request_id) => {
                        r.request_id = Some(request_id);
                        let _ = app.emit_to("popup", "popup:result", r);
                    }
                    Err(message) if popup_request_is_current(request_id) => {
                        let _ = app.emit_to(
                            "popup",
                            "popup:error",
                            PopupError {
                                request_id,
                                message,
                            },
                        );
                    }
                    _ => {}
                }
            });
        }
        Action::Replace => {
            let Some(text) = cap.text else {
                let _ = popup::show_at_cursor(&app);
                let _ = app.emit_to(
                    "popup",
                    "popup:show",
                    PopupShow {
                        request_id,
                        text: String::new(),
                        target: s.primary_lang,
                        detected: None,
                        clipboard_replaced: false,
                        can_replace: false,
                        origin: "selection",
                        phase: "translating",
                    },
                );
                return;
            };
            let result =
                tauri::async_runtime::block_on(do_translate(&app, &text, None, None, "replace"));
            match result {
                // Пробелы и переносы по краям выделения сохраняем: движки их отбрасывают.
                Ok(r) => {
                    let lead = &text[..text.len() - text.trim_start().len()];
                    let trail = &text[text.trim_end().len()..];
                    let replacement = format!("{lead}{}{trail}", r.translation.text);
                    match capture::paste_text(&app, &replacement, &text, &cap.context) {
                        Ok(outcome) => {
                            if let Err(e) = popup::show_toast(
                                &app,
                                &paste_toast_text(&r.translation.text, outcome),
                            ) {
                                eprintln!("тост: {e}");
                            }
                        }
                        Err(message) => {
                            show_popup_error(&app, &s, request_id, text.clone(), message);
                        }
                    }
                }
                Err(message) => {
                    // Оригинал не тронут; ошибка показывается тем же попапом.
                    let _ = app.emit_to(
                        "popup",
                        "popup:show",
                        PopupShow {
                            request_id,
                            text: text.clone(),
                            target: s.primary_lang,
                            detected: None,
                            clipboard_replaced: cap.clipboard_replaced,
                            can_replace: false,
                            origin: "selection",
                            phase: "translating",
                        },
                    );
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    let _ = popup::show_at_cursor(&app);
                    let _ = app.emit_to(
                        "popup",
                        "popup:error",
                        PopupError {
                            request_id,
                            message,
                        },
                    );
                }
            }
        }
        Action::Screen => unreachable!("screen action has its own capture flow"),
    }
}

/// Первые 40 символов перевода для тоста: любые пробелы и переносы схлопываются в один пробел.
fn toast_text(s: &str) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut rest = one.chars();
    let head: String = rest.by_ref().take(40).collect();
    if rest.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn paste_toast_text(s: &str, outcome: capture::PasteOutcome) -> String {
    match outcome {
        capture::PasteOutcome::ClipboardRestored => toast_text(s),
        capture::PasteOutcome::ClipboardPreserved => {
            "Вставка отправлена · буфер обмена не восстановлен".into()
        }
    }
}

#[cfg(test)]
mod toast_tests {
    use super::{
        paste_toast_text, popup_command_origin_is_valid, request_matches,
        screen_command_origin_is_valid, toast_text, PopupCaptureStore,
    };

    #[test]
    fn collapses_whitespace_and_cuts_at_40() {
        assert_eq!(toast_text("две\nстроки"), "две строки");
        assert_eq!(toast_text("  край  "), "край");
        // 45 кириллических символов: режем по символам, а не по байтам.
        let long = "я".repeat(45);
        assert_eq!(toast_text(&long), format!("{}…", "я".repeat(40)));
        assert_eq!(toast_text(&"я".repeat(40)).chars().count(), 40);
        assert_eq!(
            paste_toast_text("ignored", crate::capture::PasteOutcome::ClipboardPreserved),
            "Вставка отправлена · буфер обмена не восстановлен"
        );
    }

    #[test]
    fn popup_completion_must_match_latest_request() {
        assert!(request_matches(7, 7));
        assert!(!request_matches(8, 7));
    }

    #[test]
    fn popup_replace_rejects_wrong_or_hidden_command_window() {
        assert!(popup_command_origin_is_valid("popup", true));
        assert!(!popup_command_origin_is_valid("main", true));
        assert!(!popup_command_origin_is_valid("popup", false));
    }

    #[test]
    fn popup_replace_requires_current_generation_and_exact_source() {
        let mut store = PopupCaptureStore::default();
        store.set(7, "source".into(), "private-context");

        assert!(store
            .take_for_replace(8, 7, "source", "displayed".into())
            .is_err());
        assert!(store
            .take_for_replace(7, 7, "other", "displayed".into())
            .is_err());

        let (session, displayed) = store
            .take_for_replace(7, 7, "source", "  displayed exactly\n".into())
            .unwrap();
        assert_eq!(session.context, "private-context");
        assert_eq!(displayed, "  displayed exactly\n");
        assert!(store
            .take_for_replace(7, 7, "source", "second click".into())
            .is_err());
    }

    #[test]
    fn popup_replace_rejects_manual_or_invalid_results_without_consuming_capture() {
        let mut manual: PopupCaptureStore<()> = PopupCaptureStore::default();
        assert!(manual
            .take_for_replace(3, 3, "source", "translation".into())
            .is_err());

        let mut captured = PopupCaptureStore::default();
        captured.set(3, "source".into(), ());
        assert!(captured
            .take_for_replace(3, 3, "source", String::new())
            .is_err());
        assert!(captured
            .take_for_replace(3, 3, "source", "translation".into())
            .is_ok());
    }

    #[test]
    fn screen_capture_commands_accept_only_owned_ui_windows() {
        assert!(screen_command_origin_is_valid("main"));
        assert!(screen_command_origin_is_valid("popup"));
        assert!(!screen_command_origin_is_valid("selector"));
        assert!(!screen_command_origin_is_valid("unknown"));
    }
}

fn translation_order(settings: &Settings, engine: Option<&str>) -> Result<Vec<String>, String> {
    let Some(engine) = engine else {
        return Ok(settings.engines.clone());
    };
    if !settings::is_known_engine(engine) {
        return Err(format!("Неизвестный движок перевода: {engine}"));
    }
    if !settings.engines.iter().any(|enabled| enabled == engine) {
        return Err(format!("Движок перевода отключён в настройках: {engine}"));
    }
    Ok(vec![engine.to_string()])
}

fn should_auto_swap(
    detected: &str,
    target: &str,
    manual_engine: bool,
    explicit_target: bool,
) -> bool {
    detected == target && !(manual_engine && explicit_target)
}

fn validate_translation_edit(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("Перевод пуст; сохранение отменено".into());
    }
    if text.len() > MAX_POPUP_REPLACEMENT_BYTES {
        return Err("Перевод слишком большой для безопасного сохранения".into());
    }
    Ok(())
}

#[cfg(test)]
mod translation_policy_tests {
    use super::{
        should_auto_swap, translation_order, validate_translation_edit, MAX_POPUP_REPLACEMENT_BYTES,
    };
    use crate::settings::Settings;

    #[test]
    fn manual_engine_is_single_and_must_be_known_and_enabled() {
        let mut settings = Settings::default();
        assert_eq!(
            translation_order(&settings, None).unwrap(),
            settings.engines
        );
        assert_eq!(
            translation_order(&settings, Some("bing")).unwrap(),
            vec!["bing".to_string()]
        );

        assert!(translation_order(&settings, Some("deepl"))
            .unwrap_err()
            .contains("Неизвестный"));
        settings.engines.retain(|engine| engine != "bing");
        assert!(translation_order(&settings, Some("bing"))
            .unwrap_err()
            .contains("отключён"));
    }

    #[test]
    fn manual_explicit_target_is_never_auto_swapped() {
        assert!(!should_auto_swap("ru", "ru", true, true));
        assert!(should_auto_swap("ru", "ru", true, false));
        assert!(should_auto_swap("ru", "ru", false, true));
        assert!(!should_auto_swap("en", "ru", true, false));
    }

    #[test]
    fn edited_translation_uses_popup_replacement_size_policy() {
        assert!(validate_translation_edit(" \n\t").is_err());
        assert!(validate_translation_edit("готово").is_ok());
        assert!(validate_translation_edit(&"x".repeat(MAX_POPUP_REPLACEMENT_BYTES + 1)).is_err());
    }
}

/// Перевод с авто-swap: если движок определил, что текст уже на целевом языке, переводим на запасной.
async fn do_translate(
    app: &AppHandle,
    text: &str,
    target: Option<String>,
    engine: Option<String>,
    mode: &str,
) -> Result<TranslateResult, String> {
    let st = app.state::<AppState>();
    let s = st.settings.lock().unwrap().clone();
    let hint = engines::guess_lang(text);
    let explicit_target = target.is_some();
    let manual_engine = engine.is_some();
    let order = translation_order(&s, engine.as_deref())?;
    let target =
        target.unwrap_or_else(|| engines::pick_target(hint, &s.primary_lang, &s.secondary_lang));
    let mut t = st
        .engines
        .translate_long(text, &target, hint, &order)
        .await?;
    if should_auto_swap(&t.detected, &target, manual_engine, explicit_target) {
        let other = if target == s.primary_lang {
            s.secondary_lang.clone()
        } else {
            s.primary_lang.clone()
        };
        t = st
            .engines
            .translate_long(text, &other, Some(&target), &order)
            .await?;
    }
    let (history_id, is_favorite) = if s.history_enabled {
        st.db
            .add(text, &t.text, &t.detected, &t.target, &t.engine, mode)
            .map(|(id, favorite)| (Some(id), favorite))
            .unwrap_or((None, false))
    } else {
        (None, false)
    };
    Ok(TranslateResult {
        word_mode: engines::is_word_mode(text),
        translation: t,
        history_id,
        is_favorite,
        request_id: None,
    })
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

// ---- команды для фронтенда ----

#[tauri::command]
async fn translate_text(
    app: AppHandle,
    text: String,
    target: Option<String>,
    engine: Option<String>,
) -> Result<TranslateResult, String> {
    if text.trim().is_empty() {
        return Err("пустой текст".into());
    }
    do_translate(&app, &text, target, engine, "window").await
}

#[tauri::command]
fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
fn update_translation_text(
    state: State<AppState>,
    history_id: i64,
    source_text: String,
    expected_text: String,
    text: String,
) -> Result<bool, String> {
    validate_translation_edit(&text)?;
    state
        .db
        .update_result_text(history_id, &source_text, &expected_text, &text)
        .map_err(|error| format!("Не удалось сохранить перевод: {error}"))?
        .ok_or_else(|| "Перевод устарел или запись истории не найдена".to_string())
}

#[tauri::command]
async fn replace_popup_translation(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    request_id: u64,
    source_text: String,
    translated_text: String,
) -> Result<(), String> {
    let visible = window.is_visible().unwrap_or(false);
    if !popup_command_origin_is_valid(window.label(), visible) {
        return Err("Замену можно выполнить только из открытого окна перевода".into());
    }
    if BUSY
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("Другая операция уже выполняется".into());
    }
    let busy_reset = BusyReset;

    let (session, replacement) = state
        .popup_capture
        .lock()
        .map_err(|_| "Состояние окна перевода недоступно".to_string())?
        .take_for_replace(
            POPUP_REQUEST.load(Ordering::SeqCst),
            request_id,
            &source_text,
            translated_text,
        )?;

    tauri::async_runtime::spawn_blocking(move || {
        // Keep BUSY set for the whole native operation even if the invoking webview
        // is closed and its async response is dropped while this task is running.
        let _busy_reset = busy_reset;
        let paste_outcome = match capture::paste_text_from_popup(
            &app,
            &window,
            &replacement,
            &session.source_text,
            &session.context,
        ) {
            Ok(outcome) => outcome,
            Err(message) => {
                popup::show_after_replace_error(&app);
                return Err(message);
            }
        };

        if let Err(error) = popup::show_toast(&app, &paste_toast_text(&replacement, paste_outcome))
        {
            eprintln!("тост: {error}");
        }
        Ok(())
    })
    .await
    .map_err(|error| format!("Не удалось завершить замену: {error}"))?
}

#[tauri::command]
fn open_main(app: AppHandle, text: Option<String>) {
    popup::hide(&app);
    show_main(&app);
    if let Some(text) = text {
        let _ = app.emit_to("main", "main:prefill", text);
    }
}

fn screen_command_origin_is_valid(label: &str) -> bool {
    matches!(label, "main" | "popup")
}

#[tauri::command]
fn translate_screen(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    if !screen_command_origin_is_valid(window.label()) {
        return Err("Захват экрана можно запустить только из окна UTranslate".into());
    }
    let settings = app.state::<AppState>().settings.lock().unwrap().clone();
    dispatch_action(&app, Action::Screen, settings)
}

#[tauri::command]
fn ack_screen_capture(
    window: WebviewWindow,
    state: State<AppState>,
    request_id: u64,
) -> Result<(), String> {
    if window.label() != "popup" || !popup_request_is_current(request_id) {
        return Err("Подтверждение захвата устарело".into());
    }
    state.screen_capture_ack.acknowledge(request_id)
}

#[tauri::command]
fn history_list(
    state: State<AppState>,
    query: Option<String>,
    favorites_only: Option<bool>,
) -> Result<Vec<db::Entry>, String> {
    state
        .db
        .list(
            query.as_deref().unwrap_or(""),
            favorites_only.unwrap_or(false),
            500,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn history_set_favorite(state: State<AppState>, id: i64, favorite: bool) -> Result<(), String> {
    state
        .db
        .set_favorite(id, favorite)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn history_delete(state: State<AppState>, id: i64) -> Result<(), String> {
    state.db.delete(id).map_err(|e| e.to_string())
}

/// Пишет избранное в «Загрузки» и возвращает путь к файлу.
#[tauri::command]
fn favorites_export(app: AppHandle, state: State<AppState>) -> Result<String, String> {
    let csv = state.db.favorites_csv().map_err(|e| e.to_string())?;
    let dir = app.path().download_dir().map_err(|e| e.to_string())?;
    let path = dir.join("utranslate-favorites.csv");
    // BOM, чтобы Excel открыл кириллицу как UTF-8.
    std::fs::write(&path, format!("\u{feff}{csv}")).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
fn history_clear(state: State<AppState>) -> Result<(), String> {
    state.db.clear().map_err(|e| e.to_string())
}

#[tauri::command]
fn settings_get(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

/// Сохраняет настройки и перерегистрирует хоткеи. Память и хоткеи всегда соответствуют
/// одному и тому же набору настроек — частичных состояний нет, даже если хоткей занят.
#[tauri::command]
fn settings_set(
    app: AppHandle,
    state: State<AppState>,
    settings: Settings,
) -> Result<Vec<HotkeyStatus>, String> {
    settings::save(&state.settings_path, &settings)?;
    *state.settings.lock().unwrap() = settings.clone();
    // Попап держит свою копию настроек с момента загрузки — тема и размер шрифта доезжают сюда.
    let _ = app.emit_to("popup", "settings:changed", settings.clone());
    let active = hotkeys_should_be_active(
        state.hotkeys_enabled.load(Ordering::SeqCst),
        state.hotkeys_suspended.load(Ordering::SeqCst),
    );
    let status = register_hotkeys(&app, &settings, active);
    *state.hotkey_status.lock().unwrap() = status.clone();
    Ok(status)
}

#[tauri::command]
fn hotkeys_status(state: State<AppState>) -> Vec<HotkeyStatus> {
    state.hotkey_status.lock().unwrap().clone()
}

#[tauri::command]
fn autostart_get(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn autostart_set(app: AppHandle, enabled: bool) -> Result<(), String> {
    let al = app.autolaunch();
    let r = if enabled { al.enable() } else { al.disable() };
    r.map_err(|e| e.to_string())
}

// ---- автообновление ----

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    notes: Option<String>,
    date: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
}

fn update_info(u: &Update) -> UpdateInfo {
    UpdateInfo {
        version: u.version.clone(),
        notes: u.body.clone(),
        date: u.date.map(|d| d.date().to_string()),
    }
}

/// Текст ошибки апдейтера для пользователя: сеть отличаем от остального.
fn update_error(e: tauri_plugin_updater::Error) -> String {
    let s = e.to_string().to_lowercase();
    if [
        "error sending request",
        "dns",
        "connect",
        "timed out",
        "timeout",
    ]
    .iter()
    .any(|k| s.contains(k))
    {
        "Нет соединения".to_string()
    } else {
        format!("Не удалось проверить обновления: {e}")
    }
}

fn tray_show_update(app: &AppHandle, version: &str) {
    if let Some(item) = app.state::<AppState>().update_item.lock().unwrap().as_ref() {
        let _ = item.set_text(format!("Обновить до {version}"));
        let _ = item.set_enabled(true);
    }
}

/// Спрашивает GitHub Releases и запоминает найденное в состоянии.
async fn find_update(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let found = app
        .updater()
        .map_err(update_error)?
        .check()
        .await
        .map_err(update_error)?;
    let info = found.as_ref().map(update_info);
    *app.state::<AppState>().update.lock().unwrap() = found;
    if let Some(i) = &info {
        tray_show_update(app, &i.version);
    }
    Ok(info)
}

async fn do_install(app: AppHandle) -> Result<(), String> {
    let update = app.state::<AppState>().update.lock().unwrap().clone();
    let Some(update) = update else {
        return Err("Обновление не найдено".into());
    };
    let win = app.clone();
    let mut downloaded: u64 = 0;
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                // Событие на каждый чанк забивает мост: не чаще 10 раз в секунду, но последний кадр отдаём всегда.
                if last.elapsed() >= std::time::Duration::from_millis(100)
                    || Some(downloaded) == total
                {
                    last = std::time::Instant::now();
                    let _ = win.emit_to(
                        "main",
                        "update:progress",
                        UpdateProgress { downloaded, total },
                    );
                }
            },
            || {},
        )
        .await
        .map_err(|e| format!("Не удалось установить обновление: {e}"))?;
    // На Windows установщик перезапускает приложение сам и сюда мы не доходим; строка нужна остальным платформам.
    app.restart()
}

#[tauri::command]
async fn update_check(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    find_update(&app).await
}

/// Что нашла фоновая проверка — чтобы UI не ходил в сеть при открытии настроек.
#[tauri::command]
fn update_available(state: State<AppState>) -> Option<UpdateInfo> {
    state.update.lock().unwrap().as_ref().map(update_info)
}

#[tauri::command]
async fn update_install(app: AppHandle) -> Result<(), String> {
    do_install(app).await
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItemBuilder::with_id("open", "Открыть UTranslate").build(app)?;
    let screen = MenuItemBuilder::with_id("screen", "Перевести с экрана").build(app)?;
    let enabled = CheckMenuItemBuilder::with_id("enabled", "Хоткеи включены")
        .checked(true)
        .build(app)?;
    // Пункт обновления создаём заранее выключенным: добавить его в меню на лету нельзя.
    let update = MenuItemBuilder::with_id("update", "Обновлений нет")
        .enabled(false)
        .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Выход").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[
            &open,
            &screen,
            &enabled,
            &PredefinedMenuItem::separator(app)?,
            &update,
            &quit,
        ])
        .build()?;
    app.state::<AppState>()
        .update_item
        .lock()
        .unwrap()
        .replace(update.clone());
    let enabled_item = enabled.clone();
    TrayIconBuilder::with_id("tray")
        .icon(app.default_window_icon().cloned().expect("иконка"))
        .tooltip("UTranslate")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "screen" => {
                let settings = app.state::<AppState>().settings.lock().unwrap().clone();
                if let Err(error) = dispatch_action(app, Action::Screen, settings) {
                    eprintln!("захват экрана: {error}");
                }
            }
            "enabled" => {
                let on = enabled_item.is_checked().unwrap_or(true);
                let state = app.state::<AppState>();
                state.hotkeys_enabled.store(on, Ordering::SeqCst);
                let active =
                    hotkeys_should_be_active(on, state.hotkeys_suspended.load(Ordering::SeqCst));
                let s = state.settings.lock().unwrap().clone();
                let status = register_hotkeys(app, &s, active);
                *state.hotkey_status.lock().unwrap() = status;
            }
            "update" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = do_install(app).await {
                        eprintln!("обновление: {e}");
                    }
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main(app)
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(on_shortcut)
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            translate_text,
            copy_text,
            update_translation_text,
            replace_popup_translation,
            translate_screen,
            ack_screen_capture,
            open_main,
            history_list,
            history_set_favorite,
            history_delete,
            history_clear,
            favorites_export,
            settings_get,
            settings_set,
            hotkeys_status,
            hotkeys_suspend,
            autostart_get,
            autostart_set,
            update_check,
            update_available,
            update_install,
        ])
        .setup(|app| {
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let settings = settings::load(&settings_path);
            let db = db::Db::open(&data_dir.join("utranslate.db"))?;
            let status = register_hotkeys(app.handle(), &settings, true);
            for st in &status {
                if let Some(e) = &st.error {
                    eprintln!("хоткей {}: {e}", st.field);
                }
            }
            app.manage(AppState {
                settings: Mutex::new(settings.clone()),
                settings_path,
                engines: Engines::new(),
                db,
                hotkey_status: Mutex::new(status),
                hotkeys_enabled: AtomicBool::new(true),
                hotkeys_suspended: AtomicBool::new(false),
                popup_capture: Mutex::new(PopupCaptureStore::default()),
                update: Mutex::new(None),
                update_item: Mutex::new(None),
                screen_capture_ack: screen_translation::ScreenCaptureAck::default(),
            });
            build_tray(app.handle())?;
            // Путь в записи автозапуска протухает при переименовании или переносе exe
            // (0.1.1 ставился как app.exe). enable() перезаписывает значение текущим путём.
            // В dev-сборке не трогаем: иначе каждый `pnpm tauri dev` уводит автозапуск на target/debug.
            let al = app.autolaunch();
            if !cfg!(debug_assertions) && al.is_enabled().unwrap_or(false) {
                if let Err(e) = al.enable() {
                    eprintln!("автозапуск: не удалось обновить путь: {e}");
                }
            }
            // Фоновая проверка обновлений: через 15 с после старта, дальше раз в 6 часов.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                loop {
                    match find_update(&handle).await {
                        Ok(Some(info)) => {
                            let _ = handle.emit_to("main", "update:available", info);
                        }
                        Ok(None) => {}
                        // В dev-режиме релиза может не быть — пользователю про это знать незачем.
                        Err(e) => eprintln!("автопроверка обновлений: {e}"),
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
