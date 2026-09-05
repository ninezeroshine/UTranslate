//! Оркестрация перевода области экрана: lifecycle окон, native selector, OCR и перевод.

use std::{
    sync::{Condvar, Mutex},
    time::Duration,
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use super::{
    do_translate, engines, ocr, popup, popup_request_is_current, screen_capture, AppState,
    BusyReset, PopupError, PopupShow,
};
use crate::settings::Settings;

const CAPTURE_ACK_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_SCREEN_TEXT_CHARS: usize = 5_000;

fn validate_recognized_text(text: String) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err("Текст на снимке не найден. Выберите область с текстом ещё раз.".into());
    }
    if text.chars().count() > MAX_SCREEN_TEXT_CHARS {
        return Err(format!(
            "Распознано больше {MAX_SCREEN_TEXT_CHARS} символов. Выберите область поменьше."
        ));
    }
    Ok(text)
}

#[derive(Clone, Copy)]
struct PendingAck {
    request_id: u64,
    acknowledged: bool,
}

pub(super) struct ScreenCaptureAck {
    pending: Mutex<Option<PendingAck>>,
    changed: Condvar,
}

impl Default for ScreenCaptureAck {
    fn default() -> Self {
        Self {
            pending: Mutex::new(None),
            changed: Condvar::new(),
        }
    }
}

impl ScreenCaptureAck {
    fn begin(&self, request_id: u64) {
        *self.pending.lock().unwrap() = Some(PendingAck {
            request_id,
            acknowledged: false,
        });
    }

    pub(super) fn acknowledge(&self, request_id: u64) -> Result<(), String> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "Подтверждение захвата недоступно".to_string())?;
        match pending.as_mut() {
            Some(value) if value.request_id == request_id && !value.acknowledged => {
                value.acknowledged = true;
                self.changed.notify_one();
                Ok(())
            }
            _ => Err("Запрос захвата уже завершён или устарел".into()),
        }
    }

    fn wait(&self, request_id: u64, timeout: Duration) -> bool {
        let Ok(pending) = self.pending.lock() else {
            return false;
        };
        let Ok((mut pending, _)) = self.changed.wait_timeout_while(pending, timeout, |value| {
            matches!(value, Some(value) if value.request_id == request_id && !value.acknowledged)
        }) else {
            return false;
        };
        let acknowledged = matches!(
            pending.as_ref(),
            Some(value) if value.request_id == request_id && value.acknowledged
        );
        if matches!(pending.as_ref(), Some(value) if value.request_id == request_id) {
            *pending = None;
        }
        acknowledged
    }

    fn clear(&self, request_id: u64) {
        if let Ok(mut pending) = self.pending.lock() {
            if matches!(pending.as_ref(), Some(value) if value.request_id == request_id) {
                *pending = None;
            }
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PopupRecognized {
    request_id: u64,
    text: String,
    target: String,
    detected: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PopupCaptureLifecycle {
    request_id: u64,
}

#[derive(Clone, Copy)]
enum OwnWindowFocus {
    Main,
    Popup,
}

#[derive(Clone, Copy)]
struct OwnWindowsSnapshot {
    main_visible: bool,
    popup_visible: bool,
    focused: Option<OwnWindowFocus>,
}

/// Выполняется только с worker thread. Получатель образует настоящий completion barrier:
/// selector не стартует, пока hide и DwmFlush не завершились на UI thread.
fn run_on_main_thread<T, F>(app: &AppHandle, task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = send.send(task());
    })
    .map_err(|error| format!("Не удалось выполнить действие окна: {error}"))?;
    receive
        .recv()
        .map_err(|_| "Поток окна завершился до окончания действия".to_string())?
}

fn prepare_windows(app: &AppHandle, request_id: u64) -> Result<Option<OwnWindowsSnapshot>, String> {
    let scheduler = app.clone();
    let windows = app.clone();
    run_on_main_thread(&scheduler, move || {
        if !popup_request_is_current(request_id) {
            return Ok(None);
        }
        let main = windows.get_webview_window("main");
        let popup = windows.get_webview_window("popup");
        let main_visible = main
            .as_ref()
            .map(|window| window.is_visible().unwrap_or(false))
            .unwrap_or(false);
        let popup_visible = popup
            .as_ref()
            .map(|window| window.is_visible().unwrap_or(false))
            .unwrap_or(false);
        let focused = if main_visible
            && main
                .as_ref()
                .map(|window| window.is_focused().unwrap_or(false))
                .unwrap_or(false)
        {
            Some(OwnWindowFocus::Main)
        } else if popup_visible
            && popup
                .as_ref()
                .map(|window| window.is_focused().unwrap_or(false))
                .unwrap_or(false)
        {
            Some(OwnWindowFocus::Popup)
        } else {
            None
        };
        if popup_visible {
            // Generation may change from another worker while this UI-thread task is queued.
            // Never suspend a popup which already belongs to that newer request.
            if !popup_request_is_current(request_id) {
                return Ok(None);
            }
            windows
                .emit_to(
                    "popup",
                    "popup:capture-suspend",
                    PopupCaptureLifecycle { request_id },
                )
                .map_err(|error| format!("Не удалось подготовить окно перевода: {error}"))?;
        }
        Ok(Some(OwnWindowsSnapshot {
            main_visible,
            popup_visible,
            focused,
        }))
    })
}

fn hide_windows(app: &AppHandle, snapshot: OwnWindowsSnapshot) -> Result<isize, String> {
    let scheduler = app.clone();
    let windows = app.clone();
    run_on_main_thread(&scheduler, move || {
        if snapshot.main_visible {
            if let Some(window) = windows.get_webview_window("main") {
                window
                    .hide()
                    .map_err(|error| format!("Не удалось скрыть главное окно: {error}"))?;
            }
        }
        if snapshot.popup_visible {
            if let Some(window) = windows.get_webview_window("popup") {
                window
                    .hide()
                    .map_err(|error| format!("Не удалось скрыть окно перевода: {error}"))?;
            }
        }
        flush_desktop_composition()?;
        Ok(foreground_window_id())
    })
}

fn resume_popup(app: &AppHandle, request_id: u64) {
    let _ = app.emit_to(
        "popup",
        "popup:capture-resume",
        PopupCaptureLifecycle { request_id },
    );
}

fn restore_windows(
    app: &AppHandle,
    snapshot: OwnWindowsSnapshot,
    foreground_before_selector: isize,
    request_id: u64,
) {
    if snapshot.popup_visible {
        resume_popup(app, request_id);
    }
    let safe_to_focus = should_restore_focus(foreground_before_selector, foreground_window_id());
    let scheduler = app.clone();
    let windows = app.clone();
    let _ = run_on_main_thread(&scheduler, move || {
        let main = windows.get_webview_window("main");
        if snapshot.main_visible {
            if let Some(window) = &main {
                show_window(window, safe_to_focus)?;
            }
        }
        if snapshot.popup_visible {
            popup::restore_after_capture(&windows, safe_to_focus)
                .map_err(|error| error.to_string())?;
        }
        if safe_to_focus {
            match snapshot.focused {
                Some(OwnWindowFocus::Main) => {
                    if let Some(window) = main {
                        let _ = window.set_focus();
                    }
                }
                Some(OwnWindowFocus::Popup) => {}
                None => {}
            }
        }
        Ok(())
    });
}

fn should_restore_focus(before_selector: isize, after_selector: isize) -> bool {
    before_selector != 0 && before_selector == after_selector
}

fn flush_desktop_composition() -> Result<(), String> {
    unsafe { windows::Win32::Graphics::Dwm::DwmFlush() }
        .map_err(|error| format!("Не удалось обновить изображение рабочего стола: {error}"))
}

fn foreground_window_id() -> isize {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize
}

fn show_window(window: &WebviewWindow, activate: bool) -> Result<(), String> {
    if activate {
        return window.show().map_err(|error| error.to_string());
    }
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE},
    };
    let hwnd = HWND(window.hwnd().map_err(|error| error.to_string())?.0);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    Ok(())
}

