//! Движки перевода без ключей: Google (translate.google.com, client=at),
//! Bing (веб-токен со страницы bing.com/translator), MyMemory. Цепочка с fallback и кэшем.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";
const CHUNK: usize = 4500;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Alternative {
    pub pos: String,
    pub terms: Vec<String>,
}

#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Translation {
    pub text: String,
    pub detected: String,
    pub target: String,
    pub engine: String,
    pub alternatives: Vec<Alternative>,
    /// «google: HTTP 429» — почему первый движок в цепочке не ответил.
    pub fallback_from: Option<String>,
}

struct BingSession {
    ig: String,
    key: String,
    token: String,
    expires: Instant,
}

pub struct Engines {
    http: reqwest::Client,
    bing: Mutex<Option<BingSession>>,
    cache: Mutex<HashMap<CacheKey, Translation>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    text: String,
    target: String,
    hint: Option<String>,
    order: Vec<String>,
}

fn cache_key(text: &str, target: &str, hint: Option<&str>, order: &[String]) -> CacheKey {
    CacheKey {
        text: text.to_string(),
        target: target.to_string(),
        hint: hint.map(String::from),
        order: order.to_vec(),
    }
}

impl Engines {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(5))
            .cookie_store(true)
            .build()
            .expect("reqwest client");
        Self {
            http,
            bing: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Длинный текст режется по абзацам на куски до 4500 символов.
    pub async fn translate_long(
        &self,
        text: &str,
        target: &str,
        hint: Option<&str>,
        order: &[String],
    ) -> Result<Translation, String> {
        if text.chars().count() <= CHUNK {
            return self.translate(text, target, hint, order).await;
        }
        let mut chunks: Vec<String> = vec![String::new()];
        for para in text.split_inclusive('\n') {
            let last = chunks.last_mut().unwrap();
            if last.chars().count() + para.chars().count() > CHUNK && !last.is_empty() {
                chunks.push(String::new());
            }
            chunks.last_mut().unwrap().push_str(para);
        }
        let mut out: Option<Translation> = None;
        for chunk in chunks {
            let t = self.translate(&chunk, target, hint, order).await?;
            match &mut out {
                None => out = Some(t),
                Some(acc) => {
                    acc.text.push_str(&t.text);
                }
            }
        }
        let mut t = out.unwrap();
        t.alternatives.clear();
        Ok(t)
    }

    pub async fn translate(
        &self,
        text: &str,
        target: &str,
        hint: Option<&str>,
        order: &[String],
    ) -> Result<Translation, String> {
        let key = cache_key(text, target, hint, order);
        if let Some(t) = self.cache.lock().unwrap().get(&key) {
            return Ok(t.clone());
        }
        let mut first_err: Option<String> = None;
        for name in order {
            let r = match name.as_str() {
                "google" => self.google(text, target).await,
                "bing" => self.bing(text, target).await,
                "mymemory" => self.mymemory(text, target, hint).await,
                other => Err(format!("неизвестный движок {other}")),
            };
            match r {
                Ok(mut t) => {
                    t.fallback_from = first_err.take();
                    let mut cache = self.cache.lock().unwrap();
                    // ponytail: кэш просто сбрасывается при переполнении, LRU не нужен
                    if cache.len() >= 200 {
                        cache.clear();
                    }
                    cache.insert(key, t.clone());
                    return Ok(t);
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(format!("{name}: {e}"));
                    }
                }
            }
        }
        Err(first_err.unwrap_or_else(|| "нет ни одного движка".into()))
    }

    async fn google(&self, text: &str, target: &str) -> Result<Translation, String> {
        let resp = self
            .http
            .get("https://translate.google.com/translate_a/single")
            .query(&[
                ("client", "at"),
                ("sl", "auto"),
                ("tl", target),
                ("dt", "t"),
                ("dt", "bd"),
                ("dj", "1"),
                ("q", text),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        let out: String = v["sentences"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s["trans"].as_str())
            .collect();
        if out.is_empty() {
            return Err("пустой ответ".into());
        }
        let alternatives = v["dict"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| Alternative {
                pos: d["pos"].as_str().unwrap_or("").to_string(),
                terms: d["terms"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect(),
            })
            .collect();
        Ok(Translation {
            text: out,
            detected: v["src"].as_str().unwrap_or("").to_string(),
            target: target.to_string(),
            engine: "google".into(),
            alternatives,
            fallback_from: None,
        })
    }

    async fn bing_session(&self) -> Result<(String, String, String), String> {
        if let Some(s) = self.bing.lock().unwrap().as_ref() {
            if s.expires > Instant::now() {
                return Ok((s.ig.clone(), s.key.clone(), s.token.clone()));
            }
        }
        let page = self
            .http
            .get("https://www.bing.com/translator")
            .send()
            .await
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let ig = between(&page, "IG:\"", "\"")
            .ok_or("нет IG на странице")?
            .to_string();
        let abuse = between(&page, "params_AbusePreventionHelper = [", "]")
            .ok_or("нет токена на странице")?;
        let mut parts = abuse.split(',');
        let key = parts.next().ok_or("нет key")?.trim().to_string();
        let token = parts
            .next()
            .ok_or("нет token")?
            .trim()
            .trim_matches('"')
            .to_string();
        *self.bing.lock().unwrap() = Some(BingSession {
            ig: ig.clone(),
            key: key.clone(),
            token: token.clone(),
            expires: Instant::now() + Duration::from_secs(50 * 60),
        });
        Ok((ig, key, token))
    }

    async fn bing(&self, text: &str, target: &str) -> Result<Translation, String> {
        let (ig, key, token) = self.bing_session().await?;
        let resp = self
            .http
            .post("https://www.bing.com/ttranslatev3")
            .query(&[
                ("isVertical", "1"),
                ("IG", ig.as_str()),
                ("IID", "translator.5028"),
            ])
            .form(&[
                ("fromLang", "auto-detect"),
                ("text", text),
                ("to", target),
                ("token", token.as_str()),
                ("key", key.as_str()),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            *self.bing.lock().unwrap() = None;
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        let item = &v[0];
        let out = item["translations"][0]["text"].as_str().unwrap_or("");
        if out.is_empty() {
            *self.bing.lock().unwrap() = None;
            return Err("пустой ответ".into());
        }
        Ok(Translation {
            text: out.to_string(),
            detected: item["detectedLanguage"]["language"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            target: target.to_string(),
            engine: "bing".into(),
            alternatives: vec![],
            fallback_from: None,
        })
    }

    async fn mymemory(
        &self,
        text: &str,
        target: &str,
        hint: Option<&str>,
    ) -> Result<Translation, String> {
        let source = hint.unwrap_or("en");
        let pair = format!("{source}|{target}");
        let resp = self
            .http
            .get("https://api.mymemory.translated.net/get")
            .query(&[("q", text), ("langpair", pair.as_str())])
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        let out = v["responseData"]["translatedText"].as_str().unwrap_or("");
        if out.is_empty() || v["responseStatus"].as_u64() == Some(403) {
            return Err(v["responseDetails"]
                .as_str()
                .unwrap_or("пустой ответ")
                .to_string());
        }
        Ok(Translation {
            text: out.to_string(),
            detected: source.to_string(),
            target: target.to_string(),
            engine: "mymemory".into(),
            alternatives: vec![],
            fallback_from: None,
        })
    }
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    let j = s[i..].find(end)? + i;
    Some(&s[i..j])
}

/// Локальная догадка о языке до запроса, ISO 639-1. Движок потом уточняет.
pub fn guess_lang(text: &str) -> Option<&'static str> {
    use whatlang::{Lang::*, Script};
    let info = whatlang::detect(text)?;
    let mapped = match info.lang() {
        Eng => Some("en"),
        Rus => Some("ru"),
        Ukr => Some("uk"),
        Bel => Some("be"),
        Deu => Some("de"),
        Fra => Some("fr"),
        Spa => Some("es"),
        Ita => Some("it"),
        Por => Some("pt"),
        Pol => Some("pl"),
        Tur => Some("tr"),
        Nld => Some("nl"),
        Swe => Some("sv"),
        Ces => Some("cs"),
        Jpn => Some("ja"),
        Kor => Some("ko"),
        Cmn => Some("zh"),
        Ara => Some("ar"),
        Heb => Some("he"),
        _ => None,
    };
    match info.script() {
        // Короткую кириллицу whatlang путает с болгарским и сербским — считаем русским, движок уточнит.
        Script::Cyrillic => match info.lang() {
            Ukr | Bel if info.confidence() >= 0.8 => mapped,
            _ => Some("ru"),
        },
        _ => mapped.filter(|_| info.confidence() >= 0.3),
    }
}

/// Если текст уже на основном языке — переводим на запасной (swap).
pub fn pick_target(detected: Option<&str>, primary: &str, secondary: &str) -> String {
    if detected == Some(primary) {
        secondary.to_string()
    } else {
        primary.to_string()
    }
}

/// Словарный режим: одно-два слова без цифр и знаков.
pub fn is_word_mode(text: &str) -> bool {
    let t = text.trim();
    t.chars().count() <= 30
        && t.split_whitespace().count() <= 2
        && t.chars()
            .all(|c| c.is_alphabetic() || c == ' ' || c == '-' || c == '\'')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_and_word_mode() {
        assert_eq!(pick_target(Some("ru"), "ru", "en"), "en");
        assert_eq!(pick_target(Some("en"), "ru", "en"), "ru");
        assert_eq!(pick_target(None, "ru", "en"), "ru");
        assert!(is_word_mode("resilient"));
        assert!(is_word_mode("give up"));
        assert!(!is_word_mode("The trick is to lean"));
        assert!(!is_word_mode("v2.0"));
        assert_eq!(between("x IG:\"ABC\" y", "IG:\"", "\""), Some("ABC"));
        assert_eq!(guess_lang("давай созвонимся после обеда"), Some("ru"));
    }

    #[test]
    fn cache_key_tracks_provider_preference_and_source_hint() {
        let google = vec!["google".to_string(), "bing".to_string()];
        let bing = vec!["bing".to_string(), "google".to_string()];
        let google_only = vec!["google".to_string()];
        let base = cache_key("hello", "ru", Some("en"), &google);
        assert_ne!(base, cache_key("hello", "ru", Some("en"), &bing));
        assert_ne!(
            base,
            cache_key("hello", "ru", Some("en"), &google_only),
            "ручной выбор движка не должен читать кэш автоматической цепочки"
        );
        assert_ne!(base, cache_key("hello", "ru", Some("de"), &google));
    }
}
