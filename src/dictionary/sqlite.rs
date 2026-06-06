use std::error::Error;
use std::path::Path;
use std::sync::Mutex;

use crate::backends::SharedSqliteConnection;
use crate::prediction::ngram_backend::NgramBackend;

use super::{DictionaryBackend, DictionaryQuery, DictionaryResult, SharedQueryCache};

type SharedError = Box<dyn Error + Send + Sync>;

/// Unified SQLite backend using the Presage schema.
pub struct PresageSqliteBackend {
    conn: SharedSqliteConnection,
    writable: bool,
    created_new_file: bool,
    schema_initialized: Mutex<bool>,
    cache: SharedQueryCache,
    ngrams_level_max: usize,
}

impl PresageSqliteBackend {
    pub fn open<P: AsRef<Path>>(path: P, writable: bool) -> Result<Self, SharedError> {
        Self::open_with_max(path, writable, 3)
    }

    pub fn open_with_max<P: AsRef<Path>>(
        path: P,
        writable: bool,
        ngrams_level_max: usize,
    ) -> Result<Self, SharedError> {
        let path = path.as_ref();
        let file_existed = path.exists();
        let conn = SharedSqliteConnection::open(path)?;

        Ok(Self {
            conn,
            writable,
            created_new_file: !file_existed,
            schema_initialized: Mutex::new(false),
            cache: SharedQueryCache::new(),
            ngrams_level_max,
        })
    }

    pub fn from_shared(
        conn: SharedSqliteConnection,
        writable: bool,
        created_new_file: bool,
    ) -> Self {
        Self::from_shared_with_max(conn, writable, created_new_file, 3)
    }

    pub fn from_shared_with_max(
        conn: SharedSqliteConnection,
        writable: bool,
        created_new_file: bool,
        ngrams_level_max: usize,
    ) -> Self {
        Self {
            conn,
            writable,
            created_new_file,
            schema_initialized: Mutex::new(false),
            cache: SharedQueryCache::new(),
            ngrams_level_max,
        }
    }

    pub fn new() -> Self {
        Self::with_max(3)
    }

    pub fn with_max(ngrams_level_max: usize) -> Self {
        Self {
            conn: SharedSqliteConnection::in_memory(),
            writable: true,
            created_new_file: true,
            schema_initialized: Mutex::new(false),
            cache: SharedQueryCache::new(),
            ngrams_level_max,
        }
    }

    fn ensure_schema(&self) {
        let mut initialized = self.schema_initialized.lock().unwrap();
        if *initialized {
            return;
        }
        if !self.created_new_file {
            *initialized = true;
            return;
        }
        let mut sql_parts = Vec::new();
        for n in 1..=self.ngrams_level_max {
            let cols: Vec<String> = (0..n)
                .map(|i| {
                    if i == n - 1 {
                        "word".into()
                    } else {
                        format!("word_{}", n - 1 - i)
                    }
                })
                .collect();
            let pk = cols.join(", ");
            sql_parts.push(format!(
                "CREATE TABLE IF NOT EXISTS _{}_gram ({}, count INTEGER DEFAULT 1, UNIQUE({}))",
                n, pk, pk
            ));
        }
        let conn = self.conn.lock();
        let _ = conn.execute_batch(&sql_parts.join("; "));
        *initialized = true;
    }

    pub fn enable_cache(&self, size_limit: usize) {
        self.cache.set_limit(size_limit);
    }

    pub fn clear_cache(&self) {
        self.cache.clear();
    }
}

