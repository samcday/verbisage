/// Spell‑checking trait analogous to the Hunspell API.
///
/// Every implementor must be [`Send`] + [`Sync`] so it can be shared across
/// threads (e.g. inside an `Arc`).
pub trait SpellChecker: Send + Sync {
    /// Returns `true` when `word` is considered correctly spelled.
    fn is_correct(&self, word: &str) -> bool;

    /// Returns a list of suggested corrections, sorted by relevance
    /// (most likely first).  `context` is optional preceding words that
    /// can be used to boost candidates that form common n‑grams.
    fn suggest(&self, word: &str, context: &[&str]) -> Vec<String>;

    /// Whether this spellchecker can benefit from an attached n-gram backend.
    /// Returns `false` for spellcheckers that use their own internal
    /// suggestion engine (e.g. zspell's embedded corrections).
    fn can_use_ngram_backend(&self) -> bool {
        false
    }

    /// Whether this spellchecker currently has an n-gram backend attached.
    fn has_ngram_backend(&self) -> bool {
        false
    }

    /// Attach an n-gram backend for context-aware suggestion scoring.
    /// No-op if the spellchecker doesn't support n-gram backends.
    fn set_ngram_backend(
        &self,
        _backend: std::sync::Arc<dyn crate::prediction::ngram_backend::NgramBackend>,
    ) {
    }
}

pub mod dictionary;
pub mod suggest;

#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "hunspell")]
pub mod hunspell;

pub use dictionary::DictionarySpellChecker;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteSpellChecker;

#[cfg(feature = "hunspell")]
pub use hunspell::HunspellSpellChecker;
