//! Захват выделенного текста и безопасная вставка через Ctrl+C / Ctrl+V.
//! Clipboard сохраняется целиком: если какой-либо формат нельзя клонировать,
//! синтетический ввод не отправляется.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use tauri::{AppHandle, Manager, WebviewWindow};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND},
        Graphics::Gdi::{DeleteEnhMetaFile, DeleteObject, HENHMETAFILE, HGDIOBJ},
        System::{
            DataExchange::{
                CloseClipboard, CountClipboardFormats, EmptyClipboard, EnumClipboardFormats,
                GetClipboardData, GetClipboardOwner, GetClipboardSequenceNumber, OpenClipboard,
                SetClipboardData,
            },
            Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
        },
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, GetLastInputInfo, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD,
                KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, LASTINPUTINFO, VIRTUAL_KEY, VK_C,
                VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
            },
            WindowsAndMessaging::{
                CopyImage, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId,
                IsWindow, SetForegroundWindow, GUITHREADINFO, IMAGE_BITMAP, LR_CREATEDIBSECTION,
            },
        },
    },
};

const COPY_TIMEOUT: Duration = Duration::from_millis(1500);
const CLIPBOARD_OPEN_TIMEOUT: Duration = Duration::from_millis(250);
const MODIFIERS: [VIRTUAL_KEY; 5] = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN];
const CF_BITMAP_ID: u32 = 2;
const CF_METAFILEPICT_ID: u32 = 3;
const CF_PALETTE_ID: u32 = 9;
const CF_UNICODETEXT_ID: u32 = 13;
const CF_ENHMETAFILE_ID: u32 = 14;
const CF_DSPBITMAP_ID: u32 = 0x82;
const CF_DSPMETAFILEPICT_ID: u32 = 0x83;
const CF_DSPENHMETAFILE_ID: u32 = 0x8e;
const CF_PRIVATEFIRST: u32 = 0x0200;
const CF_PRIVATELAST: u32 = 0x02ff;
const CF_GDIOBJFIRST: u32 = 0x0300;
const CF_GDIOBJLAST: u32 = 0x03ff;

static MARKER_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputContext {
    foreground: isize,
    focus: isize,
    process_id: u32,
    last_input: u32,
}

pub struct Captured {
    pub text: Option<String>,
    pub context: InputContext,
    /// Оставлено в протоколе для совместимости; безопасный захват либо восстанавливает
    /// весь clipboard, либо возвращает ошибку.
    pub clipboard_replaced: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteOutcome {
    ClipboardRestored,
    ClipboardPreserved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FormatKind {
    Global,
    Bitmap,
    EnhMetaFile,
    Unsupported,
}

fn format_kind(format: u32) -> FormatKind {
    match format {
        CF_BITMAP_ID | CF_DSPBITMAP_ID => FormatKind::Bitmap,
        CF_ENHMETAFILE_ID | CF_DSPENHMETAFILE_ID => FormatKind::EnhMetaFile,
        CF_METAFILEPICT_ID | CF_DSPMETAFILEPICT_ID | CF_PALETTE_ID => FormatKind::Unsupported,
        CF_PRIVATEFIRST..=CF_PRIVATELAST | CF_GDIOBJFIRST..=CF_GDIOBJLAST => {
            FormatKind::Unsupported
        }
        // Обычные текстовые, DIB/DIBV5, HDROP и зарегистрированные HTML/RTF/PNG
        // форматы используют перемещаемый HGLOBAL.
        _ => FormatKind::Global,
    }
}

/// Один тип на оба состояния формата: `Bytes` — снятая копия HGLOBAL, остальные варианты —
/// готовые к `SetClipboardData` хендлы. Drop освобождает всё, что не успели передать системе.
enum FormatData {
    Bytes(Vec<u8>),
    Global(Option<HGLOBAL>),
    Bitmap(Option<HANDLE>),
    EnhMetaFile(Option<HENHMETAFILE>),
}

impl FormatData {
    fn handle(&self) -> HANDLE {
        match self {
            Self::Global(Some(h)) => HANDLE(h.0),
            Self::Bitmap(Some(h)) => *h,
            Self::EnhMetaFile(Some(h)) => HANDLE(h.0),
            _ => HANDLE::default(),
        }
    }

    fn transferred(&mut self) {
        match self {
            Self::Bytes(_) => {}
            Self::Global(h) => *h = None,
            Self::Bitmap(h) => *h = None,
            Self::EnhMetaFile(h) => *h = None,
        }
    }
}

impl Drop for FormatData {
    fn drop(&mut self) {
        unsafe {
            match self {
                Self::Global(Some(h)) => {
                    let _ = GlobalFree(Some(*h));
                }
                Self::Bitmap(Some(h)) => {
                    let _ = DeleteObject(HGDIOBJ(h.0));
                }
                Self::EnhMetaFile(Some(h)) => {
                    let _ = DeleteEnhMetaFile(Some(*h));
                }
                _ => {}
            }
        }
    }
}

struct ClipboardFormat {
    id: u32,
    data: FormatData,
}

struct ClipboardSnapshot {
    formats: Vec<ClipboardFormat>,
    sequence: u32,
}

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn open_clipboard(owner: Option<HWND>) -> Result<ClipboardGuard, String> {
    let start = Instant::now();
    loop {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return Ok(ClipboardGuard);
        }
        if start.elapsed() >= CLIPBOARD_OPEN_TIMEOUT {
            return Err("Буфер обмена занят другой программой".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn app_clipboard_owner(app: &AppHandle) -> Result<HWND, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Главное окно приложения недоступно".to_string())?;
    window
        .hwnd()
        .map(|h| HWND(h.0))
        .map_err(|e| format!("Не удалось получить окно приложения: {e}"))
}

/// Проверка sequence и запись выполняются под одним OpenClipboard, поэтому более
/// новое содержимое другого приложения не может попасть между ними.
fn write_text_owned(text: &str, expected_sequence: u32, owner: HWND) -> Result<u32, String> {
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes_len = utf16.len() * std::mem::size_of::<u16>();
    let global = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes_len) }
        .map_err(|_| "Не хватает памяти для записи в буфер обмена".to_string())?;
    let mut prepared = FormatData::Global(Some(global));
    let ptr = unsafe { GlobalLock(global) };
    if ptr.is_null() {
        return Err("Не удалось подготовить текст для буфера обмена".into());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(utf16.as_ptr().cast::<u8>(), ptr.cast::<u8>(), bytes_len);
        let _ = GlobalUnlock(global);
    }

    let _guard = open_clipboard(Some(owner))?;
    if unsafe { GetClipboardSequenceNumber() } != expected_sequence {
        return Err("Буфер обмена изменился; операция отменена".into());
    }
    unsafe { EmptyClipboard() }.map_err(|e| format!("Не удалось очистить буфер обмена: {e}"))?;
    unsafe { SetClipboardData(CF_UNICODETEXT_ID, Some(prepared.handle())) }
        .map_err(|e| format!("Не удалось записать текст в буфер обмена: {e}"))?;
    prepared.transferred();
    Ok(unsafe { GetClipboardSequenceNumber() })
}

/// Снимает один формат буфера. `None` — формат нельзя безопасно скопировать (отложенная
/// отрисовка, пустой размер, неподдерживаемый вид): такой формат пропускается, чтобы не
/// срывать перевод из-за содержимого чужого буфера, которое мы всё равно не смогли бы вернуть.
fn capture_format(id: u32) -> Option<FormatData> {
    let handle = unsafe { GetClipboardData(id) }.ok()?;
    match format_kind(id) {
        FormatKind::Global => {
            let global = HGLOBAL(handle.0);
            let size = unsafe { GlobalSize(global) };
            if size == 0 {
                return None;
            }
            let ptr = unsafe { GlobalLock(global) };
            if ptr.is_null() {
                return None;
            }
            let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), size) }.to_vec();
            unsafe {
                let _ = GlobalUnlock(global);
            }
            Some(FormatData::Bytes(bytes))
        }
        FormatKind::Bitmap => {
            let copy = unsafe { CopyImage(handle, IMAGE_BITMAP, 0, 0, LR_CREATEDIBSECTION) }.ok()?;
            Some(FormatData::Bitmap(Some(copy)))
        }
        FormatKind::EnhMetaFile => {
            let copy = unsafe {
                windows::Win32::Graphics::Gdi::CopyEnhMetaFileW(
                    HENHMETAFILE(handle.0),
                    PCWSTR::null(),
                )
            };
            if copy.0.is_null() {
                return None;
            }
            Some(FormatData::EnhMetaFile(Some(copy)))
        }
        FormatKind::Unsupported => None,
    }
}

