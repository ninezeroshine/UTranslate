mod capture;
mod db;
mod engines;
mod popup;
mod settings;

use std::{path::PathBuf, sync::Mutex};

use serde::Serialize;
use tauri::{
    menu::{CheckMenuItemBuilder, MenuBuilder, MenuItem, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};
use tauri_plugin_updater::{Update, UpdaterExt};

use engines::{Engines, Translation};
use settings::Settings;

pub struct AppState {
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    engines: Engines,
    db: db::Db,
    hotkey_status: Mutex<Vec<HotkeyStatus>>,
    /// Найденное обновление: сюда кладёт проверка, отсюда берёт установка — второй раз в сеть не ходим.
    update: Mutex<Option<Update>>,
    /// Пункт трея «Обновить до X.Y.Z»: создаётся выключенным, включается по находке.
    update_item: Mutex<Option<MenuItem<tauri::Wry>>>,
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
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PopupShow {
    text: String,
    target: String,
    detected: Option<String>,
    clipboard_replaced: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateResult {
    #[serde(flatten)]
    translation: Translation,
    history_id: Option<i64>,
    word_mode: bool,
}

#[derive(Clone, Serialize)]
struct PopupError {
    message: String,
}

fn parse_hotkey(s: &str) -> Result<Shortcut, String> {
    // Текст ошибки парсера крейта нечитаем для пользователя — заменяем понятным.
    let sc = s.parse::<Shortcut>().map_err(|_| "Некорректное сочетание клавиш".to_string())?;
    // Хоткей без модификатора перехватил бы обычную букву во всех программах; F-клавиши — исключение.
    let is_fkey = matches!(sc.key, Code::F1 | Code::F2 | Code::F3 | Code::F4 | Code::F5 | Code::F6 | Code::F7 | Code::F8 | Code::F9 | Code::F10 | Code::F11 | Code::F12
        | Code::F13 | Code::F14 | Code::F15 | Code::F16 | Code::F17 | Code::F18 | Code::F19 | Code::F20 | Code::F21 | Code::F22 | Code::F23 | Code::F24);
    if sc.mods.is_empty() && !is_fkey {
        return Err("Нужен Ctrl, Alt, Shift или Win".to_string());
    }
    Ok(sc)
}

/// Пока пользователь записывает новое сочетание, старые хоткеи не должны перехватывать нажатия.
#[tauri::command]
fn hotkeys_suspend(app: AppHandle, state: State<AppState>, suspended: bool) {
    if suspended {
        let _ = app.global_shortcut().unregister_all();
    } else {
        let s = state.settings.lock().unwrap().clone();
        let status = register_hotkeys(&app, &s);
        *state.hotkey_status.lock().unwrap() = status;
    }
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
    }
}

/// Проверяет и регистрирует все три хоткея; каждый получает свой статус, независимо от остальных.
/// Порядок: проверка (парсинг + совпадения между полями), unregister_all, затем регистрация валидных.
fn register_hotkeys(app: &AppHandle, s: &Settings) -> Vec<HotkeyStatus> {
    let gs = app.global_shortcut();
    let fields = [
        ("hotkeyPopup", "Перевести в попап", &s.hotkey_popup),
        ("hotkeyReplace", "Заменить выделенное", &s.hotkey_replace),
        ("hotkeyWindow", "Открыть окно", &s.hotkey_window),
    ];

    let mut checked: Vec<Result<Shortcut, String>> = fields.iter().map(|(_, _, hk)| parse_hotkey(hk)).collect();
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
                Ok(sc) => gs.register(sc).err().map(|_| "Занято другой программой".to_string()),
            };
            HotkeyStatus { field: field.to_string(), error }
        })
        .collect()
}

