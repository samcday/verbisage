use std::path::Path;

use crate::backends::SharedSqliteConnection;
use crate::prediction::ngram_backend::NgramBackend;

/// N‑gram data access layer backed by a SQLite table.
///
/// The expected table schema is:
///
/// ```sql
/// CREATE TABLE ngrams (
///     context_1  TEXT,
///     context_2  TEXT,       -- optional, for 3‑grams
///     next_word  TEXT   NOT NULL,
///     frequency  REAL   NOT NULL
/// );
/// ```
///
/// For unigrams, `context_1` is `NULL`.
pub struct SqliteNgramBackend {
    conn: SharedSqliteConnection,
    table_name: String,
    context_columns: Vec<String>,
    next_word_column: String,
    frequency_column: String,
    max_order: usize,
    unigram_total: u64,
    writable: bool,
}

impl SqliteNgramBackend {
    pub fn new(
        conn: SharedSqliteConnection,
        table_name: &str,
        context_columns: &[String],
        next_word_column: &str,
        frequency_column: &str,
        max_order: usize,
        writable: bool,
    ) -> Self {
        let unigram_total =
            Self::query_unigram_total(&conn, table_name, context_columns, frequency_column);
        Self {
            conn,
            table_name: table_name.to_string(),
            context_columns: context_columns.to_vec(),
            next_word_column: next_word_column.to_string(),
            frequency_column: frequency_column.to_string(),
            max_order,
            unigram_total,
            writable,
        }
    }

    pub fn from_path(
        path: &Path,
        table_name: &str,
        context_columns: &[String],
        next_word_column: &str,
        frequency_column: &str,
        max_order: usize,
        writable: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let conn = SharedSqliteConnection::open(path)?;
        Ok(Self::new(
            conn,
            table_name,
            context_columns,
            next_word_column,
            frequency_column,
            max_order,
            writable,
        ))
    }

    fn query_unigram_total(
        conn: &SharedSqliteConnection,
        table_name: &str,
        context_columns: &[String],
        frequency_column: &str,
    ) -> u64 {
        let null_conditions: Vec<String> = context_columns
            .iter()
            .map(|c| format!("{} IS NULL", c))
            .collect();
        let sql = format!(
            "SELECT COALESCE(SUM({}), 0) FROM {} WHERE {}",
            frequency_column,
            table_name,
            null_conditions.join(" AND ")
        );
        let conn = conn.lock();
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(mut rows) = stmt.query(()) {
                if let Ok(row) = rows.next() {
                    if let Ok(val) = row.unwrap().get::<_, f64>(0) {
                        return val as u64;
                    }
                }
            }
        }
        0
    }
}

impl NgramBackend for SqliteNgramBackend {
    fn max_order(&self) -> usize {
        self.max_order
    }

    fn unigram_total(&self) -> u64 {
        self.unigram_total
    }

    fn ngram_count(&self, ngram: &[&str]) -> u64 {
        let order = ngram.len();
        if order == 0 {
            return 0;
        }
        let context: Vec<&str> = ngram[..order - 1].iter().copied().collect();
        let next_word = ngram[order - 1];

        if order == 1 {
            let sql = format!(
                "SELECT COALESCE(SUM({}), 0) FROM {} WHERE {} = ?1",
                self.frequency_column, self.table_name, self.next_word_column
            );
            let conn = self.conn.lock();
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(mut rows) = stmt.query([next_word]) {
                    if let Ok(row) = rows.next() {
                        if let Ok(val) = row.unwrap().get::<_, f64>(0) {
                            return val as u64;
                        }
                    }
                }
            }
            return 0;
        }

        let mut conditions = Vec::new();
        let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::new();
        for (i, col) in self.context_columns.iter().enumerate() {
            if let Some(ctx) = context.get(i) {
                conditions.push(format!("{} = ?{}", col, i + 1));
                params.push(ctx as &dyn rusqlite::types::ToSql);
            }
        }
        conditions.push(format!("{} = ?{}", self.next_word_column, params.len() + 1));
        params.push(&next_word as &dyn rusqlite::types::ToSql);

