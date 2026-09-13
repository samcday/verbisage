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

    #[cfg(feature = "swipe")]
    fn swipe_candidates(
        &self,
        vocabulary: &crate::swipe::SwipeVocabulary,
        starts: &[String],
        ends: &[String],
        deadline: std::time::Instant,
    ) -> Result<Vec<crate::swipe::SwipeCandidate>, String> {
        use crate::swipe::{
            MAX_CANDIDATES, MAX_TRIE_NODES, MAX_WORD_BYTES, SwipeCandidate, canonical,
        };
        use patricia_dict::VisitControl;
        use std::collections::BTreeSet;
        use unicode_segmentation::UnicodeSegmentation;

        if std::time::Instant::now() >= deadline {
            return Err("swipe candidate search exceeded its time budget".into());
        }
        if starts.is_empty() || ends.is_empty() {
            return Ok(Vec::new());
        }
        let starts: BTreeSet<String> = starts.iter().map(|s| canonical(s)).collect();
        let ends: BTreeSet<String> = ends.iter().map(|s| canonical(s)).collect();
        let mut results: Vec<SwipeCandidate> = Vec::new();
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
            // Byte length is a resource bound only; graphemes decide matching.
            if node.prefix.len() > MAX_WORD_BYTES {
                return VisitControl::PruneChildren;
            }
            // A node's last grapheme may still grow a combining mark below it,
            // so only complete graphemes can prune: an unknown one anywhere
            // before the end, or a settled first grapheme no gesture starts on.
            let Some(form) = vocabulary.prefix_form(&node.prefix) else {
                return VisitControl::PruneChildren;
            };
            let mut graphemes = form.graphemes(true);
            let first = graphemes.next();
            if graphemes.next().is_some() && first.is_some_and(|start| !starts.contains(start)) {
                return VisitControl::PruneChildren;
            }
            let descend = if node.prefix.len() < MAX_WORD_BYTES {
                VisitControl::Continue
            } else {
                VisitControl::PruneChildren
            };
            let Some(attributes) = node.attributes.as_ref() else {
                return descend;
            };
            if !node.is_terminal
                || node.is_not_a_word
                || !usable(attributes)
                || attributes.represents_beginning_of_sentence
            {
                return descend;
            }
            let Some(scoring) = vocabulary.scoring_form(&node.prefix) else {
                return descend;
            };
            let first = scoring.graphemes(true).next();
            let last = scoring.graphemes(true).next_back();
            if !first.is_some_and(|start| starts.contains(start))
                || !last.is_some_and(|end| ends.contains(end))
            {
                return descend;
            }
            let candidate = SwipeCandidate {
                word: node.prefix.clone(),
                scoring,
                confidence: f64::from(attributes.probability) / 255.0,
            };
            if let Some(index) = results.iter().position(|r| r.word == candidate.word) {
                if results[index].confidence >= candidate.confidence {
                    return descend;
                }
                results.remove(index);
            }
            let index = results
                .binary_search_by(|r| SwipeCandidate::rank(r, &candidate))
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
        let vocabulary = crate::swipe::SwipeVocabulary::from_labels(
            &["a", "c", "o", "r", "s", "t", "u", "n"],
            &[],
        );
        let results = backend
            .swipe_candidates(
                &vocabulary,
                &["c".into()],
                &["t".into(), "s".into()],
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        // Every stored spelling with a complete path is its own candidate, in
        // its stored form; the capital entry is not folded into the lowercase one.
        assert_eq!(
            results.iter().map(|r| r.word.as_str()).collect::<Vec<_>>(),
            vec!["CAT", "cart", "cots", "cuts", "cat", longest.as_str()]
        );
        assert_eq!(results[0].scoring, "cat");
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
    fn swipe_keeps_a_decomposed_accent_split_across_trie_nodes() {
        use std::time::{Duration, Instant};
        // "cafe" + combining acute shares its first four code points with
        // "cafes", so the trie splits at "cafe": the accent lives in a child
        // node. A prefix judged byte by byte, or by its incomplete last
        // grapheme, would prune the accented word away.
        let (_temp, backend) = swipe_fixture(&[
            ("cafe\u{301}".into(), 200, 0),
            ("cafes".into(), 100, 0),
            ("caf\u{e9}s".into(), 90, 0),
        ]);
        let vocabulary =
            crate::swipe::SwipeVocabulary::from_labels(&["c", "a", "f", "e", "\u{e9}"], &[]);
        let results = backend
            .swipe_candidates(
                &vocabulary,
                &["c".into()],
                &["e\u{301}".into()],
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!(results[0].word, "cafe\u{301}", "stored spelling is kept");
        assert_eq!(results[0].scoring, "caf\u{e9}", "scored in composed form");

        // Without an s on the layout neither plural has a complete path, and
        // the accented base word is still found.
        let without_s = backend
            .swipe_candidates(
                &vocabulary,
                &["c".into()],
                &["\u{e9}".into(), "s".into()],
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(
            without_s
                .iter()
                .map(|r| r.word.as_str())
                .collect::<Vec<_>>(),
            vec!["cafe\u{301}"]
        );
    }

    #[cfg(feature = "swipe")]
    #[test]
    fn swipe_expired_search_rejects_even_when_no_word_matches() {
        let (_temp, backend) = swipe_fixture(&[("cat".into(), 100, 0)]);
        let result = backend.swipe_candidates(
            &latin_vocabulary(),
            &["c".into()],
            &["z".into()],
            std::time::Instant::now(),
        );
        assert!(result.unwrap_err().contains("time budget"));
    }

    #[cfg(feature = "swipe")]
    fn latin_vocabulary() -> crate::swipe::SwipeVocabulary {
        let letters: Vec<String> = ('a'..='z').map(|c| c.to_string()).collect();
        let labels: Vec<&str> = letters.iter().map(String::as_str).collect();
        crate::swipe::SwipeVocabulary::from_labels(&labels, &[])
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
            &latin_vocabulary(),
            &["a".into()],
            &["z".into()],
            Instant::now() + Duration::from_secs(60),
        );
        assert!(result.unwrap_err().contains("node budget"));
    }
}
