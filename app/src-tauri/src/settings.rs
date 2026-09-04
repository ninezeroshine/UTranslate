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
            font_size: 21, // кегль перевода в попапе по листу токенов design/bento
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if self.primary_lang == self.secondary_lang {
            return Err("Основной и запасной языки должны отличаться".into());
        }
        if self.engines.is_empty() {
            return Err("Выберите хотя бы один движок перевода".into());
        }
        Ok(())
    }
}

pub fn load(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|s: &Settings| s.validate().is_ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, s: &Settings) -> Result<(), String> {
    s.validate()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_equal_languages_and_empty_engine_chain() {
        let mut settings = Settings::default();
        settings.secondary_lang = settings.primary_lang.clone();
        assert!(settings
            .validate()
            .unwrap_err()
            .contains("должны отличаться"));

        settings.secondary_lang = "en".into();
        settings.engines.clear();
        assert!(settings.validate().unwrap_err().contains("хотя бы один"));
    }
}
