use std::error::Error;
use std::path::Path;

use crate::backends::SharedSqliteConnection;

use super::{DictionaryBackend, DictionaryQuery, DictionaryResult, SharedQueryCache};

type SharedError = Box<dyn Error + Send + Sync>;

/// Dictionary backend backed by a SQLite table.
///
/// Two modes:
///
/// 1. **Simple mode** (default): queries a `(word, frequency)` table directly.
///    Use `from_sqlite`, `from_sqlite_readonly`, `from_shared`, or `new`.
///
/// 2. **Ngram unigram mode**: queries the ngram table's unigram rows
///    (`context IS NULL`) instead of a dedicated words table. Use
///    `from_ngram_unigrams`. This is the correct mode for Presage SQLite
///    data where the `_1_gram` table is both the dictionary and the
///    unigram frequency source.
///
/// # Thread safety
///
/// The inner [`rusqlite::Connection`] is wrapped in a [`SharedSqliteConnection`]
/// so that the backend implements [`Sync`].  The connection can be shared
/// with a [`SmoothedPredictor`](crate::prediction::smoothed::SmoothedPredictor) backed
/// by a [`SqliteNgramBackend`](crate::prediction::sqlite::SqliteNgramBackend) when both
/// unigrams and n-grams come from the same database file.
pub struct SqliteDictionaryBackend {
    conn: SharedSqliteConnection,
    table_name: String,
    word_column: String,
    frequency_column: String,
    cache: SharedQueryCache,
    writable: bool,
    /// When true, queries the ngram table's unigram rows (context IS NULL)
    /// instead of a dedicated (word, frequency) table.
    ngram_unigram_mode: bool,
    /// Column names for the ngram table (only used when ngram_unigram_mode is true).
    ngram_context_columns: Vec<String>,
    ngram_next_word_column: String,
}

impl SqliteDictionaryBackend {
    /// Open a SQLite database at `path` with the given writability.
    pub fn from_sqlite<P: AsRef<Path>>(
        path: P,
        table_name: &str,
        word_column: &str,
        frequency_column: &str,
        writable: bool,
    ) -> Result<Self, SharedError> {
        Ok(Self {
            conn: SharedSqliteConnection::open(path.as_ref())?,
            table_name: table_name.to_string(),
            word_column: word_column.to_string(),
            frequency_column: frequency_column.to_string(),
            cache: SharedQueryCache::new(),
            writable,
            ngram_unigram_mode: false,
            ngram_context_columns: Vec::new(),
            ngram_next_word_column: String::new(),
        })
    }

    /// Open a SQLite database **read-only** (system dictionaries).
    pub fn from_sqlite_readonly<P: AsRef<Path>>(
        path: P,
        table_name: &str,
        word_column: &str,
        frequency_column: &str,
    ) -> Result<Self, SharedError> {
        Self::from_sqlite(path, table_name, word_column, frequency_column, false)
    }

    /// Wrap a shared connection (allows sharing with a predictor).
    pub fn from_shared(
        conn: SharedSqliteConnection,
        table_name: &str,
        word_column: &str,
        frequency_column: &str,
        writable: bool,
    ) -> Self {
        Self {
            conn,
            table_name: table_name.to_string(),
            word_column: word_column.to_string(),
            frequency_column: frequency_column.to_string(),
            cache: SharedQueryCache::new(),
            writable,
            ngram_unigram_mode: false,
            ngram_context_columns: Vec::new(),
            ngram_next_word_column: String::new(),
        }
    }

    /// Create an in-memory database (`:memory:`).
    pub fn new() -> Self {
        Self {
            conn: SharedSqliteConnection::in_memory(),
            table_name: "words".to_string(),
            word_column: "word".to_string(),
            frequency_column: "frequency".to_string(),
            cache: SharedQueryCache::new(),
            writable: true,
            ngram_unigram_mode: false,
            ngram_context_columns: Vec::new(),
            ngram_next_word_column: String::new(),
        }
    }

