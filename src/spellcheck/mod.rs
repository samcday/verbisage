/// Spell‑checking trait analogous to the Hunspell API.
///
/// Every implementor must be [`Send`] + [`Sync`] so it can be shared across
/// threads (e.g. inside an `Arc`).
pub trait SpellChecker: Send + Sync {
    /// Returns `true` when `word` is considered correctly spelled.
    fn is_correct(&self, word: &str) -> bool;

    /// Returns a list of suggested corrections, sorted by relevance
    /// (most likely first).
    fn suggest(&self, word: &str) -> Vec<String>;
}

pub mod dictionary;

#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "hunspell")]
pub mod hunspell;
