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
