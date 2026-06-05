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
    /// Language tag used when a request provides no override.
    pub default_lang: String,
}

impl DaemonHandler {
    pub fn new(
        dictionary: Box<dyn DictionaryBackend>,
        spellchecker: Option<Box<dyn SpellChecker>>,
        predictor: Option<Box<dyn Predictor>>,
        default_lang: String,
    ) -> Self {
        Self {
            dictionary: Arc::new(dictionary),
            spellchecker: spellchecker.map(|s| Arc::new(s)),
            predictor: predictor.map(|p| Arc::new(p)),
            default_lang,
        }
    }

    /// Return the effective language for a request: the request-level override
    /// when present, otherwise the handler's configured default.
    pub fn resolve_lang<'a>(&'a self, req_lang: Option<&'a str>) -> &'a str {
        req_lang.unwrap_or(&self.default_lang)
    }

    // ── Typed API (used by DBus server and stdio handler) ────────────────

    pub fn is_correct(&self, word: &str, _lang: &str) -> bool {
        match &self.spellchecker {
            Some(sc) => sc.is_correct(word),
            None => self.dictionary.contains(word),
        }
    }

    pub fn suggest(&self, word: &str, max: usize, _lang: &str) -> Vec<String> {
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

    pub fn query(&self, query: &DictionaryQuery, _lang: &str) -> Vec<DictionaryResult> {
        self.dictionary.query_prefixes(&[query.clone()])
    }

    pub fn predict(&self, context: &[&str], max: usize, _lang: &str) -> Vec<Prediction> {
        match &self.predictor {
            Some(pred) => pred.predict_next(context, max),
            None => Vec::new(),
        }
    }

    pub fn frequency(&self, word: &str, _lang: &str) -> f64 {
        self.dictionary.get_frequency(word)
    }

    // ── JSON-protocol dispatch (used by stdio daemon) ────────────────────

    pub fn handle(&self, req: DaemonRequest) -> DaemonResponse {
        let id = req.id;
        let lang = self.resolve_lang(req.lang.as_deref());

        match req.method.as_str() {
            "is_correct" => {
                let params: IsCorrectParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                DaemonResponse::success(id, json!(self.is_correct(&params.word, lang)))
            }

            "suggest" => {
                let params: SuggestParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                DaemonResponse::success(id, json!(self.suggest(&params.word, params.max, lang)))
            }

            "query" => {
                let params: QueryParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                let results = self.query(
                    &DictionaryQuery {
                        prefix: params.prefix,
                        suffix: params.suffix,
                        min_length: params.min_len,
                        max_length: params.max_len,
                    },
                    lang,
                );
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
                let predictions = self.predict(&context, params.max, lang);
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
                DaemonResponse::success(id, json!(self.frequency(&params.word, lang)))
            }

            _ => DaemonResponse::error(id, format!("unknown method: {}", req.method)),
        }
    }
}
