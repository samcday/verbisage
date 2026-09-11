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

    #[cfg(feature = "swipe")]
    fn swipe_candidates(
        &self,
        starts: &[String],
        ends: &[String],
        letters: &[u8],
        deadline: std::time::Instant,
    ) -> Result<Vec<DictionaryResult>, String> {
        use crate::swipe::{MAX_CANDIDATES, MAX_TRIE_NODES, MAX_WORD_BYTES};
        use patricia_dict::VisitControl;

        if std::time::Instant::now() >= deadline {
            return Err("swipe candidate search exceeded its time budget".into());
        }
        if starts.is_empty() || ends.is_empty() || letters.is_empty() {
            return Ok(Vec::new());
        }
        let starts: Vec<_> = starts.iter().map(|s| s.to_ascii_lowercase()).collect();
        let ends: Vec<_> = ends.iter().map(|s| s.to_ascii_lowercase()).collect();
        let mut results: Vec<DictionaryResult> = Vec::new();
        let mut visited = 0;
        let mut failure = None;
        // A filtering iterator can inspect an entire nonmatching subtree before
        // yielding once. Check every trie node, including rejected terminals,
        // so impossible endpoint combinations still obey the work budget.
        self.dictionary.traverse_nodes(&mut |node| {
            if std::time::Instant::now() >= deadline {
                failure = Some("swipe candidate search exceeded its time budget");
                return VisitControl::Stop;
            }
            if visited >= MAX_TRIE_NODES {
                failure = Some("swipe candidate search exceeded its node budget");
                return VisitControl::Stop;
            }
            visited += 1;
            if node.prefix.len() > MAX_WORD_BYTES
                || !node
                    .prefix
                    .bytes()
                    .all(|byte| letters.contains(&byte.to_ascii_lowercase()))
            {
                return VisitControl::PruneChildren;
            }
            let word = node.prefix.to_ascii_lowercase();
            if !starts
                .iter()
                .any(|start| word.starts_with(start) || start.starts_with(&word))
            {
                return VisitControl::PruneChildren;
            }
            let descend = if word.len() < MAX_WORD_BYTES {
                VisitControl::Continue
            } else {
                VisitControl::PruneChildren
            };
            let Some(attributes) = node.attributes.as_ref() else {
                return descend;
            };
            if !node.is_terminal
                || word.len() < 2
                || node.is_not_a_word
                || !usable(attributes)
                || attributes.represents_beginning_of_sentence
                || !starts.iter().any(|start| word.starts_with(start))
                || !ends.iter().any(|end| word.ends_with(end))
            {
                return descend;
            }
            let candidate = DictionaryResult {
                word,
                confidence: f64::from(attributes.probability) / 255.0,
            };
            if let Some(index) = results.iter().position(|r| r.word == candidate.word) {
                if results[index].confidence >= candidate.confidence {
                    return descend;
                }
                results.remove(index);
            }
            let index = results
                .binary_search_by(|r| rank(r, &candidate))
                .unwrap_or_else(|i| i);
            if index < MAX_CANDIDATES {
                results.insert(index, candidate);
                if results.len() > MAX_CANDIDATES {
                    results.pop();
                }
            }
            descend
        });
        if let Some(message) = failure {
            return Err(message.into());
        }
        if std::time::Instant::now() >= deadline {
            return Err("swipe candidate search exceeded its time budget".into());
        }
        Ok(results)
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
        suggestions.sort_by(|a, b| {
            self.candidate_score(context, b)
                .unwrap_or(0.0)
                .total_cmp(&self.candidate_score(context, a).unwrap_or(0.0))
                .then_with(|| a.cmp(b))
        });
        suggestions
    }
}

impl Predictor for Arc<PatriciaDictionaryBackend> {
    fn score_candidates(
        &self,
        context: &[&str],
        candidates: &[(&str, &str)],
        deadline: std::time::Instant,
    ) -> Result<Vec<Option<f64>>, String> {
        let prepared = self
            .dictionary
            .prepare_context(crate::text::after_boundary(context));
        let available = prepared.available_order();
        let mut results = Vec::with_capacity(candidates.len());
        for (candidate, _) in candidates {
            super::search::check_deadline(deadline)?;
            results.push(
                self.dictionary
                    .ngram_scores_prepared(candidate, &prepared)
                    .ok()
                    .and_then(|s| {
                        crate::prediction::scoring::interpolate_probabilities(&[
                            Some(f64::from(s.unigram) / 255.0),
                            s.bigram
                                .map(|p| f64::from(p) / 255.0)
                                .or_else(|| (available >= 2).then_some(0.0)),
                            s.trigram
                                .map(|p| f64::from(p) / 255.0)
                                .or_else(|| (available >= 3).then_some(0.0)),
                            s.quadgram
                                .map(|p| f64::from(p) / 255.0)
                                .or_else(|| (available >= 4).then_some(0.0)),
                        ])
                    }),
            );
        }
        super::search::check_deadline(deadline)?;
        Ok(results)
    }

