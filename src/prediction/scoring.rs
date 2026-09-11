//! Shared language ranking.
//!
//! The single scoring primitive is [`NgramBackend::probability`]; count stores
//! derive it from their counts and probability stores override it. All
//! interpolation/weighting policy lives here so backends stay data-only.
use super::ngram_backend::NgramBackend;

pub fn interpolate_score(
    backend: &dyn NgramBackend,
    context: &[&str],
    candidate: &str,
    max_order: usize,
) -> f64 {
    interpolate_with_weights(backend, context, candidate, max_order, &[])
}

/// Uniform interpolation unless a caller explicitly supplies nonnegative
/// per-order weights. Unknown contexts back off; smoothing must not turn an
/// absent denominator into a probability of one.
pub fn interpolate_with_weights(
    backend: &dyn NgramBackend,
    context: &[&str],
    candidate: &str,
    max_order: usize,
    weights: &[f64],
) -> f64 {
    let context =
        if context.contains(&crate::text::BOS) && !backend.supports_sentence_start() {
            crate::text::after_boundary(context)
        } else {
            context
        };
    let max_order = max_order.min(backend.max_order());

    let mut sum = 0.0;
    let mut mass = 0.0;
    for n in 1..=max_order.min(context.len() + 1) {
        let tail = &context[context.len() + 1 - n..];
        let weight = if weights.is_empty() {
            1.0
        } else {
            *weights.get(n - 1).unwrap_or(&0.0)
        };
        if !weight.is_finite() || weight <= 0.0 {
            continue;
        }
        let mut ngram: Vec<&str> = tail.to_vec();
        ngram.push(candidate);
        if let Some(probability) = backend.probability(&ngram)
            && probability.is_finite()
            && (0.0..=1.0).contains(&probability)
        {
            sum += weight * probability;
            mass += weight;
        }
    }

    if mass > 0.0 {
        (sum / mass).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Apply the same interpolation policy to already-quantized probabilities.
/// Missing higher orders back off; there are no invented counts or totals.
pub fn interpolate_probabilities(orders: &[Option<f64>]) -> Option<f64> {
    let (sum, count) = orders
        .iter()
        .filter_map(|p| *p)
        .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
        .fold((0.0, 0usize), |(sum, count), p| (sum + p, count + 1));
    (count != 0).then(|| sum / count as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Counts;
    impl NgramBackend for Counts {
        fn max_order(&self) -> usize {
            3
        }
        fn unigram_total(&self) -> u64 {
            1000
        }
        fn ngram_count(&self, n: &[&str]) -> u64 {
            match n {
                ["you"] => 100,
                ["later"] => 10,
                ["you", "later"] => 80,
                _ => 0,
            }
        }
        fn candidates(&self, _: &[&str], _: usize) -> Vec<(String, u64)> {
            vec![]
        }
    }
    #[test]
    fn shares_uniform_weighting_and_backs_off_missing_context() {
        assert_eq!(interpolate_score(&Counts, &["you"], "later", 3), 0.405);
        assert_eq!(interpolate_score(&Counts, &["unknown"], "later", 3), 0.01);
        assert_eq!(
            interpolate_score(&Counts, &["see", "you"], "later", 3),
            0.405
        );
        assert_eq!(
            interpolate_probabilities(&[Some(0.01), Some(0.8), None]),
            Some(0.405)
        );
        assert_eq!(
            interpolate_score(&Counts, &[crate::text::BOS, "you"], "later", 3),
            0.405
        );
    }
}