impl DictionaryBackend for PresageSqliteBackend {
    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        self.cache.get_or_compute(queries, |queries| {
            if queries.is_empty() {
                return Vec::new();
            }

            let mut clauses = Vec::new();
            for q in queries {
                let mut cond = Vec::new();
                if let Some(ref prefix) = q.prefix {
                    cond.push(format!("word LIKE '{}%'", prefix));
                }
                if let Some(ref suffix) = q.suffix {
                    cond.push(format!("word LIKE '%{}'", suffix));
                }
                if let Some(min) = q.min_length {
                    cond.push(format!("LENGTH(word) >= {}", min));
                }
                if let Some(max) = q.max_length {
                    if max != usize::MAX {
                        cond.push(format!("LENGTH(word) <= {}", max));
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
                "SELECT word, count FROM _1_gram WHERE {}",
                clauses.join(" OR ")
            );

            let mut all_results = Vec::new();
            let conn = self.conn.lock();
            match conn.prepare(&sql) {
                Ok(mut stmt) => {
                    if let Ok(rows) = stmt.query_map([], |row| {
                        let word: String = row.get(0)?;
                        let count: i64 = row.get(1)?;
                        Ok(DictionaryResult {
                            word,
                            confidence: if count > 0 { count as f64 } else { -1.0 },
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
        let sql = "SELECT count FROM _1_gram WHERE word = ?1 LIMIT 1";
        let conn = self.conn.lock();
        if let Ok(mut stmt) = conn.prepare(sql) {
            if let Ok(mut rows) = stmt.query_map([word], |row| {
                let count: i64 = row.get(0)?;
                Ok(count as f64)
            }) {
                if let Some(result) = rows.next() {
                    return result.unwrap_or(0.0);
                }
            }
        }
        0.0
    }

    fn contains(&self, word: &str) -> bool {
        let sql = "SELECT 1 FROM _1_gram WHERE word = ?1 LIMIT 1";
        let conn = self.conn.lock();
        conn.prepare(sql)
            .and_then(|mut stmt| stmt.exists([word]))
            .unwrap_or(false)
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

        self.ensure_schema();

        let count = frequency as i64;
        let conn = self.conn.lock();

        if allow_existing {
            let sql = "INSERT OR REPLACE INTO _1_gram (word, count) VALUES (?1, ?2)";
            conn.execute(sql, rusqlite::params![word, count])
                .map(|_| ())
                .map_err(Into::into)
        } else {
            let exists_sql = "SELECT 1 FROM _1_gram WHERE word = ?1";
            let exists: bool = conn
                .prepare(exists_sql)
                .and_then(|mut stmt| stmt.exists([word]))
                .unwrap_or(false);
            if exists {
                return Err(format!("word '{}' already exists", word).into());
            }
            let sql = "INSERT INTO _1_gram (word, count) VALUES (?1, ?2)";
            conn.execute(sql, rusqlite::params![word, count])
                .map(|_| ())
                .map_err(Into::into)
        }
    }
}

impl NgramBackend for PresageSqliteBackend {
    fn max_order(&self) -> usize {
        self.ngrams_level_max
    }

    fn unigram_total(&self) -> u64 {
        let sql = "SELECT COALESCE(SUM(count), 0) FROM _1_gram";
        let conn = self.conn.lock();
        if let Ok(mut stmt) = conn.prepare(sql) {
            if let Ok(mut rows) = stmt.query_map((), |row| {
                let val: i64 = row.get(0)?;
                Ok(val as u64)
            }) {
                if let Some(result) = rows.next() {
                    return result.unwrap_or(0);
                }
            }
        }
        0
    }

    fn ngram_count(&self, ngram: &[&str]) -> u64 {
        let order = ngram.len();
        if order == 0 || order > self.ngrams_level_max {
            return 0;
        }

        let table = format!("_{}_gram", order);
        let where_clause: Vec<String> = (0..order)
            .map(|i| {
                let col = if i == order - 1 {
                    "word".to_string()
                } else {
                    format!("word_{}", order - 1 - i)
                };
                format!("{} = ?{}", col, i + 1)
            })
            .collect();
        let sql = format!(
            "SELECT COALESCE(SUM(count), 0) FROM {} WHERE {}",
            table,
            where_clause.join(" AND ")
        );

        let conn = self.conn.lock();
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(mut rows) = stmt.query(rusqlite::params_from_iter(ngram.iter().copied())) {
                if let Ok(row) = rows.next() {
                    if let Ok(val) = row.unwrap().get::<_, i64>(0) {
                        return val as u64;
                    }
                }
            }
        }
        0
    }

    fn candidates(&self, context: &[&str], max_candidates: usize) -> Vec<(String, u64)> {
        let order = context.len() + 1;
        if order < 1 || order > self.ngrams_level_max {
            return Vec::new();
        }

        let table = format!("_{}_gram", order);
        let where_clause: Vec<String> = context
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let col = if i == order - 1 {
                    "word".to_string()
                } else {
                    format!("word_{}", order - 1 - i)
                };
                format!("{} = ?{}", col, i + 1)
            })
            .collect();

        let sql = if where_clause.is_empty() {
            format!(
                "SELECT word, count FROM {} ORDER BY count DESC LIMIT {}",
                table, max_candidates
            )
        } else {
            format!(
                "SELECT word, count FROM {} WHERE {} ORDER BY count DESC LIMIT {}",
                table,
                where_clause.join(" AND "),
                max_candidates
            )
        };

        let conn = self.conn.lock();
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(rows) =
                stmt.query_map(rusqlite::params_from_iter(context.iter().copied()), |row| {
                    let word: String = row.get(0)?;
                    let count: i64 = row.get(1)?;
                    Ok((word, count as u64))
                })
            {
                return rows.filter_map(|r| r.ok()).collect();
            }
        }
        Vec::new()
    }

    fn is_writable(&self) -> bool {
        self.writable
    }

    fn increase_ngram_frequency(
        &self,
        ngram: &[&str],
        delta: f64,
        save_unknown: bool,
    ) -> Result<(), SharedError> {
        if !self.writable {
            return Err("backend is not writable".into());
        }
        if ngram.is_empty() || ngram.len() > self.ngrams_level_max {
            return Err(format!("ngram order must be 1-{}", self.ngrams_level_max).into());
        }
        if delta < 0.0 {
            return Err("delta must be non-negative".into());
        }

        self.ensure_schema();

        let delta_int = delta as i64;
        let order = ngram.len();
        let table = format!("_{}_gram", order);

        let cols: Vec<String> = (0..order)
            .map(|i| {
                if i == order - 1 {
                    "word".to_string()
                } else {
                    format!("word_{}", order - 1 - i)
                }
            })
            .collect();
        let where_parts: Vec<String> = cols
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{} = ?{}", c, i + 1))
            .collect();
        let update_sql = format!(
            "UPDATE {} SET count = count + ?{} WHERE {}",
            table,
            order + 1,
            where_parts.join(" AND ")
        );

        let insert_cols = cols.join(", ");
        let insert_placeholders: Vec<String> = (1..=order).map(|i| format!("?{}", i)).collect();
        let insert_sql = format!(
            "INSERT OR IGNORE INTO {} ({}, count) VALUES ({}, ?{})",
            table,
            insert_cols,
            insert_placeholders.join(", "),
            order + 1
        );

        let params: Vec<Box<dyn rusqlite::types::ToSql>> = ngram
            .iter()
            .map(|w| Box::new(*w) as Box<dyn rusqlite::types::ToSql>)
            .chain(std::iter::once(
                Box::new(delta_int) as Box<dyn rusqlite::types::ToSql>
            ))
            .collect();
        let params_ref: Vec<&dyn rusqlite::types::ToSql> = params
            .iter()
            .map(|p| p.as_ref() as &dyn rusqlite::types::ToSql)
            .collect();

        let conn = self.conn.lock();
        let rows = conn.execute(&update_sql, params_ref.as_slice())?;
        if rows == 0 && save_unknown {
            conn.execute(&insert_sql, params_ref.as_slice())?;
        } else if rows == 0 {
            return Err(format!("ngram {:?} not found in backend", ngram).into());
        }

        Ok(())
    }
}

impl Clone for PresageSqliteBackend {
    fn clone(&self) -> Self {
        Self {
            conn: self.conn.clone(),
            writable: self.writable,
            created_new_file: self.created_new_file,
            schema_initialized: Mutex::new(false),
            cache: SharedQueryCache::new(),
            ngrams_level_max: self.ngrams_level_max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn setup_presage_db(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _1_gram (word TEXT, count INTEGER DEFAULT 1, UNIQUE(word));
             CREATE TABLE IF NOT EXISTS _2_gram (word_1 TEXT, word TEXT, count INTEGER DEFAULT 1, UNIQUE(word_1, word));
             CREATE TABLE IF NOT EXISTS _3_gram (word_2 TEXT, word_1 TEXT, word TEXT, count INTEGER DEFAULT 1, UNIQUE(word_2, word_1, word));
             INSERT OR REPLACE INTO _1_gram VALUES ('hello', 100);
             INSERT OR REPLACE INTO _1_gram VALUES ('world', 50);
             INSERT OR REPLACE INTO _1_gram VALUES ('goodbye', 80);
             INSERT OR REPLACE INTO _2_gram VALUES ('hello', 'world', 40);
             INSERT OR REPLACE INTO _2_gram VALUES ('hello', 'there', 30);
             INSERT OR REPLACE INTO _3_gram VALUES ('hi', 'hello', 'world', 20);",
        )
        .unwrap();
    }

    #[test]
    fn dict_contains() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, false);

        assert!(backend.contains("hello"));
        assert!(backend.contains("world"));
        assert!(!backend.contains("nonexistent"));
    }

    #[test]
    fn dict_frequency() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, false);

        assert_eq!(backend.get_frequency("hello"), 100.0);
        assert_eq!(backend.get_frequency("world"), 50.0);
        assert_eq!(backend.get_frequency("nonexistent"), 0.0);
    }

    #[test]
    fn dict_prefix_query() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, false);

        let results = backend.query_prefixes(&[DictionaryQuery {
            prefix: Some("hel".to_string()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].word, "hello");
    }

    #[test]
    fn dict_add_word() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let backend = PresageSqliteBackend::open(&db_path, true).unwrap();
        backend.add_word("hello", 10.0, false).unwrap();
        assert!(backend.contains("hello"));
        assert_eq!(backend.get_frequency("hello"), 10.0);

        let err = backend.add_word("hello", 20.0, false);
        assert!(err.is_err());

        backend.add_word("hello", 20.0, true).unwrap();
        assert_eq!(backend.get_frequency("hello"), 20.0);
    }

    #[test]
    fn lazy_schema_creation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("lazy.db");

        let backend = PresageSqliteBackend::open(&db_path, true).unwrap();
        let conn = backend.conn.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='_1_gram'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "schema should not be created until first write");

