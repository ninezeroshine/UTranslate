//! История переводов и избранное в SQLite.

use std::{path::Path, sync::Mutex};

use rusqlite::{params, Connection};
use serde::Serialize;

pub struct Db(Mutex<Connection>);

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: i64,
    pub source_text: String,
    pub result_text: String,
    pub source_lang: String,
    pub target_lang: String,
    pub engine: String,
    pub mode: String,
    pub is_favorite: bool,
    pub created_at: i64,
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS translations (
               id           INTEGER PRIMARY KEY,
               source_text  TEXT NOT NULL,
               result_text  TEXT NOT NULL,
               source_lang  TEXT NOT NULL,
               target_lang  TEXT NOT NULL,
               engine       TEXT NOT NULL,
               mode         TEXT NOT NULL,
               is_favorite  INTEGER NOT NULL DEFAULT 0,
               created_at   INTEGER NOT NULL,
               UNIQUE(source_text, target_lang)
             );
             CREATE INDEX IF NOT EXISTS idx_translations_created ON translations(created_at DESC);",
        )?;
        Ok(Self(Mutex::new(conn)))
    }

    /// Дубль по паре (оригинал, целевой язык) обновляет запись и дату, не создаёт новую.
    pub fn add(&self, source: &str, result: &str, source_lang: &str, target_lang: &str, engine: &str, mode: &str) -> rusqlite::Result<i64> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.0.lock().unwrap().query_row(
            "INSERT INTO translations (source_text, result_text, source_lang, target_lang, engine, mode, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(source_text, target_lang) DO UPDATE SET
               result_text = excluded.result_text, engine = excluded.engine, mode = excluded.mode, created_at = excluded.created_at
             RETURNING id",
            params![source, result, source_lang, target_lang, engine, mode, now],
            |r| r.get(0),
        )
    }

    pub fn list(&self, query: &str, favorites_only: bool, limit: u32) -> rusqlite::Result<Vec<Entry>> {
        let conn = self.0.lock().unwrap();
        let like = format!("%{}%", query.trim());
        let mut stmt = conn.prepare(
            "SELECT id, source_text, result_text, source_lang, target_lang, engine, mode, is_favorite, created_at
             FROM translations
             WHERE (?1 = '' OR source_text LIKE ?2 OR result_text LIKE ?2)
               AND (?3 = 0 OR is_favorite = 1)
             ORDER BY created_at DESC LIMIT ?4",
        )?;
        let rows = stmt.query_map(params![query.trim(), like, favorites_only as i32, limit], |r| {
            Ok(Entry {
                id: r.get(0)?,
                source_text: r.get(1)?,
                result_text: r.get(2)?,
                source_lang: r.get(3)?,
                target_lang: r.get(4)?,
                engine: r.get(5)?,
                mode: r.get(6)?,
                is_favorite: r.get::<_, i32>(7)? != 0,
                created_at: r.get(8)?,
            })
        })?;
        rows.collect()
    }

    pub fn set_favorite(&self, id: i64, favorite: bool) -> rusqlite::Result<()> {
        self.0
            .lock()
            .unwrap()
            .execute("UPDATE translations SET is_favorite = ?1 WHERE id = ?2", params![favorite as i32, id])?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute("DELETE FROM translations WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// CSV избранного для импорта в Anki и таблицы: оригинал; перевод; языки.
    pub fn favorites_csv(&self) -> rusqlite::Result<String> {
        let esc = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let mut out = String::from("source;translation;from;to\n");
        for e in self.list("", true, 100_000)? {
            out.push_str(&format!("{};{};{};{}\n", esc(&e.source_text), esc(&e.result_text), e.source_lang, e.target_lang));
        }
        Ok(out)
    }

    /// Очистка истории не трогает избранное.
    pub fn clear(&self) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute("DELETE FROM translations WHERE is_favorite = 0", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_and_favorites() {
        let dir = std::env::temp_dir().join(format!("utranslate-test-{}.db", std::process::id()));
        let db = Db::open(&dir).unwrap();
        let a = db.add("hello", "привет", "en", "ru", "google", "popup").unwrap();
        let b = db.add("hello", "привет!", "en", "ru", "bing", "popup").unwrap();
        assert_eq!(a, b, "дубль должен обновить запись, а не создать новую");
        assert_eq!(db.list("", false, 10).unwrap().len(), 1);
        assert_eq!(db.list("", false, 10).unwrap()[0].result_text, "привет!");
        db.set_favorite(a, true).unwrap();
        db.clear().unwrap();
        assert_eq!(db.list("", true, 10).unwrap().len(), 1, "очистка не трогает избранное");
        let _ = std::fs::remove_file(&dir);
    }
}
