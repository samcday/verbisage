use std::collections::HashMap;

use crate::dictionary::{DictionaryBackend, DictionaryQuery, DictionaryResult};
use crate::prediction::{Prediction, Predictor};

/// Presage-style merged dictionary.
///
/// Queries all inner backends independently, then merges results in memory:
///
/// - `query_prefixes`: union from all backends, deduped by word,
///   confidence values summed (capped at 1.0).
/// - `contains`: true if ANY backend contains the word.
/// - `get_frequency`: sum of frequencies from all backends.
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
        // Collect all results, then merge by word with confidence accumulation.
        let mut acc: HashMap<String, f64> = HashMap::new();

        for backend in &self.backends {
            for r in backend.query_prefixes(queries) {
                let entry = acc.entry(r.word).or_insert(-1.0);
                let conf = crate::dictionary::usable_frequency(r.confidence);
                if conf >= 0.0 {
                    *entry = (entry.max(0.0) + conf).min(1.0);
                }
            }
        }

        let mut results: Vec<DictionaryResult> = acc
            .into_iter()
            .map(|(word, confidence)| DictionaryResult { word, confidence })
            .collect();

        results.sort_by(|a, b| {
            b.confidence
                .total_cmp(&a.confidence)
                .then_with(|| a.word.cmp(&b.word))
        });

        results
    }

    fn contains(&self, word: &str) -> bool {
        self.backends.iter().any(|b| b.contains(word))
    }

    fn get_frequency(&self, word: &str) -> f64 {
        let values: Vec<_> = self
            .backends
            .iter()
            .map(|b| crate::dictionary::usable_frequency(b.get_frequency(word)))
            .filter(|f| *f >= 0.0)
            .collect();
        if values.is_empty() {
            -1.0
        } else {
            values.iter().sum::<f64>().min(1.0)
        }
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

/// Presage-style merged predictor.
///
/// Each inner predictor queries independently, then results are merged:
/// suggestions with the same word have their probabilities summed (capped
/// at 1.0), matching the MeritocracyCombiner::filter() pattern from Presage.
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
        // Collect all predictions, then merge by word with probability accumulation.
        let mut acc: HashMap<String, f64> = HashMap::new();

        for pred in &self.predictors {
            for p in pred.predict_next(context, max_suggestions) {
                let entry = acc.entry(p.word).or_insert(0.0);
                *entry = (*entry + p.confidence).min(1.0);
            }
        }

        let mut results: Vec<Prediction> = acc
            .into_iter()
            .map(|(word, confidence)| Prediction { word, confidence })
            .collect();

        results.sort_by(|a, b| {
            b.confidence
                .total_cmp(&a.confidence)
                .then_with(|| a.word.cmp(&b.word))
        });

        results.truncate(max_suggestions);
        results
    }

    fn increase_ngram_frequency(
        &self,
        ngram: &[&str],
        delta: f64,
        save_unknown: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for pred in &self.predictors {
            match pred.increase_ngram_frequency(ngram, delta, save_unknown) {
                Ok(()) => return Ok(()),
                Err(_) => continue,
            }
        }
        Err("no predictor supports n-gram frequency updates".into())
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
    fn merged_confidence_accumulates() {
        let a = make_dict(&[("hello", 0.3), ("other", 0.7)]);
        let b = make_dict(&[("hello", 0.5), ("other", 0.5)]);
        let merged = MergedDictionary::new(vec![a, b]);
        let results = merged.query_prefixes(&[DictionaryQuery {
            prefix: Some("hel".into()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        assert_eq!(results.len(), 1);
        // Confidence accumulates: 0.3 + 0.5 = 0.8
        assert_eq!(results[0].confidence, 0.8);
    }

    #[test]
    fn merged_confidence_caps_at_one() {
        let a = make_dict(&[("hello", 0.8), ("other", 0.2)]);
        let b = make_dict(&[("hello", 0.6), ("other", 0.4)]);
        let merged = MergedDictionary::new(vec![a, b]);
        let results = merged.query_prefixes(&[DictionaryQuery {
            prefix: Some("hel".into()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        assert_eq!(results.len(), 1);
        // 0.8 + 0.6 = 1.4, capped at 1.0
        assert_eq!(results[0].confidence, 1.0);
    }

    #[test]
    fn merged_frequency_sums() {
        let a = make_dict(&[("hello", 0.3), ("other", 0.7)]);
        let b = make_dict(&[("hello", 0.5), ("other", 0.5)]);
        let merged = MergedDictionary::new(vec![a, b]);
        // Frequencies sum across all backends
        assert_eq!(merged.get_frequency("hello"), 0.8);
    }
}
