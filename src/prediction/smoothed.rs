use crate::prediction::ngram_backend::NgramBackend;
use crate::prediction::{Prediction, Predictor};

/// Smoothed n‑gram predictor using linear interpolation.
///
/// For a candidate next‑word `w_i` given context tokens, the smoothed
/// probability is:
///
/// ```text
/// P(w_i | context) = Σ_{k=0}^{cardinality-1} delta_k * frequency_k
///
/// frequency_0 = count(w_i) / unigram_counts_sum
/// frequency_k = count(context[-(k-1)..], w_i) / count(context[-(k-1)..])   for k > 0
/// ```
///
/// Two-phase algorithm:
/// 1. **Candidate gathering** — backoff from highest order to unigram,
///    collecting candidate next‑words via prefix search.  Stop when we
///    have enough candidates.
/// 2. **Scoring** — for each candidate, compute the full smoothed
///    probability across all orders.
pub struct SmoothedPredictor {
    backend: std::sync::Arc<dyn NgramBackend>,
    deltas: Vec<f64>,
    count_threshold: u64,
    candidate_limit: usize,
}

impl SmoothedPredictor {
    pub fn new(backend: std::sync::Arc<dyn NgramBackend>) -> Self {
        Self {
            backend,
            deltas: Vec::new(),
            count_threshold: 1,
            candidate_limit: 100,
        }
    }

    /// Set interpolation weights.  Must match the backend's max order.
    pub fn with_deltas(mut self, deltas: Vec<f64>) -> Self {
        self.deltas = deltas;
        self
    }

    /// Minimum count for a candidate to be considered.
    pub fn with_count_threshold(mut self, threshold: u64) -> Self {
        self.count_threshold = threshold;
        self
    }

    /// Maximum number of candidates to gather before scoring.
    pub fn with_candidate_limit(mut self, limit: usize) -> Self {
        self.candidate_limit = limit;
        self
    }

    /// Exact count for a given n-gram.
    pub fn ngram_count(&self, ngram: &[&str]) -> u64 {
        self.backend.ngram_count(ngram)
    }

    /// Phase 1: gather candidates by backing off from highest order.
    fn gather_candidates(&self, context: &[&str], max_suggestions: usize) -> Vec<String> {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let max_order = self.backend.max_order();
        let target = max_suggestions.saturating_mul(3).max(20);

        for order in (1..=max_order.min(context.len() + 1)).rev() {
            if seen.len() >= target {
                break;
            }
            let ctx_slice = if order == 1 {
                &[]
            } else {
                &context[context.len().saturating_sub(order - 1)..]
            };
            let cands = self.backend.candidates(ctx_slice, target - seen.len());
            for (word, count) in cands {
                if count >= self.count_threshold && seen.insert(word.clone()) {
                    seen.insert(word);
                }
            }
        }

        // Fallback: if we got nothing from higher orders, try unigram
        if seen.is_empty() {
            let cands = self.backend.candidates(&[], target);
            for (word, count) in cands {
                if count >= self.count_threshold {
                    seen.insert(word);
                }
            }
        }

        seen.into_iter().collect()
    }

    /// Phase 2: score each candidate with full interpolation.
    fn score_candidate(&self, context: &[&str], candidate: &str) -> f64 {
        super::scoring::interpolate_with_weights(
            self.backend.as_ref(),
            context,
            candidate,
            self.backend.max_order(),
            &self.deltas,
        )
    }
}

impl Predictor for SmoothedPredictor {
    fn score_candidates(
        &self,
        context: &[&str],
        candidates: &[(&str, &str)],
        deadline: std::time::Instant,
    ) -> Result<Vec<Option<f64>>, String> {
        let scorer = super::scoring::CountScorer::new(
            self.backend.as_ref(),
            context,
            self.backend.max_order(),
            &self.deltas,
        );
        let mut scores = Vec::with_capacity(candidates.len());
        for (_, candidate) in candidates {
            crate::dictionary::search::check_deadline(deadline)?;
            scores.push(Some(scorer.score(candidate)));
        }
        crate::dictionary::search::check_deadline(deadline)?;
        Ok(scores)
    }

