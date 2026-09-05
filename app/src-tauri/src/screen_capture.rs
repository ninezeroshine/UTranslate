//! Native, in-memory screen region selector for Windows.
//!
//! The caller is responsible for hiding UTranslate and flushing the compositor before calling
//! [`select_region`]. This module captures the virtual desktop before it creates any overlay
//! windows, never touches the clipboard, and returns only the selected crop.

use std::sync::atomic::{AtomicBool, Ordering};

// At the limit, the frozen BGRA desktop and returned RGBA crop can briefly use about 512 MiB.
// Both allocations fail through Result/Win32 errors instead of aborting the process.
const MAX_CAPTURE_PIXELS: u64 = 64 * 1024 * 1024;
const MIN_SELECTION_SIDE: i64 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedRegion {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub left: i32,
    pub top: i32,
    pub anchor_x: i32,
    pub anchor_y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PointI {
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RectI {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl RectI {
    fn from_origin_size(left: i32, top: i32, width: i32, height: i32) -> Result<Self, String> {
        if width <= 0 || height <= 0 {
            return Err("Размер экрана должен быть положительным".to_string());
        }
        let right = left
            .checked_add(width)
            .ok_or_else(|| "Переполнение координаты правой границы экрана".to_string())?;
        let bottom = top
            .checked_add(height)
            .ok_or_else(|| "Переполнение координаты нижней границы экрана".to_string())?;
        Ok(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    fn from_drag(anchor: PointI, current: PointI) -> Self {
        Self {
            left: anchor.x.min(current.x),
            top: anchor.y.min(current.y),
            right: anchor.x.max(current.x),
            bottom: anchor.y.max(current.y),
        }
    }

    fn width_i64(self) -> i64 {
        i64::from(self.right) - i64::from(self.left)
    }

    fn height_i64(self) -> i64 {
        i64::from(self.bottom) - i64::from(self.top)
    }

    fn intersect(self, other: Self) -> Option<Self> {
        let result = Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (result.width_i64() > 0 && result.height_i64() > 0).then_some(result)
    }

    fn contains(self, point: PointI) -> bool {
        point.x >= self.left && point.x < self.right && point.y >= self.top && point.y < self.bottom
    }

    fn is_large_enough(self) -> bool {
        self.width_i64() >= MIN_SELECTION_SIDE && self.height_i64() >= MIN_SELECTION_SIDE
    }
}

fn checked_rgba_len(width: u32, height: u32) -> Result<usize, String> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| "Переполнение площади изображения".to_string())?;
    if pixels == 0 {
        return Err("Пустое изображение".to_string());
    }
    if pixels > MAX_CAPTURE_PIXELS {
        return Err(format!(
            "Изображение слишком велико: {pixels} пикселей (предел {MAX_CAPTURE_PIXELS})"
        ));
    }
    let bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| "Переполнение размера RGBA-изображения".to_string())?;
    usize::try_from(bytes).map_err(|_| "Изображение не помещается в память".to_string())
}

fn validated_selection(
    drag: RectI,
    desktop: RectI,
    monitors: &[RectI],
) -> Result<Option<RectI>, String> {
    if !drag.is_large_enough() {
        return Ok(None);
    }
    let Some(clamped) = drag.intersect(desktop) else {
        return Err("Выделение находится вне рабочего стола".to_string());
    };
    if !monitors
        .iter()
        .any(|monitor| clamped.intersect(*monitor).is_some())
    {
        return Err("Выделение не пересекает подключённые мониторы".to_string());
    }
    let width = u32::try_from(clamped.width_i64())
        .map_err(|_| "Недопустимая ширина выделения".to_string())?;
    let height = u32::try_from(clamped.height_i64())
        .map_err(|_| "Недопустимая высота выделения".to_string())?;
    checked_rgba_len(width, height)?;
    Ok(Some(clamped))
}

fn crop_bgra_top_down(
    pixels: &[u8],
    desktop: RectI,
    selection: RectI,
    anchor: PointI,
) -> Result<CapturedRegion, String> {
    let desktop_width = u32::try_from(desktop.width_i64())
        .map_err(|_| "Недопустимая ширина рабочего стола".to_string())?;
    let desktop_height = u32::try_from(desktop.height_i64())
        .map_err(|_| "Недопустимая высота рабочего стола".to_string())?;
    let expected_source_len = checked_rgba_len(desktop_width, desktop_height)?;
    if pixels.len() < expected_source_len {
        return Err("Буфер снимка экрана короче ожидаемого".to_string());
    }
    let selection = selection
        .intersect(desktop)
        .ok_or_else(|| "Выделение не пересекает снимок экрана".to_string())?;
    let width = u32::try_from(selection.width_i64())
        .map_err(|_| "Недопустимая ширина выделения".to_string())?;
    let height = u32::try_from(selection.height_i64())
        .map_err(|_| "Недопустимая высота выделения".to_string())?;
    let output_len = checked_rgba_len(width, height)?;
    let source_stride = usize::try_from(desktop_width)
        .ok()
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "Переполнение шага строки снимка".to_string())?;
    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "Переполнение длины строки выделения".to_string())?;
    let source_x = usize::try_from(i64::from(selection.left) - i64::from(desktop.left))
        .map_err(|_| "Отрицательная X-координата выделения".to_string())?;
    let source_y = usize::try_from(i64::from(selection.top) - i64::from(desktop.top))
        .map_err(|_| "Отрицательная Y-координата выделения".to_string())?;

    let mut rgba = Vec::new();
    rgba.try_reserve_exact(output_len)
        .map_err(|_| "Недостаточно памяти для выбранной области".to_string())?;
    rgba.resize(output_len, 0);
    for row in 0..usize::try_from(height).unwrap_or(0) {
        let source_offset = source_y
            .checked_add(row)
            .and_then(|value| value.checked_mul(source_stride))
            .and_then(|value| value.checked_add(source_x.checked_mul(4)?))
            .ok_or_else(|| "Переполнение смещения строки снимка".to_string())?;
        let source_end = source_offset
            .checked_add(row_bytes)
            .ok_or_else(|| "Переполнение конца строки снимка".to_string())?;
        let source = pixels
            .get(source_offset..source_end)
            .ok_or_else(|| "Строка выделения выходит за буфер снимка".to_string())?;
        let output_offset = row
            .checked_mul(row_bytes)
            .ok_or_else(|| "Переполнение смещения строки RGBA".to_string())?;
        let output = &mut rgba[output_offset..output_offset + row_bytes];
        for (bgra, rgba_pixel) in source.chunks_exact(4).zip(output.chunks_exact_mut(4)) {
            rgba_pixel[0] = bgra[2];
            rgba_pixel[1] = bgra[1];
            rgba_pixel[2] = bgra[0];
            // Screen-compatible DIBs do not guarantee a meaningful alpha byte.
            rgba_pixel[3] = 255;
        }
    }

    Ok(CapturedRegion {
        width,
        height,
        rgba,
        left: selection.left,
        top: selection.top,
        anchor_x: anchor.x,
        anchor_y: anchor.y,
    })
}

