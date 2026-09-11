use crate::dictionary::DictionaryBackend;
use crate::prediction::ngram_backend::NgramBackend;

/// Generate spelling suggestions using single-edit-distance candidates.
///
/// Produces candidates by:
/// 1. Adjacent character swap (transposition)
/// 2. Deletion of one character
/// 3. Insertion of one character (a-z)
/// 4. Substitution of one character (a-z)
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
) -> Vec<String> {
    let word_lower = word.to_lowercase();

    if backend.contains(&word_lower) {
        return vec![word_lower];
    }

    let mut candidates = Vec::new();
    let mut seen = std::collections::HashSet::new();
    super::edits::visit_edits(
        &word_lower,
        &super::edits::LatinAlphabet,
        |word, _weight| {
            if word != word_lower && backend.contains(&word) && seen.insert(word.clone()) {
                candidates.push(word);
            }
            true
        },
    );

    // Score candidates using best available strategy
    let use_context = ngram_backend.is_some() && !context.is_empty();

    if use_context {
        if let Some(nb) = ngram_backend {
            score_with_interpolation(&mut candidates, nb, context);
        }
    } else {
        // Fallback: rank by dictionary frequency
        candidates.sort_by(|a, b| {
            let a_freq = backend.get_frequency(a);
            let b_freq = backend.get_frequency(b);
            b_freq.total_cmp(&a_freq).then_with(|| a.cmp(b))
        });
    }

    candidates.truncate(max);
    candidates
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
