//! Окно попапа: создано заранее и спрятано, показ — это позиционирование у курсора и show().

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindow};

/// Прозрачное поле вокруг карточки — место для тени (см. docs/motion.md).
pub const MARGIN: f64 = 64.0;
const CARD_W: f64 = 430.0;
const CARD_H: f64 = 260.0;
/// Высота пилюли тоста — та же, что у пилюли состояния загрузки.
const TOAST_H: f64 = 46.0;
pub const POPUP_W: f64 = CARD_W + MARGIN * 2.0;
pub const POPUP_H: f64 = CARD_H + MARGIN * 2.0;

/// Подтверждение замены текста. `overlay` — попап уже на экране: тост рисуется поверх
/// карточки в том же окне, окно не двигаем и не переразмериваем.
#[derive(Clone, Serialize)]
pub struct Toast {
    pub text: String,
    pub overlay: bool,
}

pub fn show_at_cursor(app: &AppHandle) -> tauri::Result<()> {
    let win = app.get_webview_window("popup").ok_or(tauri::Error::WindowNotFound)?;
    // Перед каждым показом снимаем игнор кликов и запрет активации с прошлого раза.
    win.set_ignore_cursor_events(false)?;
    set_no_activate(&win, false)?;
    // Фронтенд после морфинга ужимает окно под содержимое, перед новым показом возвращаем запас.
    win.set_size(LogicalSize::new(POPUP_W, POPUP_H))?;
    place_at_cursor(app, &win)?;
    win.show()?;
    win.set_focus()?;
    watch_cursor(app.clone(), win);
    Ok(())
}

/// Тост после замены: пилюля у курсора на пару секунд. Фокус не забирает — пользователь
/// продолжает печатать в своём окне, и Ctrl+Z откатывает вставку там же.
/// Таймер и исчезновение держит фронтенд, он же прячет окно.
pub fn show_toast(app: &AppHandle, text: &str) -> tauri::Result<()> {
    let win = app.get_webview_window("popup").ok_or(tauri::Error::WindowNotFound)?;
    let overlay = win.is_visible().unwrap_or(false);
    app.emit_to("popup", "popup:toast", Toast { text: text.to_string(), overlay })?;
    if overlay {
        return Ok(());
    }
    // Фронтенд успевает отрисовать пилюлю до того, как окно появится (как в show_at_cursor).
    std::thread::sleep(Duration::from_millis(30));
    win.set_size(LogicalSize::new(POPUP_W, TOAST_H + MARGIN * 2.0))?;
    place_at_cursor(app, &win)?;
    // Тост ничего не принимает: клики проходят сквозь всё окно, опрос курсора не нужен.
    win.set_ignore_cursor_events(true)?;
    set_no_activate(&win, true)?;
    win.show()?;
    Ok(())
}

/// Ставит окно так, чтобы карточка (окно минус поля) оказалась в `курсор + (12, 16)`,
/// с прижатием к краям рабочей области по прямоугольнику карточки, а не окна.
fn place_at_cursor(app: &AppHandle, win: &WebviewWindow) -> tauri::Result<()> {
    let cur = app.cursor_position()?;
    let scale = app.monitor_from_point(cur.x, cur.y)?.map(|m| m.scale_factor()).unwrap_or(1.0);
    let margin_px = MARGIN * scale;
    let size = win.outer_size()?;
    let (card_w, card_h) = (size.width as f64 - margin_px * 2.0, size.height as f64 - margin_px * 2.0);
    let (mut card_x, mut card_y) = (cur.x + 12.0, cur.y + 16.0);
    if let Some(m) = app.monitor_from_point(cur.x, cur.y)? {
        let wa = m.work_area();
        let (right, bottom) = (
            (wa.position.x + wa.size.width as i32) as f64,
            (wa.position.y + wa.size.height as i32) as f64,
        );
        if card_x + card_w > right {
            card_x = (right - card_w).max(wa.position.x as f64);
        }
        if card_y + card_h > bottom {
            card_y = (cur.y - 16.0 - card_h).max(wa.position.y as f64);
        }
    }
    win.set_position(PhysicalPosition::new((card_x - margin_px) as i32, (card_y - margin_px) as i32))
}

/// WS_EX_NOACTIVATE на время тоста. tauri show() зовёт ShowWindow(SW_SHOW), а тот активирует
/// окно и уводит фокус из чужого приложения; с этим стилем Windows окно не активирует вообще.
#[cfg(windows)]
fn set_no_activate(win: &WebviewWindow, on: bool) -> tauri::Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE};
    // Версии крейта windows у tauri и у нас разные, общий знаменатель — сырой указатель.
    let hwnd = HWND(win.hwnd()?.0);
    let bit = WS_EX_NOACTIVATE.0 as isize;
    unsafe {
        let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let next = if on { cur | bit } else { cur & !bit };
        if next != cur {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn set_no_activate(_win: &WebviewWindow, _on: bool) -> tauri::Result<()> {
    Ok(())
}

pub fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("popup") {
        let _ = w.hide();
    }
}

/// Раз в 40 мс проверяет, попадает ли курсор в прямоугольник карточки (окно минус поля),
/// и включает/выключает игнор кликов по прозрачному полю. Останавливается, когда окно скрыто.
/// ponytail: если на этой сборке Windows set_ignore_cursor_events не срабатывает (tauri#11461),
/// подстраховка на фронтенде — mousedown по полю прячет окно.
fn watch_cursor(app: AppHandle, win: WebviewWindow) {
    std::thread::spawn(move || loop {
        match win.is_visible() {
            Ok(true) => {}
            _ => return,
        }
        if let (Ok(pos), Ok(size), Ok(cur)) = (win.outer_position(), win.outer_size(), app.cursor_position()) {
            let scale = win.scale_factor().unwrap_or(1.0);
            let margin_px = MARGIN * scale;
            let inside = cur.x >= pos.x as f64 + margin_px
                && cur.x <= pos.x as f64 + size.width as f64 - margin_px
                && cur.y >= pos.y as f64 + margin_px
                && cur.y <= pos.y as f64 + size.height as f64 - margin_px;
            let _ = win.set_ignore_cursor_events(!inside);
        }
        std::thread::sleep(Duration::from_millis(40));
    });
}
