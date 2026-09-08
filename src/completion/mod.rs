//! Current-word completion engines.
//!
//! [`CompletionEngine`] is the shared trait; [`prefix::PrefixCompleter`] is the
//! transplanted prefix + single-edit ranking algorithm. Scores are relative
//! heuristics, not probabilities.

pub mod prefix;

use std::time::Duration;

pub use prefix::{PrefixCompleter, complete};

/// A single completion candidate returned to the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionCandidate {
    pub word: String,
    /// Score in 0.0–1.0, higher is more likely. Relative heuristic, not a
    /// calibrated probability.
    pub score: f64,
    /// Whether the candidate is an intact prefix continuation of the input
    /// (as opposed to an edit-corrected candidate).
    pub is_exact: bool,
}

/// Configuration for completion engines.
///
/// Exposes the stable shape up front (cap knob, timeouts). The transplanted
/// [`prefix::PrefixCompleter`] only passes `max` through; knob enforcement and
/// timeout plumbing land with the transport layer and `android.rs`.
#[derive(Debug, Clone)]
pub struct CompletionConfig {
    /// Transport-level cap on accepted `max` values. Oversized requests are
    /// rejected, never silently clamped.
    pub max_results: usize,
    /// Deadline for a single completion request.
    pub response_deadline: Duration,
    /// Time budget for dictionary-side candidate search.
    pub search_budget: Duration,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            max_results: 200_000,
            response_deadline: Duration::from_secs(5),
            search_budget: Duration::from_secs(5),
        }
    }
}

/// Trait for completion engines. All implementors must be [`Send`] + [`Sync`].
pub trait CompletionEngine: Send + Sync {
    /// Return completion candidates for `prefix`.
    ///
    /// `prefix` is `None` when the caller provides no pretext. `max` is
    /// honored exactly; output caps are enforced by the caller/transport.
    fn complete(&self, prefix: Option<&str>, max: usize) -> Vec<CompletionCandidate>;
}

/// Smoothed interpolated n-gram probability for `candidate` given `context`,
/// using uniform per-order weights and count=1 smoothing for unseen n-grams.
///
/// Mirrors [`crate::spellcheck::suggest::interpolate_score`]; shared here so
/// future engines (`android.rs`) reuse it instead of duplicating it.
///
/// Currently unused by the transplanted prefix engine.
#[allow(dead_code)]
pub fn interpolate_score(
    backend: &dyn crate::prediction::ngram_backend::NgramBackend,
    context: &[&str],
    candidate: &str,
    max_order: usize,
) -> f64 {
    let effective_order = max_order.min(context.len() + 1);
    if effective_order == 0 {
        return 0.0;
    }

    let delta = 1.0 / effective_order as f64;
    let mut prob = 0.0;

    for k in 0..effective_order {
        let order = k + 1;
        let ctx_start = context.len().saturating_sub(order - 1);
        let ctx_slice = &context[ctx_start..];

        let mut ngram: Vec<&str> = ctx_slice.to_vec();
        ngram.push(candidate);

        let numerator = backend.ngram_count(&ngram).max(1);

        let denominator = if k == 0 {
            backend.unigram_total().max(1)
        } else if ctx_slice.is_empty() {
            1
        } else {
            backend.ngram_count(ctx_slice).max(1)
        };

        prob += delta * numerator as f64 / denominator as f64;
    }

    prob
}
