//! Окно попапа: создано заранее и спрятано, показ — это позиционирование у курсора и show().

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindow};

/// Прозрачное поле вокруг карточки — место для тени (см. docs/motion.md).
pub const MARGIN: f64 = 64.0;
const COMPACT_MARGIN: f64 = 16.0;
const COMPACT_WORK_AREA_H: f64 = 520.0;
const CARD_W: f64 = 430.0;
const CARD_H: f64 = 260.0;
/// Высота пилюли тоста — та же, что у пилюли состояния загрузки.
const TOAST_H: f64 = 46.0;

/// Подтверждение замены текста. `overlay` — попап уже на экране: тост рисуется поверх
/// карточки в том же окне, окно не двигаем и не переразмериваем.
#[derive(Clone, Serialize)]
pub struct Toast {
    pub text: String,
    pub overlay: bool,
}

pub fn show_at_cursor(app: &AppHandle) -> tauri::Result<()> {
    let win = app
        .get_webview_window("popup")
        .ok_or(tauri::Error::WindowNotFound)?;
    // Перед каждым показом снимаем игнор кликов и запрет активации с прошлого раза.
    win.set_ignore_cursor_events(false)?;
    set_no_activate(&win, false)?;
    // Фронтенд после морфинга ужимает окно под содержимое, перед новым показом возвращаем запас.
    let (width, height) = size_within_cursor_work_area(app, CARD_W, CARD_H)?;
    win.set_size(LogicalSize::new(width, height))?;
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
    let win = app
        .get_webview_window("popup")
        .ok_or(tauri::Error::WindowNotFound)?;
    let overlay = win.is_visible().unwrap_or(false);
    app.emit_to(
        "popup",
        "popup:toast",
        Toast {
            text: text.to_string(),
            overlay,
        },
    )?;
    if overlay {
        return Ok(());
    }
    // Фронтенд успевает отрисовать пилюлю до того, как окно появится (как в show_at_cursor).
    std::thread::sleep(Duration::from_millis(30));
    let (width, height) = size_within_cursor_work_area(app, CARD_W, TOAST_H)?;
    win.set_size(LogicalSize::new(width, height))?;
    place_at_cursor(app, &win)?;
    // Тост ничего не принимает: клики проходят сквозь всё окно, опрос курсора не нужен.
    win.set_ignore_cursor_events(true)?;
    set_no_activate(&win, true)?;
    win.show()?;
    Ok(())
}

/// Возвращает скрытую карточку после неудачной замены, сохраняя уже отрисованный
/// перевод. Окно показывается без активации, чтобы не отнимать только что
/// восстановленный фокус у исходного приложения.
pub fn show_after_replace_error(app: &AppHandle) {
    let Some(win) = app.get_webview_window("popup") else {
        return;
    };
    if win.is_visible().unwrap_or(false) {
        return;
    }
    let _ = win.set_ignore_cursor_events(false);
    let _ = set_no_activate(&win, true);
    let _ = win.show();
    watch_cursor(app.clone(), win);
}

/// Ставит окно так, чтобы карточка (окно минус поля) оказалась в `курсор + (12, 16)`,
/// с прижатием к краям рабочей области по прямоугольнику карточки, а не окна.
fn place_at_cursor(app: &AppHandle, win: &WebviewWindow) -> tauri::Result<()> {
    let cur = app.cursor_position()?;
    let monitor = app.monitor_from_point(cur.x, cur.y)?;
    let scale = monitor.as_ref().map(|m| m.scale_factor()).unwrap_or(1.0);
    let margin_px = monitor.as_ref().map(popup_margin).unwrap_or(MARGIN) * scale;
    let size = win.outer_size()?;
    let (card_w, card_h) = (
        (size.width as f64 - margin_px * 2.0).max(0.0),
        (size.height as f64 - margin_px * 2.0).max(0.0),
    );
    let (mut card_x, mut card_y) = (cur.x + 12.0, cur.y + 16.0);
    if let Some(m) = monitor {
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
        let outer_w = size.width as f64;
        let outer_h = size.height as f64;
        let outer_x = clamp_axis(card_x - margin_px, wa.position.x as f64, right, outer_w);
        let outer_y = clamp_axis(card_y - margin_px, wa.position.y as f64, bottom, outer_h);
        return win.set_position(PhysicalPosition::new(
            outer_x.round() as i32,
            outer_y.round() as i32,
        ));
    }
    win.set_position(PhysicalPosition::new(
        (card_x - margin_px).round() as i32,
        (card_y - margin_px).round() as i32,
    ))
}

