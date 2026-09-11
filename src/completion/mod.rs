//! Current-word completion engines.
//!
//! [`CompletionEngine`] is the shared trait; [`prefix::PrefixCompleter`] is the
//! transplanted prefix + single-edit ranking algorithm. Scores are relative
//! heuristics, not probabilities.

pub mod android;
pub mod prefix;
pub use android::AndroidCompleter;

use std::time::Duration;

pub use prefix::{PrefixCompleter, complete};

/// Current-word input and committed context have independent preparation.
#[derive(Debug, Clone, Default)]
pub struct CompletionInput<'a> {
    pub input: &'a str,
    pub input_prep: crate::text::TextPrep,
    pub context: &'a [&'a str],
    pub context_prep: crate::text::TextPrep,
    pub case_preference: crate::text::CasePreference,
}

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
    /// Transport-level cap on accepted `Complete` `max` values. Oversized
    /// requests are rejected, never silently clamped.
    pub max_complete_results: usize,
    /// Transport-level cap on accepted bounded-query `max` values.
    pub max_query_results: usize,
    /// Maximum intermediate candidates. Exceeding this budget returns an
    /// explicit error; it never silently discards low-frequency candidates.
    pub max_search_candidates: usize,
    /// Suppress corrections for known words and fragments shorter than this.
    pub suppress_known_corrections: bool,
    pub min_correction_chars: usize,
    /// Deadline for a single completion request.
    pub response_deadline: Duration,
    /// Time budget for dictionary-side candidate search.
    pub search_budget: Duration,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            max_complete_results: 1_000,
            max_query_results: 200_000,
            max_search_candidates: 200_000,
            suppress_known_corrections: true,
            min_correction_chars: 3,
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

    /// Contextual engines report search-budget failures through this API.
    /// Legacy engines keep their original context-free behavior.
    fn complete_with(
        &self,
        input: &CompletionInput<'_>,
        max: usize,
    ) -> Result<Vec<CompletionCandidate>, String> {
        Ok(self.complete(Some(input.input), max))
    }
}

// Kept as a public re-export for existing library users.
pub use crate::prediction::scoring::interpolate_score;
