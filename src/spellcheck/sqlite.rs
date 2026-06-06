use std::sync::Arc;

use crate::dictionary::{DictionaryBackend, DictionaryQuery, PresageSqliteBackend};
use crate::spellcheck::SpellChecker;

/// [`SpellChecker`] implementation backed by [`PresageSqliteBackend`].
pub struct SqliteSpellChecker {
    backend: Arc<PresageSqliteBackend>,
}

impl SqliteSpellChecker {
    pub fn new(backend: Arc<PresageSqliteBackend>) -> Self {
        Self { backend }
    }
}

impl SpellChecker for SqliteSpellChecker {
    fn is_correct(&self, word: &str) -> bool {
        self.backend.contains(word)
    }

    fn suggest(&self, word: &str) -> Vec<String> {
        let mut candidates: Vec<String> = Vec::new();

        let exact = self.backend.query_prefixes(&[DictionaryQuery {
            prefix: Some(word.to_lowercase()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        candidates.extend(exact.into_iter().map(|r| r.word));

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

        let mut seen = std::collections::HashSet::new();
        candidates.retain(|w| seen.insert(w.clone()));

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
        let b = PresageSqliteBackend::new();
        let checker = SqliteSpellChecker::new(Arc::new(b));
        assert!(!checker.is_correct("hello"));
    }
}
