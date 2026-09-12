use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::dictionary::DictionaryBackend;
use crate::prediction::Predictor;
use crate::spatial::SpatialInput;
use crate::spellcheck::edits::EditSource;
use crate::spellcheck::{SpellChecker, SuggestionInput};

/// Generic [`SpellChecker`] implementation backed by any [`DictionaryBackend`].
///
/// This owns the suggestion algorithm: one-edit candidate generation (optionally
/// layout/touch aware) plus frequency or language-model ranking. Backends only
/// supply dictionary data; an optional [`Predictor`] supplies context scores.
pub struct DictionarySpellChecker<B: DictionaryBackend> {
    backend: Arc<B>,
    predictor: Option<Arc<dyn Predictor>>,
}

impl<B: DictionaryBackend> DictionarySpellChecker<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self {
            backend,
            predictor: None,
        }
    }

    /// Attach a shared language model for context-aware ranking.
    pub fn with_predictor(mut self, predictor: Option<Arc<dyn Predictor>>) -> Self {
        self.predictor = predictor;
        self
    }
}

impl<B: DictionaryBackend> SpellChecker for DictionarySpellChecker<B> {
    fn is_correct(&self, word: &str) -> bool {
        self.backend.contains(word)
    }

    fn suggest(&self, word: &str, context: &[&str]) -> Vec<String> {
        self.suggest_with(
            &SuggestionInput {
                word,
                context,
                spatial: SpatialInput::None,
            },
            10,
        )
    }

