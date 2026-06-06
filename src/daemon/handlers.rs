use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::backends::resolve_chain_with_backcompat;
use crate::dictionary::paths::LanguagePaths;
use crate::dictionary::{
    DictionaryBackend, DictionaryQuery, DictionaryResult, FileDictionaryBackend,
};
use crate::prediction::{Prediction, Predictor};
use crate::spellcheck::SpellChecker;

use super::config::DaemonConfig;
use super::protocol::{
    DaemonRequest, DaemonResponse, FrequencyParams, IsCorrectParams, NgramBumpParams,
    PredictParams, QueryParams, SuggestParams, WordAddParams,
};

struct CachedBackend {
    loaded: bool,
    dictionary: Box<dyn DictionaryBackend>,
    spellchecker: Option<Box<dyn SpellChecker>>,
    predictor: Option<Box<dyn Predictor>>,
}

pub struct DaemonHandler {
    config: DaemonConfig,
    cache: Mutex<HashMap<String, Arc<CachedBackend>>>,
    pub default_lang: String,
}

impl DaemonHandler {
    /// Create from pre-built backends (used by tests).
    pub fn new(
        dictionary: Box<dyn DictionaryBackend>,
        spellchecker: Option<Box<dyn SpellChecker>>,
        predictor: Option<Box<dyn Predictor>>,
        default_lang: String,
    ) -> Self {
        let mut cache = HashMap::new();
        cache.insert(
            default_lang.clone(),
            Arc::new(CachedBackend {
                loaded: true,
                dictionary,
                spellchecker,
                predictor,
            }),
        );
        Self {
            config: DaemonConfig::default_for(&default_lang),
            cache: Mutex::new(cache),
            default_lang,
        }
    }

    /// Create from config with an empty cache (used by daemon).
    pub fn with_config(config: DaemonConfig) -> Self {
        let default_lang = config.default_lang.clone();
        Self {
            config,
            cache: Mutex::new(HashMap::new()),
            default_lang,
        }
    }

    /// Return the effective language for a request: the request-level override
    /// when present, otherwise the handler's configured default.
    pub fn resolve_lang<'a>(&'a self, req_lang: Option<&'a str>) -> &'a str {
        req_lang.unwrap_or(&self.default_lang)
    }

    fn get_or_load_backend(&self, lang: &str) -> Arc<CachedBackend> {
        {
            let cache = self.cache.lock().unwrap();
            if let Some(backend) = cache.get(lang) {
                return backend.clone();
            }
        }

        let backend = self.build_backend(lang);
        crate::veprintln!("[handler] loaded backend for '{}'", lang);

        let mut cache = self.cache.lock().unwrap();
        cache
            .entry(lang.to_string())
            .or_insert_with(|| Arc::new(backend))
            .clone()
    }

