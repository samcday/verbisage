use std::error::Error;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags};

use super::{DictionaryBackend, DictionaryQuery, DictionaryResult, SharedQueryCache};

type SharedError = Box<dyn Error + Send + Sync>;

/// Dictionary backend backed by a SQLite table.
///
/// The expected table schema is:
///
/// ```sql
/// CREATE TABLE words (
///     word      TEXT    NOT NULL,
///     frequency REAL    NOT NULL        -- or INTEGER
/// );
/// ```
///
/// Column names are configurable via the constructor.
///
/// # Thread safety
///
/// The inner [`rusqlite::Connection`] is wrapped in a [`Mutex`] so that the
/// backend implements [`Sync`].  Clone opens a fresh `:memory:` database
/// and is *not* connected to the original file.
pub struct SqliteDictionaryBackend {
    conn: Mutex<Connection>,
    table_name: String,
    word_column: String,
    frequency_column: String,
    cache: SharedQueryCache,
}

impl SqliteDictionaryBackend {
    /// Open a SQLite database at `path` read-write.
    ///
    /// The table is expected to already exist with the correct schema.
    /// No indexes are created — call [`ensure_table`](Self::ensure_table)
    /// explicitly (e.g. from [`add_word`](Self::add_word)) if needed.
    pub fn from_sqlite<P: AsRef<Path>>(
        path: P,
        table_name: &str,
        word_column: &str,
        frequency_column: &str,
    ) -> Result<Self, SharedError> {
        Ok(Self {
            conn: Mutex::new(Connection::open(path)?),
            table_name: table_name.to_string(),
            word_column: word_column.to_string(),
            frequency_column: frequency_column.to_string(),
            cache: SharedQueryCache::new(),
        })
    }

    /// Open a SQLite database **read-only** (system dictionaries).
    ///
    /// Does not create any files or indexes.  Returns an error if the file
    /// does not exist.
    pub fn from_sqlite_readonly<P: AsRef<Path>>(
        path: P,
        table_name: &str,
        word_column: &str,
        frequency_column: &str,
    ) -> Result<Self, SharedError> {
        Ok(Self {
            conn: Mutex::new(Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?),
            table_name: table_name.to_string(),
            word_column: word_column.to_string(),
            frequency_column: frequency_column.to_string(),
            cache: SharedQueryCache::new(),
        })
    }

    /// Check whether the expected table exists in the database.
    pub fn table_exists(&self) -> bool {
        let sql = "SELECT name FROM sqlite_master WHERE type='table' AND name=?1";
        let conn = self.conn.lock().unwrap();
        conn.prepare(sql)
            .and_then(|mut stmt| stmt.exists([&self.table_name]))
            .unwrap_or(false)
    }

