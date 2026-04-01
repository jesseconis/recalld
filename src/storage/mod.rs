pub mod crypto;
pub mod db;

use anyhow::{Context, Result};
use chacha20poly1305::Key;
use rusqlite::Connection;
use std::cmp::Ordering;
use std::collections::HashSet;
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
        context: Option<&crate::metadata::ContextSnapshot>,
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

        let enc_context = context
            .map(|ctx| serde_json::to_vec(ctx))
            .transpose()
            .context("failed to serialise capture context")?
            .map(|bytes| crypto::encrypt(&bytes, &self.dek))
            .transpose()?;

        // Insert DB entry
        let conn = self.conn.lock().unwrap();
        db::insert_entry(
            &conn,
            app,
            title,
            &enc_text,
            timestamp,
            &enc_embedding,
            enc_context.as_deref(),
            &filename,
        )?;

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

    /// Decrypt and decode a context snapshot blob.
    pub fn decrypt_context(
        &self,
        enc_context: Option<&[u8]>,
    ) -> Result<Option<crate::metadata::ContextSnapshot>> {
        let Some(enc) = enc_context else {
            return Ok(None);
        };
        let bytes = crypto::decrypt(enc, &self.dek)?;
        let snapshot = serde_json::from_slice(&bytes).context("failed to decode context JSON")?;
        Ok(Some(snapshot))
    }

    /// Search entries by hybrid similarity. Returns top-K results sorted by blended score.
    pub fn search(
        &self,
        query: &str,
        query_embedding: &[f32],
        lexical_weight: f32,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let page = self.search_paged(query, query_embedding, lexical_weight, limit, 0)?;
        Ok(page.results)
    }

    /// Search entries by hybrid similarity with pagination.
    pub fn search_paged(
        &self,
        query: &str,
        query_embedding: &[f32],
        lexical_weight: f32,
        limit: usize,
        offset: usize,
    ) -> Result<PagedSearchResults> {
        let conn = self.conn.lock().unwrap();
        let entries = db::get_all_entries(&conn)?;
        drop(conn);

        let lexical_weight = lexical_weight.clamp(0.0, 1.0);

        let mut results: Vec<SearchResult> = entries
            .into_iter()
            .filter_map(|entry| {
                let emb = self.decrypt_embedding(&entry.embedding_enc).ok()?;
                let semantic = crate::embedding::cosine_similarity(query_embedding, &emb);
                let text = self.decrypt_text(&entry.text_enc).unwrap_or_default();
                let lexical = lexical_similarity(query, &text);
                let similarity = blend_similarity(semantic, lexical, lexical_weight);
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

        results.sort_by(rank_cmp);
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
        let context = self.decrypt_context(entry.context_enc.as_deref()).unwrap_or(None);
        Ok(Some(EntryDetail {
            id: entry.id,
            app: entry.app,
            title: entry.title,
            text,
            context,
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
    pub context: Option<crate::metadata::ContextSnapshot>,
    pub timestamp: i64,
    pub screenshot_filename: String,
}

fn rank_cmp(a: &SearchResult, b: &SearchResult) -> Ordering {
    b.similarity
        .partial_cmp(&a.similarity)
        .unwrap_or(Ordering::Equal)
        .then_with(|| b.timestamp.cmp(&a.timestamp))
        .then_with(|| b.id.cmp(&a.id))
}

fn blend_similarity(semantic_cosine: f32, lexical: f32, lexical_weight: f32) -> f32 {
    let semantic_norm = ((semantic_cosine + 1.0) / 2.0).clamp(0.0, 1.0);
    ((1.0 - lexical_weight) * semantic_norm + lexical_weight * lexical.clamp(0.0, 1.0))
        .clamp(0.0, 1.0)
}

fn lexical_similarity(query: &str, text: &str) -> f32 {
    let query_tokens = normalize_tokens(query);
    if query_tokens.is_empty() {
        return 0.0;
    }

    let text_tokens = normalize_tokens(text);
    if text_tokens.is_empty() {
        return 0.0;
    }

    let text_set = text_tokens.iter().cloned().collect::<HashSet<_>>();
    let mut token_score = 0.0;

    for token in &query_tokens {
        if text_set.contains(token) {
            token_score += 1.0;
            continue;
        }
        let fuzzy_best = text_tokens
            .iter()
            .map(|candidate| normalized_similarity(token, candidate))
            .fold(0.0, f32::max);
        if fuzzy_best >= 0.82 {
            token_score += fuzzy_best;
        }
    }

    let token_component = token_score / query_tokens.len() as f32;

    let query_compact = compact_alnum_lower(query);
    let text_compact = compact_alnum_lower(text);
    let phrase_component = if query_compact.is_empty() || text_compact.is_empty() {
        0.0
    } else if text_compact.contains(&query_compact) {
        1.0
    } else {
        0.0
    };

    (0.85 * token_component + 0.15 * phrase_component).clamp(0.0, 1.0)
}

fn normalize_tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .map(compact_alnum_lower)
        .filter(|token| !token.is_empty())
        .collect()
}

fn compact_alnum_lower(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn normalized_similarity(left: &str, right: &str) -> f32 {
    let max_len = left.chars().count().max(right.chars().count());
    if max_len == 0 {
        return 1.0;
    }
    let distance = levenshtein_distance(left, right) as f32;
    (1.0 - distance / max_len as f32).clamp(0.0, 1.0)
}

fn levenshtein_distance(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut prev = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut curr = vec![0usize; right_chars.len() + 1];

    for (i, left_char) in left.chars().enumerate() {
        curr[0] = i + 1;
        for (j, right_char) in right_chars.iter().enumerate() {
            let cost = usize::from(left_char != *right_char);
            curr[j + 1] = (curr[j] + 1)
                .min(prev[j + 1] + 1)
                .min(prev[j] + cost);
        }
        prev.clone_from(&curr);
    }

    prev[right_chars.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_similarity_matches_ocr_typos() {
        let query = "github recalld";
        let text = "opened gitnub recaild repository";
        let score = lexical_similarity(query, text);
        assert!(score >= 0.70, "score was {score}");
    }

    #[test]
    fn blend_similarity_respects_weight_extremes() {
        let semantic = -0.5;
        let lexical = 0.9;
        assert_eq!(blend_similarity(semantic, lexical, 0.0), 0.25);
        assert_eq!(blend_similarity(semantic, lexical, 1.0), 0.9);
    }

    #[test]
    fn rank_sort_is_deterministic_on_ties() {
        let mut rows = vec![
            SearchResult {
                id: 1,
                app: String::new(),
                title: String::new(),
                text: String::new(),
                timestamp: 100,
                similarity: 0.8,
                screenshot_filename: String::new(),
            },
            SearchResult {
                id: 2,
                app: String::new(),
                title: String::new(),
                text: String::new(),
                timestamp: 101,
                similarity: 0.8,
                screenshot_filename: String::new(),
            },
            SearchResult {
                id: 3,
                app: String::new(),
                title: String::new(),
                text: String::new(),
                timestamp: 101,
                similarity: 0.8,
                screenshot_filename: String::new(),
            },
        ];

        rows.sort_by(rank_cmp);

        assert_eq!(rows[0].id, 3);
        assert_eq!(rows[1].id, 2);
        assert_eq!(rows[2].id, 1);
    }
}
