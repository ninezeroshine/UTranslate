//! Native, in-memory screen region selector for Windows.
//!
//! The caller is responsible for hiding UTranslate and flushing the compositor before calling
//! [`select_region`]. This module captures the virtual desktop before it creates any overlay
//! windows, never touches the clipboard, and returns only the selected crop.

use std::sync::atomic::{AtomicBool, Ordering};

// Предел размера снимка — общий с OCR: `ocr::checked_rgba_len`. На пределе замороженный
// BGRA-рабочий стол и вырезанный RGBA-кроп вместе занимают около 512 МиБ; обе аллокации
// падают через Result/Win32-ошибку, а не убивают процесс.
use crate::ocr::checked_rgba_len;

const MIN_SELECTION_SIDE: i64 = 3;
/// Затемнение вне выделения: 40 % цвета чернил `--ink` #1B252A. Чистый чёрный на снимке
/// рабочего стола выглядит дырой, чернила — тенью.
const DIM_ALPHA: u8 = 102;
/// Набор затемнения. Оверлей открывают хоткеем, поэтому ввод работает с первого кадра.
const DIM_IN_MS: u64 = 120;
/// Сколько подсказка живёт после первого настоящего движения мыши.
const HINT_HOLD_MS: u64 = 1_500;
/// Затухание подсказки.
const HINT_OUT_MS: u64 = 150;
/// Пауза курсора, после которой бейдж размера подтверждает выделение.
const BADGE_IDLE_MS: u64 = 150;
/// Появление бейджа размера.
const BADGE_IN_MS: u64 = 120;
/// Выход гаснет быстрее, чем оверлей появлялся.
const EXIT_MS: u64 = 90;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedRegion {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub left: i32,
    pub top: i32,
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

    /// Тот же прямоугольник в координатах клиентской области окна монитора.
    fn relative_to(self, origin: RectI) -> Self {
        let shift = |value: i32, base: i32| -> i32 {
            (i64::from(value) - i64::from(base)).clamp(i64::from(i32::MIN), i64::from(i32::MAX))
                as i32
        };
        Self {
            left: shift(self.left, origin.left),
            top: shift(self.top, origin.top),
            right: shift(self.right, origin.left),
            bottom: shift(self.bottom, origin.top),
        }
    }
}

/// Ease-out кубикой — единственная кривая всех переходов оверлея.
fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Затемнение вне выделения: 0 → [`DIM_ALPHA`] за [`DIM_IN_MS`].
fn dim_alpha(elapsed_ms: u64, reduce_motion: bool) -> u8 {
    if reduce_motion || elapsed_ms >= DIM_IN_MS {
        return DIM_ALPHA;
    }
    let eased = ease_out_cubic(elapsed_ms as f32 / DIM_IN_MS as f32);
    (f32::from(DIM_ALPHA) * eased).round() as u8
}

/// Видимость подсказки: появляется вместе с затемнением, уходит через [`HINT_HOLD_MS`]
/// после первого движения мыши или сразу по нажатию — что раньше. Обратно не возвращается.
fn hint_alpha(
    now_ms: u64,
    first_move_at_ms: Option<u64>,
    drag_started_at_ms: Option<u64>,
    reduce_motion: bool,
) -> f32 {
    let hide_at = match (first_move_at_ms, drag_started_at_ms) {
        (Some(moved), Some(pressed)) => Some(moved.saturating_add(HINT_HOLD_MS).min(pressed)),
        (Some(moved), None) => Some(moved.saturating_add(HINT_HOLD_MS)),
        (None, Some(pressed)) => Some(pressed),
        (None, None) => None,
    };
    let appear = if reduce_motion {
        1.0
    } else {
        ease_out_cubic(now_ms as f32 / DIM_IN_MS as f32)
    };
    let Some(hide_at) = hide_at else {
        return appear;
    };
    if now_ms < hide_at {
        return appear;
    }
    if reduce_motion {
        return 0.0;
    }
    let gone = now_ms - hide_at;
    if gone >= HINT_OUT_MS {
        return 0.0;
    }
    // min, а не умножение: подсказка, которую погасили на середине появления, не подпрыгивает.
    appear.min(1.0 - ease_out_cubic(gone as f32 / HINT_OUT_MS as f32))
}

/// Видимость бейджа размера: только в протяжке и только когда рука остановилась.
/// Любое движение убирает его мгновенно, поэтому он не мельтешит вдоль рамки.
fn badge_alpha(now_ms: u64, dragging: bool, last_move_at_ms: u64, reduce_motion: bool) -> f32 {
    if !dragging {
        return 0.0;
    }
    let idle = now_ms.saturating_sub(last_move_at_ms);
    if idle < BADGE_IDLE_MS {
        return 0.0;
    }
    if reduce_motion {
        return 1.0;
    }
    ease_out_cubic((idle - BADGE_IDLE_MS) as f32 / BADGE_IN_MS as f32)
}