    /// Create a backend that queries the ngram table's unigram rows.
    ///
    /// This is the correct constructor for Presage SQLite data where the
    /// unigram table is both the dictionary and the unigram frequency source.
    pub fn from_ngram_unigrams(
        conn: SharedSqliteConnection,
        table_name: &str,
        context_columns: &[String],
        next_word_column: &str,
        frequency_column: &str,
        writable: bool,
    ) -> Self {
        Self {
            conn,
            table_name: table_name.to_string(),
            word_column: next_word_column.to_string(),
            frequency_column: frequency_column.to_string(),
            cache: SharedQueryCache::new(),
            writable,
            ngram_unigram_mode: true,
            ngram_context_columns: context_columns.to_vec(),
            ngram_next_word_column: next_word_column.to_string(),
        }
    }

    /// Check whether the expected table exists in the database.
    pub fn table_exists(&self) -> bool {
        let sql = "SELECT name FROM sqlite_master WHERE type='table' AND name=?1";
        let conn = self.conn.lock();
        conn.prepare(sql)
            .and_then(|mut stmt| stmt.exists([&self.table_name]))
            .unwrap_or(false)
    }

    /// Ensure the table and indexes exist (idempotent).
    fn ensure_table(&self) {
        let conn = self.conn.lock();
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

    /// Return the frequency of `word`, or 0.0 if absent.
    pub fn frequency(&self, word: &str) -> f64 {
        if self.ngram_unigram_mode {
            self.frequency_ngram_unigram(word)
        } else {
            self.frequency_simple(word)
        }
    }

    fn frequency_simple(&self, word: &str) -> f64 {
        let sql = format!(
            "SELECT {} FROM {} WHERE {} = ?1 LIMIT 1",
            self.frequency_column, self.table_name, self.word_column
        );
        let conn = self.conn.lock();
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

    fn frequency_ngram_unigram(&self, word: &str) -> f64 {
        let null_conditions: Vec<String> = self
            .ngram_context_columns
            .iter()
            .map(|c| format!("{} IS NULL", c))
            .collect();
        let sql = format!(
            "SELECT {} FROM {} WHERE {} AND {} = ?1 LIMIT 1",
            self.frequency_column,
            self.table_name,
            null_conditions.join(" AND "),
            self.ngram_next_word_column
        );
        let conn = self.conn.lock();
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
        if self.ngram_unigram_mode {
            self.len_ngram_unigram()
        } else {
            self.len_simple()
        }
    }

    fn len_simple(&self) -> usize {
        let sql = format!("SELECT COUNT(*) FROM {}", self.table_name);
        let conn = self.conn.lock();
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(mut rows) = stmt.query_map([], |row| row.get::<_, i64>(0)) {
                if let Some(result) = rows.next() {
                    return result.unwrap_or(0) as usize;
                }
            }
        }
        0
    }

    fn len_ngram_unigram(&self) -> usize {
        let null_conditions: Vec<String> = self
            .ngram_context_columns
            .iter()
            .map(|c| format!("{} IS NULL", c))
            .collect();
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE {}",
            self.table_name,
            null_conditions.join(" AND ")
        );
        let conn = self.conn.lock();
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

    pub fn enable_cache(&self, size_limit: usize) {
        self.cache.set_limit(size_limit);
    }

    pub fn clear_cache(&self) {
        self.cache.clear();
    }
}

impl DictionaryBackend for SqliteDictionaryBackend {
    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        if self.ngram_unigram_mode {
            self.query_prefixes_ngram_unigram(queries)
        } else {
            self.query_prefixes_simple(queries)
        }
    }

    fn get_frequency(&self, word: &str) -> f64 {
        self.frequency(word)
    }

    fn contains(&self, word: &str) -> bool {
        if self.ngram_unigram_mode {
            self.contains_ngram_unigram(word)
        } else {
            self.contains_simple(word)
        }
    }

    fn is_writable(&self) -> bool {
        self.writable
    }

    fn add_word(
        &self,
        word: &str,
        frequency: f64,
        allow_existing: bool,
    ) -> Result<(), SharedError> {
        if !self.writable {
            return Err("backend is not writable".into());
        }

        if self.ngram_unigram_mode {
            self.add_word_ngram_unigram(word, frequency, allow_existing)
        } else {
            self.add_word_simple(word, frequency, allow_existing)
        }
    }
}

impl SqliteDictionaryBackend {
    // -- Simple mode implementations --

