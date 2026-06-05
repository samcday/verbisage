/// Trait that abstracts the n‑gram data source.
///
/// Implementors provide raw counts and candidate words without any scoring
/// logic.  The [`SmoothedPredictor`](crate::prediction::smoothed::SmoothedPredictor)
/// consumes this trait to compute interpolated probabilities.
pub trait NgramBackend: Send + Sync {
    /// Maximum n‑gram order supported (e.g., 3 for a trigram model).
    fn max_order(&self) -> usize;

    /// Total sum of all unigram counts (for the unigram denominator).
    fn unigram_total(&self) -> u64;

    /// Exact count for a given n‑gram (ordered sequence of words).
    ///
    /// Returns 0 if the n‑gram is not found.
    fn ngram_count(&self, ngram: &[&str]) -> u64;

    /// Gather candidate continuation words for a given context.
    ///
    /// The context is the last `order-1` words.  Returns `(word, count)`
    /// pairs sorted by count descending, limited to `max_candidates`.
    fn candidates(&self, context: &[&str], max_candidates: usize) -> Vec<(String, u64)>;

    /// Whether this backend supports write operations.
    fn is_writable(&self) -> bool {
        false
    }

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
