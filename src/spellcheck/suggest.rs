use crate::dictionary::DictionaryBackend;
use crate::prediction::ngram_backend::NgramBackend;
use crate::spellcheck::edits::EditSource;

/// Generate spelling suggestions using bounded edit-distance candidates.
///
/// Produces candidates by:
/// 1. Adjacent character swap (transposition)
/// 2. Deletion of one character
/// 3. Insertion of one character (a-z)
/// 4. Substitution of one character (a-z)
///
/// Up to [`MAX_EDIT_DEPTH`] edits are explored, pruned by accumulated spatial
/// cost (see [`edit_candidates`]).
///
/// Scoring strategy (in order of preference):
/// - If `ngram_backend` is provided AND `context` is non-empty: score
///   candidates using the same linear-interpolation smoothed n-gram
///   probability as [`SmoothedPredictor`](crate::prediction::smoothed::SmoothedPredictor),
///   with a count=1 fallback for unseen n-grams.
/// - Otherwise: rank by `DictionaryBackend::get_frequency`.
/// - As a last resort: lexicographic order.
///
/// Returns at most `max` suggestions, sorted by descending score.
pub fn suggest_edits(
    backend: &dyn DictionaryBackend,
    ngram_backend: Option<&dyn NgramBackend>,
    word: &str,
    context: &[&str],
    max: usize,
    source: Option<&dyn EditSource>,
) -> Vec<String> {
    let word_lower = word.to_lowercase();

    if backend.contains(&word_lower) {
        return vec![word_lower];
    }

    let mut candidates: Vec<String> = edit_candidates(backend, &word_lower, source)
        .into_iter()
        .map(|(word, _weight)| word)
        .collect();

    // Score candidates using best available strategy
    let use_context = ngram_backend.is_some() && !context.is_empty();

    if use_context {
        if let Some(nb) = ngram_backend {
            score_with_interpolation(&mut candidates, nb, context);
        }
    } else {
        // Fallback: rank by dictionary frequency
        sort_by_frequency(backend, &mut candidates);
    }

    candidates.truncate(max);
    candidates
}

/// Maximum number of edits (Damerau–Levenshtein) explored around the input.
///
/// Two covers the common double-typo (a transposition plus a substitution, or
/// two near-key substitutions) while keeping the search bounded.
const MAX_EDIT_DEPTH: usize = 2;
/// Hard cap on generated strings per call, bounding backend membership lookups
/// (notably SQLite point queries).
const MAX_EDIT_CANDIDATES: usize = 20_000;
/// Cheapest intermediate strings carried into the next edit level. Ranking them
/// by cost makes geometry drive which second edits are explored (near keys
/// first) instead of relying on enumeration order.
const MAX_FRONTIER: usize = 64;

/// Bounded edit-distance candidates present in the dictionary, with the lowest
/// spatial edit cost seen for each (lower is better). Unranked; callers handle
/// the "input is already correct" case and choose a ranking policy.
///
/// This is HeliBoard's bounded error-correction traversal adapted to a
/// string-enumeration generator: each edit accumulates a spatial cost, and a
/// node is only expanded while its accumulated cost per input character stays
/// under [`NORMALIZED_SPATIAL_DISTANCE_THRESHOLD_FOR_EDIT`]. The first edit is
/// always attempted because the root starts at zero cost, so distant single
/// substitutions are still found; expensive nodes are simply not expanded into
/// second edits, and only the cheapest `MAX_FRONTIER` intermediates are kept.
pub fn edit_candidates(
    backend: &dyn DictionaryBackend,
    word_lower: &str,
    source: Option<&dyn EditSource>,
) -> Vec<(String, f64)> {
    use crate::spatial::cost::NORMALIZED_SPATIAL_DISTANCE_THRESHOLD_FOR_EDIT;

    let latin = super::edits::LatinAlphabet;
    let source: &dyn EditSource = source.unwrap_or(&latin);

    let input_len = word_lower.chars().count();
    let max_cost = NORMALIZED_SPATIAL_DISTANCE_THRESHOLD_FOR_EDIT * (input_len as f64 + 1.0);

    let mut candidates: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut frontier: Vec<(String, f64)> = vec![(word_lower.to_string(), 0.0)];
    let mut budget = MAX_EDIT_CANDIDATES;

    for _ in 0..MAX_EDIT_DEPTH {
        if frontier.is_empty() || budget == 0 {
            break;
        }
        let mut next: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for (word, cost) in &frontier {
            if *cost >= max_cost {
                continue;
            }
            super::edits::visit_edits_cost(word, source, |candidate, edit_cost| {
                if budget == 0 {
                    return false;
                }
                budget -= 1;
                if candidate == word_lower {
                    return true;
                }
                let total = cost + edit_cost;
                if backend.contains(&candidate) {
                    candidates
                        .entry(candidate.clone())
                        .and_modify(|current| *current = current.min(total))
                        .or_insert(total);
                }
                if total < max_cost {
                    next.entry(candidate)
                        .and_modify(|current| *current = current.min(total))
                        .or_insert(total);
                }
                true
            });
        }
        let mut next: Vec<(String, f64)> = next.into_iter().collect();
        next.sort_by(|(_, a), (_, b)| a.total_cmp(b));
        next.truncate(MAX_FRONTIER);
        frontier = next;
    }
    candidates.into_iter().collect()
}

