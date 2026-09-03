//! Захват выделенного текста и вставка перевода через эмуляцию Ctrl+C / Ctrl+V,
//! как в QTranslate. Буфер обмена возвращается в прежнее состояние.

use std::{
    thread,
    time::{Duration, Instant},
};

use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_C, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
};

pub struct Captured {
    pub text: Option<String>,
    /// В буфере было не текстовое содержимое, вернуть его нельзя.
    pub clipboard_replaced: bool,
}

const MODIFIERS: [VIRTUAL_KEY; 5] = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN];

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send(keys: &[INPUT]) {
    unsafe { SendInput(keys, std::mem::size_of::<INPUT>() as i32) };
}

fn modifiers_down() -> bool {
    MODIFIERS.iter().any(|vk| unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0)
}

/// Ждём, пока пользователь физически отпустит модификаторы хоткея, иначе наш Ctrl+C
/// превратится в Ctrl+Alt+C, а автоповтор буквы хоткея улетит в приложение.
fn wait_modifiers_released() {
    let start = Instant::now();
    while modifiers_down() {
        if start.elapsed() > Duration::from_millis(1500) {
            // Клавиша залипла или пришла синтетически — отпускаем сами.
            send(&MODIFIERS.map(|vk| key(vk, true)));
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn send_ctrl_chord(vk: VIRTUAL_KEY) {
    wait_modifiers_released();
    send(&[key(VK_CONTROL, false), key(vk, false), key(vk, true), key(VK_CONTROL, true)]);
}

pub fn capture_selection(app: &AppHandle) -> Captured {
    let clip = app.clipboard();
    let old = clip.read_text().ok();
    let seq = unsafe { GetClipboardSequenceNumber() };
    send_ctrl_chord(VK_C);

    let start = Instant::now();
    while unsafe { GetClipboardSequenceNumber() } == seq {
        if start.elapsed() > Duration::from_millis(300) {
            return Captured { text: None, clipboard_replaced: false };
        }
        thread::sleep(Duration::from_millis(10));
    }
    // Дать приложению дописать остальные форматы буфера.
    thread::sleep(Duration::from_millis(20));

    let text = clip.read_text().ok().filter(|t| !t.trim().is_empty());
    match old {
        Some(old) => {
            let _ = clip.write_text(old);
            Captured { text, clipboard_replaced: false }
        }
        // ponytail: восстанавливаем только текст; картинки и файлы теряются, полное сохранение форматов — если появятся жалобы
        None => Captured { clipboard_replaced: text.is_some(), text },
    }
}

/// Вставляет текст на место выделения и возвращает буфер обратно.
pub fn paste_text(app: &AppHandle, text: &str) {
    let clip = app.clipboard();
    let old = clip.read_text().ok();
    if clip.write_text(text.to_string()).is_err() {
        return;
    }
    thread::sleep(Duration::from_millis(30));
    send_ctrl_chord(VK_V);
    thread::sleep(Duration::from_millis(200));
    if let Some(old) = old {
        let _ = clip.write_text(old);
    }
}
