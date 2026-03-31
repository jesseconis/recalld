pub mod crypto;
pub mod db;

use anyhow::{Context, Result};
use chacha20poly1305::Key;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::config::Config;

/// High-level encrypted storage manager.
///
/// Coordinates database access + encrypted file I/O.
pub struct Storage {
    conn: Mutex<Connection>,
    dek: Key,
    screenshots_dir: PathBuf,
}

impl Storage {
    /// Open storage with the provided data-encryption key.
    pub fn open(config: &Config, dek: Key) -> Result<Self> {
        let screenshots_dir = config.screenshots_dir();
        std::fs::create_dir_all(&screenshots_dir)
            .context("failed to create screenshots directory")?;

        let conn = db::open(&config.db_path())?;

        Ok(Self {
            conn: Mutex::new(conn),
            dek,
            screenshots_dir,
        })
    }

    /// Store a processed capture: encrypted screenshot file + DB entry.
    pub fn store_entry(
        &self,
        app: &str,
        title: &str,
        text: &str,
        timestamp: i64,
        embedding: &[f32],
        screenshot_webp: &[u8],
    ) -> Result<()> {
        let filename = format!("{timestamp}.webp.enc");

        // Encrypt screenshot file
        let enc_screenshot = crypto::encrypt(screenshot_webp, &self.dek)?;
        let file_path = self.screenshots_dir.join(&filename);
        std::fs::write(&file_path, &enc_screenshot)
            .context("failed to write encrypted screenshot")?;

        // Encrypt OCR text
        let enc_text = crypto::encrypt(text.as_bytes(), &self.dek)?;

        // Encrypt embedding
        let emb_bytes: Vec<u8> = embedding
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let enc_embedding = crypto::encrypt(&emb_bytes, &self.dek)?;

        // Insert DB entry
        let conn = self.conn.lock().unwrap();
        db::insert_entry(&conn, app, title, &enc_text, timestamp, &enc_embedding, &filename)?;

        Ok(())
    }

    /// Decrypt and return a screenshot's raw WebP bytes.
    pub fn get_screenshot(&self, filename: &str) -> Result<Vec<u8>> {
        let path = self.screenshots_dir.join(filename);
        let enc = std::fs::read(&path).context("screenshot file not found")?;
        crypto::decrypt(&enc, &self.dek)
    }

    /// Decrypt OCR text from an encrypted blob.
    pub fn decrypt_text(&self, enc_text: &[u8]) -> Result<String> {
        let bytes = crypto::decrypt(enc_text, &self.dek)?;
        Ok(String::from_utf8(bytes)?)
    }

    /// Decrypt an embedding vector from an encrypted blob.
    pub fn decrypt_embedding(&self, enc_emb: &[u8]) -> Result<Vec<f32>> {
        let bytes = crypto::decrypt(enc_emb, &self.dek)?;
        if bytes.len() % 4 != 0 {
            anyhow::bail!("decrypted embedding has invalid length");
        }
        let floats: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect();
        Ok(floats)
    }

    /// Search entries by semantic similarity. Returns top-K results sorted by similarity descending.
    pub fn search(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<SearchResult>> {
        let page = self.search_paged(query_embedding, limit, 0)?;
        Ok(page.results)
    }

    /// Search entries by semantic similarity with pagination.
    pub fn search_paged(
        &self,
        query_embedding: &[f32],
        limit: usize,
        offset: usize,
    ) -> Result<PagedSearchResults> {
        let conn = self.conn.lock().unwrap();
        let entries = db::get_all_entries(&conn)?;

        let mut results: Vec<SearchResult> = entries
            .into_iter()
            .filter_map(|entry| {
                let emb = self.decrypt_embedding(&entry.embedding_enc).ok()?;
                let similarity = crate::embedding::cosine_similarity(query_embedding, &emb);
                let text = self.decrypt_text(&entry.text_enc).unwrap_or_default();
                Some(SearchResult {
                    id: entry.id,
                    app: entry.app,
                    title: entry.title,
                    text,
                    timestamp: entry.timestamp,
                    similarity,
                    screenshot_filename: entry.screenshot_filename,
                })
            })
            .collect();

        results.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap());
        let total = results.len();
        let page = results
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();

        Ok(PagedSearchResults {
            total,
            results: page,
        })
    }

    /// Get timeline entries between two timestamps.
    pub fn timeline(&self, from: i64, to: i64, limit: u32) -> Result<Vec<db::Entry>> {
        let page = self.timeline_paged(from, to, limit, 0)?;
        Ok(page.entries)
    }

    /// Get timeline entries between two timestamps with pagination.
    pub fn timeline_paged(
        &self,
        from: i64,
        to: i64,
        limit: u32,
        offset: u32,
    ) -> Result<TimelinePage> {
        let conn = self.conn.lock().unwrap();
        let total = db::count_timeline(&conn, from, to)?;
        let entries = db::get_timeline_paged(&conn, from, to, limit, offset)?;
        Ok(TimelinePage { total, entries })
    }

    /// Get a single entry's metadata and decrypted OCR text.
    pub fn entry_detail(&self, id: i64) -> Result<Option<EntryDetail>> {
        let conn = self.conn.lock().unwrap();
        let Some(entry) = db::get_entry_by_id(&conn, id)? else {
            return Ok(None);
        };
        drop(conn);

        let text = self.decrypt_text(&entry.text_enc).unwrap_or_default();
        Ok(Some(EntryDetail {
            id: entry.id,
            app: entry.app,
            title: entry.title,
            text,
            timestamp: entry.timestamp,
            screenshot_filename: entry.screenshot_filename,
        }))
    }

    /// Total number of stored entries.
    pub fn count(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        db::count_entries(&conn)
    }

    /// Most recent capture timestamp, or 0 if none.
    pub fn latest_timestamp(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        db::latest_timestamp(&conn)
    }

    /// Reference to the data-encryption key.
    pub fn dek(&self) -> &Key {
        &self.dek
    }

    /// Path to screenshots directory.
    pub fn screenshots_path(&self) -> &Path {
        &self.screenshots_dir
    }
}

/// A search result with decrypted text and similarity score.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: i64,
    pub app: String,
    pub title: String,
    pub text: String,
    pub timestamp: i64,
    pub similarity: f32,
    pub screenshot_filename: String,
}

/// A paginated semantic-search result page.
#[derive(Debug, Clone)]
pub struct PagedSearchResults {
    pub total: usize,
    pub results: Vec<SearchResult>,
}

/// A paginated timeline query result page.
#[derive(Debug, Clone)]
pub struct TimelinePage {
    pub total: i64,
    pub entries: Vec<db::Entry>,
}

/// Detailed entry data for detail panels.
#[derive(Debug, Clone)]
pub struct EntryDetail {
    pub id: i64,
    pub app: String,
    pub title: String,
    pub text: String,
    pub timestamp: i64,
    pub screenshot_filename: String,
}