    /// Ensure the table and indexes exist (idempotent).
    /// Safe to call even when the table already exists.
    fn ensure_table(&self) {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {} ({} TEXT NOT NULL, {} REAL NOT NULL)",
            self.table_name, self.word_column, self.frequency_column,
        );
        if let Err(e) = conn.execute(&sql, []) {
            eprintln!("warning: sqlite ensure_table failed — {}", e);
            return;
        }
        let idx_word = format!("idx_{}_{}", self.table_name, self.word_column);
        let _ = conn.execute(
            &format!(
                "CREATE INDEX IF NOT EXISTS {} ON {}({})",
                idx_word, self.table_name, self.word_column,
            ),
            [],
        );
        let idx_len = format!("idx_{}_{}_length", self.table_name, self.word_column);
        let _ = conn.execute(
            &format!(
                "CREATE INDEX IF NOT EXISTS {} ON {}(LENGTH({}))",
                idx_len, self.table_name, self.word_column,
            ),
            [],
        );
    }

    /// Insert or update a word's frequency.
    ///
    /// Creates the table and indexes on first call (lazy initialization).
    pub fn add_word(&self, word: &str, frequency: f64) {
        self.ensure_table();
        let sql = format!(
            "INSERT OR REPLACE INTO {} ({}, {}) VALUES (?1, ?2)",
            self.table_name, self.word_column, self.frequency_column,
        );
        let conn = self.conn.lock().unwrap();
        if let Err(e) = conn.execute(&sql, [word, &frequency.to_string()]) {
            eprintln!("warning: sqlite add_word failed — {}", e);
        }
    }

    /// Create an in-memory database (`:memory:`).
    pub fn new() -> Self {
        Self {
            conn: Mutex::new(Connection::open(":memory:").unwrap()),
            table_name: "words".to_string(),
            word_column: "word".to_string(),
            frequency_column: "frequency".to_string(),
            cache: SharedQueryCache::new(),
        }
    }

    pub fn enable_cache(&self, size_limit: usize) {
        self.cache.set_limit(size_limit);
    }

    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Return the frequency of `word`, or 0.0 if absent.
    pub fn frequency(&self, word: &str) -> f64 {
        let sql = format!(
            "SELECT {} FROM {} WHERE {} = ?1 LIMIT 1",
            self.frequency_column, self.table_name, self.word_column
        );
        let conn = self.conn.lock().unwrap();
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(mut rows) = stmt.query_map([word], |row| match row.get::<_, f64>(0) {
                Ok(f) => Ok(f),
                Err(_) => {
                    let int_freq: i64 = row.get(0)?;
                    Ok(int_freq as f64)
                }
            }) {
                if let Some(result) = rows.next() {
                    return result.unwrap_or(0.0);
                }
            }
        }
        0.0
    }

    pub fn len(&self) -> usize {
        let sql = format!("SELECT COUNT(*) FROM {}", self.table_name);
        let conn = self.conn.lock().unwrap();
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(mut rows) = stmt.query_map([], |row| row.get::<_, i64>(0)) {
                if let Some(result) = rows.next() {
                    return result.unwrap_or(0) as usize;
                }
            }
        }
        0
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl DictionaryBackend for SqliteDictionaryBackend {
    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        self.cache.get_or_compute(queries, |queries| {
            if queries.is_empty() {
                return Vec::new();
            }

            let mut clauses = Vec::new();
            for q in queries {
                let mut cond = Vec::new();

                if let Some(ref prefix) = q.prefix {
                    cond.push(format!("{} LIKE '{}%'", self.word_column, prefix));
                }
                if let Some(ref suffix) = q.suffix {
                    cond.push(format!("{} LIKE '%{}'", self.word_column, suffix));
                }
                if let Some(min) = q.min_length {
                    cond.push(format!("LENGTH({}) >= {}", self.word_column, min));
                }
                if let Some(max) = q.max_length {
                    if max != usize::MAX {
                        cond.push(format!("LENGTH({}) <= {}", self.word_column, max));
                    }
                }

                let clause = if cond.is_empty() {
                    "1".to_string()
                } else {
                    cond.join(" AND ")
                };
                clauses.push(format!("({})", clause));
            }

            let sql = format!(
                "SELECT {}, {} FROM {} WHERE {}",
                self.word_column,
                self.frequency_column,
                self.table_name,
                clauses.join(" OR ")
            );

            let mut all_results = Vec::new();
            let conn = self.conn.lock().unwrap();
            match conn.prepare(&sql) {
                Ok(mut stmt) => {
                    if let Ok(rows) = stmt.query_map([], |row| {
                        let word: String = row.get(0)?;
                        let frequency = match row.get::<_, f64>(1) {
                            Ok(f) => f,
                            Err(_) => {
                                let int_freq: i64 = row.get(1)?;
                                int_freq as f64
                            }
                        };
                        Ok(DictionaryResult {
                            word,
                            confidence: if frequency > 0.0 { frequency } else { -1.0 },
                        })
                    }) {
                        for row in rows.flatten() {
                            all_results.push(row);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("warning: sqlite query failed — {}", e);
                }
            }

            all_results.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.word.cmp(&b.word))
            });

            all_results
        })
    }

    fn get_frequency(&self, word: &str) -> f64 {
        self.frequency(word)
    }

    fn contains(&self, word: &str) -> bool {
        let sql = format!(
            "SELECT 1 FROM {} WHERE {} = ?1 LIMIT 1",
            self.table_name, self.word_column
        );
        let conn = self.conn.lock().unwrap();
        conn.prepare(&sql)
            .and_then(|mut stmt| stmt.exists([word]))
            .unwrap_or(false)
    }
}

impl Clone for SqliteDictionaryBackend {
    /// Returns a new backend pointing at a fresh `:memory:` database.
    ///
    /// **Important:** this does *not* open a second connection to the
    /// original file.  The clone is fully independent and empty.
    fn clone(&self) -> Self {
        Self {
            conn: Mutex::new(Connection::open(":memory:").unwrap()),
            table_name: self.table_name.clone(),
            word_column: self.word_column.clone(),
            frequency_column: self.frequency_column.clone(),
            cache: SharedQueryCache::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn basic_sqlite_operations() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE ngrams (word TEXT NOT NULL, frequency REAL NOT NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ngrams (word, frequency) VALUES (?1, ?2)",
                ["hello", "100.0"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ngrams (word, frequency) VALUES (?1, ?2)",
                ["world", "50.5"],
            )
            .unwrap();
        }

        let backend =
            SqliteDictionaryBackend::from_sqlite(&db_path, "ngrams", "word", "frequency").unwrap();

        assert_eq!(backend.len(), 2);
        assert!(backend.contains("hello"));
        assert!(!backend.contains("foo"));
        assert_eq!(backend.frequency("hello"), 100.0);
        assert_eq!(backend.frequency("world"), 50.5);
    }

    #[test]
    fn integer_frequencies() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_int.db");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE words (term TEXT NOT NULL, count INTEGER NOT NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO words (term, count) VALUES (?1, ?2)",
                ["apple", "42"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO words (term, count) VALUES (?1, ?2)",
                ["banana", "17"],
            )
            .unwrap();
        }

        let backend =
            SqliteDictionaryBackend::from_sqlite(&db_path, "words", "term", "count").unwrap();

        assert_eq!(backend.frequency("apple"), 42.0);
        assert_eq!(backend.frequency("banana"), 17.0);
    }

    #[test]
    fn prefix_query() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_pref.db");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute("CREATE TABLE dict (w TEXT NOT NULL, f REAL NOT NULL)", [])
                .unwrap();
            for (w, f) in &[("hello", 10.0), ("help", 5.0), ("world", 1.0)] {
                conn.execute(
                    "INSERT INTO dict (w, f) VALUES (?1, ?2)",
                    [*w, &f.to_string()],
                )
                .unwrap();
            }
        }

        let backend = SqliteDictionaryBackend::from_sqlite(&db_path, "dict", "w", "f").unwrap();
        let results = backend.query_prefixes(&[DictionaryQuery {
            prefix: Some("hel".to_string()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.word == "hello"));
        assert!(results.iter().any(|r| r.word == "help"));
    }
}
