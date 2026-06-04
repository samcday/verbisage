use std::sync::Arc;

use rusqlite::Connection;

use crate::dictionary::SqliteDictionaryBackend;
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
/// # Attention points for the implementor
///
/// * The column names for context terms, next word, and frequency are
///   configurable.  The current struct accepts `context_columns` as a
///   `Vec<String>` so that the predictor works with both 2‑gram and
///   3‑gram tables.
/// * The prediction query must match *all* context columns against the
///   provided context slice.  If the context slice is shorter than
///   `context_columns`, earlier columns should be matched first and the
///   remaining columns left unconstrained (or omitted via `IS NULL`).
/// * For large n‑gram tables, ensure there is a composite index on
///   `(context_1, context_2, …, frequency DESC)`.
pub struct SqlitePredictor {
    conn: Arc<Connection>,
    table_name: String,
    context_columns: Vec<String>,
    next_word_column: String,
    frequency_column: String,
}

impl SqlitePredictor {
    /// Share the same connection as a [`SqliteDictionaryBackend`].
    ///
    /// This avoids opening a second file handle to the same database.
    /// Requires the connection to be wrapped in an `Arc` beforehand.
    pub fn new(
        conn: Arc<Connection>,
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

    /// Convenience constructor: open a new connection to `path` and build
    /// a predictor for a 2‑gram table.
    pub fn from_path(
        path: &std::path::Path,
        table_name: &str,
        context_column: &str,
        next_word_column: &str,
        frequency_column: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let conn = Connection::open(path)?;
        Ok(Self {
            conn: Arc::new(conn),
            table_name: table_name.to_string(),
            context_columns: vec![context_column.to_string()],
            next_word_column: next_word_column.to_string(),
            frequency_column: frequency_column.to_string(),
        })
    }
}

impl Predictor for SqlitePredictor {
    fn predict_next(&self, context: &[&str], max_suggestions: usize) -> Vec<Prediction> {
        // Build a WHERE clause that matches as many context columns as
        // possible.
        let mut conditions = Vec::new();
        for (i, col) in self.context_columns.iter().enumerate() {
            if let Some(word) = context.get(i) {
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

        // Bind context words as parameters.
        let params: Vec<&dyn rusqlite::types::ToSql> = context
            .iter()
            .take(self.context_columns.len())
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();

        let mut results = Vec::new();
        if let Ok(mut stmt) = self.conn.prepare(&sql) {
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

        // Normalise confidences to 0.0–1.0 if the raw values are counts.
        // The implementation should check whether the values in
        // `frequency_column` are already normalised or raw counts.
        // If they are raw counts, divide by the maximum value in the
        // result set (or by the table's total sum, if available).

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

        let predictor = SqlitePredictor::new(
            Arc::new(conn),
            "bigrams",
            &["prev".to_string()],
            "next",
            "freq",
        );

        let results = predictor.predict_next(&["how"], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].word, "are");
        assert_eq!(results[1].word, "is");
    }
}
