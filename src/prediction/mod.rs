/// Next-word prediction traits and types.
///
/// The central trait is [`Predictor`], with implementations for MARISA trie
/// backends and frequency-only (context-free) backends.  The
/// [`SmoothedPredictor`] provides linear interpolation smoothing across
/// n-gram orders.
pub mod frequency;
pub mod ngram_backend;
pub mod scoring;
pub mod smoothed;

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

    /// Score an individual candidate with the same language model used by
    /// prediction. Probability-backed dictionaries override this directly.
    fn candidate_score(&self, context: &[&str], candidate: &str) -> Option<f64> {
        self.ngram_backend().map(|backend| {
            scoring::interpolate_score(backend.as_ref(), context, candidate, backend.max_order())
        })
    }

    /// Batch scoring receives (stored word, prepared model key) pairs. Native
    /// case-preserving dictionaries use stored words; pre-folded count stores
    /// use model keys. Context is prepared separately, once per request.
    fn score_candidates(
        &self,
        context: &[&str],
        candidates: &[(&str, &str)],
        deadline: std::time::Instant,
    ) -> Result<Vec<Option<f64>>, String> {
        let mut scores = Vec::with_capacity(candidates.len());
        for (_, candidate) in candidates {
            crate::dictionary::search::check_deadline(deadline)?;
            scores.push(self.candidate_score(context, candidate));
        }
        crate::dictionary::search::check_deadline(deadline)?;
        Ok(scores)
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

    /// Return the underlying n-gram backend if available.
    ///
    /// Used by spellcheckers that want to score suggestion candidates
    /// using n-gram probabilities.  Returns `None` for frequency-only
    /// predictors that don't carry n-gram data.
    fn ngram_backend(&self) -> Option<std::sync::Arc<dyn ngram_backend::NgramBackend>> {
        None
    }
}
