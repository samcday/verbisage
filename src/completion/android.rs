//! Context-aware current-word and next-word ranking with explicit search budgets.
//! This adapts Android's language/spatial/case factors. It is not Android's
//! touch decoder: single edits stand in for spatial cost until layouts arrive.
use super::{CompletionCandidate, CompletionConfig, CompletionEngine, CompletionInput};
use crate::dictionary::search::{WordSearch, check_deadline};
use crate::dictionary::{DictionaryBackend, usable_frequency};
use crate::prediction::Predictor;
use crate::spellcheck::edits::{LatinAlphabet, visit_edits};
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
        let mut edits = HashMap::<String, f64>::new();
        if folded.chars().count() >= self.config.min_correction_chars {
            visit_edits(&folded, &LatinAlphabet, |word, weight| {
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
        let known = self.config.suppress_known_corrections
            && rows.iter().any(|r| search.prepare(&r.word) == folded);
        let mut candidates = Vec::with_capacity(rows.len());
        for row in rows {
            check_deadline(deadline)?;
            let prepared = search.prepare(&row.word);
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
        let mut results = Vec::with_capacity(candidates.len());
        for ((row, prepared, exact, weight), model) in candidates.into_iter().zip(scores) {
            check_deadline(deadline)?;
            let probability = model
                .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
                .unwrap_or_else(|| usable_frequency(row.confidence).max(0.0));
            // Lower language/spatial distance must improve the score. Scale
            // the resulting quality (not distance), preserving finite 0..1.
            // Treat the edit likelihood as a joint factor. An additive-only
            // adaptation regressed helo -> hello by favoring common unrelated
            // substitutions; this keeps the existing correction tradeoff.
            let mut quality = ((1.1214 * probability + 0.1524) * weight) / (1.1214 + 0.1524);
            if input.input_prep.fold != CaseFold::None
                && input.case_preference == CasePreference::PreferMatched
                && input.input.chars().any(|c| c.is_uppercase())
                && row.word.starts_with(input.input)
            {
                quality += 0.01;
            }
            let mut promotion = 1.0;
            if !folded.is_empty() && prepared == folded {
                promotion *= 1.1;
            }
            if !input.input.is_empty() && row.word == input.input {
                promotion *= 1.1;
            }
            results.push(CompletionCandidate {
                word: row.word,
                score: (quality * promotion / (1.01 * 1.21)).clamp(0.0, 1.0),
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