    fn candidate_score(&self, context: &[&str], candidate: &str) -> Option<f64> {
        self.attributes(candidate)?;
        self.score_candidates(
            context,
            &[(candidate, candidate)],
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        )
        .ok()?
        .into_iter()
        .next()
        .flatten()
    }

    fn predict_next(&self, context: &[&str], max_suggestions: usize) -> Vec<Prediction> {
        if context.is_empty() || max_suggestions == 0 {
            return Vec::new();
        }
        let mut candidates = HashMap::new();
        for (word, _) in self.dictionary.ngrams_for(context).unwrap_or_default() {
            if let Ok(attributes) = self.dictionary.query_with_context(&word, context) {
                if usable(&attributes) {
                    let score = self.candidate_score(context, &word).unwrap_or(0.0);
                    candidates.insert(word, score);
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

    #[cfg(feature = "swipe")]
    fn swipe_fixture(words: &[(String, u8, u8)]) -> (tempfile::TempDir, PatriciaDictionaryBackend) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("swipe.dict");
        Dictionary::create_empty_v403(&path, "en_US").unwrap();
        patricia_dict::v4::writer::append_words_batch_with_flags(&path, words).unwrap();
        let backend = PatriciaDictionaryBackend::open(&path).unwrap();
        (temp, backend)
    }

    #[cfg(feature = "swipe")]
    #[test]
    fn swipe_filters_flags_unicode_and_lengths_without_pruning_real_children() {
        use std::time::{Duration, Instant};
        let mut words: Vec<_> = [
            ("CAT", 200, 0),
            ("cat", 100, 0),
            ("cart", 190, 0),
            ("cut", 240, 0x08),
            ("cuts", 170, 0),
            ("cot", 245, 0x04),
            ("cots", 180, 0),
            ("cant", 250, 0x01),
            ("cäst", 255, 0),
            ("ca-t", 255, 0),
            ("c1t", 255, 0),
            ("cabt", 255, 0),
            ("c", 255, 0),
        ]
        .into_iter()
        .map(|(word, score, flags)| (word.into(), score, flags))
        .collect();
        let longest = format!("c{}t", "a".repeat(crate::swipe::MAX_WORD_BYTES - 2));
        words.push((longest.clone(), 90, 0));
        // Patricia's format supports at most 48 code points. Its writer does
        // not reject longer input yet; an over-limit sibling would create an
        // invalid fixture that also disrupts traversal of this valid word.
        let (_temp, backend) = swipe_fixture(&words);
        let results = backend
            .swipe_candidates(
                &["c".into()],
                &["t".into(), "s".into()],
                b"acorstun",
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(
            results.iter().map(|r| r.word.as_str()).collect::<Vec<_>>(),
            vec!["cat", "cart", "cots", "cuts", longest.as_str()]
        );
        assert_eq!(results[0].confidence, 200.0 / 255.0);
        assert_eq!(
            results.len(),
            results
                .iter()
                .map(|r| &r.word)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        );
    }

    #[cfg(feature = "swipe")]
    #[test]
    fn swipe_expired_search_rejects_even_when_no_word_matches() {
        let (_temp, backend) = swipe_fixture(&[("cat".into(), 100, 0)]);
        let result = backend.swipe_candidates(
            &["c".into()],
            &["z".into()],
            b"abcdefghijklmnopqrstuvwxyz",
            std::time::Instant::now(),
        );
        assert!(result.unwrap_err().contains("time budget"));
    }

    #[cfg(feature = "swipe")]
    #[test]
    fn swipe_node_budget_counts_nonmatching_words() {
        use std::time::{Duration, Instant};
        // No word ends in z. A yielded-word counter would never advance while
        // scanning this trie; its node count exceeds the explicit work cap.
        let words: Vec<_> = (0..=crate::swipe::MAX_TRIE_NODES)
            .map(|mut i| {
                let mut word = [b'a'; 6];
                word[5] = b'b';
                for digit in word[1..5].iter_mut().rev() {
                    *digit = b'a' + (i % 26) as u8;
                    i /= 26;
                }
                assert_eq!(i, 0, "fixture needs more base-26 digits");
                (String::from_utf8(word.to_vec()).unwrap(), 100, 0)
            })
            .collect();
        let (_temp, backend) = swipe_fixture(&words);
        let result = backend.swipe_candidates(
            &["a".into()],
            &["z".into()],
            b"abcdefghijklmnopqrstuvwxyz",
            Instant::now() + Duration::from_secs(60),
        );
        assert!(result.unwrap_err().contains("node budget"));
    }
}