fn size_within_cursor_work_area(
    app: &AppHandle,
    card_width: f64,
    card_height: f64,
) -> tauri::Result<(f64, f64)> {
    let cur = app.cursor_position()?;
    let Some(monitor) = app.monitor_from_point(cur.x, cur.y)? else {
        return Ok((card_width + MARGIN * 2.0, card_height + MARGIN * 2.0));
    };
    let scale = monitor.scale_factor();
    let work = monitor.work_area().size;
    let margin = popup_margin(&monitor);
    Ok(fit_logical_size(
        card_width + margin * 2.0,
        card_height + margin * 2.0,
        work.width as f64,
        work.height as f64,
        scale,
    ))
}

fn popup_margin(monitor: &tauri::Monitor) -> f64 {
    let logical_height = monitor.work_area().size.height as f64 / monitor.scale_factor();
    margin_for_work_area(logical_height)
}

fn margin_for_work_area(logical_height: f64) -> f64 {
    if logical_height < COMPACT_WORK_AREA_H {
        COMPACT_MARGIN
    } else {
        MARGIN
    }
}

fn fit_logical_size(
    width: f64,
    height: f64,
    work_width_px: f64,
    work_height_px: f64,
    scale: f64,
) -> (f64, f64) {
    let safe_scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (
        width.min(work_width_px / safe_scale),
        height.min(work_height_px / safe_scale),
    )
}

fn clamp_axis(value: f64, start: f64, end: f64, length: f64) -> f64 {
    value.max(start).min((end - length).max(start))
}

/// WS_EX_NOACTIVATE на время тоста. tauri show() зовёт ShowWindow(SW_SHOW), а тот активирует
/// окно и уводит фокус из чужого приложения; с этим стилем Windows окно не активирует вообще.
#[cfg(windows)]
fn set_no_activate(win: &WebviewWindow, on: bool) -> tauri::Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE,
    };
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
        if let (Ok(pos), Ok(size), Ok(cur)) = (
            win.outer_position(),
            win.outer_size(),
            app.cursor_position(),
        ) {
            let scale = win.scale_factor().unwrap_or(1.0);
            let center_x = pos.x as f64 + size.width as f64 / 2.0;
            let center_y = pos.y as f64 + size.height as f64 / 2.0;
            let margin = app
                .monitor_from_point(center_x, center_y)
                .ok()
                .flatten()
                .as_ref()
                .map(popup_margin)
                .unwrap_or(MARGIN);
            let margin_px = margin * scale;
            let inside = cur.x >= pos.x as f64 + margin_px
                && cur.x <= pos.x as f64 + size.width as f64 - margin_px
                && cur.y >= pos.y as f64 + margin_px
                && cur.y <= pos.y as f64 + size.height as f64 - margin_px;
            let _ = win.set_ignore_cursor_events(!inside);
        }
        std::thread::sleep(Duration::from_millis(40));
    });
}

#[cfg(test)]
mod tests {
    use super::{clamp_axis, fit_logical_size, margin_for_work_area};

    #[test]
    fn logical_size_is_limited_by_physical_work_area_at_monitor_dpi() {
        assert_eq!(
            fit_logical_size(558.0, 500.0, 900.0, 420.0, 1.5),
            (558.0, 280.0)
        );
    }

    #[test]
    fn clamp_axis_handles_negative_monitor_origins() {
        assert_eq!(clamp_axis(-900.0, -1400.0, -500.0, 600.0), -1100.0);
        assert_eq!(clamp_axis(-1600.0, -1400.0, -500.0, 600.0), -1400.0);
    }

    #[test]
    fn compact_work_areas_spend_less_height_on_shadow_space() {
        assert_eq!(margin_for_work_area(280.0), 16.0);
        assert_eq!(margin_for_work_area(700.0), 64.0);
    }
}
