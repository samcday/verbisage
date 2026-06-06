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

    let chars: Vec<char> = word_lower.chars().collect();
    let len = chars.len();
    let mut candidates: Vec<String> = Vec::new();

    // 1. Adjacent swap
    for i in 0..len.saturating_sub(1) {
        let mut c = chars.clone();
        c.swap(i, i + 1);
        let w: String = c.into_iter().collect();
        if w != word_lower && backend.contains(&w) {
            candidates.push(w);
        }
    }

    // 2. Delete one char
    for i in 0..len {
        let mut c = chars.clone();
        c.remove(i);
        let w: String = c.into_iter().collect();
        if backend.contains(&w) {
            candidates.push(w);
        }
    }

    // 3. Insert one char (a-z)
    for i in 0..=len {
        for ch in 'a'..='z' {
            let mut c = chars.clone();
            c.insert(i, ch);
            let w: String = c.into_iter().collect();
            if backend.contains(&w) {
                candidates.push(w);
            }
        }
    }

    // 4. Substitute one char (a-z)
    for i in 0..len {
        for ch in 'a'..='z' {
            if ch == chars[i] {
                continue;
            }
            let mut c = chars.clone();
            c[i] = ch;
            let w: String = c.into_iter().collect();
            if backend.contains(&w) {
                candidates.push(w);
            }
        }
    }

    // Deduplicate preserving order
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|w| seen.insert(w.clone()));

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
            b_freq
                .partial_cmp(&a_freq)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cmp(b))
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
        b_score
            .partial_cmp(&a_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cmp(b))
    });
}

/// Compute the smoothed interpolated probability for `candidate` given
/// `context`, using uniform deltas (equal weight per order).  Applies
/// count=1 smoothing for unseen n-grams.
///
/// Mirrors the formula in [`SmoothedPredictor`]:
/// ```text
/// P(w | context) = Σ_k delta_k * freq_k
///
/// freq_0 = count(w) / unigram_total
/// freq_k = count(context[-(k-1)..], w) / count(context[-(k-1)..])
/// ```
fn interpolate_score(
    backend: &dyn NgramBackend,
    context: &[&str],
    candidate: &str,
    max_order: usize,
) -> f64 {
    let effective_order = max_order.min(context.len() + 1);
    if effective_order == 0 {
        return 0.0;
    }

    // Uniform deltas: equal weight per order
    let delta = 1.0 / effective_order as f64;
    let mut prob = 0.0;

    for k in 0..effective_order {
        let order = k + 1;
        let ctx_start = context.len().saturating_sub(order - 1);
        let ctx_slice = &context[ctx_start..];

        let mut ngram: Vec<&str> = ctx_slice.to_vec();
        ngram.push(candidate);

        // count=1 smoothing for the numerator
        let numerator = backend.ngram_count(&ngram).max(1);

        let denominator = if k == 0 {
            backend.unigram_total().max(1)
        } else if ctx_slice.is_empty() {
            1
        } else {
            backend.ngram_count(ctx_slice).max(1)
        };

        let freq = numerator as f64 / denominator as f64;
        prob += delta * freq;
    }

    prob
}
