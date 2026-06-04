use std::sync::Arc;

use crate::dictionary::{DictionaryBackend, DictionaryQuery};
use crate::spellcheck::SpellChecker;

/// Generic [`SpellChecker`] implementation backed by any [`DictionaryBackend`].
///
/// * `is_correct` — delegates to [`DictionaryBackend::contains`].
/// * `suggest` — uses subsequence matching via the dictionary's
///   `query_prefixes` to find words that share a common subsequence with the
///   input.  This gives a quick (though not edit‑distance‑aware) set of
///   candidates.
///
/// # Attention points for the implementor
///
/// * The current suggestion strategy is naive (subsequence matching).
///   Replace with a full Levenshtein / Damerau‑Levenshtein sweep for
///   production use.
/// * Subsequence matching is done via the standard `query_prefixes` path,
///   which works with any [`DictionaryBackend`] without needing backend‑
///   specific code.
pub struct DictionarySpellChecker<B: DictionaryBackend> {
    backend: Arc<B>,
}

impl<B: DictionaryBackend> DictionarySpellChecker<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B: DictionaryBackend> SpellChecker for DictionarySpellChecker<B> {
    fn is_correct(&self, word: &str) -> bool {
        self.backend.contains(word)
    }

    fn suggest(&self, word: &str) -> Vec<String> {
        // Strategy: generate prefix queries for each prefix of the input,
        // plus a subsequence-matching query, and collect candidate words
        // that are "close" to the input.
        //
        // The implementation must:
        //   1. Build a `DictionaryQuery` for each prefix and suffix variation
        //      that could match the input.
        //   2. Query the backend and score candidates by similarity.
        //   3. Return the top N candidates, sorted by relevance.
        //
        // Current naive approach: query with the word as a prefix (catches
        // exact matches and longer words starting with the input), and also
        // query with the first few characters to catch common typos.
        let mut candidates = Vec::new();

        // Exact prefix match
        let exact = self.backend.query_prefixes(&[DictionaryQuery {
            prefix: Some(word.to_lowercase()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        candidates.extend(exact.into_iter().map(|r| r.word));

        // Relaxed prefix — first 3 characters (catches completions of
        // partial input).
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

        // Deduplicate while preserving order.
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|w| seen.insert(w.clone()));

        // Score candidates by shared prefix length (simple heuristic).
        // Production replacement: use Levenshtein distance.
        candidates.sort_by(|a, b| {
            let a_score = SharedPrefix::score(word, a);
            let b_score = SharedPrefix::score(word, b);
            b_score
                .partial_cmp(&a_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cmp(b))
        });

        candidates.truncate(10);
        candidates
    }
}

// ---------------------------------------------------------------------------
// Helper: simple prefix‑overlap score
// ---------------------------------------------------------------------------

struct SharedPrefix;

impl SharedPrefix {
    /// Score how well `candidate` matches `input` based on shared prefix.
    ///
    /// Exact match → f64::MAX (always first).  Otherwise the ratio of
    /// shared prefix length to the max length of either word.
    fn score(input: &str, candidate: &str) -> f64 {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn basic_spellcheck_with_file_backend() {
        let mut dict = crate::dictionary::FileDictionaryBackend::new();
        dict.add_word("hello".to_string(), 1.0);
        dict.add_word("world".to_string(), 1.0);

        let checker = DictionarySpellChecker::new(Arc::new(dict));
        assert!(checker.is_correct("hello"));
        assert!(!checker.is_correct("helo"));
    }

    #[test]
    fn suggests_something() {
        let mut dict = crate::dictionary::FileDictionaryBackend::new();
        dict.add_word("hello".to_string(), 1.0);
        dict.add_word("help".to_string(), 1.0);
        dict.add_word("helm".to_string(), 1.0);

        let checker = DictionarySpellChecker::new(Arc::new(dict));
        let suggestions = checker.suggest("hel");
        assert!(!suggestions.is_empty());
        assert!(suggestions.contains(&"hello".to_string()));
    }
}
