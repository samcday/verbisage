use std::sync::Arc;

use crate::dictionary::DictionaryBackend;
use crate::spellcheck::edits::EditSource;
use crate::spellcheck::{SpellChecker, SuggestionInput};

/// Generic [`SpellChecker`] implementation backed by any [`DictionaryBackend`].
///
/// * `is_correct` — delegates to [`DictionaryBackend::contains`].
/// * `suggest` — uses subsequence matching via the dictionary's
///   `query_prefixes` to find words that share a common subsequence with the
///   input.  This gives a quick (though not edit‑distance‑aware) set of
///   candidates.
///
/// # Attention points for the implementor
///
/// * The current suggestion strategy is naive (subsequence matching).
///   Replace with a full Levenshtein / Damerau‑Levenshtein sweep for
///   production use.
/// * Subsequence matching is done via the standard `query_prefixes` path,
///   which works with any [`DictionaryBackend`] without needing backend‑
///   specific code.
pub struct DictionarySpellChecker<B: DictionaryBackend> {
    backend: Arc<B>,
}

impl<B: DictionaryBackend> DictionarySpellChecker<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B: DictionaryBackend> SpellChecker for DictionarySpellChecker<B> {
    fn is_correct(&self, word: &str) -> bool {
        self.backend.contains(word)
    }

    fn suggest(&self, word: &str, context: &[&str]) -> Vec<String> {
        crate::spellcheck::suggest::suggest_edits(&*self.backend, None, word, context, 10, None)
    }

    fn suggest_with(&self, input: &SuggestionInput<'_>, max: usize) -> Vec<String> {
        let source = input.spatial.edit_source();
        let source_ref: Option<&dyn EditSource> = if input.spatial.is_none() {
            None
        } else {
            Some(&source)
        };
        let mut suggestions = crate::spellcheck::suggest::suggest_edits(
            &*self.backend,
            None,
            input.word,
            input.context,
            max,
            source_ref,
        );
        if !input.spatial.is_none() {
            suggestions.sort_by(|a, b| {
                let distance_a = input.spatial.word_distance(input.word, a).unwrap_or(0.0);
                let distance_b = input.spatial.word_distance(input.word, b).unwrap_or(0.0);
                distance_a.total_cmp(&distance_b).then_with(|| a.cmp(b))
            });
        }
        suggestions
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
