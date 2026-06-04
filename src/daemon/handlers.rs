use std::sync::Arc;

use serde_json::json;

use crate::dictionary::{DictionaryBackend, DictionaryQuery};
use crate::prediction::Predictor;
use crate::spellcheck::SpellChecker;

use super::protocol::{
    DaemonRequest, DaemonResponse, FrequencyParams, IsCorrectParams, PredictParams, QueryParams,
    SuggestParams,
};

/// Concrete handler that owns the three backend trait objects and dispatches
/// incoming requests to the appropriate one.
pub struct DaemonHandler {
    dictionary: Arc<Box<dyn DictionaryBackend>>,
    spellchecker: Option<Arc<Box<dyn SpellChecker>>>,
    predictor: Option<Arc<Box<dyn Predictor>>>,
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

    /// Dispatch a single request and return the response.
    pub fn handle(&self, req: DaemonRequest) -> DaemonResponse {
        let id = req.id;

        match req.method.as_str() {
            "is_correct" => {
                let params: IsCorrectParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };

                let result = match &self.spellchecker {
                    Some(sc) => json!(sc.is_correct(&params.word)),
                    None => json!(self.dictionary.contains(&params.word)),
                };

                DaemonResponse::success(id, result)
            }

            "suggest" => {
                let params: SuggestParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };

                let mut suggestions = match &self.spellchecker {
                    Some(sc) => sc.suggest(&params.word),
                    None => {
                        // Fallback: use dictionary prefix query as a crude
                        // suggestion mechanism.
                        let results = self.dictionary.query_prefixes(&[DictionaryQuery {
                            prefix: Some(params.word.clone()),
                            suffix: None,
                            min_length: None,
                            max_length: None,
                        }]);
                        results.into_iter().map(|r| r.word).collect()
                    }
                };
                suggestions.truncate(params.max);
                DaemonResponse::success(id, json!(suggestions))
            }

            "query" => {
                let params: QueryParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };

                let results = self.dictionary.query_prefixes(&[DictionaryQuery {
                    prefix: params.prefix,
                    suffix: params.suffix,
                    min_length: params.min_len,
                    max_length: params.max_len,
                }]);

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

                match &self.predictor {
                    Some(pred) => {
                        let context: Vec<&str> =
                            params.context.iter().map(|s| s.as_str()).collect();
                        let predictions = pred.predict_next(&context, params.max);
                        let items: Vec<serde_json::Value> = predictions
                            .into_iter()
                            .map(|p| json!({"word": p.word, "confidence": p.confidence}))
                            .collect();
                        DaemonResponse::success(id, json!(items))
                    }
                    None => DaemonResponse::error(id, "no predictor backend configured"),
                }
            }

            "frequency" => {
                let params: FrequencyParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };

                let freq = self.dictionary.get_frequency(&params.word);
                DaemonResponse::success(id, json!(freq))
            }

            _ => DaemonResponse::error(id, format!("unknown method: {}", req.method)),
        }
    }
}