        let sql = format!(
            "SELECT COALESCE(SUM({}), 0) FROM {} WHERE {}",
            self.frequency_column,
            self.table_name,
            conditions.join(" AND ")
        );
        let conn = self.conn.lock();
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(mut rows) = stmt.query(params.as_slice()) {
                if let Ok(row) = rows.next() {
                    if let Ok(val) = row.unwrap().get::<_, f64>(0) {
                        return val as u64;
                    }
                }
            }
        }
        0
    }

    fn candidates(&self, context: &[&str], max_candidates: usize) -> Vec<(String, u64)> {
        let order = context.len() + 1;

        if order == 1 {
            let sql = format!(
                "SELECT {}, {} FROM {} WHERE {} IS NULL ORDER BY {} DESC LIMIT {}",
                self.next_word_column,
                self.frequency_column,
                self.table_name,
                self.context_columns
                    .first()
                    .unwrap_or(&"context_1".to_string()),
                self.frequency_column,
                max_candidates
            );
            let conn = self.conn.lock();
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(rows) = stmt.query_map((), |row| {
                    let word: String = row.get(0)?;
                    let freq: f64 = row.get(1)?;
                    Ok((word, freq as u64))
                }) {
                    return rows.filter_map(|r| r.ok()).collect();
                }
            }
            return Vec::new();
        }

        let mut conditions = Vec::new();
        let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::new();
        for (i, col) in self.context_columns.iter().enumerate() {
            if let Some(ctx) = context.get(i) {
                conditions.push(format!("{} = ?{}", col, i + 1));
                params.push(ctx as &dyn rusqlite::types::ToSql);
            }
        }

        let sql = format!(
            "SELECT {}, {} FROM {} WHERE {} ORDER BY {} DESC LIMIT {}",
            self.next_word_column,
            self.frequency_column,
            self.table_name,
            if conditions.is_empty() {
                "1=1".to_string()
            } else {
                conditions.join(" AND ")
            },
            self.frequency_column,
            max_candidates
        );
        let conn = self.conn.lock();
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
                let word: String = row.get(0)?;
                let freq: f64 = match row.get::<_, f64>(1) {
                    Ok(f) => f,
                    Err(_) => {
                        let int_freq: i64 = row.get(1)?;
                        int_freq as f64
                    }
                };
                Ok((word, freq as u64))
            }) {
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
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.writable {
            return Err("backend is not writable".into());
        }
        if ngram.is_empty() {
            return Err("ngram must have at least one element".into());
        }
        if delta < 0.0 {
            return Err("delta must be non-negative".into());
        }

        let order = ngram.len();
        let context: Vec<&str> = ngram[..order - 1].iter().copied().collect();
        let next_word = ngram[order - 1];

        let conn = self.conn.lock();
        let rows_changed = if order == 1 {
            self.update_unigram(&conn, next_word, delta, save_unknown)?
        } else {
            self.update_context_ngram(&conn, &context, next_word, delta, save_unknown)?
        };

        if !save_unknown && rows_changed == 0 {
            return Err(format!("ngram {:?} not found in backend", ngram).into());
        }

        Ok(())
    }
}

impl SqliteNgramBackend {
    fn update_unigram(
        &self,
        conn: &rusqlite::Connection,
        next_word: &str,
        delta: f64,
        save_unknown: bool,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let null_conditions: Vec<String> = self
            .context_columns
            .iter()
            .map(|c| format!("{} IS NULL", c))
            .collect();
        let where_clause = format!(
            "{} AND {} = ?1",
            null_conditions.join(" AND "),
            self.next_word_column
        );

        let update_sql = format!(
            "UPDATE {} SET {} = {} + ?2 WHERE {}",
            self.table_name, self.frequency_column, self.frequency_column, where_clause
        );
        let rows = conn.execute(
            &update_sql,
            [&next_word as &dyn rusqlite::types::ToSql, &delta],
        )?;

        if rows == 0 && save_unknown {
            let context_placeholders: Vec<String> = self
                .context_columns
                .iter()
                .map(|_| "NULL".to_string())
                .collect();
            let columns = format!(
                "({}, {}, {})",
                self.context_columns.join(", "),
                self.next_word_column,
                self.frequency_column
            );
            let values = format!("({}, ?, ?)", context_placeholders.join(", "));
            let insert_sql = format!(
                "INSERT INTO {} {} VALUES {}",
                self.table_name, columns, values
            );
            conn.execute(
                &insert_sql,
                [&next_word as &dyn rusqlite::types::ToSql, &delta],
            )?;
            Ok(1)
        } else {
            Ok(rows)
        }
    }