    fn predict_next(&self, context: &[&str], max_suggestions: usize) -> Vec<Prediction> {
        if max_suggestions == 0 {
            return Vec::new();
        }

        let candidates = self.gather_candidates(context, max_suggestions);

        let mut scored: Vec<(String, f64)> = candidates
            .into_iter()
            .map(|word| {
                let prob = self.score_candidate(context, &word);
                (word, prob)
            })
            .filter(|(_, prob)| *prob > 0.0)
            .collect();

        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        scored
            .into_iter()
            .take(max_suggestions)
            .map(|(word, confidence)| Prediction { word, confidence })
            .collect()
    }

    fn candidate_score(&self, context: &[&str], candidate: &str) -> Option<f64> {
        Some(self.score_candidate(context, candidate))
    }

    fn increase_ngram_frequency(
        &self,
        ngram: &[&str],
        delta: f64,
        save_unknown: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.backend
            .increase_ngram_frequency(ngram, delta, save_unknown)
    }

    fn ngram_backend(&self) -> Option<std::sync::Arc<dyn NgramBackend>> {
        Some(self.backend.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::SharedSqliteConnection;
    use crate::dictionary::PresageSqliteBackend;
    use rusqlite::Connection;

    fn make_backend() -> Box<dyn NgramBackend> {
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE _1_gram (word TEXT PRIMARY KEY, count INTEGER DEFAULT 1);
             CREATE TABLE _2_gram (word_1 TEXT, word TEXT, count INTEGER DEFAULT 1, UNIQUE(word_1, word));
             CREATE TABLE _3_gram (word_2 TEXT, word_1 TEXT, word TEXT, count INTEGER DEFAULT 1, UNIQUE(word_2, word_1, word));
             INSERT OR REPLACE INTO _1_gram VALUES ('hello', 1000);
             INSERT OR REPLACE INTO _1_gram VALUES ('goodbye', 800);
             INSERT OR REPLACE INTO _1_gram VALUES ('thanks', 600);
             INSERT OR REPLACE INTO _1_gram VALUES ('hi', 500);
             INSERT OR REPLACE INTO _1_gram VALUES ('hey', 300);
             INSERT OR REPLACE INTO _1_gram VALUES ('morning', 200);
             INSERT OR REPLACE INTO _2_gram VALUES ('hi', 'hello', 500);
             INSERT OR REPLACE INTO _2_gram VALUES ('hi', 'hey', 300);
             INSERT OR REPLACE INTO _2_gram VALUES ('morning', 'hello', 200);
             INSERT OR REPLACE INTO _3_gram VALUES ('hi', 'morning', 'hello', 100);
             INSERT OR REPLACE INTO _3_gram VALUES ('hi', 'morning', 'goodbye', 50);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        Box::new(PresageSqliteBackend::from_shared(shared, false, false))
    }

    #[test]
    fn smooth_unigram_context() {
        let backend = make_backend();
        let predictor = SmoothedPredictor::new(backend.into());
        let results = predictor.predict_next(&[], 2);
        assert_eq!(results.len(), 2);
        assert!(results[0].confidence > 0.0);
        assert!(results[0].confidence <= 1.0);
        assert!(results[1].confidence <= 1.0);
    }

    #[test]
    fn smooth_bigram_context() {
        let backend = make_backend();
        let predictor = SmoothedPredictor::new(backend.into());
        let results = predictor.predict_next(&["hi"], 3);
        assert!(!results.is_empty());
        assert!(
            results
                .iter()
                .all(|r| r.confidence > 0.0 && r.confidence <= 1.0)
        );
    }

    #[test]
    fn smooth_trigram_context() {
        let backend = make_backend();
        let predictor = SmoothedPredictor::new(backend.into());
        let results = predictor.predict_next(&["hi", "morning"], 3);
        assert!(!results.is_empty());
        assert!(
            results
                .iter()
                .all(|r| r.confidence > 0.0 && r.confidence <= 1.0)
        );
    }

    #[test]
    fn zero_max_suggestions() {
        let backend = make_backend();
        let predictor = SmoothedPredictor::new(backend.into());
        let results = predictor.predict_next(&["hi"], 0);
        assert!(results.is_empty());
    }
}