/// Rank existing candidates by descending dictionary frequency, lexical tie-break.
pub fn sort_by_frequency(backend: &dyn DictionaryBackend, candidates: &mut [String]) {
    candidates.sort_by(|a, b| {
        let a_freq = backend.get_frequency(a);
        let b_freq = backend.get_frequency(b);
        b_freq.total_cmp(&a_freq).then_with(|| a.cmp(b))
    });
}

/// Score candidates using the same linear-interpolation smoothed n-gram
/// probability as [`SmoothedPredictor`](crate::prediction::smoothed::SmoothedPredictor).
///
/// The key difference: we use count=1 smoothing for unseen n-grams rather
/// than dropping candidates, because edit-distance candidates may be words
/// that exist in the dictionary but haven't appeared in the n-gram data.
fn score_with_interpolation(
    candidates: &mut [String],
    ngram_backend: &dyn NgramBackend,
    context: &[&str],
) {
    let max_order = ngram_backend.max_order();

    candidates.sort_by(|a, b| {
        let a_score = interpolate_score(ngram_backend, context, a, max_order);
        let b_score = interpolate_score(ngram_backend, context, b, max_order);
        b_score.total_cmp(&a_score).then_with(|| a.cmp(b))
    });
}

use crate::prediction::scoring::interpolate_score;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::FileDictionaryBackend;
    use crate::spatial::cost::cost_to_quality;

    /// Substitutions cost 0.1 each, cheap enough for two to fit the budget.
    struct Cheap;
    impl EditSource for Cheap {
        fn letters(&self) -> &[char] {
            &['a', 'b']
        }
        fn substitutions(&self, ch: char, _index: usize) -> Vec<(char, f64)> {
            self.letters()
                .iter()
                .copied()
                .filter(|candidate| *candidate != ch)
                .map(|candidate| (candidate, cost_to_quality(0.1)))
                .collect()
        }
    }

    #[test]
    fn finds_two_edit_correction_when_cost_fits() {
        let mut dict = FileDictionaryBackend::new();
        dict.add_word_mut("bbaaaa".to_string(), 1.0);
        let source = Cheap;
        let candidates = edit_candidates(&dict, "aaaaaa", Some(&source));
        assert!(
            candidates.iter().any(|(word, _)| word == "bbaaaa"),
            "{candidates:?}"
        );
    }

    #[test]
    fn prunes_second_edit_when_first_edit_exceeds_budget() {
        let mut dict = FileDictionaryBackend::new();
        dict.add_word_mut("bbaaaa".to_string(), 1.0);
        let source = Expensive;
        let candidates = edit_candidates(&dict, "aaaaaa", Some(&source));
        assert!(
            !candidates.iter().any(|(word, _)| word == "bbaaaa"),
            "a first edit above the budget must not seed a second: {candidates:?}"
        );
    }

    /// Substitutions cost 0.8 each, above the budget for a six-character input.
    struct Expensive;
    impl EditSource for Expensive {
        fn letters(&self) -> &[char] {
            &['a', 'b']
        }
        fn substitutions(&self, ch: char, _index: usize) -> Vec<(char, f64)> {
            self.letters()
                .iter()
                .copied()
                .filter(|candidate| *candidate != ch)
                .map(|candidate| (candidate, cost_to_quality(0.8)))
                .collect()
        }
    }
}
