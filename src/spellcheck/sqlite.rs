use std::sync::Arc;

use crate::dictionary::{DictionaryBackend, PresageSqliteBackend};
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

    fn suggest(&self, word: &str, context: &[&str]) -> Vec<String> {
        crate::spellcheck::suggest::suggest_edits(
            &*self.backend,
            Some(&*self.backend),
            word,
            context,
            10,
        )
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
