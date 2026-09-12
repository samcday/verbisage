//! Android Patricia dictionaries: stored word probabilities are ranking scores,
//! not corpus counts. Context prediction uses Patricia's probability API directly.
use std::collections::HashMap;
use std::path::Path;

use patricia_dict::{Dictionary, SearchParams, WordAttributes};

use super::{DictionaryBackend, DictionaryQuery, DictionaryResult};
use crate::prediction::ngram_backend::NgramBackend;

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
        self.dictionary.query(word).ok().filter(usable)
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
            params = params.with_prefix(prefix);
        }
        if let Some(suffix) = &query.suffix {
            params = params.with_suffix(suffix);
        }
        params
    }
}

impl DictionaryBackend for PatriciaDictionaryBackend {
    fn search_words(
        &self,
        search: &super::search::WordSearch<'_>,
        deadline: std::time::Instant,
    ) -> Result<Vec<DictionaryResult>, String> {
        use patricia_dict::VisitControl;
        super::search::check_deadline(deadline)?;
        let mut results = Vec::new();
        let mut failure = None;
        self.dictionary.traverse_nodes(&mut |node| {
            if let Err(error) = super::search::check_deadline(deadline) {
                failure = Some(error);
                return VisitControl::Stop;
            }
            if !search.may_descend(&node.prefix) {
                return VisitControl::PruneChildren;
            }
            if node.is_terminal && !node.is_not_a_word {
                if let Some(attributes) = node
                    .attributes
                    .as_ref()
                    .filter(|a| usable(a) && !a.represents_beginning_of_sentence)
                {
                    if let Err(error) = search.push(
                        &mut results,
                        &node.prefix,
                        f64::from(attributes.probability) / 255.0,
                        deadline,
                    ) {
                        failure = Some(error);
                        return VisitControl::Stop;
                    }
                }
            }
            VisitControl::Continue
        });
        if let Some(error) = failure {
            return Err(error);
        }
        super::search::check_deadline(deadline)?;
        Ok(results)
    }

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
            .map_or(-1.0, |a| f64::from(a.probability) / 255.0)
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

impl NgramBackend for PatriciaDictionaryBackend {
    fn supports_sentence_start(&self) -> bool {
        true
    }

    fn max_order(&self) -> usize {
        4
    }

    /// Patricia stores quantized probabilities, not counts; `255` is the
    /// denominator used by the stored `u8` probabilities.
    fn unigram_total(&self) -> u64 {
        255
    }

    fn ngram_count(&self, ngram: &[&str]) -> u64 {
        let order = ngram.len();
        if order == 0 || order > self.max_order() {
            return 0;
        }
        let candidate = ngram[order - 1];
        let context = &ngram[..order - 1];
        let Ok(scores) = self.dictionary.ngram_scores(candidate, context) else {
            return 0;
        };
        let value = match order {
            1 => Some(scores.unigram),
            2 => scores.bigram,
            3 => scores.trigram,
            _ => scores.quadgram,
        };
        value.map(u64::from).unwrap_or(0)
    }

    fn probability(&self, ngram: &[&str]) -> Option<f64> {
        let order = ngram.len();
        if order == 0 || order > self.max_order() {
            return None;
        }
        let candidate = ngram[order - 1];
        let context = &ngram[..order - 1];
        let prepared = self.dictionary.prepare_context(context);
        let available = prepared.available_order();
        let scores = self
            .dictionary
            .ngram_scores_prepared(candidate, &prepared)
            .ok()?;
        let value = match order {
            1 => Some(scores.unigram),
            2 => scores.bigram,
            3 => scores.trigram,
            _ => scores.quadgram,
        };
        let probability = match value {
            Some(value) => value,
            // The context had this order available but the specific n-gram is
            // absent: back off to zero rather than dropping the order.
            None if order <= available => 0,
            None => return None,
        };
        Some(f64::from(probability) / 255.0)
    }

    fn candidates(&self, context: &[&str], max_candidates: usize) -> Vec<(String, u64)> {
        let mut candidates: Vec<(String, u64)> = self
            .dictionary
            .ngrams_for(context)
            .unwrap_or_default()
            .into_iter()
            .map(|(word, score)| (word, u64::from(score)))
            .collect();
        candidates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        candidates.truncate(max_candidates);
        candidates
    }

    fn is_writable(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prediction::Predictor;
    use crate::prediction::smoothed::SmoothedPredictor;
    use crate::spellcheck::{DictionarySpellChecker, SpellChecker};
    use std::sync::Arc;

    #[test]
    fn spelling_ranks_context_before_truncating_unigram_candidates() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("spelling.dict");
        let mut dictionary = Dictionary::create_empty_v403(&path, "en_US").unwrap();
        dictionary.append("see", 100).unwrap();
        for letter in "bcdfghjklmnpqrstvwz".chars() {
            dictionary
                .append(&format!("{letter}at"), if letter == 'z' { 1 } else { 200 })
                .unwrap();
        }
        dictionary.add_ngram("zat", &["see"], 250).unwrap();
        drop(dictionary);
        let backend = Arc::new(PatriciaDictionaryBackend::open(&path).unwrap());
        let checker = DictionarySpellChecker::new(backend.clone())
            .with_predictor(Some(Arc::new(SmoothedPredictor::new(backend.clone()))));
        assert!(!checker.suggest("xat", &[]).contains(&"zat".into()));
        let suggestions = checker.suggest("xat", &["see"]);
        assert_eq!(suggestions.len(), 10);
        assert_eq!(suggestions[0], "zat");
    }

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
        let predictor = SmoothedPredictor::new(backend.clone());
        let checker = DictionarySpellChecker::new(backend.clone())
            .with_predictor(Some(Arc::new(SmoothedPredictor::new(backend.clone()))));
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
        assert!(checker.suggest("helo", &[]).contains(&"hello".into()));
        assert!(crate::completion::complete(backend.as_ref(), "linux", 6).is_empty());
        assert_eq!(predictor.predict_next(&["hello"], 1)[0].word, "world");
        assert!(predictor.predict_next(&["unrecognized"], 3).is_empty());
        assert!(predictor.predict_next(&["hello"], 0).is_empty());
        assert!(!crate::dictionary::DictionaryBackend::is_writable(
            backend.as_ref()
        ));
    }
}
