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
        let word_lower = word.to_lowercase();

        // If the word is correct, return it.
        if self.backend.contains(&word_lower) {
            return vec![word_lower];
        }

        // Generate single-edit-distance candidates and check against DB.
        let mut candidates: Vec<String> = Vec::new();
        let chars: Vec<char> = word_lower.chars().collect();
        let len = chars.len();

        // 1. Adjacent swap (e.g. "hrello" -> "hello")
        for i in 0..len.saturating_sub(1) {
            let mut c = chars.clone();
            c.swap(i, i + 1);
            let w: String = c.into_iter().collect();
            if w != word_lower && self.backend.contains(&w) {
                candidates.push(w);
            }
        }

        // 2. Delete one char (extra char in input)
        for i in 0..len {
            let mut c = chars.clone();
            c.remove(i);
            let w: String = c.into_iter().collect();
            if self.backend.contains(&w) {
                candidates.push(w);
            }
        }

        // 3. Insert one char at each position
        for i in 0..=len {
            for ch in 'a'..='z' {
                let mut c = chars.clone();
                c.insert(i, ch);
                let w: String = c.into_iter().collect();
                if self.backend.contains(&w) {
                    candidates.push(w);
                }
            }
        }

        // 4. Substitute one char
        for i in 0..len {
            for ch in 'a'..='z' {
                if ch == chars[i] {
                    continue;
                }
                let mut c = chars.clone();
                c[i] = ch;
                let w: String = c.into_iter().collect();
                if self.backend.contains(&w) {
                    candidates.push(w);
                }
            }
        }

        // Also try prefix match as fallback for partial input
        let prefix_exact = self.backend.query_prefixes(&[DictionaryQuery {
            prefix: Some(word_lower.clone()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        for r in prefix_exact {
            candidates.push(r.word);
        }

        // Deduplicate preserving order
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|w| seen.insert(w.clone()));

        // Score: edit-distance candidates first (by frequency), then prefix matches
        candidates.sort_by(|a, b| {
            let a_freq = self.backend.get_frequency(a);
            let b_freq = self.backend.get_frequency(b);
            b_freq
                .partial_cmp(&a_freq)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cmp(b))
        });

        candidates.truncate(10);
        candidates
    }
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
