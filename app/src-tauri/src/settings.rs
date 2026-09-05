//! Настройки в JSON, применяются без перезапуска.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub const KNOWN_ENGINES: [&str; 3] = ["google", "bing", "mymemory"];

pub fn is_known_engine(engine: &str) -> bool {
    KNOWN_ENGINES.contains(&engine)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub hotkey_popup: String,
    pub hotkey_replace: String,
    pub hotkey_window: String,
    pub hotkey_screen: String,
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
            hotkey_screen: "Ctrl+Alt+S".into(),
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

    /// Чинит только испорченные поля. Выбрасывать файл целиком нельзя: в 0.2.0 старый экран
    /// настроек умел поставить `secondaryLang == primaryLang`, и вместе с языком пользователь
    /// терял хоткеи, тему и кегль.
    fn sanitized(mut self) -> Self {
        let defaults = Settings::default();
        if self.secondary_lang == self.primary_lang {
            self.secondary_lang = if self.primary_lang == defaults.primary_lang {
                defaults.secondary_lang.clone()
            } else {
                defaults.primary_lang.clone()
            };
        }
        self.engines.retain(|engine| is_known_engine(engine));
        if self.engines.is_empty() {
            self.engines = defaults.engines;
        }
        self
    }
}

pub fn load(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
        .map(Settings::sanitized)
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

    #[test]
    fn knows_supported_translation_engines() {
        for engine in KNOWN_ENGINES {
            assert!(is_known_engine(engine));
        }
        assert!(!is_known_engine("deepl"));
    }

    #[test]
    fn old_settings_receive_the_default_screen_hotkey() {
        let old = r#"{
            "hotkeyPopup":"Ctrl+Alt+T",
            "hotkeyReplace":"Ctrl+Alt+R",
            "hotkeyWindow":"Ctrl+Alt+W",
            "primaryLang":"ru",
            "secondaryLang":"en",
            "engines":["google"],
            "theme":"system",
            "uiLang":"ru",
            "historyEnabled":true,
            "showOriginal":false,
            "fontSize":21
        }"#;
        let settings: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.hotkey_screen, "Ctrl+Alt+S");
    }

    #[test]
    fn broken_settings_are_repaired_field_by_field() {
        let old = r#"{
            "hotkeyPopup":"Ctrl+Shift+9",
            "hotkeyReplace":"Ctrl+Alt+R",
            "primaryLang":"ru",
            "secondaryLang":"ru",
            "engines":["deepl","google"],
            "theme":"dark",
            "fontSize":17
        }"#;
        let repaired = serde_json::from_str::<Settings>(old).unwrap().sanitized();

        // Испорчено было одно поле — остальное пользователю сохраняем.
        assert_eq!(repaired.hotkey_popup, "Ctrl+Shift+9");
        assert_eq!(repaired.theme, "dark");
        assert_eq!(repaired.font_size, 17);
        assert_eq!(repaired.engines, vec!["google".to_string()]);
        assert_eq!(repaired.secondary_lang, "en");
        assert!(repaired.validate().is_ok());

        let no_engines = Settings {
            engines: vec!["deepl".into()],
            primary_lang: "en".into(),
            secondary_lang: "en".into(),
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(no_engines.engines, Settings::default().engines);
        assert_eq!(no_engines.secondary_lang, "ru");
        assert!(no_engines.validate().is_ok());
    }
}
