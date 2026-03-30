use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;

/// A row from the `entries` table.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: i64,
    pub app: String,
    pub title: String,
    /// Encrypted OCR text blob — decrypt with storage crypto before use.
    pub text_enc: Vec<u8>,
    pub timestamp: i64,
    /// Encrypted embedding vector blob — decrypt, then reinterpret as `Vec<f32>`.
    pub embedding_enc: Vec<u8>,
    pub screenshot_filename: String,
}

/// Initialise the database schema.
pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entries (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            app                 TEXT    NOT NULL,
            title               TEXT    NOT NULL,
            text_enc            BLOB    NOT NULL,
            timestamp           INTEGER NOT NULL UNIQUE,
            embedding_enc       BLOB    NOT NULL,
            screenshot_filename TEXT    NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_entries_timestamp ON entries (timestamp);",
    )
    .context("failed to initialise database schema")?;
    Ok(())
}

/// Open (or create) the database at `path` and initialise the schema.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path).context("failed to open database")?;
    // Performance tuning
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    init(&conn)?;
    Ok(conn)
}

/// Insert a new entry. Returns the new row ID.
pub fn insert_entry(
    conn: &Connection,
    app: &str,
    title: &str,
    text_enc: &[u8],
    timestamp: i64,
    embedding_enc: &[u8],
    screenshot_filename: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO entries (app, title, text_enc, timestamp, embedding_enc, screenshot_filename)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(timestamp) DO NOTHING",
        params![app, title, text_enc, timestamp, embedding_enc, screenshot_filename],
    )
    .context("failed to insert entry")?;
    Ok(conn.last_insert_rowid())
}

/// Retrieve all entries (for embedding search).
pub fn get_all_entries(conn: &Connection) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "SELECT id, app, title, text_enc, timestamp, embedding_enc, screenshot_filename
         FROM entries ORDER BY timestamp DESC",
    )?;
    let entries = stmt
        .query_map([], |row| {
            Ok(Entry {
                id: row.get(0)?,
                app: row.get(1)?,
                title: row.get(2)?,
                text_enc: row.get(3)?,
                timestamp: row.get(4)?,
                embedding_enc: row.get(5)?,
                screenshot_filename: row.get(6)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read entries")?;
    Ok(entries)
}

/// Get a timeline of entries between two timestamps (inclusive).
pub fn get_timeline(
    conn: &Connection,
    from: i64,
    to: i64,
    limit: u32,
) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "SELECT id, app, title, text_enc, timestamp, embedding_enc, screenshot_filename
         FROM entries
         WHERE timestamp >= ?1 AND timestamp <= ?2
         ORDER BY timestamp DESC
         LIMIT ?3",
    )?;
    let entries = stmt
        .query_map(params![from, to, limit], |row| {
            Ok(Entry {
                id: row.get(0)?,
                app: row.get(1)?,
                title: row.get(2)?,
                text_enc: row.get(3)?,
                timestamp: row.get(4)?,
                embedding_enc: row.get(5)?,
                screenshot_filename: row.get(6)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read timeline entries")?;
    Ok(entries)
}

/// Get the total number of entries.
pub fn count_entries(conn: &Connection) -> Result<i64> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))?;
    Ok(count)
}

/// Get the most recent timestamp, or 0 if no entries.
pub fn latest_timestamp(conn: &Connection) -> Result<i64> {
    let ts: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(timestamp), 0) FROM entries",
            [],
            |row| row.get(0),
        )?;
    Ok(ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        conn
    }

    #[test]
    fn insert_and_count() {
        let conn = mem_db();
        insert_entry(&conn, "firefox", "GitHub", b"enc-text", 1000, b"enc-emb", "1000.webp")
            .unwrap();
        assert_eq!(count_entries(&conn).unwrap(), 1);
    }

    #[test]
    fn duplicate_timestamp_ignored() {
        let conn = mem_db();
        insert_entry(&conn, "firefox", "A", b"t1", 1000, b"e1", "a.webp").unwrap();
        insert_entry(&conn, "firefox", "B", b"t2", 1000, b"e2", "b.webp").unwrap();
        assert_eq!(count_entries(&conn).unwrap(), 1);
    }

    #[test]
    fn timeline_query() {
        let conn = mem_db();
        for ts in [100, 200, 300, 400, 500] {
            insert_entry(&conn, "app", "t", b"t", ts, b"e", &format!("{ts}.webp")).unwrap();
        }
        let entries = get_timeline(&conn, 200, 400, 100).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].timestamp, 400); // DESC order
    }
}
