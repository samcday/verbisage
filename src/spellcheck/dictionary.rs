use std::sync::Arc;

use crate::dictionary::DictionaryBackend;
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

    fn suggest(&self, word: &str, context: &[&str]) -> Vec<String> {
        crate::spellcheck::suggest::suggest_edits(&*self.backend, None, word, context, 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn basic_spellcheck_with_file_backend() {
        let mut dict = crate::dictionary::FileDictionaryBackend::new();
        dict.add_word_mut("hello".to_string(), 1.0);
        dict.add_word_mut("world".to_string(), 1.0);

        let checker = DictionarySpellChecker::new(Arc::new(dict));
        assert!(checker.is_correct("hello"));
        assert!(!checker.is_correct("helo"));
    }

    #[test]
    fn suggests_something() {
        let mut dict = crate::dictionary::FileDictionaryBackend::new();
        dict.add_word_mut("hello".to_string(), 1.0);
        dict.add_word_mut("help".to_string(), 1.0);
        dict.add_word_mut("helm".to_string(), 1.0);

        let checker = DictionarySpellChecker::new(Arc::new(dict));
        let suggestions = checker.suggest("helo", &[]);
        assert!(!suggestions.is_empty());
        assert!(suggestions.contains(&"hello".to_string()));
    }

    #[test]
    fn suggests_hrello_to_hello() {
        let mut dict = crate::dictionary::FileDictionaryBackend::new();
        dict.add_word_mut("hello".to_string(), 1.0);

        let checker = DictionarySpellChecker::new(Arc::new(dict));
        let suggestions = checker.suggest("hrello", &[]);
        assert!(
            suggestions.contains(&"hello".to_string()),
            "expected 'hello' in suggestions, got: {:?}",
            suggestions
        );
    }
}
