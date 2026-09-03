//! Окно попапа: создано заранее и спрятано, показ — это позиционирование у курсора и show().

use std::time::Duration;
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewWindow};

/// Прозрачное поле вокруг карточки — место для тени (см. docs/motion.md).
pub const MARGIN: f64 = 64.0;
const CARD_W: f64 = 430.0;
const CARD_H: f64 = 260.0;
pub const POPUP_W: f64 = CARD_W + MARGIN * 2.0;
pub const POPUP_H: f64 = CARD_H + MARGIN * 2.0;

pub fn show_at_cursor(app: &AppHandle) -> tauri::Result<()> {
    let win = app.get_webview_window("popup").ok_or(tauri::Error::WindowNotFound)?;
    // Перед каждым показом снимаем игнор кликов с прошлого раза.
    win.set_ignore_cursor_events(false)?;
    // Фронтенд после морфинга ужимает окно под содержимое, перед новым показом возвращаем запас.
    win.set_size(LogicalSize::new(POPUP_W, POPUP_H))?;
    let cur = app.cursor_position()?;
    let scale = app.monitor_from_point(cur.x, cur.y)?.map(|m| m.scale_factor()).unwrap_or(1.0);
    let margin_px = MARGIN * scale;
    let size = win.outer_size()?;
    let (card_w, card_h) = (size.width as f64 - margin_px * 2.0, size.height as f64 - margin_px * 2.0);
    // Позиционируем карточку, а не окно: курсор + (12, 16) — угол карточки.
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
    win.set_position(PhysicalPosition::new((card_x - margin_px) as i32, (card_y - margin_px) as i32))?;
    win.show()?;
    win.set_focus()?;
    watch_cursor(app.clone(), win);
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