    fn build_backend(&self, lang: &str) -> CachedBackend {
        let named_backends = if self.config.named_backends.is_empty() {
            None
        } else {
            Some(&self.config.named_backends)
        };

        let (assignment, warnings) =
            match resolve_chain_with_backcompat(&self.config.backend_chain, named_backends) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("warning: failed to resolve backend chain: {}", e);
                    return CachedBackend {
                        loaded: false,
                        dictionary: Box::new(FileDictionaryBackend::new()),
                        spellchecker: None,
                        predictor: None,
                    };
                }
            };

        for w in &warnings {
            eprintln!("warning: {}", w);
        }

        let lp = self.lang_paths(lang);
        let composed = crate::backends::build::compose_chain(&assignment, lang, &lp, None, None);

        CachedBackend {
            loaded: composed.loaded,
            dictionary: composed.dictionary,
            spellchecker: composed.spellchecker,
            predictor: composed.predictor,
        }
    }

    fn lang_paths(&self, lang: &str) -> LanguagePaths {
        let mut lp = LanguagePaths::new(lang);
        lp.system_dir = self.config.language_paths.system_dir.clone();
        lp.user_dir = self.config.language_paths.user_dir.clone();
        lp.system_file_override = self.config.language_paths.system_file_override.clone();
        lp.user_file_override = self.config.language_paths.user_file_override.clone();
        lp.system_dict_patterns = self.config.language_paths.system_dict_patterns.clone();
        lp.user_dict_patterns = self.config.language_paths.user_dict_patterns.clone();
        lp.system_sqlite_patterns = self.config.language_paths.system_sqlite_patterns.clone();
        lp.user_sqlite_patterns = self.config.language_paths.user_sqlite_patterns.clone();
        lp
    }

    // ── Typed API ─────────────────────────────────────────────────────────

    pub fn is_correct(&self, word: &str, lang: &str) -> Result<bool, String> {
        let backend = self.get_or_load_backend(lang);
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        Ok(match &backend.spellchecker {
            Some(sc) => sc.is_correct(word),
            None => backend.dictionary.contains(word),
        })
    }

    pub fn suggest(&self, word: &str, max: usize, lang: &str) -> Result<Vec<String>, String> {
        let backend = self.get_or_load_backend(lang);
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        let mut suggestions = match &backend.spellchecker {
            Some(sc) => sc.suggest(word, &[]),
            None => {
                let results = backend.dictionary.query_prefixes(&[DictionaryQuery {
                    prefix: Some(word.to_string()),
                    suffix: None,
                    min_length: None,
                    max_length: None,
                }]);
                results.into_iter().map(|r| r.word).collect()
            }
        };
        suggestions.truncate(max);
        Ok(suggestions)
    }

    pub fn query(
        &self,
        queries: &[DictionaryQuery],
        lang: &str,
    ) -> Result<Vec<DictionaryResult>, String> {
        let backend = self.get_or_load_backend(lang);
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        Ok(backend.dictionary.query_prefixes(queries))
    }

    pub fn predict(
        &self,
        context: &[&str],
        max: usize,
        lang: &str,
    ) -> Result<Vec<Prediction>, String> {
        let backend = self.get_or_load_backend(lang);
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        match &backend.predictor {
            Some(pred) => Ok(pred.predict_next(context, max)),
            None => Ok(Vec::new()),
        }
    }

    pub fn frequency(&self, word: &str, lang: &str) -> Result<f64, String> {
        let backend = self.get_or_load_backend(lang);
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        Ok(backend.dictionary.get_frequency(word))
    }

    pub fn add_word(
        &self,
        word: &str,
        frequency: f64,
        allow_existing: bool,
        lang: &str,
    ) -> Result<(), String> {
        let backend = self.get_or_load_backend(lang);
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        backend
            .dictionary
            .add_word(word, frequency, allow_existing)
            .map_err(|e| e.to_string())
    }

    pub fn increase_ngram_frequency(
        &self,
        ngram: &[String],
        delta: f64,
        save_unknown: bool,
        lang: &str,
    ) -> Result<(), String> {
        let backend = self.get_or_load_backend(lang);
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        match &backend.predictor {
            Some(pred) => {
                let refs: Vec<&str> = ngram.iter().map(|s| s.as_str()).collect();
                pred.increase_ngram_frequency(&refs, delta, save_unknown)
                    .map_err(|e| e.to_string())
            }
            None => Err("no predictor loaded for n-gram frequency updates".into()),
        }
    }

    // ── JSON-protocol dispatch ────────────────────────────────────────────

    pub fn handle(&self, req: DaemonRequest) -> DaemonResponse {
        let id = req.id;
        let lang = self.resolve_lang(req.lang.as_deref());
        crate::veprintln!("[handler] <- {}(lang={})", req.method, lang);

        match req.method.as_str() {
            "is_correct" => {
                let params: IsCorrectParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                match self.is_correct(&params.word, lang) {
                    Ok(v) => DaemonResponse::success(id, json!(v)),
                    Err(e) => DaemonResponse::error(id, e),
                }
            }

            "suggest" => {
                let params: SuggestParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                match self.suggest(&params.word, params.max, lang) {
                    Ok(v) => DaemonResponse::success(id, json!(v)),
                    Err(e) => DaemonResponse::error(id, e),
                }
            }

            "query" => {
                let params: QueryParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                let queries: Vec<DictionaryQuery> = params.into_queries();
                match self.query(&queries, lang) {
                    Ok(results) => {
                        let items: Vec<serde_json::Value> = results
                            .into_iter()
                            .map(|r| json!({"word": r.word, "confidence": r.confidence}))
                            .collect();
                        DaemonResponse::success(id, json!(items))
                    }
                    Err(e) => DaemonResponse::error(id, e),
                }
            }

            "predict" => {
                let params: PredictParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                let context: Vec<&str> = params.context.iter().map(|s| s.as_str()).collect();
                match self.predict(&context, params.max, lang) {
                    Ok(predictions) => {
                        let items: Vec<serde_json::Value> = predictions
                            .into_iter()
                            .map(|p| json!({"word": p.word, "confidence": p.confidence}))
                            .collect();
                        DaemonResponse::success(id, json!(items))
                    }
                    Err(e) => DaemonResponse::error(id, e),
                }
            }

            "frequency" => {
                let params: FrequencyParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                match self.frequency(&params.word, lang) {
                    Ok(v) => DaemonResponse::success(id, json!(v)),
                    Err(e) => DaemonResponse::error(id, e),
                }
            }

            "word_add" => {
                let params: WordAddParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                match self.add_word(&params.word, params.frequency, params.allow_existing, lang) {
                    Ok(()) => DaemonResponse::success(id, json!(true)),
                    Err(e) => DaemonResponse::error(id, e),
                }
            }

            "ngram_bump" => {
                let params: NgramBumpParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                match self.increase_ngram_frequency(
                    &params.ngram,
                    params.delta,
                    params.save_unknown,
                    lang,
                ) {
                    Ok(()) => DaemonResponse::success(id, json!(true)),
                    Err(e) => DaemonResponse::error(id, e),
                }
            }

            _ => DaemonResponse::error(id, format!("unknown method: {}", req.method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::SharedSqliteConnection;
    use crate::dictionary::{FileDictionaryBackend, PresageSqliteBackend};
    use crate::prediction::smoothed::SmoothedPredictor;
    use crate::spellcheck::{DictionarySpellChecker, SpellChecker};
    use serde_json::json;

    #[test]
    fn handler_add_word() {
        let dir = tempfile::tempdir().unwrap();
        let dict_path = dir.path().join("words.txt");
        {
            let mut f = std::fs::File::create(&dict_path).unwrap();
            use std::io::Write;
            writeln!(f, "hello").unwrap();
        }
        let file_dict =
            Arc::new(FileDictionaryBackend::from_multiple_files(&[dict_path], true).unwrap());
        let dict_for_handler = file_dict.clone();
        let sc: Box<dyn SpellChecker> = Box::new(DictionarySpellChecker::new(file_dict));
        let handler =
            DaemonHandler::new(Box::new(dict_for_handler), Some(sc), None, "en_US".into());

        assert!(handler.is_correct("hello", "en_US").unwrap());
        assert!(!handler.is_correct("newword", "en_US").unwrap());
        handler.add_word("newword", 1.0, false, "en_US").unwrap();
        assert!(handler.is_correct("newword", "en_US").unwrap());
    }

    #[test]
    fn handler_ngram_bump() {
        let conn = rusqlite::Connection::open(":memory:").unwrap();
        conn.execute_batch(
            "CREATE TABLE _1_gram (word TEXT PRIMARY KEY, count INTEGER DEFAULT 1);
             CREATE TABLE _2_gram (word_1 TEXT, word TEXT, count INTEGER DEFAULT 1, UNIQUE(word_1, word));
             CREATE TABLE _3_gram (word_2 TEXT, word_1 TEXT, word TEXT, count INTEGER DEFAULT 1, UNIQUE(word_2, word_1, word));
             INSERT OR REPLACE INTO _1_gram VALUES ('hello', 10);",
        )
        .unwrap();
        let shared = SharedSqliteConnection::new(conn);
        let backend = PresageSqliteBackend::from_shared(shared, true, false);
        let predictor: Box<dyn crate::prediction::Predictor> =
            Box::new(SmoothedPredictor::new(Box::new(backend)).with_deltas(vec![0.4, 0.4, 0.2]));

        let dict = FileDictionaryBackend::new();
        let handler = DaemonHandler::new(Box::new(dict), None, Some(predictor), "en_US".into());

        let ngram = vec!["hello".to_string()];
        handler
            .increase_ngram_frequency(&ngram, 5.0, false, "en_US")
            .unwrap();
    }

    #[test]
    fn handler_dispatch_word_add() {
        let dir = tempfile::tempdir().unwrap();
        let dict_path = dir.path().join("words.txt");
        {
            let mut f = std::fs::File::create(&dict_path).unwrap();
            use std::io::Write;
            writeln!(f, "hello").unwrap();
        }
        let file_dict = FileDictionaryBackend::from_multiple_files(&[dict_path], true).unwrap();
        let dict_clone = file_dict.clone();
        let sc: Box<dyn SpellChecker> =
            Box::new(DictionarySpellChecker::new(std::sync::Arc::new(file_dict)));
        let handler = DaemonHandler::new(Box::new(dict_clone), Some(sc), None, "en_US".into());

        let req = DaemonRequest {
            id: Some(1),
            method: "word_add".to_string(),
            params: json!({"word": "newword", "frequency": 1.0, "allow_existing": false}),
            lang: None,
        };
        let resp = handler.handle(req);
        assert!(resp.error.is_none());
        assert!(resp.result.unwrap()["id"].is_null());
    }

    #[test]
    fn handler_dispatch_ngram_bump_no_predictor() {
        let dict = FileDictionaryBackend::new();
        let handler = DaemonHandler::new(Box::new(dict), None, None, "en_US".into());

        let req = DaemonRequest {
            id: Some(1),
            method: "ngram_bump".to_string(),
            params: json!({"ngram": ["hello"], "delta": 1.0, "save_unknown": true}),
            lang: None,
        };
        let resp = handler.handle(req);
        assert!(resp.error.is_some());
        assert!(resp.error.as_ref().unwrap().contains("no predictor"));
    }
}