impl ClipboardSnapshot {
    fn capture() -> Result<Self, String> {
        let _guard = open_clipboard(None)?;
        let count = unsafe { CountClipboardFormats() };
        if count < 0 {
            return Err("Не удалось прочитать форматы буфера обмена".into());
        }
        let mut formats = Vec::with_capacity(count as usize);
        let mut seen = 0usize;
        let mut id = 0;
        loop {
            id = unsafe { EnumClipboardFormats(id) };
            if id == 0 {
                break;
            }
            seen += 1;
            // Возврат буфера — вежливость, а не условие перевода: формат, который нельзя
            // снять (отложенная отрисовка, пустой, неподдерживаемый), пропускаем, иначе одна
            // экзотика в чужом буфере сорвала бы весь захват выделения.
            if let Some(data) = capture_format(id) {
                formats.push(ClipboardFormat { id, data });
            }
        }
        if seen != count as usize {
            return Err("Список форматов буфера обмена изменился во время чтения".into());
        }
        let sequence = unsafe { GetClipboardSequenceNumber() };
        Ok(Self { formats, sequence })
    }

    fn prepare(mut self) -> Result<Vec<ClipboardFormat>, String> {
        let mut prepared = Vec::with_capacity(self.formats.len());
        for mut item in self.formats.drain(..) {
            let data = match &mut item.data {
                FormatData::Bytes(bytes) => {
                    let bytes = std::mem::take(bytes);
                    let global =
                        unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }.map_err(|_| {
                            "Не хватает памяти для восстановления буфера обмена".to_string()
                        })?;
                    let ptr = unsafe { GlobalLock(global) };
                    if ptr.is_null() {
                        unsafe {
                            let _ = GlobalFree(Some(global));
                        }
                        return Err("Не удалось подготовить буфер обмена к восстановлению".into());
                    }
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            bytes.as_ptr(),
                            ptr.cast::<u8>(),
                            bytes.len(),
                        );
                        let _ = GlobalUnlock(global);
                    }
                    FormatData::Global(Some(global))
                }
                FormatData::Global(handle) => FormatData::Global(handle.take()),
                FormatData::Bitmap(handle) => FormatData::Bitmap(handle.take()),
                FormatData::EnhMetaFile(handle) => FormatData::EnhMetaFile(handle.take()),
            };
            prepared.push(ClipboardFormat { id: item.id, data });
        }
        Ok(prepared)
    }

    /// Единственный путь восстановления. `expected_text` задаётся только после отправленного
    /// Ctrl+V: системный синтез форматов может увеличить sequence, не меняя owner и текст, и
    /// такое своё состояние перезаписать можно. Чужую запись не трогаем в обоих случаях.
    fn restore(
        self,
        expected_sequence: u32,
        owner: HWND,
        expected_text: Option<&str>,
    ) -> Result<(), String> {
        let mut prepared = self.prepare()?;
        let _guard = open_clipboard(Some(owner))?;
        let current_sequence = unsafe { GetClipboardSequenceNumber() };
        if current_sequence != expected_sequence {
            let (owner_matches, text_matches) = match expected_text {
                Some(text) => (
                    unsafe { GetClipboardOwner() }.ok() == Some(owner),
                    read_unicode_text_from_open_clipboard().as_deref() == Some(text),
                ),
                None => (false, false),
            };
            if clipboard_restore_decision(
                expected_sequence,
                current_sequence,
                owner_matches,
                text_matches,
            ) == ClipboardRestoreDecision::PreserveExternal
            {
                return Err("Буфер обмена изменился; новое содержимое сохранено".into());
            }
        }
        unsafe { EmptyClipboard() }
            .map_err(|e| format!("Не удалось очистить буфер для восстановления: {e}"))?;
        for item in &mut prepared {
            unsafe { SetClipboardData(item.id, Some(item.data.handle())) }.map_err(|e| {
                format!(
                    "Не удалось восстановить формат буфера обмена {}: {e}",
                    item.id
                )
            })?;
            item.data.transferred();
        }
        Ok(())
    }

    /// После отправленного Ctrl+V ошибка восстановления уже не означает, что вставка
    /// не ушла: вызывающей стороне важен только факт, вернулся буфер или нет.
    fn restore_after_paste(
        self,
        expected_sequence: u32,
        owner: HWND,
        expected_text: &str,
    ) -> PasteOutcome {
        match self.restore(expected_sequence, owner, Some(expected_text)) {
            Ok(()) => PasteOutcome::ClipboardRestored,
            Err(_) => PasteOutcome::ClipboardPreserved,
        }
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send(keys: &[INPUT]) -> Result<(), String> {
    let sent = unsafe { SendInput(keys, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == keys.len() {
        Ok(())
    } else {
        Err("Windows заблокировал синтетический ввод".into())
    }
}

fn modifiers_down() -> bool {
    MODIFIERS
        .iter()
        .any(|vk| unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0)
}

fn wait_modifiers_released() -> Result<(), String> {
    let start = Instant::now();
    while modifiers_down() {
        if start.elapsed() > Duration::from_millis(1500) {
            return Err("Отпустите клавиши хоткея и повторите".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn send_ctrl_chord(vk: VIRTUAL_KEY) -> Result<(), String> {
    send(&[
        key(VK_CONTROL, false),
        key(vk, false),
        key(vk, true),
        key(VK_CONTROL, true),
    ])
}

fn last_input_time() -> u32 {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        info.dwTime
    } else {
        0
    }
}

fn process_id(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn current_context() -> Result<InputContext, String> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return Err("Не удалось определить активное окно".into());
    }
    context_for_window(foreground)
}

fn context_for_window(foreground: HWND) -> Result<InputContext, String> {
    if foreground.0.is_null() || !unsafe { IsWindow(Some(foreground)) }.as_bool() {
        return Err("Исходное окно больше не существует".into());
    }
    let mut pid = 0;
    let thread_id = unsafe { GetWindowThreadProcessId(foreground, Some(&mut pid)) };
    if thread_id == 0 || pid == 0 {
        return Err("Не удалось определить активное приложение".into());
    }
    let mut gui = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    let focus =
        if unsafe { GetGUIThreadInfo(thread_id, &mut gui) }.is_ok() && !gui.hwndFocus.0.is_null() {
            gui.hwndFocus
        } else {
            foreground
        };
    Ok(InputContext {
        foreground: foreground.0 as isize,
        focus: focus.0 as isize,
        process_id: pid,
        last_input: last_input_time(),
    })
}

fn same_target(a: &InputContext, b: &InputContext) -> bool {
    a.foreground == b.foreground && a.focus == b.focus && a.process_id == b.process_id
}

fn popup_restore_identity_is_valid(
    original: &InputContext,
    foreground_pid: u32,
    focus_exists: bool,
    focus_pid: u32,
) -> bool {
    foreground_pid == original.process_id && focus_exists && focus_pid == original.process_id
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopupRestoreAction {
    AlreadyRestored,
    RestoreOriginal,
    Abort,
}

fn popup_restore_action(
    foreground: isize,
    popup_foreground: isize,
    original_foreground: isize,
) -> PopupRestoreAction {
    if foreground == original_foreground {
        PopupRestoreAction::AlreadyRestored
    } else if foreground == popup_foreground || foreground == 0 {
        PopupRestoreAction::RestoreOriginal
    } else {
        PopupRestoreAction::Abort
    }
}

/// Возвращает фокус исходному окну после осознанного клика в текущем popup.
/// `SetFocus` намеренно не используется: если контрол внутри исходного окна уже сменился,
/// операция отменяется, а не переводит фокус назад вслепую.
fn restore_target_from_popup(
    popup: &WebviewWindow,
    original: &InputContext,
) -> Result<InputContext, String> {
    let visible = popup.is_visible().unwrap_or(false);
    if popup.label() != "popup" || !visible {
        return Err("Окно перевода уже закрыто; замена отменена".into());
    }
    let popup_hwnd = HWND(popup.hwnd().map_err(|e| e.to_string())?.0);
    let foreground_is_popup = unsafe { GetForegroundWindow() } == popup_hwnd;
    if !foreground_is_popup {
        return Err("Фокус уже перешёл в другое окно; замена отменена".into());
    }

    let original_hwnd = HWND(original.foreground as *mut std::ffi::c_void);
    let original_focus = HWND(original.focus as *mut std::ffi::c_void);
    let foreground_exists = unsafe { IsWindow(Some(original_hwnd)) }.as_bool();
    let focus_exists = unsafe { IsWindow(Some(original_focus)) }.as_bool();
    if !foreground_exists
        || !popup_restore_identity_is_valid(
            original,
            process_id(original_hwnd),
            focus_exists,
            process_id(original_focus),
        )
    {
        return Err("Исходное поле изменилось; замена отменена".into());
    }

    popup.hide().map_err(|e| e.to_string())?;
    let foreground_after_hide = unsafe { GetForegroundWindow() };
    let action = popup_restore_action(
        foreground_after_hide.0 as isize,
        popup_hwnd.0 as isize,
        original.foreground,
    );
    match action {
        PopupRestoreAction::AlreadyRestored => {}
        PopupRestoreAction::RestoreOriginal => {
            let restored = unsafe { SetForegroundWindow(original_hwnd) }.as_bool();
            if !restored {
                return Err("Не удалось вернуть фокус в исходное поле; замена отменена".into());
            }
        }
        PopupRestoreAction::Abort => {
            return Err("Фокус уже перешёл в другое окно; замена отменена".into());
        }
    }

    let start = Instant::now();
    loop {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.0.is_null() {
            if start.elapsed() >= Duration::from_millis(350) {
                return Err("Не удалось вернуть фокус в исходное поле; замена отменена".into());
            }
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        if foreground != original_hwnd && foreground != popup_hwnd {
            return Err("Фокус уже перешёл в другое окно; замена отменена".into());
        }
        let current = context_for_window(foreground)?;
        if same_target(original, &current) {
            // Это новая допустимая эпоха ввода: она создаётся только после проверки popup,
            // живого HWND/PID/контрола и успешного возврата foreground.
            return Ok(current);
        }
        if start.elapsed() >= Duration::from_millis(350) {
            return Err("Не удалось вернуть фокус в исходное поле; замена отменена".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Между захватом и заменой лежит сетевой перевод, поэтому эпоха ввода здесь не сверяется:
/// `GetLastInputInfo` растёт от любого движения мыши, а движение мышью замену не отменяет.
/// Защиту держат окно, фокусный контрол и точное совпадение выделенного текста.
fn replacement_unchanged(
    original: &InputContext,
    current: &InputContext,
    original_text: &str,
    current_text: &str,
) -> bool {
    same_target(original, current) && original_text == current_text
}

fn marker() -> String {
    let id = MARKER_ID.fetch_add(1, Ordering::Relaxed);
    format!("\u{2063}UTranslate:{id}:{}\u{2063}", std::process::id())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyOwnerRelation {
    Marker,
    Target,
    Unrelated,
}

#[derive(Clone, Debug)]
struct CopyObservation {
    sequence: u32,
    context: InputContext,
}

#[derive(Clone, Debug)]
struct ClipboardTextObservation {
    text: Option<String>,
    sequence: u32,
    owner: CopyOwnerRelation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipboardRestoreDecision {
    Restore,
    PreserveExternal,
}

fn clipboard_restore_decision(
    expected_sequence: u32,
    current_sequence: u32,
    owner_matches: bool,
    text_matches: bool,
) -> ClipboardRestoreDecision {
    if current_sequence == expected_sequence || (owner_matches && text_matches) {
        ClipboardRestoreDecision::Restore
    } else {
        ClipboardRestoreDecision::PreserveExternal
    }
}

fn read_unicode_text_from_open_clipboard() -> Option<String> {
    unsafe { GetClipboardData(CF_UNICODETEXT_ID) }
        .ok()
        .and_then(|handle| {
            let global = HGLOBAL(handle.0);
            let size = unsafe { GlobalSize(global) };
            if size < std::mem::size_of::<u16>() {
                return None;
            }
            let ptr = unsafe { GlobalLock(global) };
            if ptr.is_null() {
                return None;
            }
            let units = unsafe {
                std::slice::from_raw_parts(ptr.cast::<u16>(), size / std::mem::size_of::<u16>())
            };
            let end = units
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(units.len());
            let text = String::from_utf16_lossy(&units[..end]);
            unsafe {
                let _ = GlobalUnlock(global);
            }
            Some(text)
        })
}

fn read_clipboard_text_observation(
    marker_owner: HWND,
    target_pid: u32,
) -> Result<ClipboardTextObservation, String> {
    // Text, sequence and owner must describe one clipboard state. Keeping the
    // clipboard open also covers delayed rendering triggered by GetClipboardData.
    let _guard = open_clipboard(None)?;
    let text = read_unicode_text_from_open_clipboard();
    let owner = unsafe { GetClipboardOwner() }
        .map_err(|_| "Не удалось определить владельца буфера обмена".to_string())?;
    let owner = if owner.0 == marker_owner.0 {
        CopyOwnerRelation::Marker
    } else if process_id(owner) == target_pid {
        CopyOwnerRelation::Target
    } else {
        CopyOwnerRelation::Unrelated
    };
    Ok(ClipboardTextObservation {
        text,
        sequence: unsafe { GetClipboardSequenceNumber() },
        owner,
    })
}

/// Всё, что после отправленного Ctrl+C делается с системой: опрос буфера и окна, чтение
/// текста, возврат снимка и пауза. Отдельный объект нужен, чтобы опрос проверялся тестами
/// по скрипту, а не только вживую.
trait CopyProbe {
    fn observe(&mut self) -> Result<CopyObservation, String>;
    fn read_text(&mut self) -> Result<ClipboardTextObservation, String>;
    fn restore(&mut self, sequence: u32) -> Result<(), String>;
    fn wait(&mut self) -> bool;
}

enum Copied {
    /// Текст прочитан (или выделение оказалось пустым).
    Text(Option<String>),
    /// Копия не появилась за отведённое время.
    Timeout,
}

/// Возвращает буфер и склеивает ошибки: пользователю нужна исходная причина, а не только
/// отказ восстановления. `restore` сам откажется писать поверх чужого содержимого по sequence.
fn restore_clipboard(
    probe: &mut impl CopyProbe,
    sequence: u32,
    error: Option<String>,
) -> Result<(), String> {
    match (probe.restore(sequence), error) {
        (Ok(()), None) => Ok(()),
        (Ok(()), Some(error)) => Err(error),
        (Err(restore_error), None) => Err(restore_error),
        (Err(restore_error), Some(error)) => Err(format!("{error}. {restore_error}")),
    }
}

/// Опрос сам ничего не восстанавливает: sequence, под которым снимок ещё можно вернуть,
/// копится в `restore_sequence`, а возврат делает единственный выход в вызывающей функции.
fn poll_copied_text(
    probe: &mut impl CopyProbe,
    target: &InputContext,
    mark: &str,
    marker_sequence: u32,
    restore_sequence: &mut u32,
) -> Result<Copied, String> {
    loop {
        let observation = probe.observe()?;
        if observation.sequence != marker_sequence {
            let copied = probe.read_text()?;
            match copied.owner {
                CopyOwnerRelation::Marker => {
                    if copied.text.as_deref() != Some(mark) {
                        return Err("Контекст копирования изменился; операция отменена".into());
                    }
                    let after_read = probe.observe()?;
                    if after_read.sequence != copied.sequence {
                        continue;
                    }
                    *restore_sequence = copied.sequence;
                    if !same_target(target, &after_read.context) {
                        return Err("Активное поле изменилось во время копирования".into());
                    }
                    if !probe.wait() {
                        return Ok(Copied::Timeout);
                    }
                    continue;
                }
                CopyOwnerRelation::Unrelated => {
                    return Err("Контекст копирования изменился; операция отменена".into());
                }
                CopyOwnerRelation::Target => {}
            }

            let after_read = probe.observe()?;
            if after_read.sequence != copied.sequence {
                continue;
            }
            *restore_sequence = after_read.sequence;
            if !same_target(target, &after_read.context) {
                return Err("Активное поле изменилось во время копирования".into());
            }
            return Ok(Copied::Text(
                copied
                    .text
                    .filter(|text| text != mark && !text.trim().is_empty()),
            ));
        }

        if !same_target(target, &observation.context) {
            return Err("Активное поле изменилось во время копирования".into());
        }
        if !probe.wait() {
            return Ok(Copied::Timeout);
        }
    }
}

/// Буфер пользователя возвращается на любом исходе, кроме отданного дальше текста: иначе
/// маркер остаётся в буфере после первой же ошибки опроса или чтения.
fn finish_capture_after_copy(
    probe: &mut impl CopyProbe,
    target: &InputContext,
    mark: &str,
    marker_sequence: u32,
) -> Result<Captured, String> {
    let mut restore_sequence = marker_sequence;
    let copied =
        match poll_copied_text(probe, target, mark, marker_sequence, &mut restore_sequence) {
            Ok(copied) => copied,
            Err(error) => {
                return Err(restore_clipboard(probe, restore_sequence, Some(error))
                    .expect_err("возврат буфера после ошибки всегда возвращает ошибку"))
            }
        };
    // Текст уже снят — возврат прежнего буфера теперь вежливость, а не условие успеха:
    // если вернуть его не удалось (отложенный формат, чужая новая запись), перевод не теряем.
    let _ = restore_clipboard(probe, restore_sequence, None);
    match copied {
        Copied::Timeout => Ok(Captured {
            text: None,
            context: target.clone(),
            clipboard_replaced: false,
        }),
        Copied::Text(text) => {
            let context = probe.observe()?.context;
            if !same_target(target, &context) {
                return Err("Активное поле изменилось после копирования".into());
            }
            Ok(Captured {
                text,
                context,
                clipboard_replaced: false,
            })
        }
    }
}

struct Win32CopyProbe {
    owner: HWND,
    target_pid: u32,
    start: Instant,
    /// Снимок отдаётся ровно один раз: `restore` потребляет его целиком.
    snapshot: Option<ClipboardSnapshot>,
}

impl CopyProbe for Win32CopyProbe {
    fn observe(&mut self) -> Result<CopyObservation, String> {
        let sequence = unsafe { GetClipboardSequenceNumber() };
        let context = current_context()?;
        Ok(CopyObservation { sequence, context })
    }

    fn read_text(&mut self) -> Result<ClipboardTextObservation, String> {
        read_clipboard_text_observation(self.owner, self.target_pid)
    }

    fn restore(&mut self, sequence: u32) -> Result<(), String> {
        self.snapshot
            .take()
            .ok_or_else(|| "Буфер обмена уже восстановлен".to_string())?
            .restore(sequence, self.owner, None)
    }

    fn wait(&mut self) -> bool {
        if self.start.elapsed() >= COPY_TIMEOUT {
            false
        } else {
            thread::sleep(Duration::from_millis(10));
            true
        }
    }
}

pub fn capture_selection(app: &AppHandle) -> Result<Captured, String> {
    let snapshot = ClipboardSnapshot::capture()?;
    wait_modifiers_released()?;
    let target = current_context()?;
    let app_owner = app_clipboard_owner(app)?;
    let mark = marker();
    let marker_sequence = write_text_owned(&mark, snapshot.sequence, app_owner)?;
    let before_copy = match current_context() {
        Ok(context) => context,
        Err(context_error) => {
            return match snapshot.restore(marker_sequence, app_owner, None) {
                Ok(()) => Err(context_error),
                Err(restore_error) => Err(format!("{context_error}. {restore_error}")),
            };
        }
    };
    // Здесь между снимком эпохи и проверкой прошли миллисекунды, поэтому сравнение
    // `last_input` ещё имеет смысл: любой ввод в этом окне означает другое выделение.
    let same_target_before_copy = same_target(&target, &before_copy);
    let same_epoch_before_copy = target.last_input == before_copy.last_input;
    if !same_target_before_copy || !same_epoch_before_copy {
        return match snapshot.restore(marker_sequence, app_owner, None) {
            Ok(()) => Err("Фокус или выделение изменились перед копированием".into()),
            Err(restore_error) => Err(format!(
                "Фокус или выделение изменились перед копированием. {restore_error}"
            )),
        };
    }
    if let Err(input_error) = send_ctrl_chord(VK_C) {
        return match snapshot.restore(marker_sequence, app_owner, None) {
            Ok(()) => Err(input_error),
            Err(restore_error) => Err(format!("{input_error}. {restore_error}")),
        };
    }

    let mut probe = Win32CopyProbe {
        owner: app_owner,
        target_pid: target.process_id,
        start: Instant::now(),
        snapshot: Some(snapshot),
    };
    finish_capture_after_copy(&mut probe, &target, &mark, marker_sequence)
}

/// Вставляет перевод, только если окно, контрол и выделенный текст остались теми же, что при
/// исходном захвате. Эпоха ввода здесь не проверяется: между захватом и вставкой лежит сетевой
/// перевод, и `GetLastInputInfo` успевает вырасти от простого движения мыши.
///
/// Повторный `capture_selection` — не дубль первого захвата, а проверка перед перезаписью:
/// он читает то, что выделено прямо сейчас, и сравнивает с исходным текстом.
pub fn paste_text(
    app: &AppHandle,
    text: &str,
    original_text: &str,
    original: &InputContext,
) -> Result<PasteOutcome, String> {
    if !same_target(original, &current_context()?) {
        return Err("Фокус или выделение изменились; замена отменена".into());
    }

    let verified = capture_selection(app)?;
    let Some(selected) = verified.text.as_deref() else {
        return Err("Выделение больше не активно; замена отменена".into());
    };
    if !replacement_unchanged(original, &verified.context, original_text, selected) {
        return Err("Фокус или выделенный текст изменились; замена отменена".into());
    }

    let snapshot = ClipboardSnapshot::capture()?;
    wait_modifiers_released()?;
    if !same_target(original, &current_context()?) {
        return Err("Активное поле изменилось перед вставкой; замена отменена".into());
    }
    let app_owner = app_clipboard_owner(app)?;
    let paste_sequence = write_text_owned(text, snapshot.sequence, app_owner)?;
    let before_input = match current_context() {
        Ok(context) => context,
        Err(context_error) => {
            return match snapshot.restore(paste_sequence, app_owner, None) {
                Ok(()) => Err(context_error),
                Err(restore_error) => Err(format!("{context_error}. {restore_error}")),
            };
        }
    };
    if !same_target(original, &before_input) {
        return match snapshot.restore(paste_sequence, app_owner, None) {
            Ok(()) => Err("Активное поле изменилось перед вставкой; замена отменена".into()),
            Err(restore_error) => Err(format!(
                "Активное поле изменилось перед вставкой; замена отменена. {restore_error}"
            )),
        };
    }
    if let Err(input_error) = send_ctrl_chord(VK_V) {
        return match snapshot.restore(paste_sequence, app_owner, None) {
            Ok(()) => Err(input_error),
            Err(restore_error) => Err(format!("{input_error}. {restore_error}")),
        };
    }
    thread::sleep(Duration::from_millis(250));
    let outcome = snapshot.restore_after_paste(paste_sequence, app_owner, text);
    let same_target_after_paste = same_target(original, &current_context()?);
    if !same_target_after_paste {
        return Err("Активное поле изменилось во время вставки".into());
    }
    Ok(outcome)
}

/// Вставка из popup получает отдельный безопасный путь: сначала подтверждает, что invoke
/// пришёл из текущего активного popup, затем возвращает только foreground-окно и повторно
/// проверяет исходный контрол и точный выделенный текст обычным `paste_text`.
pub fn paste_text_from_popup(
    app: &AppHandle,
    popup: &WebviewWindow,
    text: &str,
    original_text: &str,
    original: &InputContext,
) -> Result<PasteOutcome, String> {
    let fresh = restore_target_from_popup(popup, original)?;
    paste_text(app, text, original_text, &fresh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use windows::{
        core::PCWSTR,
        Win32::{
            System::{
                DataExchange::RegisterClipboardFormatW,
                StationsAndDesktops::{
                    CreateDesktopW, CreateWindowStationW, SetProcessWindowStation,
                    SetThreadDesktop, DESKTOP_CREATEWINDOW, DESKTOP_READOBJECTS,
                    DESKTOP_WRITEOBJECTS,
                },
            },
            UI::WindowsAndMessaging::{CreateWindowExW, WS_POPUP},
        },
    };

    fn context(foreground: isize, focus: isize, process_id: u32, last_input: u32) -> InputContext {
        InputContext {
            foreground,
            focus,
            process_id,
            last_input,
        }
    }

    #[test]
    fn classifies_common_and_unsafe_clipboard_formats() {
        assert_eq!(format_kind(13), FormatKind::Global); // CF_UNICODETEXT
        assert_eq!(format_kind(8), FormatKind::Global); // CF_DIB
        assert_eq!(format_kind(17), FormatKind::Global); // CF_DIBV5
        assert_eq!(format_kind(15), FormatKind::Global); // CF_HDROP
        assert_eq!(format_kind(0xc001), FormatKind::Global); // registered HTML/RTF/PNG
        assert_eq!(format_kind(CF_BITMAP_ID), FormatKind::Bitmap);
        assert_eq!(format_kind(CF_ENHMETAFILE_ID), FormatKind::EnhMetaFile);
        assert_eq!(format_kind(CF_PALETTE_ID), FormatKind::Unsupported);
        assert_eq!(format_kind(CF_PRIVATEFIRST), FormatKind::Unsupported);
    }

    #[test]
    fn post_paste_restore_accepts_only_our_exact_synthesized_payload() {
        assert_eq!(
            clipboard_restore_decision(10, 13, true, true),
            ClipboardRestoreDecision::Restore
        );
        assert_eq!(
            clipboard_restore_decision(10, 13, false, true),
            ClipboardRestoreDecision::PreserveExternal
        );
        assert_eq!(
            clipboard_restore_decision(10, 13, true, false),
            ClipboardRestoreDecision::PreserveExternal
        );
        assert_eq!(
            clipboard_restore_decision(10, 10, false, false),
            ClipboardRestoreDecision::Restore
        );
    }

    #[test]
    fn replacement_requires_same_control_and_text_but_survives_mouse_movement() {
        let original = context(1, 2, 3, 4);
        assert!(replacement_unchanged(
            &original,
            &context(1, 2, 3, 4),
            "hello",
            "hello"
        ));
        // Между захватом и вставкой лежит сетевой перевод: last_input успевает вырасти
        // от движения мыши, и это не повод отменять замену.
        assert!(replacement_unchanged(
            &original,
            &context(1, 2, 3, 99),
            "hello",
            "hello"
        ));
        assert!(!replacement_unchanged(
            &original,
            &context(1, 9, 3, 4),
            "hello",
            "hello"
        ));
        assert!(!replacement_unchanged(
            &original,
            &context(9, 2, 3, 4),
            "hello",
            "hello"
        ));
        assert!(!replacement_unchanged(
            &original,
            &context(1, 2, 9, 4),
            "hello",
            "hello"
        ));
        assert!(!replacement_unchanged(
            &original,
            &context(1, 2, 3, 4),
            "hello",
            "other"
        ));
    }

    #[test]
    fn popup_restore_validates_window_and_control_ownership_before_new_epoch() {
        let original = context(10, 20, 30, 40);
        assert!(popup_restore_identity_is_valid(&original, 30, true, 30));
        assert!(!popup_restore_identity_is_valid(&original, 31, true, 30));
        assert!(!popup_restore_identity_is_valid(&original, 30, false, 30));
        assert!(!popup_restore_identity_is_valid(&original, 30, true, 31));
    }

    #[test]
    fn popup_restore_never_takes_focus_back_from_an_unrelated_window() {
        assert_eq!(
            popup_restore_action(10, 20, 10),
            PopupRestoreAction::AlreadyRestored
        );
        assert_eq!(
            popup_restore_action(20, 20, 10),
            PopupRestoreAction::RestoreOriginal
        );
        assert_eq!(
            popup_restore_action(0, 20, 10),
            PopupRestoreAction::RestoreOriginal
        );
        assert_eq!(popup_restore_action(30, 20, 10), PopupRestoreAction::Abort);
    }

    /// Скриптованный `CopyProbe`: очередь наблюдений и чтений, счётчик пауз и журнал
    /// восстановлений — по нему проверяется, что буфер возвращается ровно один раз.
    #[derive(Default)]
    struct ScriptedProbe {
        observations: VecDeque<Result<CopyObservation, String>>,
        reads: VecDeque<Result<ClipboardTextObservation, String>>,
        restored: Vec<u32>,
        restore_error: Option<String>,
        waits: usize,
        keep_waiting: bool,
    }

    impl CopyProbe for ScriptedProbe {
        fn observe(&mut self) -> Result<CopyObservation, String> {
            self.observations
                .pop_front()
                .unwrap_or_else(|| Err("сценарий кончился: observe".into()))
        }

        fn read_text(&mut self) -> Result<ClipboardTextObservation, String> {
            self.reads
                .pop_front()
                .unwrap_or_else(|| Err("сценарий кончился: read_text".into()))
        }

        fn restore(&mut self, sequence: u32) -> Result<(), String> {
            self.restored.push(sequence);
            match &self.restore_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }

        fn wait(&mut self) -> bool {
            self.waits += 1;
            self.keep_waiting
        }
    }

    fn scripted(
        observations: Vec<Result<CopyObservation, String>>,
        reads: Vec<Result<ClipboardTextObservation, String>>,
    ) -> ScriptedProbe {
        ScriptedProbe {
            observations: observations.into(),
            reads: reads.into(),
            keep_waiting: true,
            ..Default::default()
        }
    }

    fn seen(sequence: u32, context: &InputContext) -> Result<CopyObservation, String> {
        Ok(CopyObservation {
            sequence,
            context: context.clone(),
        })
    }

    fn copied(
        text: &str,
        sequence: u32,
        owner: CopyOwnerRelation,
    ) -> Result<ClipboardTextObservation, String> {
        Ok(ClipboardTextObservation {
            text: Some(text.to_string()),
            sequence,
            owner,
        })
    }

    #[test]
    fn capture_polling_waits_past_marker_owned_sequence_change() {
        let target = context(11, 12, 13, 14);
        let mut probe = scripted(
            vec![
                seen(101, &target),
                seen(101, &target),
                seen(102, &target),
                seen(102, &target),
                seen(103, &target),
            ],
            vec![
                copied("marker", 101, CopyOwnerRelation::Marker),
                copied("selected text", 102, CopyOwnerRelation::Target),
            ],
        );

        let captured = match finish_capture_after_copy(&mut probe, &target, "marker", 100) {
            Ok(captured) => captured,
            Err(error) => panic!("valid copy rejected with exact regression error: {error}"),
        };
        assert_eq!(captured.text.as_deref(), Some("selected text"));
        assert_eq!(probe.restored, vec![102]);
    }

    #[test]
    fn capture_polling_accepts_target_that_writes_before_marker_read() {
        let target = context(11, 12, 13, 14);
        let mut probe = scripted(
            vec![seen(101, &target), seen(102, &target), seen(103, &target)],
            vec![copied("selected text", 102, CopyOwnerRelation::Target)],
        );

        let captured = finish_capture_after_copy(&mut probe, &target, "marker", 100)
            .expect("target copy observed during marker read must be accepted");

        assert_eq!(captured.text.as_deref(), Some("selected text"));
        assert_eq!(probe.restored, vec![102]);
        assert_eq!(probe.waits, 0, "стабильная копия не должна ждать");
    }

    #[test]
    fn capture_polling_returns_clipboard_before_rejecting_unrelated_change() {
        let target = context(11, 12, 13, 14);
        let mut probe = scripted(
            vec![seen(101, &target)],
            vec![copied("unrelated", 101, CopyOwnerRelation::Unrelated)],
        );

        let error = finish_capture_after_copy(&mut probe, &target, "marker", 100)
            .err()
            .expect("unrelated clipboard owner must be rejected");

        assert_eq!(error, "Контекст копирования изменился; операция отменена");
        // Восстановление зовётся и здесь: под sequence маркера оно само откажется писать
        // поверх чужой записи, но своё содержимое вернёт, если запись была нашей.
        assert_eq!(probe.restored, vec![100]);
    }

    #[test]
    fn capture_polling_returns_clipboard_when_marker_owner_holds_foreign_content() {
        let target = context(11, 12, 13, 14);
        let mut probe = scripted(
            vec![seen(101, &target)],
            vec![copied("other app state", 101, CopyOwnerRelation::Marker)],
        );

        let error = finish_capture_after_copy(&mut probe, &target, "marker", 100)
            .err()
            .expect("same owner HWND with different content must be rejected");

        assert_eq!(error, "Контекст копирования изменился; операция отменена");
        assert_eq!(probe.restored, vec![100]);
    }

    #[test]
    fn capture_polling_returns_clipboard_after_change_following_marker_read() {
        let target = context(11, 12, 13, 14);
        let mut probe = scripted(
            vec![seen(101, &target), seen(102, &target), seen(102, &target)],
            vec![
                copied("marker", 101, CopyOwnerRelation::Marker),
                copied("unrelated", 102, CopyOwnerRelation::Unrelated),
            ],
        );

        let error = finish_capture_after_copy(&mut probe, &target, "marker", 100)
            .err()
            .expect("unrelated write after marker read must be rejected");

        assert_eq!(error, "Контекст копирования изменился; операция отменена");
        assert_eq!(probe.restored, vec![100]);
        assert_eq!(probe.waits, 0, "изменившийся sequence перечитывается без паузы");
    }

    #[test]
    fn capture_polling_returns_clipboard_when_window_probe_fails() {
        let target = context(11, 12, 13, 14);
        let mut probe = scripted(vec![Err("Не удалось определить активное окно".into())], vec![]);

        let error = finish_capture_after_copy(&mut probe, &target, "marker", 100)
            .err()
            .expect("window probe failure must abort the capture");

        assert_eq!(error, "Не удалось определить активное окно");
        assert_eq!(probe.restored, vec![100], "маркер не должен остаться в буфере");
    }

    #[test]
    fn capture_polling_returns_clipboard_when_text_read_fails() {
        let target = context(11, 12, 13, 14);
        let mut probe = scripted(
            vec![seen(101, &target)],
            vec![Err("Буфер обмена занят другой программой".into())],
        );

        let error = finish_capture_after_copy(&mut probe, &target, "marker", 100)
            .err()
            .expect("clipboard read failure must abort the capture");

        assert_eq!(error, "Буфер обмена занят другой программой");
        assert_eq!(probe.restored, vec![100], "маркер не должен остаться в буфере");
    }

    #[test]
    fn capture_polling_reports_restore_failure_next_to_the_original_error() {
        let target = context(11, 12, 13, 14);
        let mut probe = ScriptedProbe {
            observations: VecDeque::from([Err("Не удалось определить активное окно".to_string())]),
            restore_error: Some("Буфер обмена изменился; новое содержимое сохранено".into()),
            ..Default::default()
        };

        let error = finish_capture_after_copy(&mut probe, &target, "marker", 100)
            .err()
            .expect("window probe failure must abort the capture");

        assert_eq!(
            error,
            "Не удалось определить активное окно. Буфер обмена изменился; новое содержимое сохранено"
        );
        assert_eq!(probe.restored, vec![100]);
    }

    #[test]
    fn capture_polling_returns_clipboard_when_copy_never_arrives() {
        let target = context(11, 12, 13, 14);
        let mut probe = ScriptedProbe {
            observations: VecDeque::from([seen(100, &target)]),
            ..Default::default()
        };

        let captured = finish_capture_after_copy(&mut probe, &target, "marker", 100)
            .expect("timeout must return an empty capture, not an error");

        assert!(captured.text.is_none());
        assert_eq!(probe.restored, vec![100]);
        assert_eq!(probe.waits, 1);
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn unicode_bytes(value: &str) -> Vec<u8> {
        let wide = wide(value);
        unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2) }.to_vec()
    }

    fn dib_1x1() -> Vec<u8> {
        let mut bytes = vec![0u8; 44];
        bytes[0..4].copy_from_slice(&40u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&1i32.to_le_bytes());
        bytes[8..12].copy_from_slice(&1i32.to_le_bytes());
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&32u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&4u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&[0x11, 0x22, 0x33, 0xff]);
        bytes
    }

    fn hdrop_one_file() -> Vec<u8> {
        let path = wide(r"C:\isolated\fixture.txt");
        let mut bytes = vec![0u8; 20 + path.len() * 2 + 2];
        bytes[0..4].copy_from_slice(&20u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&1i32.to_le_bytes());
        let path_bytes =
            unsafe { std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), path.len() * 2) };
        bytes[20..20 + path_bytes.len()].copy_from_slice(path_bytes);
        bytes
    }

    fn put_global_formats(owner: HWND, formats: &[(u32, Vec<u8>)]) -> Result<u32, String> {
        let mut prepared = Vec::new();
        for (id, bytes) in formats {
            let global =
                unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }.map_err(|e| e.to_string())?;
            let data = FormatData::Global(Some(global));
            let ptr = unsafe { GlobalLock(global) };
            if ptr.is_null() {
                return Err("GlobalLock failed".into());
            }
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len());
                let _ = GlobalUnlock(global);
            }
            prepared.push(ClipboardFormat { id: *id, data });
        }
        let _guard = open_clipboard(Some(owner))?;
        unsafe { EmptyClipboard() }.map_err(|e| e.to_string())?;
        for item in &mut prepared {
            unsafe { SetClipboardData(item.id, Some(item.data.handle())) }
                .map_err(|e| e.to_string())?;
            item.data.transferred();
        }
        Ok(unsafe { GetClipboardSequenceNumber() })
    }

    fn read_global_format(id: u32) -> Result<Vec<u8>, String> {
        let _guard = open_clipboard(None)?;
        let handle = unsafe { GetClipboardData(id) }.map_err(|e| e.to_string())?;
        let global = HGLOBAL(handle.0);
        let size = unsafe { GlobalSize(global) };
        let ptr = unsafe { GlobalLock(global) };
        if ptr.is_null() {
            return Err("GlobalLock failed".into());
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), size) }.to_vec();
        unsafe {
            let _ = GlobalUnlock(global);
        }
        Ok(bytes)
    }

    fn isolated_owner_window() -> Result<HWND, String> {
        let station_name = wide(&format!("UTranslateTestStation-{}", std::process::id()));
        let station = unsafe {
            CreateWindowStationW(
                PCWSTR(station_name.as_ptr()),
                0,
                0x0200_0000, // MAXIMUM_ALLOWED; the station DACL chooses the safe subset.
                None,
            )
        }
        .map_err(|e| format!("CreateWindowStationW: {e}"))?;
        unsafe { SetProcessWindowStation(station) }
            .map_err(|e| format!("SetProcessWindowStation: {e}"))?;

        let desktop_name = wide("UTranslateTestDesktop");
        let access = DESKTOP_CREATEWINDOW.0 | DESKTOP_READOBJECTS.0 | DESKTOP_WRITEOBJECTS.0;
        let desktop = unsafe {
            CreateDesktopW(
                PCWSTR(desktop_name.as_ptr()),
                PCWSTR::null(),
                None,
                Default::default(),
                access,
                None,
            )
        }
        .map_err(|e| format!("CreateDesktopW: {e}"))?;
        unsafe { SetThreadDesktop(desktop) }.map_err(|e| format!("SetThreadDesktop: {e}"))?;

        unsafe {
            CreateWindowExW(
                Default::default(),
                windows::core::w!("STATIC"),
                windows::core::w!("UTranslate clipboard test"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )
        }
        .map_err(|e| format!("CreateWindowExW: {e}"))
    }

    fn run_isolated_clipboard_roundtrip() -> Result<(), String> {
        let owner = isolated_owner_window()?;
        let html = unsafe { RegisterClipboardFormatW(windows::core::w!("HTML Format")) };
        if html == 0 {
            return Err("RegisterClipboardFormatW failed".into());
        }
        let original = vec![
            (CF_UNICODETEXT_ID, unicode_bytes("original")),
            (html, b"Version:0.9\r\n<html>fixture</html>\0".to_vec()),
            (8, dib_1x1()),
            (15, hdrop_one_file()),
        ];
        put_global_formats(owner, &original)?;
        let snapshot = ClipboardSnapshot::capture()?;
        let temporary_sequence =
            put_global_formats(owner, &[(CF_UNICODETEXT_ID, unicode_bytes("temporary"))])?;
        snapshot.restore(temporary_sequence, owner, None)?;
        for (id, expected) in &original {
            if read_global_format(*id)? != *expected {
                return Err(format!("format {id} was not restored byte-for-byte"));
            }
        }

        let stale = ClipboardSnapshot::capture()?;
        let stale_sequence = stale.sequence;
        put_global_formats(owner, &[(CF_UNICODETEXT_ID, unicode_bytes("newer"))])?;
        if stale.restore(stale_sequence, owner, None).is_ok() {
            return Err("stale snapshot unexpectedly overwrote newer clipboard".into());
        }
        if read_global_format(CF_UNICODETEXT_ID)? != unicode_bytes("newer") {
            return Err("newer clipboard content was overwritten".into());
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires permission to create an isolated Windows window station"]
    fn native_clipboard_roundtrip_uses_private_window_station() {
        let exe = std::env::current_exe().unwrap();
        let output = std::process::Command::new(exe)
            .args([
                "--exact",
                "capture::tests::native_clipboard_roundtrip_child",
                "--ignored",
                "--nocapture",
            ])
            .env("UTRANSLATE_CLIPBOARD_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated clipboard child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "runs only in the private-window-station child process"]
    fn native_clipboard_roundtrip_child() {
        if std::env::var_os("UTRANSLATE_CLIPBOARD_CHILD").is_none() {
            return;
        }
        run_isolated_clipboard_roundtrip().unwrap();
    }
}
