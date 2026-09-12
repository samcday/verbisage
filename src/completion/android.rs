//! Context-aware current-word and next-word ranking with explicit search budgets.
//! This adapts Android's language/spatial/case factors. It is not Android's
//! touch decoder: single edits stand in for spatial cost until layouts arrive.
use super::{CompletionCandidate, CompletionConfig, CompletionEngine, CompletionInput};
use crate::dictionary::search::{WordSearch, check_deadline};
use crate::dictionary::{DictionaryBackend, usable_frequency};
use crate::prediction::Predictor;
use crate::spatial::cost::{
    COST_FIRST_COMPLETION, DISTANCE_WEIGHT_LANGUAGE, DISTANCE_WEIGHT_LENGTH, combined_score,
    cost_to_quality, quality_to_cost,
};
use crate::spatial::{NORMALIZED_SPATIAL_DISTANCE_THRESHOLD_FOR_EDIT, SpatialInput};
use crate::spellcheck::edits::visit_edits;
use crate::text::{CaseFold, CasePreference, LangDb, prepare_context};
use std::collections::HashMap;
use std::time::Instant;

pub struct AndroidCompleter<'a> {
    backend: &'a dyn DictionaryBackend,
    predictor: Option<&'a dyn Predictor>,
    languages: LangDb,
    lang: Option<&'a str>,
    config: CompletionConfig,
}
impl<'a> AndroidCompleter<'a> {
    pub fn new(backend: &'a dyn DictionaryBackend) -> Self {
        Self {
            backend,
            predictor: None,
            languages: LangDb::default(),
            lang: None,
            config: CompletionConfig::default(),
        }
    }
    pub fn with_predictor(mut self, predictor: Option<&'a dyn Predictor>) -> Self {
        self.predictor = predictor;
        self
    }
    pub fn with_language(mut self, lang: &'a str) -> Self {
        self.lang = Some(lang);
        self
    }
    pub fn with_languages(mut self, languages: LangDb) -> Self {
        self.languages = languages;
        self
    }
    pub fn with_config(mut self, config: CompletionConfig) -> Self {
        self.config = config;
        self
    }
}
impl CompletionEngine for AndroidCompleter<'_> {
    /// Compatibility convenience. Use complete_with to distinguish an empty
    /// dictionary from a budget error.
    fn complete(&self, prefix: Option<&str>, max: usize) -> Vec<CompletionCandidate> {
        self.complete_with(
            &CompletionInput {
                input: prefix.unwrap_or(""),
                ..Default::default()
            },
            max,
        )
        .unwrap_or_default()
    }
    fn complete_with(
        &self,
        input: &CompletionInput<'_>,
        max: usize,
    ) -> Result<Vec<CompletionCandidate>, String> {
        if max == 0 {
            return Ok(Vec::new());
        }
        let now = Instant::now();
        let deadline = now + self.config.response_deadline;
        let search_deadline = deadline.min(now + self.config.search_budget);
        check_deadline(search_deadline)?;
        let folded = input
            .input_prep
            .apply(input.input, &self.languages, self.lang);
        let context = prepare_context(
            input.context,
            input.context_prep,
            &self.languages,
            self.lang,
        );
        let context: Vec<_> = context.iter().map(String::as_str).collect();
        let source = input.spatial.edit_source();
        let mut edits = HashMap::<String, f64>::new();
        if folded.chars().count() >= self.config.min_correction_chars {
            visit_edits(&folded, &source, |word, weight| {
                edits
                    .entry(word)
                    .and_modify(|v| *v = v.max(weight))
                    .or_insert(weight);
                Instant::now() < search_deadline
            });
        }
        check_deadline(search_deadline)?;
        let search = WordSearch::new(
            folded.clone(),
            edits,
            input.input_prep,
            &self.languages,
            self.lang,
            self.config.max_search_candidates,
        );
        let rows = self.backend.search_words(&search, search_deadline)?;
        check_deadline(search_deadline)?;
        let known = !folded.is_empty()
            && self.config.suppress_known_corrections
            && rows.iter().any(|r| search.prepare(&r.word) == folded);
        let mut candidates = Vec::with_capacity(rows.len());
        for row in rows {
            check_deadline(deadline)?;
            let prepared = if folded.is_empty() {
                String::new()
            } else {
                search.prepare(&row.word)
            };
            let exact = prepared.starts_with(&folded);
            if !exact && known {
                continue;
            }
            let weight = if exact {
                1.0
            } else {
                *search.edits.get(&prepared).unwrap_or(&0.0)
            };
            candidates.push((row, prepared, exact, weight));
        }
        // Context models own the spelling of their keys. Candidate preparation
        // is independent from input matching; stored spelling is still returned.
        let keys: Vec<_> = candidates
            .iter()
            .map(|(r, _, _, _)| {
                input
                    .context_prep
                    .apply(&r.word, &self.languages, self.lang)
            })
            .collect();
        let key_refs: Vec<_> = candidates
            .iter()
            .zip(&keys)
            .map(|((row, _, _, _), key)| (row.word.as_str(), key.as_str()))
            .collect();
        let scores = match self.predictor {
            Some(p) => p.score_candidates(&context, &key_refs, deadline)?,
            None => vec![None; candidates.len()],
        };
        let spatial_active = !input.spatial.is_none();
        let input_len = folded.chars().count();
        // HeliBoard gates error corrections on how accurately the user touched
        // the intended keys. Only meaningful when touch points are present.
        let corrections_allowed = match &input.spatial {
            SpatialInput::Touch { .. } => input
                .spatial
                .input_distance(&folded)
                .map(|distance| distance < NORMALIZED_SPATIAL_DISTANCE_THRESHOLD_FOR_EDIT)
                .unwrap_or(false),
            _ => true,
        };
        let mut results = Vec::with_capacity(candidates.len());
        for ((row, prepared, exact, weight), model) in candidates.into_iter().zip(scores) {
            check_deadline(deadline)?;
            let probability = model
                .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
                .unwrap_or_else(|| usable_frequency(row.confidence).max(0.0));
            let case_bonus = input.input_prep.fold != CaseFold::None
                && input.case_preference == CasePreference::PreferMatched
                && input.input.chars().any(|c| c.is_uppercase())
                && row.word.starts_with(input.input);
            let mut promotion = 1.0;
            if !folded.is_empty() && prepared == folded {
                promotion *= 1.1;
            }
            if !input.input.is_empty() && row.word == input.input {
                promotion *= 1.1;
            }

            let completion_cost = if exact { COST_FIRST_COMPLETION } else { 0.0 };
            let score = if spatial_active {
                // HeliBoard's additive model over distances. Corrections are
                // suppressed when the touch accuracy gate is not met.
                if !exact && !corrections_allowed {
                    continue;
                }
                let spatial_cost = input
                    .spatial
                    .word_distance(&folded, &prepared)
                    .unwrap_or(0.0)
                    * DISTANCE_WEIGHT_LENGTH
                    + completion_cost;
                let base = combined_score(spatial_cost, 1.0 - probability, input_len);
                let base = if case_bonus { base + 0.01 } else { base };
                (base * promotion).clamp(0.0, 1.0)
            } else {
                // Geometry-free/legacy path: fold the completion cost into the
                // edit weight and keep the length-independent blend, so a
                // prediction and the matching prefix completion stay equal.
                // An additive-only adaptation regressed helo -> hello.
                let weight = cost_to_quality(quality_to_cost(weight) + completion_cost);
                let mut quality =
                    ((DISTANCE_WEIGHT_LANGUAGE * probability + DISTANCE_WEIGHT_LENGTH) * weight)
                        / (DISTANCE_WEIGHT_LANGUAGE + DISTANCE_WEIGHT_LENGTH);
                if case_bonus {
                    quality += 0.01;
                }
                (quality * promotion / (1.01 * 1.21)).clamp(0.0, 1.0)
            };
            results.push(CompletionCandidate {
                word: row.word,
                score,
                is_exact: exact,
            });
        }
        results.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.word.cmp(&b.word))
        });
        results.dedup_by(|a, b| a.word == b.word);
        results.truncate(max);
        check_deadline(deadline)?;
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::FileDictionaryBackend;
    use crate::spatial::{SpatialInput, TouchPoint};
    use keyboard_layout::{RectKey, RectKeyLayout};
    use std::sync::Arc;

    fn layout() -> Arc<RectKeyLayout> {
        Arc::new(RectKeyLayout::new(
            vec![
                RectKey::from_rect(Some("c".into()), vec![], 0.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("a".into()), vec![], 10.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("z".into()), vec![], 20.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("t".into()), vec![], 20.0, 10.0, 10.0, 10.0),
                RectKey::from_rect(Some("r".into()), vec![], 10.0, 10.0, 10.0, 10.0),
            ],
            &[],
        ))
    }

    #[test]
    fn touch_proximity_prefers_the_nearer_candidate() {
        let mut dict = FileDictionaryBackend::new();
        dict.add_word_mut("cat".into(), 10.0);
        dict.add_word_mut("car".into(), 10.0);

        // Touches at the centres of the typed c, a, z keys.
        let points = vec![
            TouchPoint::new(5.0, 5.0),
            TouchPoint::new(15.0, 5.0),
            TouchPoint::new(25.0, 5.0),
        ];
        let input = CompletionInput {
            input: "caz",
            spatial: SpatialInput::from_parts(Some(layout()), points),
            ..Default::default()
        };

        let completer = AndroidCompleter::new(&dict);
        let results = completer.complete_with(&input, 10).unwrap();
        let cat = results.iter().position(|c| c.word == "cat");
        let car = results.iter().position(|c| c.word == "car");
        assert!(cat.is_some() && car.is_some(), "{results:?}");
        assert!(
            cat < car,
            "the correction nearer the touch should rank first: {results:?}"
        );
    }
}