        drop(conn);
        backend.add_word("test", 1.0, false).unwrap();

        let conn = backend.conn.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='_1_gram'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "schema should be created after first write");
    }

    #[test]
    fn ngram_unigram_count() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, false);

        assert_eq!(backend.ngram_count(&["hello"]), 100);
        assert_eq!(backend.ngram_count(&["world"]), 50);
        assert_eq!(backend.ngram_count(&["nonexistent"]), 0);
    }

    #[test]
    fn ngram_bigram_count() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, false);

        assert_eq!(backend.ngram_count(&["hello", "world"]), 40);
        assert_eq!(backend.ngram_count(&["hello", "there"]), 30);
        assert_eq!(backend.ngram_count(&["hello", "moon"]), 0);
    }

    #[test]
    fn ngram_trigram_count() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, false);

        assert_eq!(backend.ngram_count(&["hi", "hello", "world"]), 20);
        assert_eq!(backend.ngram_count(&["hi", "hello", "moon"]), 0);
    }

    #[test]
    fn ngram_unigram_total() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, false);

        assert_eq!(backend.unigram_total(), 230);
    }

    #[test]
    fn ngram_candidates_unigram() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, false);

        let results = backend.candidates(&[], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], ("hello".to_string(), 100));
        assert_eq!(results[1], ("goodbye".to_string(), 80));
    }

    #[test]
    fn ngram_candidates_bigram() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, false);

        let results = backend.candidates(&["hello"], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], ("world".to_string(), 40));
        assert_eq!(results[1], ("there".to_string(), 30));
    }

    #[test]
    fn ngram_increase_frequency_unigram() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, true);

        assert!(DictionaryBackend::is_writable(&backend));
        backend
            .increase_ngram_frequency(&["hello"], 5.0, false)
            .unwrap();
        assert_eq!(backend.ngram_count(&["hello"]), 105);

        backend
            .increase_ngram_frequency(&["newword"], 10.0, true)
            .unwrap();
        assert_eq!(backend.ngram_count(&["newword"]), 10);
    }

    #[test]
    fn ngram_increase_frequency_bigram() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, true);

        backend
            .increase_ngram_frequency(&["hello", "world"], 3.0, false)
            .unwrap();
        assert_eq!(backend.ngram_count(&["hello", "world"]), 43);

        backend
            .increase_ngram_frequency(&["hello", "moon"], 20.0, true)
            .unwrap();
        assert_eq!(backend.ngram_count(&["hello", "moon"]), 20);
    }

    #[test]
    fn ngram_readonly_rejected() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, false, false);

        assert!(!NgramBackend::is_writable(&backend));
        let err = backend.increase_ngram_frequency(&["test"], 1.0, true);
        assert!(err.is_err());
    }

    #[test]
    fn ngram_save_unknown_false_rejected() {
        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, true);

        let err = backend.increase_ngram_frequency(&["nonexistent"], 1.0, false);
        assert!(err.is_err());
    }

    #[test]
    fn via_smoothed_predictor() {
        use crate::prediction::Predictor;
        use crate::prediction::smoothed::SmoothedPredictor;

        let conn = Connection::open(":memory:").unwrap();
        setup_presage_db(&conn);
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, false);

        let ngram_backend: Box<dyn NgramBackend> = Box::new(backend);
        let predictor = SmoothedPredictor::new(ngram_backend).with_deltas(vec![0.4, 0.4, 0.2]);

        assert_eq!(predictor.ngram_count(&["hello"]), 100);

        let predictions = predictor.predict_next(&["hello"], 2);
        assert!(!predictions.is_empty());
    }
}