    fn update_context_ngram(
        &self,
        conn: &rusqlite::Connection,
        context: &[&str],
        next_word: &str,
        delta: f64,
        save_unknown: bool,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let mut conditions = Vec::new();
        let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::new();
        for (i, col) in self.context_columns.iter().enumerate() {
            if let Some(ctx) = context.get(i) {
                conditions.push(format!("{} = ?{}", col, i + 1));
                params.push(ctx as &dyn rusqlite::types::ToSql);
            }
        }
        conditions.push(format!("{} = ?{}", self.next_word_column, params.len() + 1));
        params.push(&next_word as &dyn rusqlite::types::ToSql);

        let where_clause = conditions.join(" AND ");
        let delta_param_idx = params.len() + 1;
        let update_sql = format!(
            "UPDATE {} SET {} = {} + ?{} WHERE {}",
            self.table_name,
            self.frequency_column,
            self.frequency_column,
            delta_param_idx,
            where_clause
        );

        let mut all_params = params.to_vec();
        all_params.push(&delta as &dyn rusqlite::types::ToSql);
        let rows = conn.execute(&update_sql, all_params.as_slice())?;

        if rows == 0 && save_unknown {
            let mut insert_params: Vec<&dyn rusqlite::types::ToSql> = Vec::new();
            for ctx in context {
                insert_params.push(ctx as &dyn rusqlite::types::ToSql);
            }
            insert_params.push(&next_word as &dyn rusqlite::types::ToSql);
            insert_params.push(&delta as &dyn rusqlite::types::ToSql);

            let placeholders: Vec<String> = (1..=insert_params.len())
                .map(|i| format!("?{}", i))
                .collect();
            let columns = format!(
                "({}, {}, {})",
                self.context_columns.join(", "),
                self.next_word_column,
                self.frequency_column
            );
            let insert_sql = format!(
                "INSERT INTO {} {} VALUES ({})",
                self.table_name,
                columns,
                placeholders.join(", ")
            );
            conn.execute(&insert_sql, insert_params.as_slice())?;
            Ok(1)
        } else {
            Ok(rows)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn unigram_total() {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE ngrams (context_1 TEXT, context_2 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
             INSERT INTO ngrams VALUES (NULL, NULL, 'der', 1000);
             INSERT INTO ngrams VALUES (NULL, NULL, 'die', 800);
             INSERT INTO ngrams VALUES (NULL, NULL, 'und', 600);
             INSERT INTO ngrams VALUES ('der', NULL, 'stadt', 500);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let backend = SqliteNgramBackend::new(
            shared,
            "ngrams",
            &["context_1".to_string(), "context_2".to_string()],
            "next_word",
            "frequency",
            3,
            false,
        );
        assert_eq!(backend.unigram_total(), 2400);
    }

    #[test]
    fn ngram_count_unigram() {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
             INSERT INTO ngrams VALUES (NULL, 'der', 1000);
             INSERT INTO ngrams VALUES ('how', 'are', 50);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let backend = SqliteNgramBackend::new(
            shared,
            "ngrams",
            &["context_1".to_string()],
            "next_word",
            "frequency",
            2,
            false,
        );
        assert_eq!(backend.ngram_count(&["der"]), 1000);
        assert_eq!(backend.ngram_count(&["foo"]), 0);
    }

    #[test]
    fn ngram_count_bigram() {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
             INSERT INTO ngrams VALUES ('how', 'are', 50);
             INSERT INTO ngrams VALUES ('how', 'is', 30);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let backend = SqliteNgramBackend::new(
            shared,
            "ngrams",
            &["context_1".to_string()],
            "next_word",
            "frequency",
            2,
            false,
        );
        assert_eq!(backend.ngram_count(&["how", "are"]), 50);
        assert_eq!(backend.ngram_count(&["how", "you"]), 0);
    }

    #[test]
    fn candidates_unigram() {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
             INSERT INTO ngrams VALUES (NULL, 'der', 1000);
             INSERT INTO ngrams VALUES (NULL, 'die', 800);
             INSERT INTO ngrams VALUES (NULL, 'und', 600);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let backend = SqliteNgramBackend::new(
            shared,
            "ngrams",
            &["context_1".to_string()],
            "next_word",
            "frequency",
            2,
            false,
        );
        let results = backend.candidates(&[], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], ("der".to_string(), 1000));
        assert_eq!(results[1], ("die".to_string(), 800));
    }

    #[test]
    fn candidates_bigram() {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE bigrams (prev TEXT, next TEXT, freq REAL);
             INSERT INTO bigrams VALUES ('how', 'are', 50);
             INSERT INTO bigrams VALUES ('how', 'is', 30);
             INSERT INTO bigrams VALUES ('how', 'you', 20);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let backend = SqliteNgramBackend::new(
            shared,
            "bigrams",
            &["prev".to_string()],
            "next",
            "freq",
            2,
            false,
        );
        let results = backend.candidates(&["how"], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], ("are".to_string(), 50));
        assert_eq!(results[1], ("is".to_string(), 30));
    }

    #[test]
    fn increase_ngram_freq_unigram_save_unknown() {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
             INSERT INTO ngrams VALUES (NULL, 'der', 1000);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let backend = SqliteNgramBackend::new(
            shared,
            "ngrams",
            &["context_1".to_string()],
            "next_word",
            "frequency",
            2,
            true,
        );
        assert!(backend.is_writable());
        backend
            .increase_ngram_frequency(&["der"], 5.0, false)
            .unwrap();
        assert_eq!(backend.ngram_count(&["der"]), 1005);

        backend
            .increase_ngram_frequency(&["newword"], 10.0, true)
            .unwrap();
        assert_eq!(backend.ngram_count(&["newword"]), 10);
    }

    #[test]
    fn increase_ngram_freq_bigram_save_unknown() {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
             INSERT INTO ngrams VALUES ('how', 'are', 50);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let backend = SqliteNgramBackend::new(
            shared,
            "ngrams",
            &["context_1".to_string()],
            "next_word",
            "frequency",
            2,
            true,
        );
        backend
            .increase_ngram_frequency(&["how", "are"], 3.0, false)
            .unwrap();
        assert_eq!(backend.ngram_count(&["how", "are"]), 53);

        backend
            .increase_ngram_frequency(&["how", "is"], 20.0, true)
            .unwrap();
        assert_eq!(backend.ngram_count(&["how", "is"]), 20);
    }

    #[test]
    fn increase_ngram_freq_readonly_rejected() {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let backend = SqliteNgramBackend::new(
            shared,
            "ngrams",
            &["context_1".to_string()],
            "next_word",
            "frequency",
            2,
            false,
        );
        assert!(!backend.is_writable());
        let err = backend.increase_ngram_frequency(&["test"], 1.0, true);
        assert!(err.is_err());
    }

    #[test]
    fn increase_ngram_freq_save_unknown_false_rejected() {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let backend = SqliteNgramBackend::new(
            shared,
            "ngrams",
            &["context_1".to_string()],
            "next_word",
            "frequency",
            2,
            true,
        );
        let err = backend.increase_ngram_frequency(&["nonexistent"], 1.0, false);
        assert!(err.is_err());
    }

    #[test]
    fn increase_ngram_freq_via_predictor_trait() {
        use crate::prediction::Predictor;
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
             INSERT INTO ngrams VALUES (NULL, 'hello', 10.0);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let backend = SqliteNgramBackend::new(
            shared,
            "ngrams",
            &["context_1".to_string()],
            "next_word",
            "frequency",
            2,
            true,
        );
        let predictor = crate::prediction::smoothed::SmoothedPredictor::new(Box::new(backend))
            .with_deltas(vec![0.4, 0.4, 0.2]);

        assert_eq!(predictor.ngram_count(&["hello"]), 10);
        predictor
            .increase_ngram_frequency(&["hello"], 5.0, false)
            .unwrap();
        assert_eq!(predictor.ngram_count(&["hello"]), 15);
    }
}