static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn on_shortcut(app: &AppHandle, sc: &Shortcut, ev: ShortcutEvent) {
    if ev.state() != ShortcutState::Pressed {
        return;
    }
    let s = app.state::<AppState>().settings.lock().unwrap().clone();
    let action = [(&s.hotkey_popup, Action::Popup), (&s.hotkey_replace, Action::Replace), (&s.hotkey_window, Action::Window)]
        .into_iter()
        .find(|(hk, _)| parse_hotkey(hk).map(|p| &p == sc).unwrap_or(false))
        .map(|(_, a)| a);
    if let Some(action) = action {
        // Автоповтор зажатого хоткея не должен запускать второй захват.
        if BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let app = app.clone();
        // Захват спит до 300 мс — не на потоке событий.
        std::thread::spawn(move || {
            run_action(app, action, s);
            BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }
}

fn run_action(app: AppHandle, action: Action, s: Settings) {
    let cap = capture::capture_selection(&app);
    eprintln!("захват: {} символов", cap.text.as_deref().map(|t| t.chars().count()).unwrap_or(0));
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
                let _ = app.emit_to("popup", "popup:show", PopupShow { text: String::new(), target: s.primary_lang, detected: None, clipboard_replaced: false });
                std::thread::sleep(std::time::Duration::from_millis(30));
                if let Err(e) = popup::show_at_cursor(&app) {
                    eprintln!("попап: {e}");
                }
                return;
            };
            let hint = engines::guess_lang(&text);
            let target = engines::pick_target(hint, &s.primary_lang, &s.secondary_lang);
            let _ = app.emit_to(
                "popup",
                "popup:show",
                PopupShow { text: text.clone(), target: target.clone(), detected: hint.map(String::from), clipboard_replaced: cap.clipboard_replaced },
            );
            std::thread::sleep(std::time::Duration::from_millis(30));
            if let Err(e) = popup::show_at_cursor(&app) {
                eprintln!("попап: {e}");
            }
            tauri::async_runtime::spawn(async move {
                match do_translate(&app, &text, Some(target), "popup").await {
                    Ok(r) => { let _ = app.emit_to("popup", "popup:result", r); }
                    Err(message) => { let _ = app.emit_to("popup", "popup:error", PopupError { message }); }
                }
            });
        }
        Action::Replace => {
            let Some(text) = cap.text else {
                let _ = popup::show_at_cursor(&app);
                let _ = app.emit_to("popup", "popup:show", PopupShow { text: String::new(), target: s.primary_lang, detected: None, clipboard_replaced: false });
                return;
            };
            let result = tauri::async_runtime::block_on(do_translate(&app, &text, None, "replace"));
            match result {
                // Пробелы и переносы по краям выделения сохраняем: движки их отбрасывают.
                Ok(r) => {
                    let lead = &text[..text.len() - text.trim_start().len()];
                    let trail = &text[text.trim_end().len()..];
                    capture::paste_text(&app, &format!("{lead}{}{trail}", r.translation.text));
                    // Сработала вставка или нет (страница, PDF, защищённое поле), приложение
                    // не знает — поэтому подтверждаем всегда. paste_text уже вернул буфер обмена,
                    // так что тост показывается после вставки и после восстановления буфера.
                    if let Err(e) = popup::show_toast(&app, &toast_text(&r.translation.text)) {
                        eprintln!("тост: {e}");
                    }
                }
                Err(message) => {
                    // Оригинал не тронут; ошибка показывается тем же попапом.
                    let _ = app.emit_to("popup", "popup:show", PopupShow { text: text.clone(), target: s.primary_lang, detected: None, clipboard_replaced: cap.clipboard_replaced });
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    let _ = popup::show_at_cursor(&app);
                    let _ = app.emit_to("popup", "popup:error", PopupError { message });
                }
            }
        }
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

#[cfg(test)]
mod toast_tests {
    use super::toast_text;

    #[test]
    fn collapses_whitespace_and_cuts_at_40() {
        assert_eq!(toast_text("две\nстроки"), "две строки");
        assert_eq!(toast_text("  край  "), "край");
        // 45 кириллических символов: режем по символам, а не по байтам.
        let long = "я".repeat(45);
        assert_eq!(toast_text(&long), format!("{}…", "я".repeat(40)));
        assert_eq!(toast_text(&"я".repeat(40)).chars().count(), 40);
    }
}

/// Перевод с авто-swap: если движок определил, что текст уже на целевом языке, переводим на запасной.
async fn do_translate(app: &AppHandle, text: &str, target: Option<String>, mode: &str) -> Result<TranslateResult, String> {
    let st = app.state::<AppState>();
    let s = st.settings.lock().unwrap().clone();
    let hint = engines::guess_lang(text);
    let target = target.unwrap_or_else(|| engines::pick_target(hint, &s.primary_lang, &s.secondary_lang));
    let mut t = st.engines.translate_long(text, &target, hint, &s.engines).await?;
    if t.detected == target {
        let other = if target == s.primary_lang { s.secondary_lang.clone() } else { s.primary_lang.clone() };
        t = st.engines.translate_long(text, &other, Some(&target), &s.engines).await?;
    }
    let history_id = if s.history_enabled {
        st.db.add(text, &t.text, &t.detected, &t.target, &t.engine, mode).ok()
    } else {
        None
    };
    Ok(TranslateResult { word_mode: engines::is_word_mode(text), translation: t, history_id })
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
async fn translate_text(app: AppHandle, text: String, target: Option<String>) -> Result<TranslateResult, String> {
    if text.trim().is_empty() {
        return Err("пустой текст".into());
    }
    do_translate(&app, &text, target, "window").await
}

#[tauri::command]
fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_main(app: AppHandle, text: Option<String>) {
    popup::hide(&app);
    show_main(&app);
    if let Some(text) = text {
        let _ = app.emit_to("main", "main:prefill", text);
    }
}

#[tauri::command]
fn history_list(state: State<AppState>, query: Option<String>, favorites_only: Option<bool>) -> Result<Vec<db::Entry>, String> {
    state.db.list(query.as_deref().unwrap_or(""), favorites_only.unwrap_or(false), 500).map_err(|e| e.to_string())
}

#[tauri::command]
fn history_set_favorite(state: State<AppState>, id: i64, favorite: bool) -> Result<(), String> {
    state.db.set_favorite(id, favorite).map_err(|e| e.to_string())
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
fn settings_set(app: AppHandle, state: State<AppState>, settings: Settings) -> Result<Vec<HotkeyStatus>, String> {
    settings::save(&state.settings_path, &settings)?;
    *state.settings.lock().unwrap() = settings.clone();
    // Попап держит свою копию настроек с момента загрузки — тема и размер шрифта доезжают сюда.
    let _ = app.emit_to("popup", "settings:changed", settings.clone());
    let status = register_hotkeys(&app, &settings);
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
    UpdateInfo { version: u.version.clone(), notes: u.body.clone(), date: u.date.map(|d| d.date().to_string()) }
}

/// Текст ошибки апдейтера для пользователя: сеть отличаем от остального.
fn update_error(e: tauri_plugin_updater::Error) -> String {
    let s = e.to_string().to_lowercase();
    if ["error sending request", "dns", "connect", "timed out", "timeout"].iter().any(|k| s.contains(k)) {
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
    let found = app.updater().map_err(update_error)?.check().await.map_err(update_error)?;
    let info = found.as_ref().map(update_info);
    *app.state::<AppState>().update.lock().unwrap() = found;
    if let Some(i) = &info {
        tray_show_update(app, &i.version);
    }
    Ok(info)
}

async fn do_install(app: AppHandle) -> Result<(), String> {
    let update = app.state::<AppState>().update.lock().unwrap().clone();
    let Some(update) = update else { return Err("Обновление не найдено".into()) };
    let win = app.clone();
    let mut downloaded: u64 = 0;
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                // Событие на каждый чанк забивает мост: не чаще 10 раз в секунду, но последний кадр отдаём всегда.
                if last.elapsed() >= std::time::Duration::from_millis(100) || Some(downloaded) == total {
                    last = std::time::Instant::now();
                    let _ = win.emit_to("main", "update:progress", UpdateProgress { downloaded, total });
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
    let enabled = CheckMenuItemBuilder::with_id("enabled", "Хоткеи включены").checked(true).build(app)?;
    // Пункт обновления создаём заранее выключенным: добавить его в меню на лету нельзя.
    let update = MenuItemBuilder::with_id("update", "Обновлений нет").enabled(false).build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Выход").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&open, &enabled, &PredefinedMenuItem::separator(app)?, &update, &quit])
        .build()?;
    app.state::<AppState>().update_item.lock().unwrap().replace(update.clone());
    let enabled_item = enabled.clone();
    TrayIconBuilder::with_id("tray")
        .icon(app.default_window_icon().cloned().expect("иконка"))
        .tooltip("UTranslate")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "enabled" => {
                let on = enabled_item.is_checked().unwrap_or(true);
                if on {
                    let state = app.state::<AppState>();
                    let s = state.settings.lock().unwrap().clone();
                    let status = register_hotkeys(app, &s);
                    *state.hotkey_status.lock().unwrap() = status;
                } else {
                    let _ = app.global_shortcut().unregister_all();
                }
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
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| show_main(app)))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(on_shortcut).build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            translate_text,
            copy_text,
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
            let status = register_hotkeys(app.handle(), &settings);
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
                update: Mutex::new(None),
                update_item: Mutex::new(None),
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
                        Ok(Some(info)) => { let _ = handle.emit_to("main", "update:available", info); }
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