    fn query_prefixes_simple(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
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
            let conn = self.conn.lock();
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

    fn contains_simple(&self, word: &str) -> bool {
        let sql = format!(
            "SELECT 1 FROM {} WHERE {} = ?1 LIMIT 1",
            self.table_name, self.word_column
        );
        let conn = self.conn.lock();
        conn.prepare(&sql)
            .and_then(|mut stmt| stmt.exists([word]))
            .unwrap_or(false)
    }

    fn add_word_simple(
        &self,
        word: &str,
        frequency: f64,
        allow_existing: bool,
    ) -> Result<(), SharedError> {
        self.ensure_table();

        let sql = if allow_existing {
            format!(
                "INSERT OR REPLACE INTO {} ({}, {}) VALUES (?1, ?2)",
                self.table_name, self.word_column, self.frequency_column
            )
        } else {
            format!(
                "INSERT INTO {} ({}, {}) VALUES (?1, ?2)",
                self.table_name, self.word_column, self.frequency_column
            )
        };

        let conn = self.conn.lock();
        let result = conn.execute(&sql, [word, &frequency.to_string()]);

        if !allow_existing {
            if let Err(ref e) = result {
                let err_str = e.to_string();
                if err_str.contains("UNIQUE") || err_str.contains("unique") {
                    return Err(format!("word '{}' already exists", word).into());
                }
            }
        }

        result.map(|_| ()).map_err(Into::into)
    }

    // -- Ngram unigram mode implementations --

    fn query_prefixes_ngram_unigram(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        self.cache.get_or_compute(queries, |queries| {
            if queries.is_empty() {
                return Vec::new();
            }

            let null_conditions: Vec<String> = self
                .ngram_context_columns
                .iter()
                .map(|c| format!("{} IS NULL", c))
                .collect();
            let base_where = null_conditions.join(" AND ");

            let mut clauses = Vec::new();
            for q in queries {
                let mut cond = vec![base_where.clone()];

                if let Some(ref prefix) = q.prefix {
                    cond.push(format!(
                        "{} LIKE '{}%'",
                        self.ngram_next_word_column, prefix
                    ));
                }
                if let Some(ref suffix) = q.suffix {
                    cond.push(format!(
                        "{} LIKE '%{}'",
                        self.ngram_next_word_column, suffix
                    ));
                }
                if let Some(min) = q.min_length {
                    cond.push(format!(
                        "LENGTH({}) >= {}",
                        self.ngram_next_word_column, min
                    ));
                }
                if let Some(max) = q.max_length {
                    if max != usize::MAX {
                        cond.push(format!(
                            "LENGTH({}) <= {}",
                            self.ngram_next_word_column, max
                        ));
                    }
                }

                let clause = cond.join(" AND ");
                clauses.push(format!("({})", clause));
            }

            let sql = format!(
                "SELECT {}, {} FROM {} WHERE {}",
                self.ngram_next_word_column,
                self.frequency_column,
                self.table_name,
                clauses.join(" OR ")
            );

            let mut all_results = Vec::new();
            let conn = self.conn.lock();
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

    fn contains_ngram_unigram(&self, word: &str) -> bool {
        let null_conditions: Vec<String> = self
            .ngram_context_columns
            .iter()
            .map(|c| format!("{} IS NULL", c))
            .collect();
        let sql = format!(
            "SELECT 1 FROM {} WHERE {} AND {} = ?1 LIMIT 1",
            self.table_name,
            null_conditions.join(" AND "),
            self.ngram_next_word_column
        );
        let conn = self.conn.lock();
        conn.prepare(&sql)
            .and_then(|mut stmt| stmt.exists([word]))
            .unwrap_or(false)
    }

    fn add_word_ngram_unigram(
        &self,
        word: &str,
        frequency: f64,
        allow_existing: bool,
    ) -> Result<(), SharedError> {
        let context_placeholders: Vec<String> = self
            .ngram_context_columns
            .iter()
            .map(|_| "NULL".to_string())
            .collect();

        let columns = format!(
            "({}, {}, {})",
            self.ngram_context_columns.join(", "),
            self.ngram_next_word_column,
            self.frequency_column
        );
        let values = format!("({}, ?, ?)", context_placeholders.join(", "));

        let sql = if allow_existing {
            format!(
                "INSERT OR REPLACE INTO {} {} VALUES {}",
                self.table_name, columns, values
            )
        } else {
            format!(
                "INSERT INTO {} {} VALUES {}",
                self.table_name, columns, values
            )
        };

        let conn = self.conn.lock();
        let result = conn.execute(&sql, [word, &frequency.to_string()]);

        if !allow_existing {
            if let Err(ref e) = result {
                let err_str = e.to_string();
                if err_str.contains("UNIQUE") || err_str.contains("unique") {
                    return Err(format!("word '{}' already exists", word).into());
                }
            }
        }

        result.map(|_| ()).map_err(Into::into)
    }
}

impl Clone for SqliteDictionaryBackend {
    /// Returns a new backend sharing the same underlying connection.
    fn clone(&self) -> Self {
        Self {
            conn: self.conn.clone(),
            table_name: self.table_name.clone(),
            word_column: self.word_column.clone(),
            frequency_column: self.frequency_column.clone(),
            cache: SharedQueryCache::new(),
            writable: self.writable,
            ngram_unigram_mode: self.ngram_unigram_mode,
            ngram_context_columns: self.ngram_context_columns.clone(),
            ngram_next_word_column: self.ngram_next_word_column.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
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
            SqliteDictionaryBackend::from_sqlite(&db_path, "ngrams", "word", "frequency", true)
                .unwrap();

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
            SqliteDictionaryBackend::from_sqlite(&db_path, "words", "term", "count", false)
                .unwrap();

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

        let backend =
            SqliteDictionaryBackend::from_sqlite(&db_path, "dict", "w", "f", true).unwrap();
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

    #[test]
    fn shared_connection() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("shared.db");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute("CREATE TABLE words (w TEXT, f REAL)", [])
                .unwrap();
            conn.execute("INSERT INTO words VALUES ('hello', 1.0)", [])
                .unwrap();
        }

        let shared = SharedSqliteConnection::open(&db_path).unwrap();
        let be = SqliteDictionaryBackend::from_shared(shared, "words", "w", "f", true);
        assert!(be.contains("hello"));
        assert_eq!(be.frequency("hello"), 1.0);
    }

    #[test]
    fn ngram_unigram_mode() {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE ngrams (context_1 TEXT, context_2 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
             INSERT INTO ngrams VALUES (NULL, NULL, 'der', 1000);
             INSERT INTO ngrams VALUES (NULL, NULL, 'die', 800);
             INSERT INTO ngrams VALUES (NULL, NULL, 'und', 600);
             INSERT INTO ngrams VALUES ('der', NULL, 'stadt', 500);
             INSERT INTO ngrams VALUES ('der', NULL, 'fluss', 300);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let be = SqliteDictionaryBackend::from_ngram_unigrams(
            shared,
            "ngrams",
            &["context_1".to_string(), "context_2".to_string()],
            "next_word",
            "frequency",
            true,
        );

        assert_eq!(be.len(), 3);
        assert!(be.contains("der"));
        assert!(be.contains("die"));
        assert!(!be.contains("stadt"));
        assert_eq!(be.frequency("der"), 1000.0);
        assert_eq!(be.frequency("stadt"), 0.0);

        let results = be.query_prefixes(&[DictionaryQuery {
            prefix: Some("d".to_string()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.word == "der"));
        assert!(results.iter().any(|r| r.word == "die"));
    }

    #[test]
    fn readonly_not_writable() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("ro.db");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute("CREATE TABLE words (w TEXT, f REAL)", [])
                .unwrap();
        }

        let be =
            SqliteDictionaryBackend::from_sqlite_readonly(&db_path, "words", "w", "f").unwrap();
        assert!(!be.is_writable());
        let err = be.add_word("test", 1.0, true);
        assert!(err.is_err());
    }

    #[test]
    fn add_word_simple() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("aw.db");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute("CREATE TABLE words (w TEXT NOT NULL UNIQUE, f REAL)", [])
                .unwrap();
        }

        let be = SqliteDictionaryBackend::from_sqlite(&db_path, "words", "w", "f", true).unwrap();

        be.add_word("hello", 10.0, false).unwrap();
        assert!(be.contains("hello"));
        assert_eq!(be.frequency("hello"), 10.0);

        let err = be.add_word("hello", 20.0, false);
        assert!(err.is_err());

        be.add_word("hello", 20.0, true).unwrap();
        assert_eq!(be.frequency("hello"), 20.0);
    }
}
