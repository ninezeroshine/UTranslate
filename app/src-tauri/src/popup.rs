//! Окно попапа: создано заранее и спрятано, показ — это позиционирование у курсора и show().

use std::time::Duration;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewWindow,
};

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

/// Якорь, под которым встаёт карточка, в физических координатах экрана. Ширина якоря на
/// размещение не влияет: карточка выравнивается по его левому краю и уходит вниз или вверх.
/// Для курсорного пути это вырожденный якорь в точке курсора.
#[derive(Clone, Copy)]
pub struct Anchor {
    pub left: f64,
    pub top: f64,
    pub bottom: f64,
}

impl Anchor {
    pub fn point(x: f64, y: f64) -> Self {
        Self {
            left: x,
            top: y,
            bottom: y,
        }
    }

    pub fn region(left: f64, top: f64, height: f64) -> Self {
        Self {
            left,
            top,
            bottom: top + height,
        }
    }
}

/// Курсорный путь: карточка на 12 px правее и 16 px ниже курсора.
pub fn show_at_cursor(app: &AppHandle) -> tauri::Result<()> {
    let cur = app.cursor_position()?;
    show_near(app, Anchor::point(cur.x, cur.y), 12.0, 16.0)
}

/// Показывает карточку под прямоугольником-якорем; если снизу не помещается — над ним.
/// Экранный перевод передаёт сюда выделенную область, чтобы карточка не легла на её текст.
/// Координаты физические и не переводятся через DPI другого монитора.
pub fn show_near(app: &AppHandle, anchor: Anchor, dx: f64, gap: f64) -> tauri::Result<()> {
    let win = app
        .get_webview_window("popup")
        .ok_or(tauri::Error::WindowNotFound)?;
    // Перед каждым показом снимаем игнор кликов и запрет активации с прошлого раза.
    win.set_ignore_cursor_events(false)?;
    set_no_activate(&win, false)?;
    // Фронтенд после морфинга ужимает окно под содержимое, перед новым показом возвращаем запас.
    let (width, height) =
        size_within_point_work_area(app, anchor.left, anchor.bottom, CARD_W, CARD_H)?;
    let scale = app
        .monitor_from_point(anchor.left, anchor.bottom)?
        .map(|monitor| monitor.scale_factor())
        .unwrap_or(1.0);
    let (width, height) = logical_to_physical_size(width, height, scale);
    win.set_size(PhysicalSize::new(width, height))?;
    place_near(app, &win, anchor, dx, gap)?;
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
    let cur = app.cursor_position()?;
    place_near(app, &win, Anchor::point(cur.x, cur.y), 12.0, 16.0)?;
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

/// Возвращает карточку после отмены выбора области. При небезопасном foreground
/// показывает её без активации, но всегда возобновляет обработку прозрачных полей.
pub fn restore_after_capture(app: &AppHandle, activate: bool) -> tauri::Result<()> {
    let win = app
        .get_webview_window("popup")
        .ok_or(tauri::Error::WindowNotFound)?;
    win.set_ignore_cursor_events(false)?;
    set_no_activate(&win, !activate)?;
    win.show()?;
    if activate {
        win.set_focus()?;
    }
    watch_cursor(app.clone(), win);
    Ok(())
}

/// Ставит окно так, чтобы карточка (окно минус поля) встала под якорем в `+dx` по X и `+gap`
/// по Y, с прижатием к краям рабочей области по прямоугольнику карточки, а не окна.
fn place_near(
    app: &AppHandle,
    win: &WebviewWindow,
    anchor: Anchor,
    dx: f64,
    gap: f64,
) -> tauri::Result<()> {
    let monitor = app.monitor_from_point(anchor.left, anchor.bottom)?;
    let scale = monitor.as_ref().map(|m| m.scale_factor()).unwrap_or(1.0);
    let margin_px = monitor.as_ref().map(popup_margin).unwrap_or(MARGIN) * scale;
    let size = win.outer_size()?;
    let (card_w, card_h) = (
        (size.width as f64 - margin_px * 2.0).max(0.0),
        (size.height as f64 - margin_px * 2.0).max(0.0),
    );
    let (mut card_x, mut card_y) = (anchor.left + dx, anchor.bottom + gap);
    if let Some(m) = monitor {
        let wa = m.work_area();
        let (left, top) = (wa.position.x as f64, wa.position.y as f64);
        let (right, bottom) = (
            (wa.position.x + wa.size.width as i32) as f64,
            (wa.position.y + wa.size.height as i32) as f64,
        );
        card_x = clamp_axis(card_x, left, right, card_w);
        card_y = card_y_near_anchor(anchor.top, anchor.bottom, gap, card_h, top, bottom);
        let outer_w = size.width as f64;
        let outer_h = size.height as f64;
        let outer_x = clamp_axis(card_x - margin_px, left, right, outer_w);
        let outer_y = clamp_axis(card_y - margin_px, top, bottom, outer_h);
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

/// Карточка идёт под якорь; если снизу рабочей области не помещается — переворачивается наверх.
fn card_y_near_anchor(
    anchor_top: f64,
    anchor_bottom: f64,
    gap: f64,
    card_h: f64,
    work_top: f64,
    work_bottom: f64,
) -> f64 {
    let below = anchor_bottom + gap;
    if below + card_h <= work_bottom {
        below
    } else {
        (anchor_top - gap - card_h).max(work_top)
    }
}

fn size_within_cursor_work_area(
    app: &AppHandle,
    card_width: f64,
    card_height: f64,
) -> tauri::Result<(f64, f64)> {
    let cur = app.cursor_position()?;
    size_within_point_work_area(app, cur.x, cur.y, card_width, card_height)
}

fn size_within_point_work_area(
    app: &AppHandle,
    x: f64,
    y: f64,
    card_width: f64,
    card_height: f64,
) -> tauri::Result<(f64, f64)> {
    let Some(monitor) = app.monitor_from_point(x, y)? else {
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

fn logical_to_physical_size(width: f64, height: f64, scale: f64) -> (u32, u32) {
    let safe_scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (
        (width * safe_scale).round().max(1.0) as u32,
        (height * safe_scale).round().max(1.0) as u32,
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
    use super::{
        card_y_near_anchor, clamp_axis, fit_logical_size, logical_to_physical_size,
        margin_for_work_area,
    };

    #[test]
    fn card_goes_under_the_anchor_and_flips_above_it_at_the_bottom_edge() {
        // Выделение 300..340 по Y, карточка 260, рабочая область 0..1080 — влезает снизу.
        assert_eq!(
            card_y_near_anchor(300.0, 340.0, 12.0, 260.0, 0.0, 1080.0),
            352.0
        );
        // То же выделение у нижнего края: карточка переворачивается над ним.
        assert_eq!(
            card_y_near_anchor(900.0, 940.0, 12.0, 260.0, 0.0, 1080.0),
            628.0
        );
        // Курсорный путь — вырожденный якорь в точке.
        assert_eq!(
            card_y_near_anchor(500.0, 500.0, 16.0, 260.0, 0.0, 1080.0),
            516.0
        );
        // Карточка выше всей рабочей области прижимается к её верху, а не уезжает за экран.
        assert_eq!(
            card_y_near_anchor(10.0, 300.0, 12.0, 400.0, 0.0, 320.0),
            0.0
        );
    }

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

    #[test]
    fn explicit_point_size_uses_target_monitor_scale() {
        assert_eq!(logical_to_physical_size(558.0, 388.0, 1.5), (837, 582));
        assert_eq!(logical_to_physical_size(558.0, 388.0, 0.0), (558, 388));
    }
}
