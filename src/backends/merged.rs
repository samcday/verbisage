use std::collections::HashSet;

use crate::dictionary::{DictionaryBackend, DictionaryQuery, DictionaryResult};
use crate::prediction::{Prediction, Predictor};

/// A merged dictionary that queries multiple `DictionaryBackend` instances
/// and combines their results.
///
/// - `query_prefixes`: union from all inner backends, deduped by word,
///   first-source confidence wins.
/// - `contains`: true if any inner backend has the word.
/// - `get_frequency`: first non-zero from inner backends (order: dict then freq).
pub struct MergedDictionary {
    backends: Vec<Box<dyn DictionaryBackend>>,
}

impl MergedDictionary {
    pub fn new(backends: Vec<Box<dyn DictionaryBackend>>) -> Self {
        Self { backends }
    }
}

impl DictionaryBackend for MergedDictionary {
    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        let mut seen = HashSet::new();
        let mut results = Vec::new();

        for backend in &self.backends {
            for r in backend.query_prefixes(queries) {
                if seen.insert(r.word.clone()) {
                    results.push(r);
                }
            }
        }

        results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.word.cmp(&b.word))
        });

        results
    }

    fn contains(&self, word: &str) -> bool {
        self.backends.iter().any(|b| b.contains(word))
    }

    fn get_frequency(&self, word: &str) -> f64 {
        for backend in &self.backends {
            let freq = backend.get_frequency(word);
            if freq > 0.0 {
                return freq;
            }
        }
        0.0
    }

    fn is_writable(&self) -> bool {
        self.backends.iter().any(|b| b.is_writable())
    }

    fn add_word(
        &self,
        word: &str,
        frequency: f64,
        allow_existing: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for backend in &self.backends {
            if backend.is_writable() {
                return backend.add_word(word, frequency, allow_existing);
            }
        }
        Err("no writable backend found".into())
    }
}

/// A merged predictor that queries multiple `Predictor` instances.
///
/// Results are merged by word (first-source wins), then sorted by confidence
/// descending.
pub struct MergedPredictor {
    predictors: Vec<Box<dyn Predictor>>,
}

impl MergedPredictor {
    pub fn new(predictors: Vec<Box<dyn Predictor>>) -> Self {
        Self { predictors }
    }
}

impl Predictor for MergedPredictor {
    fn predict_next(&self, context: &[&str], max_suggestions: usize) -> Vec<Prediction> {
        let mut seen = HashSet::new();
        let mut results = Vec::new();

        for pred in &self.predictors {
            for p in pred.predict_next(context, max_suggestions) {
                if seen.insert(p.word.clone()) {
                    results.push(p);
                }
            }
        }

        results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.word.cmp(&b.word))
        });

        results.truncate(max_suggestions);
        results
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::FileDictionaryBackend;

    fn make_dict(words: &[(&str, f64)]) -> Box<dyn DictionaryBackend> {
        let mut d = FileDictionaryBackend::new();
        for (w, f) in words {
            d.add_word_mut(w.to_string(), *f);
        }
        Box::new(d)
    }

    #[test]
    fn merged_contains() {
        let a = make_dict(&[("hello", 1.0)]);
        let b = make_dict(&[("world", 1.0)]);
        let merged = MergedDictionary::new(vec![a, b]);
        assert!(merged.contains("hello"));
        assert!(merged.contains("world"));
        assert!(!merged.contains("foo"));
    }

    #[test]
    fn merged_dedupes() {
        let a = make_dict(&[("hello", 1.0)]);
        let b = make_dict(&[("hello", 0.5)]);
        let merged = MergedDictionary::new(vec![a, b]);
        let results = merged.query_prefixes(&[DictionaryQuery {
            prefix: Some("hel".into()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        assert_eq!(results.len(), 1);
        // First source confidence wins
        assert_eq!(results[0].confidence, 1.0);
    }

    #[test]
    fn merged_frequency_first_nonzero() {
        let a = make_dict(&[("hello", 0.0)]);
        let b = make_dict(&[("hello", 0.8)]);
        let merged = MergedDictionary::new(vec![a, b]);
        assert_eq!(merged.get_frequency("hello"), 0.8);
    }
}
