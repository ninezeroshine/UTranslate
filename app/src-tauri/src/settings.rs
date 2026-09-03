//! Настройки в JSON, применяются без перезапуска.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub hotkey_popup: String,
    pub hotkey_replace: String,
    pub hotkey_window: String,
    pub primary_lang: String,
    pub secondary_lang: String,
    pub engines: Vec<String>,
    pub theme: String,
    pub ui_lang: String,
    pub history_enabled: bool,
    pub show_original: bool,
    pub font_size: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey_popup: "Ctrl+Alt+T".into(),
            hotkey_replace: "Ctrl+Alt+R".into(),
            hotkey_window: "Ctrl+Alt+W".into(),
            primary_lang: "ru".into(),
            secondary_lang: "en".into(),
            engines: vec!["google".into(), "bing".into(), "mymemory".into()],
            theme: "system".into(),
            ui_lang: "ru".into(),
            history_enabled: true,
            show_original: false,
            font_size: 16,
        }
    }
}

pub fn load(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, s: &Settings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}