static SELECTOR_ACTIVE: AtomicBool = AtomicBool::new(false);

struct ActiveSelectorGuard;

impl ActiveSelectorGuard {
    fn acquire() -> Result<Self, String> {
        if SELECTOR_ACTIVE.swap(true, Ordering::AcqRel) {
            Err("Выбор области уже открыт".to_string())
        } else {
            Ok(Self)
        }
    }
}

impl Drop for ActiveSelectorGuard {
    fn drop(&mut self) {
        SELECTOR_ACTIVE.store(false, Ordering::Release);
    }
}

/// Captures the desktop and blocks until the user accepts or cancels a native selector.
///
/// On Windows the complete capture and window message pump live on a dedicated worker thread.
/// `Ok(None)` means Escape, right click, focus/capture loss, or a display/DPI change cancelled
/// the selection. Concurrent selectors are rejected.
#[cfg(windows)]
pub fn select_region() -> Result<Option<CapturedRegion>, String> {
    let _active = ActiveSelectorGuard::acquire()?;
    std::thread::Builder::new()
        .name("utranslate-screen-selector".to_string())
        .spawn(platform::run_selector)
        .map_err(|error| format!("Не удалось запустить поток выбора области: {error}"))?
        .join()
        .map_err(|_| "Поток выбора области аварийно завершился".to_string())?
}

#[cfg(not(windows))]
pub fn select_region() -> Result<Option<CapturedRegion>, String> {
    Err("Выбор области экрана поддерживается только в Windows".to_string())
}