/// Прогресс выхода: 0 — оверлей на месте, 1 — всё погасло и окна можно закрывать.
fn exit_progress(now_ms: u64, closing_at_ms: Option<u64>, reduce_motion: bool) -> f32 {
    let Some(closing_at) = closing_at_ms else {
        return 0.0;
    };
    if reduce_motion {
        return 1.0;
    }
    let elapsed = now_ms.saturating_sub(closing_at);
    if elapsed >= EXIT_MS {
        return 1.0;
    }
    ease_out_cubic(elapsed as f32 / EXIT_MS as f32)
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
pub fn select_region() -> Result<Option<CapturedRegion>, String> {
    let _active = ActiveSelectorGuard::acquire()?;
    std::thread::Builder::new()
        .name("utranslate-screen-selector".to_string())
        .spawn(platform::run_selector)
        .map_err(|error| format!("Не удалось запустить поток выбора области: {error}"))?
        .join()
        .map_err(|_| "Поток выбора области аварийно завершился".to_string())?
}

mod platform {
    use super::{
        badge_alpha, crop_bgra_top_down, dim_alpha, exit_progress, hint_alpha, validated_selection,
        CapturedRegion, PointI, RectI,
    };
    use std::{
        ffi::c_void,
        mem::{size_of, zeroed},
        panic::{catch_unwind, AssertUnwindSafe},
        ptr::null_mut,
        slice,
        sync::OnceLock,
        time::Instant,
    };
    use windows::{
        core::{w, Error as WinError, BOOL, PCWSTR},
        Win32::{
            Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
            Graphics::{
                Gdi::{
                    AlphaBlend, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
                    CreateDIBSection, CreateFontW, CreatePen, CreateRoundRectRgn, CreateSolidBrush,
                    DeleteDC, DeleteObject, DrawTextW, EndPaint, EnumDisplayMonitors, FillRect,
                    GetDC, GetStockObject, InvalidateRect, LineTo, MoveToEx, ReleaseDC, RoundRect,
                    SelectClipRgn, SelectObject, SetBkMode, SetTextColor, UpdateWindow,
                    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, CAPTUREBLT,
                    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
                    DIB_RGB_COLORS, DT_CALCRECT, DT_LEFT, DT_NOCLIP, DT_SINGLELINE, DT_TOP,
                    FF_DONTCARE, FW_SEMIBOLD, HBITMAP, HDC, HFONT, HGDIOBJ, HRGN, NULL_PEN,
                    OUT_TT_PRECIS, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
                },
                GdiPlus::{
                    FillModeAlternate, FontStyleRegular, GdipAddPathArc, GdipAddPathLine,
                    GdipCloneStringFormat, GdipClosePathFigure, GdipCreateFont,
                    GdipCreateFontFamilyFromName, GdipCreateFromHDC, GdipCreatePath,
                    GdipCreatePen1, GdipCreateSolidFill, GdipCreateStringFormat, GdipDeleteBrush,
                    GdipDeleteFont, GdipDeleteFontFamily, GdipDeleteGraphics, GdipDeletePath,
                    GdipDeletePen, GdipDeleteStringFormat, GdipDrawLines, GdipDrawPath,
                    GdipDrawString, GdipFillPath, GdipMeasureString, GdipSetPenEndCap,
                    GdipSetPenLineJoin, GdipSetPenStartCap, GdipSetSmoothingMode,
                    GdipSetStringFormatFlags, GdipSetTextRenderingHint,
                    GdipStringFormatGetGenericTypographic, GdiplusStartup, GdiplusStartupInput,
                    GpBrush, GpFont, GpFontFamily, GpGraphics, GpPath, GpPen, GpSolidFill,
                    GpStringFormat, LineCapRound, LineJoinRound, Ok as GDIP_OK, PointF, RectF,
                    SmoothingModeAntiAlias, StringFormatFlagsMeasureTrailingSpaces,
                    StringFormatFlagsNoClip, StringFormatFlagsNoWrap,
                    TextRenderingHintClearTypeGridFit, UnitPixel,
                },
            },
            System::LibraryLoader::GetModuleHandleW,
            UI::{
                HiDpi::{
                    GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                },
                Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE},
                WindowsAndMessaging::{
                    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
                    GetMessageW, GetSystemMetrics, GetWindowLongPtrW, KillTimer, LoadCursorW,
                    PostQuitMessage, RegisterClassExW, SetForegroundWindow, SetTimer,
                    SetWindowLongPtrW, SetWindowPos, ShowWindow, SystemParametersInfoW,
                    TranslateMessage, UnregisterClassW, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW,
                    GWLP_USERDATA, HWND_TOPMOST, IDC_CROSS, MSG, SM_CXVIRTUALSCREEN,
                    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
                    SPI_GETCLIENTAREAANIMATION, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                    SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
                    WM_ACTIVATE, WM_CAPTURECHANGED, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE,
                    WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN,
                    WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_PAINT,
                    WM_RBUTTONDOWN, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
                    WS_POPUP,
                },
            },
        },
    };

    const CLASS_NAME: PCWSTR = w!("UTranslate.NativeScreenSelector");
    /// Подсказка запасного GDI-пути: одной строкой, без кейкапа и иконки.
    const HINT: &str = "Выделите область с текстом · Esc — отмена";
    /// Палитра «туман и вода» (см. app/src/index.css). COLORREF — это 0x00BBGGRR.
    /// `--water` #63b6c6 — рамка выделения.
    const WATER: COLORREF = COLORREF(0x00_c6_b6_63);
    /// `--ink` светлой темы #1b252a — заливка пилюль и цвет затемнения.
    const PILL_BG: COLORREF = COLORREF(0x00_2a_25_1b);
    /// `--ink` тёмной темы #e4edf0 — текст на пилюле.
    const PILL_TEXT: COLORREF = COLORREF(0x00_f0_ed_e4);
    /// Те же цвета для GDI+, где нужен порядок 0xRRGGBB.
    const INK_RGB: u32 = 0x1b_25_2a;
    const TEXT_RGB: u32 = 0xe4_ed_f0;
    /// `--ink-2` тёмной темы #9badb5 — слово «отмена».
    const MUTED_RGB: u32 = 0x9b_ad_b5;
    const WATER_RGB: u32 = 0x63_b6_c6;
    const WHITE_RGB: u32 = 0xff_ff_ff;
    /// Скругление выреза и рамки выделения, логические пиксели.
    const SELECTION_RADIUS: i32 = 6;
    /// Единственный таймер анимации. Живёт на одном окне всю сессию селектора, остальные
    /// окна перерисовываются вместе с ним.
    const ANIM_TIMER_ID: usize = 1;
    /// Шаг анимации. Кадр пересчитывается каждый тик, но перерисовка идёт только при
    /// изменении квантованных альф — иначе неподвижный оверлей жёг бы CPU.
    const ANIM_TICK_MS: u32 = 8;
    /// Иконка «рамка экрана» из lib/icons.tsx: четыре уголка в системе координат viewBox 16.
    const SCREEN_CORNERS: [[(f32, f32); 3]; 4] = [
        [(3.0, 2.5), (1.8, 2.5), (1.8, 5.5)],
        [(13.0, 2.5), (14.2, 2.5), (14.2, 5.5)],
        [(3.0, 13.5), (1.8, 13.5), (1.8, 10.5)],
        [(13.0, 13.5), (14.2, 13.5), (14.2, 10.5)],
    ];

    #[derive(Debug, Clone, Copy)]
    enum Outcome {
        Accepted { rect: RectI },
        Cancelled,
    }

    /// Альфы слоёв одного кадра, уже с учётом выхода. Сравнение по значению решает, нужна ли
    /// перерисовка.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct Alphas {
        dim: u8,
        hint: u8,
        badge: u8,
        border: u8,
    }

    impl Alphas {
        fn faded(self, factor: f32) -> Self {
            let factor = factor.clamp(0.0, 1.0);
            let scale = |value: u8| (f32::from(value) * factor).round() as u8;
            Self {
                dim: scale(self.dim),
                hint: scale(self.hint),
                badge: scale(self.badge),
                border: scale(self.border),
            }
        }
    }

    fn to_byte(value: f32) -> u8 {
        (value.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    fn alpha_of(byte: u8) -> f32 {
        f32::from(byte) / 255.0
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
        opened_at: Instant,
        /// Системная настройка «показывать анимацию»: выключена — все значения конечные сразу.
        reduce_motion: bool,
        first_move_at: Option<u64>,
        drag_started_at: Option<u64>,
        last_move_at: u64,
        /// Кадр, замороженный в момент начала выхода: гаснет ровно то, что было на экране.
        frozen: Option<Alphas>,
        closing_at: Option<u64>,
        quit_posted: bool,
        alphas: Alphas,
    }

    impl SelectorState {
        fn now_ms(&self) -> u64 {
            u64::try_from(self.opened_at.elapsed().as_millis()).unwrap_or(u64::MAX)
        }

        fn live_alphas(&self, now: u64) -> Alphas {
            Alphas {
                dim: dim_alpha(now, self.reduce_motion),
                hint: to_byte(hint_alpha(
                    now,
                    self.first_move_at,
                    self.drag_started_at,
                    self.reduce_motion,
                )),
                badge: to_byte(badge_alpha(
                    now,
                    self.dragging,
                    self.last_move_at,
                    self.reduce_motion,
                )),
                border: 255,
            }
        }

        fn current_alphas(&self) -> Alphas {
            let now = self.now_ms();
            let base = match self.frozen {
                Some(frozen) => frozen,
                None => self.live_alphas(now),
            };
            base.faded(1.0 - exit_progress(now, self.closing_at, self.reduce_motion))
        }

        /// Пересчитывает кадр и говорит, изменился ли он с прошлого раза.
        fn refresh(&mut self) -> bool {
            let next = self.current_alphas();
            let changed = next != self.alphas;
            self.alphas = next;
            changed
        }

        fn exit_finished(&self) -> bool {
            self.closing_at.is_some()
                && exit_progress(self.now_ms(), self.closing_at, self.reduce_motion) >= 1.0
        }
    }

    struct WindowData {
        state: *mut SelectorState,
        monitor: RectI,
        buffer: Option<BackBuffer>,
    }

    /// Свой back-buffer на каждое окно: кадр собирается целиком в памяти и уезжает на экран
    /// одним BitBlt, иначе на каждом WM_MOUSEMOVE видно мерцание.
    struct BackBuffer {
        dc: HDC,
        bitmap: HBITMAP,
        old_bitmap: HGDIOBJ,
        width: i32,
        height: i32,
    }

    impl BackBuffer {
        unsafe fn create(reference: HDC, width: i32, height: i32) -> Option<Self> {
            let dc = CreateCompatibleDC(Some(reference));
            if dc.is_invalid() {
                return None;
            }
            let bitmap = CreateCompatibleBitmap(reference, width, height);
            if bitmap.is_invalid() {
                let _ = DeleteDC(dc);
                return None;
            }
            let old_bitmap = SelectObject(dc, HGDIOBJ(bitmap.0));
            if old_bitmap.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
                return None;
            }
            Some(Self {
                dc,
                bitmap,
                old_bitmap,
                width,
                height,
            })
        }
    }

    impl Drop for BackBuffer {
        fn drop(&mut self) {
            unsafe {
                let _ = SelectObject(self.dc, self.old_bitmap);
                let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
                let _ = DeleteDC(self.dc);
            }
        }
    }

    impl WindowData {
        /// Буфер создаётся при первом paint (нужен совместимый DC) и переживает перерисовки.
        unsafe fn buffer_dc(&mut self, reference: HDC, width: i32, height: i32) -> Option<HDC> {
            if !matches!(&self.buffer, Some(buffer) if buffer.width == width && buffer.height == height)
            {
                self.buffer = BackBuffer::create(reference, width, height);
            }
            self.buffer.as_ref().map(|buffer| buffer.dc)
        }
    }

    /// Шрифт интерфейса под DPI конкретного окна; освобождается по выходе из paint.
    /// Нужен только запасному GDI-пути: GDI+ рисует текст своими шрифтами.
    struct ScopedFont {
        font: HFONT,
    }

    impl ScopedFont {
        unsafe fn create(points: i32, dpi: u32) -> Option<Self> {
            // -MulDiv(points, dpi, 72): отрицательная высота задаёт кегль, а не bounding box.
            let height = -(points * dpi.max(1) as i32 / 72);
            let font = CreateFontW(
                height,
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                w!("Segoe UI"),
            );
            (!font.is_invalid()).then_some(Self { font })
        }
    }

    impl Drop for ScopedFont {
        fn drop(&mut self) {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(self.font.0));
            }
        }
    }

    /// Отступы и радиусы в логических пикселях → физические под DPI окна.
    fn scaled(value: i32, dpi: u32) -> i32 {
        (value * dpi.max(1) as i32 / 96).max(1)
    }

    /// Тот же масштаб дробью — макет пилюль считается во float, чтобы скругления не дрожали.
    fn scale_of(dpi: u32) -> f32 {
        dpi.max(1) as f32 / 96.0
    }

    /// Регион выреза; освобождается вместе с областью видимости.
    struct ScopedRegion(HRGN);

    impl ScopedRegion {
        unsafe fn round_rect(rect: RectI, radius: i32) -> Option<Self> {
            let region = CreateRoundRectRgn(
                rect.left,
                rect.top,
                rect.right,
                rect.bottom,
                radius * 2,
                radius * 2,
            );
            (!region.is_invalid()).then_some(Self(region))
        }
    }

    impl Drop for ScopedRegion {
        fn drop(&mut self) {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(self.0 .0));
            }
        }
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
            // 1×1 цвета чернил: AlphaBlend растянет его на монитор с нужной плотностью.
            if !fill_dim_pixel(dim_dc) {
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

    unsafe fn fill_dim_pixel(dim_dc: HDC) -> bool {
        let brush = CreateSolidBrush(PILL_BG);
        if brush.is_invalid() {
            return false;
        }
        let rect = RECT {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        };
        let filled = FillRect(dim_dc, &rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
        filled != 0
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

    /// Системная настройка «показывать анимацию в окнах»: выключена — переходы мгновенные.
    unsafe fn reduce_motion_enabled() -> bool {
        let mut enabled = BOOL(1);
        let queried = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut enabled).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        queried.is_ok() && !enabled.as_bool()
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
                opened_at: Instant::now(),
                reduce_motion: reduce_motion_enabled(),
                first_move_at: None,
                drag_started_at: None,
                last_move_at: 0,
                frozen: None,
                closing_at: None,
                quit_posted: false,
                alphas: Alphas::default(),
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
                    buffer: None,
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
            // Окна показаны мгновенно; отсюда идёт отсчёт всех переходов.
            state.opened_at = Instant::now();
            SetTimer(Some(focus_window), ANIM_TIMER_ID, ANIM_TICK_MS, None);

            let message_error = message_loop();
            overlay_cleanup.destroy_now();
            drop(window_data);
            message_error?;
            if state.callback_panicked {
                return Err("В обработчике окна селектора произошла ошибка".to_string());
            }

            match state.outcome.unwrap_or(Outcome::Cancelled) {
                Outcome::Cancelled => Ok(None),
                Outcome::Accepted { rect } => {
                    let pixels = state.snapshot.pixels();
                    crop_bgra_top_down(pixels, desktop, rect).map(Some)
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
            let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowData;
            if let Some(data) = data_ptr.as_mut() {
                // GDI-объекты буфера освобождаются вместе с окном, а не в конце селектора.
                data.buffer = None;
            }
            let _ = KillTimer(Some(hwnd), ANIM_TIMER_ID);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }

        let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowData;
        let Some(data) = data_ptr.as_mut() else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        let state_ptr = data.state;
        let Some(state) = state_ptr.as_mut() else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        // Исход зафиксирован — идёт выходная анимация, ввод больше ничего не меняет.
        let live = state.outcome.is_none();

        match message {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(hwnd, data, state);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == ANIM_TIMER_ID => {
                if state.refresh() {
                    invalidate_all(state);
                }
                if state.exit_finished() && !state.quit_posted {
                    state.quit_posted = true;
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN if live => {
                if let Ok(point) = cursor_position() {
                    let now = state.now_ms();
                    state.anchor = Some(point);
                    state.cursor = Some(point);
                    state.dragging = true;
                    state.drag_started_at = Some(now);
                    state.last_move_at = now;
                    SetCapture(hwnd);
                    state.refresh();
                    invalidate_all(state);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE if live => {
                // Windows шлёт WM_MOUSEMOVE и при появлении окна под курсором: движением
                // считается только смена позиции, иначе подсказка гасла бы сама собой.
                if let Ok(point) = cursor_position() {
                    if state.cursor != Some(point) {
                        let now = state.now_ms();
                        state.cursor = Some(point);
                        state.last_move_at = now;
                        if state.first_move_at.is_none() {
                            state.first_move_at = Some(now);
                        }
                        state.refresh();
                        invalidate_all(state);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP if live && state.dragging => {
                match (state.anchor, cursor_position()) {
                    (Some(anchor), Ok(current)) => {
                        state.cursor = Some(current);
                        let drag = RectI::from_drag(anchor, current);
                        match validated_selection(drag, state.desktop, &state.monitors) {
                            Ok(Some(rect)) => begin_exit(state, Outcome::Accepted { rect }),
                            Ok(None) => {
                                // A click or tiny accidental drag leaves the selector open.
                                // ReleaseCapture синхронно шлёт WM_CAPTURECHANGED, поэтому
                                // протяжка снимается до вызова.
                                state.dragging = false;
                                let _ = ReleaseCapture();
                                state.anchor = None;
                                state.refresh();
                                invalidate_all(state);
                            }
                            Err(_) => begin_exit(state, Outcome::Cancelled),
                        }
                    }
                    _ => begin_exit(state, Outcome::Cancelled),
                }
                LRESULT(0)
            }
            WM_KEYDOWN if live && wparam.0 as u16 == VK_ESCAPE.0 => {
                // Клавиатура закрывает мгновенно: анимация выхода тут только мешала бы.
                finish(state, Outcome::Cancelled);
                LRESULT(0)
            }
            WM_RBUTTONDOWN if live => {
                begin_exit(state, Outcome::Cancelled);
                LRESULT(0)
            }
            WM_CLOSE | WM_DISPLAYCHANGE | WM_DPICHANGED => {
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

    /// Мгновенное закрытие: клавиатура, потеря фокуса, смена монитора или DPI.
    unsafe fn finish(state: &mut SelectorState, outcome: Outcome) {
        if state.outcome.is_some() {
            return;
        }
        // Исход ставится первым: ReleaseCapture синхронно вернётся сюда через
        // WM_CAPTURECHANGED, и повторный finish не должен ничего переписать.
        state.outcome = Some(outcome);
        if state.dragging {
            state.dragging = false;
            let _ = ReleaseCapture();
        }
        if !state.quit_posted {
            state.quit_posted = true;
            PostQuitMessage(0);
        }
    }

    /// Закрытие с выходной анимацией: принятие и отмена правой кнопкой.
    unsafe fn begin_exit(state: &mut SelectorState, outcome: Outcome) {
        if state.outcome.is_some() {
            return;
        }
        if state.reduce_motion {
            finish(state, outcome);
            return;
        }
        let now = state.now_ms();
        // Кадр замораживается до снятия протяжки, иначе бейдж исчез бы вместо затухания.
        state.frozen = Some(state.live_alphas(now));
        state.outcome = Some(outcome);
        if state.dragging {
            state.dragging = false;
            let _ = ReleaseCapture();
        }
        state.closing_at = Some(now);
        state.refresh();
        invalidate_all(state);
    }

    unsafe fn invalidate_all(state: &SelectorState) {
        for hwnd in state.windows.iter().copied() {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    unsafe fn paint(hwnd: HWND, data: &mut WindowData, state: &SelectorState) {
        let mut paint: PAINTSTRUCT = zeroed();
        let hdc = BeginPaint(hwnd, &mut paint);
        if hdc.is_invalid() {
            return;
        }
        let monitor = data.monitor;
        let width = i32::try_from(monitor.width_i64()).unwrap_or(0);
        let height = i32::try_from(monitor.height_i64()).unwrap_or(0);
        let dpi = GetDpiForWindow(hwnd);
        // Не получилось создать буфер — рисуем прямо в окно: мерцание лучше пустого экрана.
        let target = data.buffer_dc(hdc, width, height).unwrap_or(hdc);
        draw_frame(target, monitor, state, width, height, dpi);
        if target != hdc {
            let _ = BitBlt(hdc, 0, 0, width, height, Some(target), 0, 0, SRCCOPY);
        }
        let _ = EndPaint(hwnd, &paint);
    }

    /// Порядок кадра: снимок → затемнение → вырез выделения → рамка → бейдж → подсказка.
    unsafe fn draw_frame(
        hdc: HDC,
        monitor: RectI,
        state: &SelectorState,
        width: i32,
        height: i32,
        dpi: u32,
    ) {
        let alphas = state.alphas;
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
        if alphas.dim > 0 {
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alphas.dim,
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
        }

        let selection = match (state.anchor, state.cursor) {
            (Some(anchor), Some(cursor)) => Some(RectI::from_drag(anchor, cursor)),
            _ => None,
        };
        let smooth = gdiplus_ready();
        // Без GDI+ скруглять нечем: ступеньки региона нечему прятать, режем прямым углом.
        let radius = if smooth {
            scaled(SELECTION_RADIUS, dpi)
        } else {
            0
        };
        let visible = selection.and_then(|rect| rect.intersect(monitor));
        if let (Some(selection), Some(visible)) = (selection, visible) {
            let clip = if radius > 0 {
                ScopedRegion::round_rect(selection.relative_to(monitor), radius)
            } else {
                None
            };
            if let Some(clip) = &clip {
                SelectClipRgn(hdc, Some(clip.0));
            }
            let _ = BitBlt(
                hdc,
                visible.left - monitor.left,
                visible.top - monitor.top,
                i32::try_from(visible.width_i64()).unwrap_or(0),
                i32::try_from(visible.height_i64()).unwrap_or(0),
                Some(state.snapshot.dc),
                visible.left - state.desktop.left,
                visible.top - state.desktop.top,
                SRCCOPY,
            );
            if clip.is_some() {
                SelectClipRgn(hdc, None);
            }
        }

        let graphics = if smooth {
            GpGraphicsRef::create(hdc)
        } else {
            None
        };
        let Some(graphics) = graphics else {
            draw_frame_gdi(hdc, monitor, selection, visible, alphas, width, height, dpi);
            return;
        };
        if alphas.border > 0 && visible.is_some() {
            if let Some(selection) = selection {
                draw_selection_border(
                    graphics.0,
                    selection.relative_to(monitor),
                    dpi,
                    alpha_of(alphas.border),
                );
            }
        }
        if alphas.badge > 0 {
            if let (Some(selection), Some(visible)) = (selection, visible) {
                draw_size_badge(
                    graphics.0,
                    monitor,
                    selection,
                    visible,
                    BadgeLimits {
                        width,
                        height,
                        dpi,
                        alpha: alpha_of(alphas.badge),
                    },
                );
            }
        }
        if alphas.hint > 0 {
            draw_hint(graphics.0, width, dpi, alpha_of(alphas.hint));
        }
    }

    // ------------------------------------------------------------------ GDI+

    /// GDI+ поднимается один раз на процесс: селектор открывают многократно, а пара
    /// Startup/Shutdown на каждый показ стоит дороже одного живого токена.
    static GDIPLUS: OnceLock<bool> = OnceLock::new();

    fn gdiplus_ready() -> bool {
        *GDIPLUS.get_or_init(|| unsafe {
            let input = GdiplusStartupInput {
                GdiplusVersion: 1,
                ..Default::default()
            };
            let mut token: usize = 0;
            let status = GdiplusStartup(&mut token, &input, null_mut());
            if status != GDIP_OK {
                eprintln!("GDI+ не поднялся ({status:?}); оверлей рисуется через GDI");
                return false;
            }
            true
        })
    }

    /// ARGB для GDI+ из доли непрозрачности и 0xRRGGBB.
    fn argb(alpha: f32, rgb: u32) -> u32 {
        let alpha = u32::from(to_byte(alpha));
        (alpha << 24) | (rgb & 0x00_ff_ff_ff)
    }

    /// Прямоугольник со скруглением в координатах окна.
    #[derive(Debug, Clone, Copy)]
    struct RoundBox {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radius: f32,
    }

    struct GpGraphicsRef(*mut GpGraphics);

    impl GpGraphicsRef {
        unsafe fn create(hdc: HDC) -> Option<Self> {
            let mut raw: *mut GpGraphics = null_mut();
            if GdipCreateFromHDC(hdc, &mut raw) != GDIP_OK || raw.is_null() {
                return None;
            }
            let graphics = Self(raw);
            let _ = GdipSetSmoothingMode(raw, SmoothingModeAntiAlias);
            // Поверхность непрозрачная (снимок рабочего стола), поэтому ClearType уместен.
            let _ = GdipSetTextRenderingHint(raw, TextRenderingHintClearTypeGridFit);
            Some(graphics)
        }
    }

    impl Drop for GpGraphicsRef {
        fn drop(&mut self) {
            unsafe {
                let _ = GdipDeleteGraphics(self.0);
            }
        }
    }

    struct GpPathRef(*mut GpPath);

    impl GpPathRef {
        unsafe fn round_rect(shape: RoundBox) -> Option<Self> {
            let mut raw: *mut GpPath = null_mut();
            if GdipCreatePath(FillModeAlternate, &mut raw) != GDIP_OK || raw.is_null() {
                return None;
            }
            let path = Self(raw);
            let RoundBox {
                x,
                y,
                width,
                height,
                radius,
            } = shape;
            if width <= 0.0 || height <= 0.0 {
                return Some(path);
            }
            let radius = radius.min(width / 2.0).min(height / 2.0).max(0.0);
            if radius < 0.5 {
                let _ = GdipAddPathLine(raw, x, y, x + width, y);
                let _ = GdipAddPathLine(raw, x + width, y, x + width, y + height);
                let _ = GdipAddPathLine(raw, x + width, y + height, x, y + height);
            } else {
                let side = radius * 2.0;
                let _ = GdipAddPathArc(raw, x, y, side, side, 180.0, 90.0);
                let _ = GdipAddPathArc(raw, x + width - side, y, side, side, 270.0, 90.0);
                let _ = GdipAddPathArc(
                    raw,
                    x + width - side,
                    y + height - side,
                    side,
                    side,
                    0.0,
                    90.0,
                );
                let _ = GdipAddPathArc(raw, x, y + height - side, side, side, 90.0, 90.0);
            }
            let _ = GdipClosePathFigure(raw);
            Some(path)
        }
    }

    impl Drop for GpPathRef {
        fn drop(&mut self) {
            unsafe {
                let _ = GdipDeletePath(self.0);
            }
        }
    }

    struct GpBrushRef(*mut GpBrush);

    impl GpBrushRef {
        unsafe fn solid(color: u32) -> Option<Self> {
            let mut raw: *mut GpSolidFill = null_mut();
            if GdipCreateSolidFill(color, &mut raw) != GDIP_OK || raw.is_null() {
                return None;
            }
            Some(Self(raw.cast()))
        }
    }

    impl Drop for GpBrushRef {
        fn drop(&mut self) {
            unsafe {
                let _ = GdipDeleteBrush(self.0);
            }
        }
    }

    struct GpPenRef(*mut GpPen);

    impl GpPenRef {
        unsafe fn solid(color: u32, width: f32) -> Option<Self> {
            let mut raw: *mut GpPen = null_mut();
            if GdipCreatePen1(color, width.max(0.5), UnitPixel, &mut raw) != GDIP_OK
                || raw.is_null()
            {
                return None;
            }
            let pen = Self(raw);
            let _ = GdipSetPenLineJoin(raw, LineJoinRound);
            let _ = GdipSetPenStartCap(raw, LineCapRound);
            let _ = GdipSetPenEndCap(raw, LineCapRound);
            Some(pen)
        }
    }

    impl Drop for GpPenRef {
        fn drop(&mut self) {
            unsafe {
                let _ = GdipDeletePen(self.0);
            }
        }
    }

    /// UTF-16 с нулём на конце: GDI+ берёт длину отдельно, но нуль спасает от чужих ошибок.
    struct Utf16(Vec<u16>);

    impl Utf16 {
        fn new(text: &str) -> Self {
            Self(text.encode_utf16().chain(std::iter::once(0)).collect())
        }

        fn ptr(&self) -> PCWSTR {
            PCWSTR(self.0.as_ptr())
        }

        fn len(&self) -> i32 {
            i32::try_from(self.0.len().saturating_sub(1)).unwrap_or(i32::MAX)
        }
    }

    /// Шрифт GDI+ вместе с семейством и форматом строки.
    struct GpTextStyle {
        family: *mut GpFontFamily,
        font: *mut GpFont,
        format: *mut GpStringFormat,
    }

    impl GpTextStyle {
        /// `points` — кегль в логических пунктах; в пиксели переводит DPI окна, а не DC,
        /// иначе на втором мониторе шрифт уезжал бы.
        unsafe fn create(family_name: PCWSTR, points: f32, dpi: u32) -> Option<Self> {
            let mut family: *mut GpFontFamily = null_mut();
            if GdipCreateFontFamilyFromName(family_name, null_mut(), &mut family) != GDIP_OK
                || family.is_null()
            {
                // Начертания Semibold может не быть — обычный Segoe UI есть всегда.
                family = null_mut();
                if GdipCreateFontFamilyFromName(w!("Segoe UI"), null_mut(), &mut family) != GDIP_OK
                    || family.is_null()
                {
                    return None;
                }
            }
            let em = points * dpi.max(1) as f32 / 72.0;
            let mut font: *mut GpFont = null_mut();
            if GdipCreateFont(family, em, FontStyleRegular.0, UnitPixel, &mut font) != GDIP_OK
                || font.is_null()
            {
                let _ = GdipDeleteFontFamily(family);
                return None;
            }
            // Типографские метрики: generic default добавляет по em-полю с каждой стороны,
            // и пилюля от них раздувается. Клон, а не сам generic: его удалять нельзя.
            let mut generic: *mut GpStringFormat = null_mut();
            let mut format: *mut GpStringFormat = null_mut();
            if GdipStringFormatGetGenericTypographic(&mut generic) == GDIP_OK && !generic.is_null()
            {
                let _ = GdipCloneStringFormat(generic, &mut format);
            }
            if format.is_null()
                && (GdipCreateStringFormat(0, 0, &mut format) != GDIP_OK || format.is_null())
            {
                let _ = GdipDeleteFont(font);
                let _ = GdipDeleteFontFamily(family);
                return None;
            }
            let _ = GdipSetStringFormatFlags(
                format,
                StringFormatFlagsNoWrap.0
                    | StringFormatFlagsNoClip.0
                    | StringFormatFlagsMeasureTrailingSpaces.0,
            );
            Some(Self {
                family,
                font,
                format,
            })
        }

        unsafe fn measure(&self, graphics: *mut GpGraphics, text: &Utf16) -> (f32, f32) {
            let layout = RectF {
                X: 0.0,
                Y: 0.0,
                Width: 8192.0,
                Height: 8192.0,
            };
            let mut measured = RectF::default();
            if GdipMeasureString(
                graphics,
                text.ptr(),
                text.len(),
                self.font,
                &layout,
                self.format,
                &mut measured,
                null_mut(),
                null_mut(),
            ) != GDIP_OK
            {
                return (0.0, 0.0);
            }
            (measured.Width, measured.Height)
        }

        unsafe fn draw(&self, graphics: *mut GpGraphics, text: &Utf16, x: f32, y: f32, color: u32) {
            let Some(brush) = GpBrushRef::solid(color) else {
                return;
            };
            let layout = RectF {
                X: x,
                Y: y,
                Width: 8192.0,
                Height: 8192.0,
            };
            let _ = GdipDrawString(
                graphics,
                text.ptr(),
                text.len(),
                self.font,
                &layout,
                self.format,
                brush.0,
            );
        }
    }

    impl Drop for GpTextStyle {
        fn drop(&mut self) {
            unsafe {
                let _ = GdipDeleteStringFormat(self.format);
                let _ = GdipDeleteFont(self.font);
                let _ = GdipDeleteFontFamily(self.family);
            }
        }
    }

    unsafe fn fill_round(graphics: *mut GpGraphics, shape: RoundBox, color: u32) {
        let (Some(path), Some(brush)) = (GpPathRef::round_rect(shape), GpBrushRef::solid(color))
        else {
            return;
        };
        let _ = GdipFillPath(graphics, brush.0, path.0);
    }

    unsafe fn stroke_round(graphics: *mut GpGraphics, shape: RoundBox, color: u32, width: f32) {
        let (Some(path), Some(pen)) = (GpPathRef::round_rect(shape), GpPenRef::solid(color, width))
        else {
            return;
        };
        let _ = GdipDrawPath(graphics, pen.0, path.0);
    }

    /// Рамка выделения: 2 px цвета воды, скругление прячет ступеньки региона выреза.
    unsafe fn draw_selection_border(graphics: *mut GpGraphics, rect: RectI, dpi: u32, alpha: f32) {
        let shape = RoundBox {
            x: rect.left as f32,
            y: rect.top as f32,
            width: rect.width_i64() as f32,
            height: rect.height_i64() as f32,
            radius: scaled(SELECTION_RADIUS, dpi) as f32,
        };
        stroke_round(
            graphics,
            shape,
            argb(alpha, WATER_RGB),
            scaled(2, dpi) as f32,
        );
    }

    struct BadgeLimits {
        width: i32,
        height: i32,
        dpi: u32,
        alpha: f32,
    }

    /// Бейдж размера у нижнего правого угла выделения; на краю монитора уходит внутрь угла.
    unsafe fn draw_size_badge(
        graphics: *mut GpGraphics,
        monitor: RectI,
        selection: RectI,
        visible: RectI,
        limits: BadgeLimits,
    ) {
        // Бейдж рисует только окно того монитора, где лежит правый нижний угол выделения:
        // иначе выделение через границу экранов показало бы размер дважды.
        let corner = PointI {
            x: selection.right - 1,
            y: selection.bottom - 1,
        };
        if !monitor.contains(corner) {
            return;
        }
        let dpi = limits.dpi;
        let scale = scale_of(dpi);
        let Some(style) = GpTextStyle::create(w!("Segoe UI"), 12.0, dpi) else {
            return;
        };
        let text = Utf16::new(&format!(
            "{} × {}",
            selection.width_i64(),
            selection.height_i64()
        ));
        let (text_width, text_height) = style.measure(graphics, &text);
        let height = 24.0 * scale;
        let padding = 10.0 * scale;
        let gap = 8.0 * scale;
        let chip_width = text_width + padding * 2.0;
        let anchor_right = (visible.right - monitor.left) as f32;
        let anchor_bottom = (visible.bottom - monitor.top) as f32;
        let outside = anchor_bottom + gap;
        let top = if outside + height <= limits.height as f32 {
            outside
        } else {
            (anchor_bottom - gap - height).max(0.0)
        };
        let left = (anchor_right - chip_width)
            .min(limits.width as f32 - chip_width)
            .max(0.0);
        let shape = RoundBox {
            x: left,
            y: top,
            width: chip_width,
            height,
            radius: height / 2.0,
        };
        fill_round(graphics, shape, argb(0.86 * limits.alpha, INK_RGB));
        style.draw(
            graphics,
            &text,
            left + padding,
            top + (height - text_height) / 2.0,
            argb(limits.alpha, TEXT_RGB),
        );
    }

    /// Подсказка направления «Тихий»: иконка рамки, текст, кейкап Esc и слово «отмена».
    unsafe fn draw_hint(graphics: *mut GpGraphics, width: i32, dpi: u32, alpha: f32) {
        let scale = scale_of(dpi);
        let (Some(label_style), Some(key_style)) = (
            GpTextStyle::create(w!("Segoe UI Semibold"), 12.0, dpi),
            GpTextStyle::create(w!("Segoe UI Semibold"), 11.0, dpi),
        ) else {
            return;
        };
        let label = Utf16::new("Выделите область с текстом");
        let key = Utf16::new("Esc");
        let tail = Utf16::new("отмена");
        let (label_width, label_height) = label_style.measure(graphics, &label);
        let (key_width, key_height) = key_style.measure(graphics, &key);
        let (tail_width, tail_height) = label_style.measure(graphics, &tail);

        let height = 32.0 * scale;
        let padding = 14.0 * scale;
        let gap = 9.0 * scale;
        let icon = 14.0 * scale;
        let key_padding = 7.0 * scale;
        let key_box_width = key_width + key_padding * 2.0;
        let key_box_height = 20.0 * scale;
        let total =
            padding * 2.0 + icon + gap + label_width + gap + key_box_width + gap + tail_width;
        let left = ((width as f32 - total) / 2.0).max(0.0);
        let top = 16.0 * scale;

        fill_round(
            graphics,
            RoundBox {
                x: left,
                y: top,
                width: total,
                height,
                radius: height / 2.0,
            },
            argb(0.86 * alpha, INK_RGB),
        );

        let mut x = left + padding;
        draw_screen_icon(
            graphics,
            x,
            top + (height - icon) / 2.0,
            icon,
            argb(alpha, TEXT_RGB),
        );
        x += icon + gap;
        label_style.draw(
            graphics,
            &label,
            x,
            top + (height - label_height) / 2.0,
            argb(alpha, TEXT_RGB),
        );
        x += label_width + gap;
        let key_top = top + (height - key_box_height) / 2.0;
        let key_shape = RoundBox {
            x,
            y: key_top,
            width: key_box_width,
            height: key_box_height,
            radius: 6.0 * scale,
        };
        fill_round(graphics, key_shape, argb(0.10 * alpha, WHITE_RGB));
        stroke_round(
            graphics,
            key_shape,
            argb(0.14 * alpha, WHITE_RGB),
            scale.max(1.0),
        );
        key_style.draw(
            graphics,
            &key,
            x + key_padding,
            key_top + (key_box_height - key_height) / 2.0,
            argb(alpha, TEXT_RGB),
        );
        x += key_box_width + gap;
        label_style.draw(
            graphics,
            &tail,
            x,
            top + (height - tail_height) / 2.0,
            argb(alpha, MUTED_RGB),
        );
    }

    unsafe fn draw_screen_icon(graphics: *mut GpGraphics, x: f32, y: f32, size: f32, color: u32) {
        let unit = size / 16.0;
        let Some(pen) = GpPenRef::solid(color, 1.5 * unit) else {
            return;
        };
        for corner in SCREEN_CORNERS {
            let points = corner.map(|(px, py)| PointF {
                X: x + px * unit,
                Y: y + py * unit,
            });
            let _ = GdipDrawLines(graphics, pen.0, points.as_ptr(), 3);
        }
    }

    // ------------------------------------------------------------------ запасной GDI

    /// GDI+ не поднялся: те же слои, но без скруглений и без альфы. Полупрозрачность
    /// заменяет порог — пилюля либо есть, либо нет.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_frame_gdi(
        hdc: HDC,
        monitor: RectI,
        selection: Option<RectI>,
        visible: Option<RectI>,
        alphas: Alphas,
        width: i32,
        height: i32,
        dpi: u32,
    ) {
        if let (Some(visible), true) = (visible, alphas.border > 128) {
            draw_border_gdi(hdc, monitor, visible, dpi);
        }
        if alphas.badge > 128 {
            if let (Some(selection), Some(visible)) = (selection, visible) {
                draw_size_badge_gdi(hdc, monitor, selection, visible, width, height, dpi);
            }
        }
        if alphas.hint > 128 {
            if let Some(font) = ScopedFont::create(13, dpi) {
                draw_pill(
                    hdc,
                    font.font,
                    HINT,
                    PillAnchor::TopCenter {
                        center_x: width / 2,
                        top: scaled(16, dpi),
                    },
                    dpi,
                );
            }
        }
    }

    unsafe fn draw_size_badge_gdi(
        hdc: HDC,
        monitor: RectI,
        selection: RectI,
        visible: RectI,
        width: i32,
        height: i32,
        dpi: u32,
    ) {
        let corner = PointI {
            x: selection.right - 1,
            y: selection.bottom - 1,
        };
        if !monitor.contains(corner) {
            return;
        }
        let Some(font) = ScopedFont::create(11, dpi) else {
            return;
        };
        let label = format!("{} × {}", selection.width_i64(), selection.height_i64());
        draw_pill(
            hdc,
            font.font,
            &label,
            PillAnchor::BottomRight {
                right: visible.right - monitor.left,
                bottom: visible.bottom - monitor.top,
                gap: scaled(8, dpi),
                limit_width: width,
                limit_height: height,
            },
            dpi,
        );
    }

    enum PillAnchor {
        TopCenter {
            center_x: i32,
            top: i32,
        },
        BottomRight {
            right: i32,
            bottom: i32,
            gap: i32,
            limit_width: i32,
            limit_height: i32,
        },
    }

    /// Пилюля палитры «мягкое бенто» на GDI: скругление в высоту, ровная тёмная заливка.
    unsafe fn draw_pill(hdc: HDC, font: HFONT, text: &str, anchor: PillAnchor, dpi: u32) {
        let old_font = SelectObject(hdc, HGDIOBJ(font.0));
        let mut measured: Vec<u16> = text.encode_utf16().collect();
        let mut measure = RECT::default();
        let _ = DrawTextW(
            hdc,
            &mut measured,
            &mut measure,
            DT_CALCRECT | DT_SINGLELINE | DT_LEFT,
        );
        let pad_x = scaled(14, dpi);
        let pad_y = scaled(8, dpi);
        let pill_w = measure.right - measure.left + pad_x * 2;
        let pill_h = measure.bottom - measure.top + pad_y * 2;
        let (left, top) = match anchor {
            PillAnchor::TopCenter { center_x, top } => ((center_x - pill_w / 2).max(0), top),
            PillAnchor::BottomRight {
                right,
                bottom,
                gap,
                limit_width,
                limit_height,
            } => {
                let outside_top = bottom + gap;
                let top = if outside_top + pill_h <= limit_height {
                    outside_top
                } else {
                    bottom - gap - pill_h
                };
                (
                    (right - pill_w).min(limit_width - pill_w).max(0),
                    top.max(0),
                )
            }
        };

        let brush = CreateSolidBrush(PILL_BG);
        if brush.is_invalid() {
            if !old_font.is_invalid() {
                let _ = SelectObject(hdc, old_font);
            }
            return;
        }
        let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
        let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
        let _ = RoundRect(hdc, left, top, left + pill_w, top + pill_h, pill_h, pill_h);
        if !old_pen.is_invalid() {
            let _ = SelectObject(hdc, old_pen);
        }
        if !old_brush.is_invalid() {
            let _ = SelectObject(hdc, old_brush);
        }
        let _ = DeleteObject(HGDIOBJ(brush.0));

        let old_mode = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, PILL_TEXT);
        let mut drawn: Vec<u16> = text.encode_utf16().collect();
        let mut text_rect = RECT {
            left: left + pad_x,
            top: top + pad_y,
            right: left + pill_w - pad_x,
            bottom: top + pill_h - pad_y,
        };
        let _ = DrawTextW(
            hdc,
            &mut drawn,
            &mut text_rect,
            DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOCLIP,
        );
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

    unsafe fn draw_border_gdi(hdc: HDC, monitor: RectI, visible: RectI, dpi: u32) {
        let pen = CreatePen(PS_SOLID, scaled(2, dpi), WATER);
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
    fn selection_moves_into_window_coordinates_without_overflow() {
        assert_eq!(
            rect(100, 50, 300, 250).relative_to(rect(100, 50, 1000, 900)),
            rect(0, 0, 200, 200)
        );
        // Выделение на соседнем мониторе: координаты уходят в минус, но не переполняются.
        assert_eq!(
            rect(i32::MIN, i32::MIN, -10, -10).relative_to(rect(0, 0, 100, 100)),
            rect(i32::MIN, i32::MIN, -10, -10)
        );
    }

    #[test]
    fn crop_converts_bgra_to_opaque_rgba_without_flipping_rows() {
        // Top row: red, green. Bottom row: blue, white. Alpha bytes are intentionally varied.
        let pixels = [0, 0, 255, 0, 0, 255, 0, 17, 255, 0, 0, 99, 255, 255, 255, 1];
        let result = crop_bgra_top_down(&pixels, rect(-1, -1, 1, 1), rect(-1, -1, 1, 1)).unwrap();
        assert_eq!(result.width, 2);
        assert_eq!(result.height, 2);
        assert_eq!((result.left, result.top), (-1, -1));
        assert_eq!(
            result.rgba,
            [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,]
        );
    }

    #[test]
    fn crop_rejects_short_source_buffer() {
        let error = crop_bgra_top_down(&[0; 15], rect(0, 0, 2, 2), rect(0, 0, 2, 2)).unwrap_err();
        assert!(error.contains("короче"));
    }

    #[test]
    fn easing_stays_inside_the_unit_interval() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(-5.0), 0.0);
        assert_eq!(ease_out_cubic(5.0), 1.0);
        // Ease-out: половина времени даёт заметно больше половины пути.
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    #[test]
    fn dimming_eases_out_and_stops_at_the_final_alpha() {
        assert_eq!(dim_alpha(0, false), 0);
        assert_eq!(dim_alpha(DIM_IN_MS, false), DIM_ALPHA);
        assert_eq!(dim_alpha(DIM_IN_MS * 5, false), DIM_ALPHA);
        for elapsed in 0..=(DIM_IN_MS * 3) {
            assert!(dim_alpha(elapsed, false) <= DIM_ALPHA);
        }
        let half = dim_alpha(DIM_IN_MS / 2, false);
        assert!(half > DIM_ALPHA / 2 && half < DIM_ALPHA);
        assert!(dim_alpha(10, false) < dim_alpha(20, false));
    }

    #[test]
    fn hint_holds_after_the_first_move_and_then_fades() {
        // До движения подсказка появляется вместе с затемнением и остаётся.
        assert!(hint_alpha(0, None, None, false) < 0.1);
        assert_eq!(hint_alpha(DIM_IN_MS, None, None, false), 1.0);
        assert_eq!(hint_alpha(10_000, None, None, false), 1.0);

        let moved = Some(1_000);
        assert_eq!(
            hint_alpha(1_000 + HINT_HOLD_MS - 1, moved, None, false),
            1.0
        );
        let mid = hint_alpha(1_000 + HINT_HOLD_MS + HINT_OUT_MS / 2, moved, None, false);
        assert!(mid > 0.0 && mid < 1.0);
        assert_eq!(
            hint_alpha(1_000 + HINT_HOLD_MS + HINT_OUT_MS, moved, None, false),
            0.0
        );
        assert_eq!(hint_alpha(60_000, moved, None, false), 0.0);
    }

    #[test]
    fn pressing_the_button_hides_the_hint_earlier_than_the_hold() {
        let moved = Some(1_000);
        let pressed = Some(1_200);
        // Нажатие раньше, чем истекли 1,5 с: гаснем от него.
        assert_eq!(hint_alpha(1_200, moved, pressed, false), 1.0);
        assert_eq!(hint_alpha(1_200 + HINT_OUT_MS, moved, pressed, false), 0.0);
        // Без движения нажатие тоже гасит подсказку.
        assert_eq!(hint_alpha(500 + HINT_OUT_MS, None, Some(500), false), 0.0);
    }

    #[test]
    fn badge_waits_for_a_still_cursor_and_vanishes_on_movement() {
        // Не в протяжке бейджа нет вообще.
        assert_eq!(badge_alpha(10_000, false, 0, false), 0.0);
        // Курсор движется — покоя нет.
        assert_eq!(badge_alpha(1_000, true, 1_000, false), 0.0);
        assert_eq!(
            badge_alpha(1_000 + BADGE_IDLE_MS - 1, true, 1_000, false),
            0.0
        );
        // Пауза выдержана — бейдж набирает за 120 мс.
        let start = badge_alpha(1_000 + BADGE_IDLE_MS, true, 1_000, false);
        assert_eq!(start, 0.0);
        let mid = badge_alpha(1_000 + BADGE_IDLE_MS + BADGE_IN_MS / 2, true, 1_000, false);
        assert!(mid > 0.0 && mid < 1.0);
        assert_eq!(
            badge_alpha(1_000 + BADGE_IDLE_MS + BADGE_IN_MS, true, 1_000, false),
            1.0
        );
        // Новое движение сбрасывает отсчёт: бейдж исчезает мгновенно.
        assert_eq!(badge_alpha(5_000, true, 5_000, false), 0.0);
    }

    #[test]
    fn exit_takes_ninety_milliseconds_and_saturates() {
        assert_eq!(exit_progress(1_000, None, false), 0.0);
        assert_eq!(exit_progress(1_000, Some(1_000), false), 0.0);
        let mid = exit_progress(1_000 + EXIT_MS / 2, Some(1_000), false);
        assert!(mid > 0.0 && mid < 1.0);
        assert_eq!(exit_progress(1_000 + EXIT_MS, Some(1_000), false), 1.0);
        assert_eq!(exit_progress(9_000, Some(1_000), false), 1.0);
        // Выход быстрее входа: к 90 мс он закончен, а затемнение в эту точку ещё набирает.
        assert!(dim_alpha(EXIT_MS, false) < DIM_ALPHA);
    }

    #[test]
    fn reduced_motion_jumps_straight_to_the_final_values() {
        assert_eq!(dim_alpha(0, true), DIM_ALPHA);
        assert_eq!(hint_alpha(0, None, None, true), 1.0);
        assert_eq!(hint_alpha(1_000, Some(0), None, true), 1.0);
        assert_eq!(hint_alpha(HINT_HOLD_MS, Some(0), None, true), 0.0);
        assert_eq!(hint_alpha(5, None, Some(5), true), 0.0);
        assert_eq!(badge_alpha(0, true, 0, true), 0.0);
        assert_eq!(badge_alpha(BADGE_IDLE_MS, true, 0, true), 1.0);
        assert_eq!(exit_progress(0, Some(0), true), 1.0);
    }
}
