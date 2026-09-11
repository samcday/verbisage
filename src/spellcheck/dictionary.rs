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
        let mut candidates = crate::spellcheck::suggest::edit_candidates(
            &*self.backend,
            &word_lower,
            source_ref,
        );

        let mut ranked_by_model = false;
        if let Some(predictor) = &self.predictor {
            let keys: Vec<_> = candidates
                .iter()
                .map(|word| (word.as_str(), word.as_str()))
                .collect();
            let deadline = Instant::now() + Duration::from_secs(5);
            if let Ok(scores) = predictor.score_candidates(input.context, &keys, deadline) {
                let mut ranked: Vec<_> = candidates.into_iter().zip(scores).collect();
                ranked.sort_by(|(a, a_score), (b, b_score)| {
                    b_score
                        .unwrap_or(0.0)
                        .total_cmp(&a_score.unwrap_or(0.0))
                        .then_with(|| a.cmp(b))
                });
                candidates = ranked.into_iter().map(|(word, _)| word).collect();
                ranked_by_model = true;
            }
        }
        if !ranked_by_model {
            crate::spellcheck::suggest::sort_by_frequency(&*self.backend, &mut candidates);
        }

        if !input.spatial.is_none() {
            candidates.sort_by(|a, b| {
                let distance_a = input.spatial.word_distance(input.word, a).unwrap_or(0.0);
                let distance_b = input.spatial.word_distance(input.word, b).unwrap_or(0.0);
                distance_a.total_cmp(&distance_b).then_with(|| a.cmp(b))
            });
        }

        candidates.truncate(max);
        candidates
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
    fn layout_restricts_spelling_corrections() {
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
        assert!(with_layout.contains(&"cat".to_string()), "{with_layout:?}");
        assert!(
            !with_layout.contains(&"cay".to_string()),
            "a far-key correction must be pruned: {with_layout:?}"
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