#[cfg(windows)]
mod platform {
    use super::{crop_bgra_top_down, validated_selection, CapturedRegion, PointI, RectI};
    use std::{
        ffi::c_void,
        mem::{size_of, zeroed},
        panic::{catch_unwind, AssertUnwindSafe},
        ptr::null_mut,
        slice,
    };
    use windows::{
        core::{w, Error as WinError, BOOL, PCWSTR},
        Win32::{
            Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
            Graphics::Gdi::{
                AlphaBlend, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
                CreateDIBSection, CreatePen, DeleteDC, DeleteObject, DrawTextW, EndPaint,
                EnumDisplayMonitors, GetDC, GetStockObject, InvalidateRect, LineTo, MoveToEx,
                PatBlt, ReleaseDC, SelectObject, SetBkMode, SetTextColor, UpdateWindow,
                AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLACKNESS, BLENDFUNCTION,
                CAPTUREBLT, DEFAULT_GUI_FONT, DIB_RGB_COLORS, DT_CENTER, DT_SINGLELINE, DT_TOP,
                HBITMAP, HDC, HGDIOBJ, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
            },
            System::LibraryLoader::GetModuleHandleW,
            UI::{
                HiDpi::{
                    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                },
                Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE},
                WindowsAndMessaging::{
                    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
                    GetMessageW, GetSystemMetrics, GetWindowLongPtrW, LoadCursorW, PostQuitMessage,
                    RegisterClassExW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,
                    ShowWindow, TranslateMessage, UnregisterClassW, CREATESTRUCTW, CS_HREDRAW,
                    CS_VREDRAW, GWLP_USERDATA, HWND_TOPMOST, IDC_CROSS, MSG, SM_CXVIRTUALSCREEN,
                    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE,
                    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WM_ACTIVATE,
                    WM_CAPTURECHANGED, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED,
                    WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP,
                    WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_RBUTTONDOWN, WNDCLASSEXW,
                    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
                },
            },
        },
    };

    const CLASS_NAME: PCWSTR = w!("UTranslate.NativeScreenSelector");
    const HINT: &str = "Выделите область с текстом · Esc — отмена";
    const ACCENT: COLORREF = COLORREF(0x00_c7_d6_5e);
    const WHITE: COLORREF = COLORREF(0x00_ff_ff_ff);
    const SHADOW: COLORREF = COLORREF(0x00_20_20_20);

    #[derive(Debug, Clone, Copy)]
    enum Outcome {
        Accepted { rect: RectI, anchor: PointI },
        Cancelled,
    }

    struct SelectorState {
        desktop: RectI,
        monitors: Vec<RectI>,
        snapshot: Snapshot,
        windows: Vec<HWND>,
        anchor: Option<PointI>,
        cursor: Option<PointI>,
        dragging: bool,
        outcome: Option<Outcome>,
        callback_panicked: bool,
    }

    struct WindowData {
        state: *mut SelectorState,
        monitor: RectI,
    }

    /// Keeps HWND userdata valid while windows are destroyed, including on early returns.
    struct OverlayCleanup {
        state: *mut SelectorState,
    }

    impl OverlayCleanup {
        unsafe fn destroy_now(&mut self) {
            let Some(state) = self.state.as_mut() else {
                return;
            };
            // Clear first so re-entrant destruction messages cannot observe stale HWNDs.
            let windows = std::mem::take(&mut state.windows);
            for hwnd in windows {
                let _ = DestroyWindow(hwnd);
            }
        }
    }

    impl Drop for OverlayCleanup {
        fn drop(&mut self) {
            unsafe {
                self.destroy_now();
            }
        }
    }

    struct Snapshot {
        dc: HDC,
        bitmap: HBITMAP,
        old_bitmap: HGDIOBJ,
        bits: *mut u8,
        byte_len: usize,
        dim_dc: HDC,
        dim_bitmap: HBITMAP,
        old_dim_bitmap: HGDIOBJ,
    }

    impl Snapshot {
        unsafe fn capture(desktop: RectI) -> Result<Self, String> {
            let width = i32::try_from(desktop.width_i64())
                .map_err(|_| "Недопустимая ширина рабочего стола".to_string())?;
            let height = i32::try_from(desktop.height_i64())
                .map_err(|_| "Недопустимая высота рабочего стола".to_string())?;
            let byte_len = super::checked_rgba_len(width as u32, height as u32)?;

            let screen_dc = ScreenDc::acquire()?;
            let dc = CreateCompatibleDC(Some(screen_dc.0));
            if dc.is_invalid() {
                return Err(win_error("Не удалось создать DC снимка"));
            }

            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Negative height creates a top-down DIB whose first row is the desktop top.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: u32::try_from(byte_len).unwrap_or(0),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = null_mut();
            let bitmap = match CreateDIBSection(
                Some(screen_dc.0),
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            ) {
                Ok(bitmap) => bitmap,
                Err(error) => {
                    let _ = DeleteDC(dc);
                    return Err(format!("Не удалось создать DIB снимка: {error}"));
                }
            };
            if bits.is_null() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
                return Err("Windows вернула пустой указатель DIB".to_string());
            }
            let old_bitmap = SelectObject(dc, HGDIOBJ(bitmap.0));
            if old_bitmap.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
                return Err(win_error("Не удалось выбрать DIB в DC"));
            }
            let raster_op = windows::Win32::Graphics::Gdi::ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0);
            if let Err(error) = BitBlt(
                dc,
                0,
                0,
                width,
                height,
                Some(screen_dc.0),
                desktop.left,
                desktop.top,
                raster_op,
            ) {
                let _ = SelectObject(dc, old_bitmap);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
                return Err(format!("Не удалось снять рабочий стол: {error}"));
            }

            let dim_dc = CreateCompatibleDC(Some(screen_dc.0));
            if dim_dc.is_invalid() {
                let _ = SelectObject(dc, old_bitmap);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
                return Err(win_error("Не удалось создать DC затемнения"));
            }
            let dim_bitmap = CreateCompatibleBitmap(screen_dc.0, 1, 1);
            if dim_bitmap.is_invalid() {
                let _ = DeleteDC(dim_dc);
                let _ = SelectObject(dc, old_bitmap);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
                return Err(win_error("Не удалось создать bitmap затемнения"));
            }
            let old_dim_bitmap = SelectObject(dim_dc, HGDIOBJ(dim_bitmap.0));
            if old_dim_bitmap.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(dim_bitmap.0));
                let _ = DeleteDC(dim_dc);
                let _ = SelectObject(dc, old_bitmap);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
                return Err(win_error("Не удалось выбрать bitmap затемнения"));
            }
            if !PatBlt(dim_dc, 0, 0, 1, 1, BLACKNESS).as_bool() {
                let _ = SelectObject(dim_dc, old_dim_bitmap);
                let _ = DeleteObject(HGDIOBJ(dim_bitmap.0));
                let _ = DeleteDC(dim_dc);
                let _ = SelectObject(dc, old_bitmap);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
                return Err(win_error("Не удалось подготовить затемнение"));
            }

            Ok(Self {
                dc,
                bitmap,
                old_bitmap,
                bits: bits.cast(),
                byte_len,
                dim_dc,
                dim_bitmap,
                old_dim_bitmap,
            })
        }

        unsafe fn pixels(&self) -> &[u8] {
            slice::from_raw_parts(self.bits.cast_const(), self.byte_len)
        }
    }

    impl Drop for Snapshot {
        fn drop(&mut self) {
            unsafe {
                let _ = SelectObject(self.dim_dc, self.old_dim_bitmap);
                let _ = DeleteObject(HGDIOBJ(self.dim_bitmap.0));
                let _ = DeleteDC(self.dim_dc);
                let _ = SelectObject(self.dc, self.old_bitmap);
                let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
                let _ = DeleteDC(self.dc);
            }
        }
    }

    struct ScreenDc(HDC);

    impl ScreenDc {
        unsafe fn acquire() -> Result<Self, String> {
            let dc = GetDC(None);
            if dc.is_invalid() {
                Err(win_error("Не удалось получить DC рабочего стола"))
            } else {
                Ok(Self(dc))
            }
        }
    }

    impl Drop for ScreenDc {
        fn drop(&mut self) {
            unsafe {
                let _ = ReleaseDC(None, self.0);
            }
        }
    }

    struct DpiContextGuard(DPI_AWARENESS_CONTEXT);

    impl DpiContextGuard {
        unsafe fn enter() -> Result<Self, String> {
            let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            if previous.is_invalid() {
                Err(win_error(
                    "Не удалось включить Per-Monitor V2 DPI для селектора",
                ))
            } else {
                Ok(Self(previous))
            }
        }
    }

    impl Drop for DpiContextGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = SetThreadDpiAwarenessContext(self.0);
            }
        }
    }

    struct RegisteredClass {
        instance: HINSTANCE,
    }

    impl Drop for RegisteredClass {
        fn drop(&mut self) {
            unsafe {
                let _ = UnregisterClassW(CLASS_NAME, Some(self.instance));
            }
        }
    }

    pub(super) fn run_selector() -> Result<Option<CapturedRegion>, String> {
        // This catches Rust panics in worker setup as a second boundary around the WndProc guard.
        catch_unwind(AssertUnwindSafe(run_selector_inner))
            .map_err(|_| "Селектор аварийно завершил работу".to_string())?
    }

    fn run_selector_inner() -> Result<Option<CapturedRegion>, String> {
        unsafe {
            let _dpi = DpiContextGuard::enter()?;
            let desktop = virtual_desktop_rect()?;
            let monitors = enumerate_monitors()?;
            if monitors.is_empty() {
                return Err("Windows не сообщила ни об одном мониторе".to_string());
            }

            // Capture must happen before class registration/window creation to keep overlays out.
            let snapshot = Snapshot::capture(desktop)?;
            let module = GetModuleHandleW(None)
                .map_err(|error| format!("Не удалось получить HINSTANCE: {error}"))?;
            let instance = HINSTANCE(module.0);
            let cursor = LoadCursorW(None, IDC_CROSS)
                .map_err(|error| format!("Не удалось загрузить курсор: {error}"))?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                hCursor: cursor,
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 {
                return Err(win_error(
                    "Не удалось зарегистрировать класс окна селектора",
                ));
            }
            let _registered_class = RegisteredClass { instance };

            let initial_cursor = cursor_position().ok();
            let mut state = Box::new(SelectorState {
                desktop,
                monitors: monitors.clone(),
                snapshot,
                windows: Vec::with_capacity(monitors.len()),
                anchor: None,
                cursor: initial_cursor,
                dragging: false,
                outcome: None,
                callback_panicked: false,
            });
            let state_ptr: *mut SelectorState = &mut *state;
            let mut window_data: Vec<Box<WindowData>> = Vec::with_capacity(monitors.len());
            // Declared after `window_data`, so it drops first and HWND userdata remains valid.
            let mut overlay_cleanup = OverlayCleanup { state: state_ptr };

            for monitor in monitors {
                let width = i32::try_from(monitor.width_i64())
                    .map_err(|_| "Недопустимая ширина монитора".to_string())?;
                let height = i32::try_from(monitor.height_i64())
                    .map_err(|_| "Недопустимая высота монитора".to_string())?;
                let mut data = Box::new(WindowData {
                    state: state_ptr,
                    monitor,
                });
                let data_ptr: *mut WindowData = &mut *data;
                let hwnd = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                    CLASS_NAME,
                    w!("UTranslate screen selector"),
                    WS_POPUP,
                    monitor.left,
                    monitor.top,
                    width,
                    height,
                    None,
                    None,
                    Some(instance),
                    Some(data_ptr.cast()),
                )
                .map_err(|error| format!("Не удалось создать окно селектора: {error}"))?;
                state.windows.push(hwnd);
                window_data.push(data);
            }

            for hwnd in state.windows.iter().copied() {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                )
                .map_err(|error| format!("Не удалось показать окно селектора: {error}"))?;
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = UpdateWindow(hwnd);
            }
            let focus_window = initial_cursor
                .and_then(|point| {
                    window_data
                        .iter()
                        .position(|data| data.monitor.contains(point))
                })
                .and_then(|index| state.windows.get(index).copied())
                .or_else(|| state.windows.first().copied())
                .ok_or_else(|| "Не создано окно селектора".to_string())?;
            let _ = SetForegroundWindow(focus_window);
            let _ = SetFocus(Some(focus_window));

            let message_error = message_loop();
            overlay_cleanup.destroy_now();
            drop(window_data);
            message_error?;
            if state.callback_panicked {
                return Err("В обработчике окна селектора произошла ошибка".to_string());
            }

            match state.outcome.unwrap_or(Outcome::Cancelled) {
                Outcome::Cancelled => Ok(None),
                Outcome::Accepted { rect, anchor } => {
                    let pixels = state.snapshot.pixels();
                    crop_bgra_top_down(pixels, desktop, rect, anchor).map(Some)
                }
            }
        }
    }

    unsafe fn message_loop() -> Result<(), String> {
        let mut message: MSG = zeroed();
        loop {
            let result = GetMessageW(&mut message, None, 0, 0);
            if result.0 == -1 {
                return Err(win_error("Ошибка цикла сообщений селектора"));
            }
            if !result.as_bool() {
                return Ok(());
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match catch_unwind(AssertUnwindSafe(|| {
            window_proc_inner(hwnd, message, wparam, lparam)
        })) {
            Ok(result) => result,
            Err(_) => {
                let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowData;
                if let Some(data) = data_ptr.as_mut() {
                    if let Some(state) = data.state.as_mut() {
                        state.callback_panicked = true;
                        state.outcome = Some(Outcome::Cancelled);
                    }
                }
                PostQuitMessage(1);
                LRESULT(0)
            }
        }
    }

    unsafe fn window_proc_inner(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_NCCREATE {
            let create = lparam.0 as *const CREATESTRUCTW;
            if create.is_null() {
                return LRESULT(0);
            }
            let data = (*create).lpCreateParams as *mut WindowData;
            if data.is_null() || (*data).state.is_null() {
                return LRESULT(0);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, data as isize);
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        if message == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }

        let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowData;
        let Some(data) = data_ptr.as_mut() else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        let Some(state) = data.state.as_mut() else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };

        match message {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(hwnd, data.monitor, state);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                if let Ok(point) = cursor_position() {
                    state.anchor = Some(point);
                    state.cursor = Some(point);
                    state.dragging = true;
                    SetCapture(hwnd);
                    invalidate_all(state);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if let Ok(point) = cursor_position() {
                    state.cursor = Some(point);
                    invalidate_all(state);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if state.dragging {
                    // ReleaseCapture synchronously sends WM_CAPTURECHANGED. Mark the normal drag
                    // complete first so the message is not mistaken for capture loss.
                    state.dragging = false;
                    let _ = ReleaseCapture();
                    match (state.anchor, cursor_position()) {
                        (Some(anchor), Ok(current)) => {
                            state.cursor = Some(current);
                            let drag = RectI::from_drag(anchor, current);
                            match validated_selection(drag, state.desktop, &state.monitors) {
                                Ok(Some(rect)) => finish(state, Outcome::Accepted { rect, anchor }),
                                Ok(None) => {
                                    // A click or tiny accidental drag leaves the selector open.
                                    state.anchor = None;
                                    invalidate_all(state);
                                }
                                Err(_) => finish(state, Outcome::Cancelled),
                            }
                        }
                        _ => finish(state, Outcome::Cancelled),
                    }
                }
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
                finish(state, Outcome::Cancelled);
                LRESULT(0)
            }
            WM_RBUTTONDOWN | WM_CLOSE | WM_DISPLAYCHANGE | WM_DPICHANGED => {
                finish(state, Outcome::Cancelled);
                LRESULT(0)
            }
            WM_CAPTURECHANGED if state.dragging => {
                state.dragging = false;
                finish(state, Outcome::Cancelled);
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                // WM_KILLFOCUS passes the window receiving focus in wParam.
                let next = HWND(wparam.0 as *mut c_void);
                if !state.windows.contains(&next) {
                    finish(state, Outcome::Cancelled);
                }
                LRESULT(0)
            }
            WM_ACTIVATE => {
                // LOWORD(wParam) is zero only while this HWND is being deactivated. On activation
                // lParam names the previous external window and must not cancel the selector.
                let is_deactivating = (wparam.0 & 0xffff) == 0;
                let next = HWND(lparam.0 as *mut c_void);
                if is_deactivating && !state.windows.contains(&next) {
                    finish(state, Outcome::Cancelled);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }

    unsafe fn finish(state: &mut SelectorState, outcome: Outcome) {
        if state.outcome.is_none() {
            if state.dragging {
                state.dragging = false;
                let _ = ReleaseCapture();
            }
            state.outcome = Some(outcome);
            PostQuitMessage(0);
        }
    }

    unsafe fn invalidate_all(state: &SelectorState) {
        for hwnd in state.windows.iter().copied() {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    unsafe fn paint(hwnd: HWND, monitor: RectI, state: &SelectorState) {
        let mut paint: PAINTSTRUCT = zeroed();
        let hdc = BeginPaint(hwnd, &mut paint);
        if hdc.is_invalid() {
            return;
        }
        let width = i32::try_from(monitor.width_i64()).unwrap_or(0);
        let height = i32::try_from(monitor.height_i64()).unwrap_or(0);
        let source_x = monitor.left - state.desktop.left;
        let source_y = monitor.top - state.desktop.top;
        let _ = BitBlt(
            hdc,
            0,
            0,
            width,
            height,
            Some(state.snapshot.dc),
            source_x,
            source_y,
            SRCCOPY,
        );
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 112,
            AlphaFormat: 0,
        };
        let _ = AlphaBlend(
            hdc,
            0,
            0,
            width,
            height,
            state.snapshot.dim_dc,
            0,
            0,
            1,
            1,
            blend,
        );

        if let (Some(anchor), Some(cursor)) = (state.anchor, state.cursor) {
            let selection = RectI::from_drag(anchor, cursor);
            if let Some(visible) = selection.intersect(monitor) {
                let local_left = visible.left - monitor.left;
                let local_top = visible.top - monitor.top;
                let visible_width = i32::try_from(visible.width_i64()).unwrap_or(0);
                let visible_height = i32::try_from(visible.height_i64()).unwrap_or(0);
                let _ = BitBlt(
                    hdc,
                    local_left,
                    local_top,
                    visible_width,
                    visible_height,
                    Some(state.snapshot.dc),
                    visible.left - state.desktop.left,
                    visible.top - state.desktop.top,
                    SRCCOPY,
                );
            }
            draw_selection_border(hdc, monitor, selection);
        }
        if let Some(cursor) = state.cursor.filter(|point| monitor.contains(*point)) {
            draw_crosshair(hdc, monitor, cursor, width, height);
        }
        draw_hint(hdc, width);
        let _ = EndPaint(hwnd, &paint);
    }

    unsafe fn draw_selection_border(hdc: HDC, monitor: RectI, selection: RectI) {
        let Some(visible) = selection.intersect(monitor) else {
            return;
        };
        let pen = CreatePen(PS_SOLID, 2, ACCENT);
        if pen.is_invalid() {
            return;
        }
        let old = SelectObject(hdc, HGDIOBJ(pen.0));
        if !old.is_invalid() {
            let left = visible.left - monitor.left;
            let top = visible.top - monitor.top;
            let right = visible.right - monitor.left - 1;
            let bottom = visible.bottom - monitor.top - 1;
            let _ = MoveToEx(hdc, left, top, None);
            let _ = LineTo(hdc, right, top);
            let _ = LineTo(hdc, right, bottom);
            let _ = LineTo(hdc, left, bottom);
            let _ = LineTo(hdc, left, top);
            let _ = SelectObject(hdc, old);
        }
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }

    unsafe fn draw_crosshair(hdc: HDC, monitor: RectI, cursor: PointI, width: i32, height: i32) {
        let pen = CreatePen(PS_SOLID, 1, WHITE);
        if pen.is_invalid() {
            return;
        }
        let old = SelectObject(hdc, HGDIOBJ(pen.0));
        if !old.is_invalid() {
            let x = cursor.x - monitor.left;
            let y = cursor.y - monitor.top;
            let _ = MoveToEx(hdc, 0, y, None);
            let _ = LineTo(hdc, width, y);
            let _ = MoveToEx(hdc, x, 0, None);
            let _ = LineTo(hdc, x, height);
            let _ = SelectObject(hdc, old);
        }
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }

    unsafe fn draw_hint(hdc: HDC, width: i32) {
        let font = GetStockObject(DEFAULT_GUI_FONT);
        let old_font = SelectObject(hdc, font);
        let old_mode = SetBkMode(hdc, TRANSPARENT);
        let mut shadow_rect = RECT {
            left: 1,
            top: 17,
            right: width + 1,
            bottom: 53,
        };
        let mut text_rect = RECT {
            left: 0,
            top: 16,
            right: width,
            bottom: 52,
        };
        let format = DT_CENTER | DT_TOP | DT_SINGLELINE;
        let mut shadow: Vec<u16> = HINT.encode_utf16().collect();
        let _ = SetTextColor(hdc, SHADOW);
        let _ = DrawTextW(hdc, &mut shadow, &mut shadow_rect, format);
        let mut text: Vec<u16> = HINT.encode_utf16().collect();
        let _ = SetTextColor(hdc, WHITE);
        let _ = DrawTextW(hdc, &mut text, &mut text_rect, format);
        if old_mode > 0 {
            let _ = SetBkMode(
                hdc,
                windows::Win32::Graphics::Gdi::BACKGROUND_MODE(old_mode as u32),
            );
        }
        if !old_font.is_invalid() {
            let _ = SelectObject(hdc, old_font);
        }
    }

    unsafe fn cursor_position() -> Result<PointI, String> {
        let mut point = POINT::default();
        GetCursorPos(&mut point)
            .map_err(|error| format!("Не удалось получить позицию курсора: {error}"))?;
        Ok(PointI {
            x: point.x,
            y: point.y,
        })
    }

    unsafe fn virtual_desktop_rect() -> Result<RectI, String> {
        RectI::from_origin_size(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }

    unsafe fn enumerate_monitors() -> Result<Vec<RectI>, String> {
        unsafe extern "system" fn callback(
            _monitor: windows::Win32::Graphics::Gdi::HMONITOR,
            _dc: HDC,
            rect: *mut RECT,
            data: LPARAM,
        ) -> BOOL {
            let result = catch_unwind(AssertUnwindSafe(|| {
                if rect.is_null() || data.0 == 0 {
                    return false;
                }
                let monitors = &mut *(data.0 as *mut Vec<RectI>);
                let rect = *rect;
                if rect.right <= rect.left || rect.bottom <= rect.top {
                    return false;
                }
                monitors.push(RectI {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                });
                true
            }));
            BOOL::from(result.unwrap_or(false))
        }

        let mut monitors = Vec::new();
        let ok = EnumDisplayMonitors(
            None,
            None,
            Some(callback),
            LPARAM((&mut monitors as *mut Vec<RectI>) as isize),
        );
        if !ok.as_bool() {
            return Err(win_error("Не удалось перечислить мониторы"));
        }
        Ok(monitors)
    }

    fn win_error(context: &str) -> String {
        format!("{context}: {}", WinError::from_thread())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RectI {
        RectI {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn reversed_drag_is_normalized_in_signed_coordinates() {
        assert_eq!(
            RectI::from_drag(PointI { x: 40, y: 20 }, PointI { x: -10, y: -30 }),
            rect(-10, -30, 40, 20)
        );
    }

    #[test]
    fn negative_origin_selection_is_clamped_to_virtual_desktop() {
        let desktop = rect(-1920, -200, 1920, 1080);
        let monitors = [rect(-1920, -200, 0, 880), rect(0, 0, 1920, 1080)];
        let selection = validated_selection(rect(-2000, -100, -1800, 100), desktop, &monitors)
            .unwrap()
            .unwrap();
        assert_eq!(selection, rect(-1920, -100, -1800, 100));
    }

    #[test]
    fn cross_monitor_selection_keeps_the_full_bounding_rectangle() {
        let desktop = rect(-800, 0, 1200, 900);
        let monitors = [rect(-800, 0, 0, 600), rect(0, 0, 1200, 900)];
        let selection = validated_selection(rect(-50, 100, 50, 200), desktop, &monitors)
            .unwrap()
            .unwrap();
        assert_eq!(selection, rect(-50, 100, 50, 200));
    }

    #[test]
    fn gap_only_and_tiny_selections_are_rejected() {
        let desktop = rect(0, 0, 300, 100);
        let monitors = [rect(0, 0, 100, 100), rect(200, 0, 300, 100)];
        assert!(validated_selection(rect(120, 10, 180, 50), desktop, &monitors).is_err());
        assert_eq!(
            validated_selection(rect(10, 10, 12, 80), desktop, &monitors).unwrap(),
            None
        );
    }

    #[test]
    fn invalid_overflow_and_oversized_dimensions_are_rejected() {
        assert!(RectI::from_origin_size(i32::MAX, 0, 1, 1).is_err());
        assert!(RectI::from_origin_size(0, 0, 0, 1).is_err());
        assert!(checked_rgba_len(16_384, 16_384).is_err());
    }

    #[test]
    fn crop_converts_bgra_to_opaque_rgba_without_flipping_rows() {
        // Top row: red, green. Bottom row: blue, white. Alpha bytes are intentionally varied.
        let pixels = [0, 0, 255, 0, 0, 255, 0, 17, 255, 0, 0, 99, 255, 255, 255, 1];
        let result = crop_bgra_top_down(
            &pixels,
            rect(-1, -1, 1, 1),
            rect(-1, -1, 1, 1),
            PointI { x: -1, y: -1 },
        )
        .unwrap();
        assert_eq!(result.width, 2);
        assert_eq!(result.height, 2);
        assert_eq!(
            result.rgba,
            [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,]
        );
    }

    #[test]
    fn crop_rejects_short_source_buffer() {
        let error = crop_bgra_top_down(
            &[0; 15],
            rect(0, 0, 2, 2),
            rect(0, 0, 2, 2),
            PointI { x: 0, y: 0 },
        )
        .unwrap_err();
        assert!(error.contains("короче"));
    }
}
