use std::path::Path;

use crate::backends::SharedSqliteConnection;
use crate::prediction::{Prediction, Predictor};

/// Predictor backed by an n‑gram table in SQLite.
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
/// # Thread safety
///
/// The inner [`rusqlite::Connection`] is wrapped in a [`SharedSqliteConnection`]
/// so that the backend implements [`Sync`] and can share a connection with a
/// [`SqliteDictionaryBackend`](crate::dictionary::SqliteDictionaryBackend).
pub struct SqlitePredictor {
    conn: SharedSqliteConnection,
    table_name: String,
    context_columns: Vec<String>,
    next_word_column: String,
    frequency_column: String,
}

impl SqlitePredictor {
    /// Create a new predictor wrapping a shared connection.
    pub fn new(
        conn: SharedSqliteConnection,
        table_name: &str,
        context_columns: &[String],
        next_word_column: &str,
        frequency_column: &str,
    ) -> Self {
        Self {
            conn,
            table_name: table_name.to_string(),
            context_columns: context_columns.to_vec(),
            next_word_column: next_word_column.to_string(),
            frequency_column: frequency_column.to_string(),
        }
    }

    /// Open a new connection to `path` and build a predictor for a 2‑gram table.
    pub fn from_path(
        path: &Path,
        table_name: &str,
        context_column: &str,
        next_word_column: &str,
        frequency_column: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let conn = SharedSqliteConnection::open(path)?;
        Ok(Self {
            conn,
            table_name: table_name.to_string(),
            context_columns: vec![context_column.to_string()],
            next_word_column: next_word_column.to_string(),
            frequency_column: frequency_column.to_string(),
        })
    }
}

impl Predictor for SqlitePredictor {
    fn predict_next(&self, context: &[&str], max_suggestions: usize) -> Vec<Prediction> {
        let mut conditions = Vec::new();
        for (i, col) in self.context_columns.iter().enumerate() {
            if context.get(i).is_some() {
                conditions.push(format!("{} = ?{}", col, i + 1));
            }
        }

        if conditions.is_empty() {
            return Vec::new();
        }

        let sql = format!(
            "SELECT {}, {} FROM {} WHERE {} ORDER BY {} DESC LIMIT {}",
            self.next_word_column,
            self.frequency_column,
            self.table_name,
            conditions.join(" AND "),
            self.frequency_column,
            max_suggestions
        );

        let params: Vec<&dyn rusqlite::types::ToSql> = context
            .iter()
            .take(self.context_columns.len())
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();

        let mut results = Vec::new();
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
                Ok(Prediction {
                    word,
                    confidence: freq,
                })
            }) {
                for row in rows.flatten() {
                    results.push(row);
                }
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn basic_prediction() {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute("CREATE TABLE bigrams (prev TEXT, next TEXT, freq REAL)", [])
            .unwrap();
        conn.execute("INSERT INTO bigrams VALUES ('how', 'are', 50.0)", [])
            .unwrap();
        conn.execute("INSERT INTO bigrams VALUES ('how', 'is', 30.0)", [])
            .unwrap();
        conn.execute("INSERT INTO bigrams VALUES ('how', 'you', 20.0)", [])
            .unwrap();

        let shared = SharedSqliteConnection::new(conn);
        let predictor =
            SqlitePredictor::new(shared, "bigrams", &["prev".to_string()], "next", "freq");

        let results = predictor.predict_next(&["how"], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].word, "are");
        assert_eq!(results[1].word, "is");
    }
}