fn emit_ocr_error(app: &AppHandle, request_id: u64, message: String) {
    if popup_request_is_current(request_id) {
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

pub(super) fn run(app: AppHandle, settings: Settings, request_id: u64) {
    let busy_reset = BusyReset;
    if !popup_request_is_current(request_id) {
        return;
    }
    app.state::<AppState>().screen_capture_ack.begin(request_id);
    let snapshot = match prepare_windows(&app, request_id) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            app.state::<AppState>().screen_capture_ack.clear(request_id);
            return;
        }
        Err(error) => {
            app.state::<AppState>().screen_capture_ack.clear(request_id);
            eprintln!("захват экрана: {error}");
            return;
        }
    };
    if snapshot.popup_visible
        && !app
            .state::<AppState>()
            .screen_capture_ack
            .wait(request_id, CAPTURE_ACK_TIMEOUT)
    {
        resume_popup(&app, request_id);
        eprintln!("захват экрана: окно перевода не подтвердило скрытие");
        return;
    }
    if !snapshot.popup_visible {
        app.state::<AppState>().screen_capture_ack.clear(request_id);
    }
    if !popup_request_is_current(request_id) {
        if snapshot.popup_visible {
            resume_popup(&app, request_id);
        }
        return;
    }
    let foreground_before_selector = match hide_windows(&app, snapshot) {
        Ok(value) => value,
        Err(error) => {
            restore_windows(&app, snapshot, foreground_window_id(), request_id);
            eprintln!("захват экрана: {error}");
            return;
        }
    };
    let captured = screen_capture::select_region();

    let captured = match captured {
        Ok(Some(captured)) => captured,
        Ok(None) => {
            if popup_request_is_current(request_id) {
                restore_windows(&app, snapshot, foreground_before_selector, request_id);
            }
            return;
        }
        Err(error) => {
            if popup_request_is_current(request_id) {
                restore_windows(&app, snapshot, foreground_before_selector, request_id);
            }
            eprintln!("захват экрана: {error}");
            return;
        }
    };
    if !popup_request_is_current(request_id) {
        return;
    }
    if let Err(error) = app.emit_to(
        "popup",
        "popup:show",
        PopupShow {
            request_id,
            text: String::new(),
            target: settings.primary_lang.clone(),
            detected: None,
            clipboard_replaced: false,
            can_replace: false,
            origin: "screen",
            phase: "recognizing",
        },
    ) {
        if popup_request_is_current(request_id) {
            restore_windows(&app, snapshot, foreground_before_selector, request_id);
        }
        eprintln!("попап: не удалось подготовить окно распознавания: {error}");
        return;
    }
    std::thread::sleep(Duration::from_millis(30));
    if !popup_request_is_current(request_id) {
        return;
    }
    // Карточка встаёт под выделенной областью, а не поверх распознаваемого текста.
    let anchor = popup::Anchor::region(
        captured.left as f64,
        captured.top as f64,
        captured.height as f64,
    );
    if let Err(error) = popup::show_near(&app, anchor, 0.0, 12.0) {
        if popup_request_is_current(request_id) {
            emit_ocr_error(
                &app,
                request_id,
                format!("Не удалось показать окно распознавания: {error}"),
            );
            restore_windows(&app, snapshot, foreground_before_selector, request_id);
        }
        eprintln!("попап: {error}");
        return;
    }
    // OCR и сеть не запрещают начать следующий native capture; их отсечёт request_id.
    drop(busy_reset);
    let resource_dir = match app.path().resource_dir() {
        Ok(path) => path,
        Err(error) => {
            emit_ocr_error(
                &app,
                request_id,
                format!("Не удалось открыть файлы распознавания: {error}"),
            );
            return;
        }
    };
    let recognized = ocr::recognize_cancellable(
        &resource_dir,
        captured.width,
        captured.height,
        &captured.rgba,
        || popup_request_is_current(request_id),
    );
    drop(captured.rgba);
    if !popup_request_is_current(request_id) {
        return;
    }
    let text = match recognized {
        Ok(text) => match validate_recognized_text(text) {
            Ok(text) => text,
            Err(error) => {
                emit_ocr_error(&app, request_id, error);
                return;
            }
        },
        Err(error) => {
            emit_ocr_error(
                &app,
                request_id,
                format!("Не удалось распознать текст: {error}"),
            );
            return;
        }
    };
    let hint = engines::guess_lang(&text);
    let target = engines::pick_target(hint, &settings.primary_lang, &settings.secondary_lang);
    if !popup_request_is_current(request_id) {
        return;
    }
    let _ = app.emit_to(
        "popup",
        "popup:recognized",
        PopupRecognized {
            request_id,
            text: text.clone(),
            target: target.clone(),
            detected: hint.map(String::from),
        },
    );
    if !popup_request_is_current(request_id) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if !popup_request_is_current(request_id) {
            return;
        }
        match do_translate(&app, &text, Some(target), None, "screen").await {
            Ok(mut result) if popup_request_is_current(request_id) => {
                result.request_id = Some(request_id);
                let _ = app.emit_to("popup", "popup:result", result);
            }
            Err(message) if popup_request_is_current(request_id) => {
                emit_ocr_error(&app, request_id, message);
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        should_restore_focus, validate_recognized_text, ScreenCaptureAck, MAX_SCREEN_TEXT_CHARS,
    };
    use std::time::Duration;

    #[test]
    fn ack_is_current_and_one_shot() {
        let ack = ScreenCaptureAck::default();
        ack.begin(11);
        assert!(ack.acknowledge(10).is_err());
        ack.acknowledge(11).unwrap();
        assert!(ack.acknowledge(11).is_err());
        assert!(ack.wait(11, Duration::from_millis(1)));
        assert!(!ack.wait(11, Duration::from_millis(1)));
    }

    #[test]
    fn recognized_text_must_fit_the_translation_ui_limit() {
        assert!(validate_recognized_text(" \n ".into()).is_err());
        assert!(validate_recognized_text("я".repeat(MAX_SCREEN_TEXT_CHARS)).is_ok());
        let error = validate_recognized_text("я".repeat(MAX_SCREEN_TEXT_CHARS + 1)).unwrap_err();
        assert!(error.contains("поменьше"));
    }

    #[test]
    fn alt_tab_prevents_restoring_focus_to_utranslate() {
        assert!(should_restore_focus(41, 41));
        assert!(!should_restore_focus(41, 52));
        assert!(!should_restore_focus(0, 0));
    }
}
