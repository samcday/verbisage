/// Next‑word prediction traits and types.
///
/// The central trait is [`Predictor`], with implementations for SQLite
/// n‑gram tables, MARISA trie backends, and frequency‑only (context‑free)
/// backends.  The [`SmoothedPredictor`] provides linear interpolation
/// smoothing across n‑gram orders.
pub mod frequency;
pub mod ngram_backend;
pub mod smoothed;

#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "marisa")]
pub mod marisa;

/// A single next‑word prediction.
#[derive(Debug, Clone, PartialEq)]
pub struct Prediction {
    pub word: String,
    /// Confidence score in 0.0–1.0, where higher is more likely.
    pub confidence: f64,
}

/// Trait for predicting the next word given a sequence of preceding words.
///
/// All implementors must be [`Send`] + [`Sync`] so the trait object can be
/// shared across threads.
pub trait Predictor: Send + Sync {
    /// Return up to `max_suggestions` likely continuations for `context`.
    fn predict_next(&self, context: &[&str], max_suggestions: usize) -> Vec<Prediction>;

    /// Increase the frequency of an n-gram by `delta`.
    ///
    /// `ngram` is the full sequence including context and next word
    /// (e.g., `["hello", "world"]` for bigram "hello world").
    /// For unigrams, `ngram` is `["world"]`.
    ///
    /// `save_unknown`: if true, create the n-gram with frequency = `delta`
    /// when it doesn't exist; if false, return Err for unknown n-grams.
    fn increase_ngram_frequency(
        &self,
        _ngram: &[&str],
        _delta: f64,
        _save_unknown: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err("not supported".into())
    }
}
