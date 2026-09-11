//! Cancellable candidate searches. Result budgets fail explicitly, never truncate.
use super::DictionaryResult;
use crate::text::{BOS, BOS_WIRE, CaseFold, LangDb, TextPrep};
use std::collections::HashMap;
use std::time::Instant;

pub fn check_deadline(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("candidate search exceeded its time budget".into())
    } else {
        Ok(())
    }
}

/// Prepared matching constraints for one current-word request. Empty prefix
/// selects next-word candidates. Edits are exact alternatives to the prefix.
pub struct WordSearch<'a> {
    pub prefix: String,
    pub edits: HashMap<String, f64>,
    edit_words: Vec<String>,
    pub prep: TextPrep,
    pub languages: &'a LangDb,
    pub lang: Option<&'a str>,
    pub limit: usize,
}
impl<'a> WordSearch<'a> {
    pub fn new(
        prefix: String,
        edits: HashMap<String, f64>,
        prep: TextPrep,
        languages: &'a LangDb,
        lang: Option<&'a str>,
        limit: usize,
    ) -> Self {
        let mut edit_words: Vec<_> = edits.keys().cloned().collect();
        edit_words.sort_unstable();
        Self {
            prefix,
            edits,
            edit_words,
            prep,
            languages,
            lang,
            limit,
        }
    }
    pub fn prepare(&self, word: &str) -> String {
        self.prep.apply(word, self.languages, self.lang)
    }
    pub fn matches(&self, word: &str) -> bool {
        if word.is_empty() || word == BOS || word == BOS_WIRE {
            return false;
        }
        let folded = self.prepare(word);
        folded.starts_with(&self.prefix) || self.edits.contains_key(&folded)
    }
    /// Only prune prefixes whose preparation cannot depend on later characters.
    /// Non-ASCII branches and registered language transforms are conservative.
    pub fn may_descend(&self, prefix: &str) -> bool {
        if !prefix.is_ascii() || self.prep.fold == CaseFold::LangSpecific {
            return true;
        }
        let prefix = self.prepare(prefix);
        self.prefix.starts_with(&prefix)
            || prefix.starts_with(&self.prefix)
            || self
                .edit_words
                .get(self.edit_words.partition_point(|word| word < &prefix))
                .is_some_and(|word| word.starts_with(&prefix))
    }
    pub fn push(
        &self,
        out: &mut Vec<DictionaryResult>,
        word: &str,
        confidence: f64,
        deadline: Instant,
    ) -> Result<(), String> {
        check_deadline(deadline)?;
        if self.matches(word) {
            if out.len() >= self.limit {
                return Err(format!(
                    "candidate search exceeds its {}-word budget",
                    self.limit
                ));
            }
            out.push(DictionaryResult {
                word: word.into(),
                confidence,
            });
        }
        Ok(())
    }
}