    fn suggest_with(&self, input: &SuggestionInput<'_>, max: usize) -> Vec<String> {
        let word_lower = input.word.to_lowercase();
        if self.backend.contains(&word_lower) {
            return vec![word_lower];
        }

        let source = input.spatial.edit_source();
        let source_ref: Option<&dyn EditSource> = if input.spatial.is_none() {
            None
        } else {
            Some(&source)
        };
        let candidates = crate::spellcheck::suggest::edit_candidates(
            &*self.backend,
            &word_lower,
            source_ref,
        );
        if candidates.is_empty() {
            return Vec::new();
        }

        // Raw language/frequency score per candidate, in candidate order.
        let mut scores: Vec<f64> = match &self.predictor {
            Some(predictor) => {
                let keys: Vec<_> = candidates
                    .iter()
                    .map(|(word, _)| (word.as_str(), word.as_str()))
                    .collect();
                let deadline = Instant::now() + Duration::from_secs(5);
                match predictor.score_candidates(input.context, &keys, deadline) {
                    Ok(scores) => scores.into_iter().map(|s| s.unwrap_or(0.0)).collect(),
                    Err(_) => candidates
                        .iter()
                        .map(|(word, _)| self.backend.get_frequency(word))
                        .collect(),
                }
            }
            None => candidates
                .iter()
                .map(|(word, _)| self.backend.get_frequency(word))
                .collect(),
        };

        if input.spatial.is_none() {
            // No geometry: keep the pure language/frequency ordering.
            let mut ranked: Vec<_> = candidates.into_iter().zip(scores).collect();
            ranked.sort_by(|((a, _), a_score), ((b, _), b_score)| {
                b_score.total_cmp(a_score).then_with(|| a.cmp(b))
            });
            ranked.truncate(max);
            return ranked.into_iter().map(|((word, _), _)| word).collect();
        }

        // Layout/touch active: combine the edit cost (edit class plus proximity,
        // already encoded in the edit weight) with the language-model cost on a
        // single HeliBoard-style scale. Geometry refines ranking; it must not
        // bury the language model (a transposition like `teh` -> `the` is far
        // on the keyboard but the most likely word).
        let max_score = scores.iter().cloned().fold(f64::MIN, f64::max);
        let min_score = scores.iter().cloned().fold(f64::MAX, f64::min);
        let range = max_score - min_score;
        let input_len = input.word.chars().count();
        let max_distance = crate::spatial::DISTANCE_WEIGHT_LANGUAGE
            + input_len as f64 * crate::spatial::TYPING_MAX_OUTPUT_SCORE_PER_INPUT;
        let mut scored: Vec<(String, f64)> = candidates
            .into_iter()
            .zip(scores.drain(..))
            .map(|((word, edit_weight), raw)| {
                let language = if range > 0.0 {
                    1.0 - (raw - min_score) / range
                } else {
                    0.0
                };
                let edit_cost = 1.0 - edit_weight.clamp(0.0, 1.0);
                let cost = edit_cost + language * crate::spatial::DISTANCE_WEIGHT_LANGUAGE;
                (word, (1.0 - cost / max_distance).clamp(0.0, 1.0))
            })
            .collect();
        scored.sort_by(|(a, a_score), (b, b_score)| {
            b_score.total_cmp(a_score).then_with(|| a.cmp(b))
        });
        scored.truncate(max);
        scored.into_iter().map(|(word, _)| word).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::{SpatialInput, TouchPoint};
    use keyboard_layout::{RectKey, RectKeyLayout};
    use std::sync::Arc;

    #[test]
    fn touch_distance_ranks_spelling_corrections() {
        let mut dict = crate::dictionary::FileDictionaryBackend::new();
        dict.add_word_mut("cat".to_string(), 10.0);
        dict.add_word_mut("car".to_string(), 10.0);
        let checker = DictionarySpellChecker::new(Arc::new(dict));

        let layout = Arc::new(RectKeyLayout::new(
            vec![
                RectKey::from_rect(Some("c".into()), vec![], 0.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("a".into()), vec![], 10.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("z".into()), vec![], 20.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("t".into()), vec![], 20.0, 10.0, 10.0, 10.0),
                RectKey::from_rect(Some("r".into()), vec![], 10.0, 10.0, 10.0, 10.0),
            ],
            &[],
        ));
        let points = vec![
            TouchPoint::new(5.0, 5.0),
            TouchPoint::new(15.0, 5.0),
            TouchPoint::new(25.0, 5.0),
        ];
        let input = SuggestionInput {
            word: "caz",
            context: &[],
            spatial: SpatialInput::from_parts(Some(layout), points),
        };
        let suggestions = checker.suggest_with(&input, 10);
        assert_eq!(
            suggestions.first().map(String::as_str),
            Some("cat"),
            "{suggestions:?}"
        );
    }

    #[test]
    fn layout_ranks_near_key_corrections_first() {
        let mut dict = crate::dictionary::FileDictionaryBackend::new();
        dict.add_word_mut("cat".to_string(), 10.0);
        dict.add_word_mut("cay".to_string(), 10.0);
        let checker = DictionarySpellChecker::new(Arc::new(dict));

        let no_layout = checker.suggest_with(
            &SuggestionInput {
                word: "caz",
                context: &[],
                spatial: crate::spatial::SpatialInput::None,
            },
            10,
        );
        assert!(no_layout.contains(&"cat".to_string()), "{no_layout:?}");
        assert!(no_layout.contains(&"cay".to_string()), "{no_layout:?}");

        let layout = Arc::new(RectKeyLayout::new(
            vec![
                RectKey::from_rect(Some("c".into()), vec![], 0.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("a".into()), vec![], 10.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("t".into()), vec![], 20.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("z".into()), vec![], 20.0, 10.0, 10.0, 10.0),
                RectKey::from_rect(Some("y".into()), vec![], 90.0, 0.0, 10.0, 10.0),
            ],
            &[],
        ));
        let with_layout = checker.suggest_with(
            &SuggestionInput {
                word: "caz",
                context: &[],
                spatial: crate::spatial::SpatialInput::from_parts(Some(layout), Vec::new()),
            },
            10,
        );
        assert_eq!(
            with_layout.first().map(String::as_str),
            Some("cat"),
            "a near-key correction must rank first: {with_layout:?}"
        );
        assert!(
            with_layout.contains(&"cay".to_string()),
            "a far-key correction must still be offered: {with_layout:?}"
        );
    }

    #[test]
    fn basic_spellcheck_with_file_backend() {
        let mut dict = crate::dictionary::FileDictionaryBackend::new();
        dict.add_word_mut("hello".to_string(), 1.0);
        dict.add_word_mut("world".to_string(), 1.0);

        let checker = DictionarySpellChecker::new(Arc::new(dict));
        assert!(checker.is_correct("hello"));
        assert!(!checker.is_correct("helo"));
    }

    #[test]
    fn suggests_something() {
        let mut dict = crate::dictionary::FileDictionaryBackend::new();
        dict.add_word_mut("hello".to_string(), 1.0);
        dict.add_word_mut("help".to_string(), 1.0);
        dict.add_word_mut("helm".to_string(), 1.0);

        let checker = DictionarySpellChecker::new(Arc::new(dict));
        let suggestions = checker.suggest("helo", &[]);
        assert!(!suggestions.is_empty());
        assert!(suggestions.contains(&"hello".to_string()));
    }

    #[test]
    fn suggests_hrello_to_hello() {
        let mut dict = crate::dictionary::FileDictionaryBackend::new();
        dict.add_word_mut("hello".to_string(), 1.0);

        let checker = DictionarySpellChecker::new(Arc::new(dict));
        let suggestions = checker.suggest("hrello", &[]);
        assert!(
            suggestions.contains(&"hello".to_string()),
            "expected 'hello' in suggestions, got: {:?}",
            suggestions
        );
    }
}
