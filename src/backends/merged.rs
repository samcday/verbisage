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
    fn search_words(
        &self,
        search: &crate::dictionary::search::WordSearch<'_>,
        deadline: std::time::Instant,
    ) -> Result<Vec<DictionaryResult>, String> {
        let mut words: HashMap<String, f64> = HashMap::new();
        for backend in &self.backends {
            for row in backend.search_words(search, deadline)? {
                crate::dictionary::search::check_deadline(deadline)?;
                if !words.contains_key(&row.word) && words.len() >= search.limit {
                    return Err("merged search exceeds candidate budget".into());
                }
                let value = words.entry(row.word).or_insert(-1.0);
                if crate::dictionary::usable_frequency(row.confidence) >= 0.0 {
                    *value = (value.max(0.0) + row.confidence).min(1.0);
                }
            }
        }
        Ok(words
            .into_iter()
            .map(|(word, confidence)| DictionaryResult { word, confidence })
            .collect())
    }

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
    fn score_candidates(
        &self,
        context: &[&str],
        candidates: &[(&str, &str)],
        deadline: std::time::Instant,
    ) -> Result<Vec<Option<f64>>, String> {
        // Preserve each model's stored-word versus prepared-key policy and
        // context batching instead of resolving it afresh for every candidate.
        let mut scores: Vec<Option<f64>> = vec![None; candidates.len()];
        for predictor in &self.predictors {
            let model = predictor.score_candidates(context, candidates, deadline)?;
            if model.len() != scores.len() {
                return Err("predictor returned an invalid batch length".into());
            }
            for (sum, value) in scores.iter_mut().zip(model) {
                crate::dictionary::search::check_deadline(deadline)?;
                if let Some(value) = value.filter(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
                    *sum = Some((sum.unwrap_or(0.0) + value).min(1.0));
                }
            }
        }
        Ok(scores)
    }

    fn candidate_score(&self, context: &[&str], candidate: &str) -> Option<f64> {
        let scores: Vec<_> = self
            .predictors
            .iter()
            .filter_map(|p| p.candidate_score(context, candidate))
            .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
            .collect();
        (!scores.is_empty()).then(|| scores.iter().sum::<f64>().min(1.0))
    }

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

    struct BatchModel(f64);
    impl Predictor for BatchModel {
        fn predict_next(&self, _: &[&str], _: usize) -> Vec<Prediction> {
            vec![]
        }
        fn candidate_score(&self, _: &[&str], _: &str) -> Option<f64> {
            panic!("merging must preserve the model's batch key policy")
        }
        fn score_candidates(
            &self,
            context: &[&str],
            candidates: &[(&str, &str)],
            _: std::time::Instant,
        ) -> Result<Vec<Option<f64>>, String> {
            assert_eq!(context, ["see"]);
            assert_eq!(candidates, [("London", "london"), ("missing", "missing")]);
            Ok(vec![Some(self.0), None])
        }
    }
    #[test]
    fn merged_batch_preserves_native_keys_and_absent_scores() {
        let merged =
            MergedPredictor::new(vec![Box::new(BatchModel(0.6)), Box::new(BatchModel(0.7))]);
        let result = merged
            .score_candidates(
                &["see"],
                &[("London", "london"), ("missing", "missing")],
                std::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(result, [Some(1.0), None]);
    }

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
