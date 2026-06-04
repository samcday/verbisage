use std::sync::Arc;

use crate::dictionary::DictionaryBackend;
use crate::prediction::{Prediction, Predictor};

/// Context‑free predictor that returns the most frequent words in the
/// dictionary.
///
/// This is a fallback for when no n‑gram data is available — it ignores
/// the context entirely and returns the top‑N words by global frequency.
///
/// # Attention points for the implementor
///
/// * The current implementation does a full scan of the dictionary because
///   [`DictionaryBackend`] has no "top‑N by frequency" method.  For
///   production use either:
///   - Add a `top_n(n: usize) -> Vec<DictionaryResult>` method to the
///     backend trait (or to specific backends).
///   - Cache a pre‑sorted frequency list during backend construction.
/// * The 10 hard‑coded entries below are placeholder values.
pub struct FrequencyPredictor<B: DictionaryBackend> {
    backend: Arc<B>,
}

impl<B: DictionaryBackend> FrequencyPredictor<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B: DictionaryBackend> Predictor for FrequencyPredictor<B> {
    fn predict_next(&self, _context: &[&str], max_suggestions: usize) -> Vec<Prediction> {
        // Placeholder: Without a backend API to enumerate words sorted by
        // frequency, we return an empty vec.  The implementation must:
        //
        //   1. Obtain N words from the backend, sorted by get_frequency
        //      descending.
        //   2. Map to Prediction { word, confidence }.
        //   3. Truncate to max_suggestions.
        //
        // For FileDictionaryBackend, this could iterate `words_sorted` and
        // call `get_frequency` on each word, keeping the top N.  For
        // SqliteDictionaryBackend, use `ORDER BY frequency_column DESC`.
        let _ = max_suggestions;
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_predictor_returns_empty_for_now() {
        let backend = Arc::new(crate::dictionary::FileDictionaryBackend::new());
        let predictor = FrequencyPredictor::new(backend);
        let results = predictor.predict_next(&["hello"], 5);
        assert!(results.is_empty());
    }
}
