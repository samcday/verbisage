use std::sync::Arc;

use serde_json::json;

use crate::dictionary::{DictionaryBackend, DictionaryQuery, DictionaryResult};
use crate::prediction::{Prediction, Predictor};
use crate::spellcheck::SpellChecker;

use super::protocol::{
    DaemonRequest, DaemonResponse, FrequencyParams, IsCorrectParams, PredictParams, QueryParams,
    SuggestParams,
};

/// Concrete handler that owns the three backend trait objects and dispatches
/// incoming requests to the appropriate one.
pub struct DaemonHandler {
    pub dictionary: Arc<Box<dyn DictionaryBackend>>,
    pub spellchecker: Option<Arc<Box<dyn SpellChecker>>>,
    pub predictor: Option<Arc<Box<dyn Predictor>>>,
}

impl DaemonHandler {
    pub fn new(
        dictionary: Box<dyn DictionaryBackend>,
        spellchecker: Option<Box<dyn SpellChecker>>,
        predictor: Option<Box<dyn Predictor>>,
    ) -> Self {
        Self {
            dictionary: Arc::new(dictionary),
            spellchecker: spellchecker.map(|s| Arc::new(s)),
            predictor: predictor.map(|p| Arc::new(p)),
        }
    }

    // ── Typed API (used by DBus server and stdio handler) ────────────────

    pub fn is_correct(&self, word: &str) -> bool {
        match &self.spellchecker {
            Some(sc) => sc.is_correct(word),
            None => self.dictionary.contains(word),
        }
    }

    pub fn suggest(&self, word: &str, max: usize) -> Vec<String> {
        let mut suggestions = match &self.spellchecker {
            Some(sc) => sc.suggest(word),
            None => {
                let results = self.dictionary.query_prefixes(&[DictionaryQuery {
                    prefix: Some(word.to_string()),
                    suffix: None,
                    min_length: None,
                    max_length: None,
                }]);
                results.into_iter().map(|r| r.word).collect()
            }
        };
        suggestions.truncate(max);
        suggestions
    }

    pub fn query(&self, query: &DictionaryQuery) -> Vec<DictionaryResult> {
        self.dictionary.query_prefixes(&[query.clone()])
    }

    pub fn predict(&self, context: &[&str], max: usize) -> Vec<Prediction> {
        match &self.predictor {
            Some(pred) => pred.predict_next(context, max),
            None => Vec::new(),
        }
    }

    pub fn frequency(&self, word: &str) -> f64 {
        self.dictionary.get_frequency(word)
    }

    // ── JSON-protocol dispatch (used by stdio daemon) ────────────────────

    pub fn handle(&self, req: DaemonRequest) -> DaemonResponse {
        let id = req.id;

        match req.method.as_str() {
            "is_correct" => {
                let params: IsCorrectParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                DaemonResponse::success(id, json!(self.is_correct(&params.word)))
            }

            "suggest" => {
                let params: SuggestParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                DaemonResponse::success(id, json!(self.suggest(&params.word, params.max)))
            }

            "query" => {
                let params: QueryParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                let results = self.query(&DictionaryQuery {
                    prefix: params.prefix,
                    suffix: params.suffix,
                    min_length: params.min_len,
                    max_length: params.max_len,
                });
                let items: Vec<serde_json::Value> = results
                    .into_iter()
                    .map(|r| json!({"word": r.word, "confidence": r.confidence}))
                    .collect();
                DaemonResponse::success(id, json!(items))
            }

            "predict" => {
                let params: PredictParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                let context: Vec<&str> = params.context.iter().map(|s| s.as_str()).collect();
                let predictions = self.predict(&context, params.max);
                let items: Vec<serde_json::Value> = predictions
                    .into_iter()
                    .map(|p| json!({"word": p.word, "confidence": p.confidence}))
                    .collect();
                DaemonResponse::success(id, json!(items))
            }

            "frequency" => {
                let params: FrequencyParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                DaemonResponse::success(id, json!(self.frequency(&params.word)))
            }

            _ => DaemonResponse::error(id, format!("unknown method: {}", req.method)),
        }
    }
}
