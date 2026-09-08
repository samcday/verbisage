//! Android Patricia dictionaries: stored word probabilities are ranking scores,
//! not corpus counts. Context prediction uses Patricia's probability API directly.
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use patricia_dict::{Dictionary, SearchParams, WordAttributes};

use super::{DictionaryBackend, DictionaryQuery, DictionaryResult};
use crate::prediction::{Prediction, Predictor};
use crate::spellcheck::SpellChecker;

pub struct PatriciaDictionaryBackend {
    dictionary: Dictionary,
}

fn usable(attributes: &WordAttributes) -> bool {
    !attributes.is_blacklisted && !attributes.is_not_a_word
}

fn rank(a: &DictionaryResult, b: &DictionaryResult) -> std::cmp::Ordering {
    b.confidence
        .total_cmp(&a.confidence)
        .then_with(|| a.word.cmp(&b.word))
}

impl PatriciaDictionaryBackend {
    pub fn open(path: &Path) -> patricia_dict::Result<Self> {
        Ok(Self {
            dictionary: Dictionary::open(path)?,
        })
    }

    fn attributes(&self, word: &str) -> Option<WordAttributes> {
        self.dictionary
            .query(word)
            .or_else(|_| self.dictionary.query(&word.to_lowercase()))
            .ok()
            .filter(usable)
    }

    fn search_params(query: &DictionaryQuery) -> SearchParams {
        let mut params = SearchParams::new()
            .with_include_not_a_word(false)
            .with_include_blacklisted(false)
            .with_length_range(
                query.min_length.unwrap_or(1),
                query.max_length.unwrap_or(usize::MAX),
            );
        if let Some(prefix) = &query.prefix {
            params = params
                .with_prefix(prefix)
                .with_prefix(prefix.to_lowercase());
        }
        if let Some(suffix) = &query.suffix {
            params = params
                .with_suffix(suffix)
                .with_suffix(suffix.to_lowercase());
        }
        params
    }
}

impl DictionaryBackend for PatriciaDictionaryBackend {
    fn is_empty(&self) -> bool {
        self.dictionary
            .filter(
                SearchParams::new()
                    .with_include_not_a_word(false)
                    .with_include_blacklisted(false)
                    .with_max_results(1),
            )
            .next()
            .is_none()
    }

    fn contains(&self, word: &str) -> bool {
        self.attributes(word).is_some()
    }

    fn get_frequency(&self, word: &str) -> f64 {
        self.attributes(word)
            .map_or(0.0, |a| f64::from(a.probability) / 255.0)
    }

    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        let mut words = HashMap::new();
        for query in queries {
            for entry in self.dictionary.filter(Self::search_params(query)) {
                words.insert(entry.word, f64::from(entry.probability) / 255.0);
            }
        }
        let mut results: Vec<_> = words
            .into_iter()
            .map(|(word, confidence)| DictionaryResult { word, confidence })
            .collect();
        results.sort_by(rank);
        results
    }

    fn query_limited(&self, queries: &[DictionaryQuery], max: usize) -> Vec<DictionaryResult> {
        let mut results: Vec<DictionaryResult> = Vec::new();
        if max == 0 {
            return results;
        }
        for query in queries {
            for entry in self.dictionary.filter(Self::search_params(query)) {
                if results.iter().any(|r| r.word == entry.word) {
                    continue;
                }
                let candidate = DictionaryResult {
                    word: entry.word,
                    confidence: f64::from(entry.probability) / 255.0,
                };
                let index = results
                    .binary_search_by(|r| rank(r, &candidate))
                    .unwrap_or_else(|i| i);
                if index < max {
                    results.insert(index, candidate);
                    if results.len() > max {
                        results.pop();
                    }
                }
            }
        }
        results
    }
}

impl SpellChecker for Arc<PatriciaDictionaryBackend> {
    fn is_correct(&self, word: &str) -> bool {
        self.contains(word)
    }

    fn suggest(&self, word: &str, context: &[&str]) -> Vec<String> {
        // Keep Verbisage's current English single-edit generator; Patricia
        // supplies membership and frequency instead of Hunspell expansion.
        let mut suggestions =
            crate::spellcheck::suggest::suggest_edits(self.as_ref(), None, word, context, 10);
        if !context.is_empty() {
            suggestions.sort_by_cached_key(|candidate| {
                std::cmp::Reverse(
                    self.dictionary
                        .query_with_context(candidate, context)
                        .map_or(0, |a| a.probability),
                )
            });
        }
        suggestions
    }
}

impl Predictor for Arc<PatriciaDictionaryBackend> {
    fn predict_next(&self, context: &[&str], max_suggestions: usize) -> Vec<Prediction> {
        if context.is_empty() || max_suggestions == 0 {
            return Vec::new();
        }
        let mut candidates = HashMap::new();
        for (word, _) in self.dictionary.ngrams_for(context).unwrap_or_default() {
            if let Ok(attributes) = self.dictionary.query_with_context(&word, context) {
                if usable(&attributes) {
                    candidates.insert(word, f64::from(attributes.probability) / 255.0);
                }
            }
        }
        let mut results: Vec<_> = candidates
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patricia_completion_correction_and_context() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("en_US.dict");
        let mut dictionary = Dictionary::create_empty_v403(&path, "en_US").unwrap();
        dictionary.append("hello", 200).unwrap();
        dictionary.append("help", 100).unwrap();
        dictionary.append("world", 150).unwrap();
        dictionary.append("Linux", 150).unwrap();
        dictionary.append("linum", 100).unwrap();
        dictionary.add_ngram("world", &["hello"], 230).unwrap();
        drop(dictionary);
        let backend = Arc::new(PatriciaDictionaryBackend::open(&path).unwrap());
        assert!(backend.contains("hello"));
        assert!(!backend.contains("helo"));
        assert!((backend.get_frequency("hello") - 200.0 / 255.0).abs() < 0.001);
        let query = DictionaryQuery {
            prefix: Some("hel".into()),
            suffix: None,
            min_length: None,
            max_length: None,
        };
        assert_eq!(backend.query_limited(&[query.clone()], 1)[0].word, "hello");
        assert_eq!(backend.query_prefixes(&[query]).len(), 2);
        assert!(backend.suggest("helo", &[]).contains(&"hello".into()));
        assert!(crate::completion::complete(backend.as_ref(), "linux", 6).is_empty());
        assert_eq!(backend.predict_next(&["hello"], 1)[0].word, "world");
        assert!(backend.predict_next(&["unrecognized"], 3).is_empty());
        assert!(backend.predict_next(&["hello"], 0).is_empty());
        assert!(!backend.is_writable());
    }
}
