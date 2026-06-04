use std::sync::Arc;

use crate::dictionary::{DictionaryBackend, DictionaryQuery, SqliteDictionaryBackend};
use crate::spellcheck::SpellChecker;

/// [`SpellChecker`] implementation backed by [`SqliteDictionaryBackend`].
///
/// # Attention points for the implementor
///
/// * The `suggest` implementation currently relies on prefix‑only queries
///   through the standard [`DictionaryBackend::query_prefixes`] path.
/// * For production use, consider registering a custom Levenshtein SQL
///   function on the connection and querying `ORDER BY levenshtein(word, ?)
///   LIMIT n`.  This gives proper edit‑distance ranking.
/// * The SQLite backend's `LIKE`‑based suffix matching can also be used
///   for end‑character typos.
pub struct SqliteSpellChecker {
    backend: Arc<SqliteDictionaryBackend>,
}

impl SqliteSpellChecker {
    pub fn new(backend: Arc<SqliteDictionaryBackend>) -> Self {
        Self { backend }
    }
}

impl SpellChecker for SqliteSpellChecker {
    fn is_correct(&self, word: &str) -> bool {
        self.backend.contains(word)
    }

    fn suggest(&self, word: &str) -> Vec<String> {
        let mut candidates: Vec<String> = Vec::new();

        // 1. Query with exact prefix.
        let exact = self.backend.query_prefixes(&[DictionaryQuery {
            prefix: Some(word.to_lowercase()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        candidates.extend(exact.into_iter().map(|r| r.word));

        // 2. Query with first 3 characters as a relaxed prefix.
        if word.chars().count() > 3 {
            let prefix: String = word.chars().take(3).collect();
            let relaxed = self.backend.query_prefixes(&[DictionaryQuery {
                prefix: Some(prefix),
                suffix: None,
                min_length: None,
                max_length: None,
            }]);
            candidates.extend(relaxed.into_iter().map(|r| r.word));
        }

        // 3. Deduplicate preserving insertion order.
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|w| seen.insert(w.clone()));

        // 4. Sort: exact matches first, then by prefix‑overlap score.
        candidates.sort_by(|a, b| {
            let a_score = prefix_overlap(word, a);
            let b_score = prefix_overlap(word, b);
            b_score
                .partial_cmp(&a_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cmp(b))
        });

        candidates.truncate(10);
        candidates
    }
}

/// Score `candidate` against `input` using shared‑prefix length relative
/// to the longer string.
fn prefix_overlap(input: &str, candidate: &str) -> f64 {
    if input.eq_ignore_ascii_case(candidate) {
        return f64::MAX;
    }

    let shared = input
        .chars()
        .zip(candidate.chars())
        .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
        .count();

    let max_len = input.len().max(candidate.len());
    if max_len == 0 {
        return 0.0;
    }

    shared as f64 / max_len as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_backend_contains_nothing() {
        let b = SqliteDictionaryBackend::new();
        let checker = SqliteSpellChecker::new(Arc::new(b));
        assert!(!checker.is_correct("hello"));
    }
}
