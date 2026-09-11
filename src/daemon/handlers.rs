use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::backends::resolve_chain_with_backcompat;
use crate::completion::{AndroidCompleter, CompletionEngine, CompletionInput};
use crate::dictionary::paths::LanguagePaths;
use crate::dictionary::{
    DictionaryBackend, DictionaryQuery, DictionaryResult, FileDictionaryBackend,
};
use crate::prediction::{Prediction, Predictor};
use crate::spellcheck::SpellChecker;

use super::config::DaemonConfig;
use super::protocol::{
    CompleteParams, DaemonRequest, DaemonResponse, FrequencyParams, IsCorrectParams,
    LimitedQueryParams, NgramBumpParams, PredictParams, QueryParams, SuggestParams, WordAddParams,
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

    fn get_or_load_backend(&self, lang: &str) -> Result<Arc<CachedBackend>, String> {
        // Language tags are substituted into dictionary paths.
        if lang.is_empty()
            || lang.len() > 64
            || !lang
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'@' | b'.'))
        {
            return Err("invalid language tag".into());
        }
        {
            let cache = self.cache.lock().unwrap();
            if let Some(backend) = cache.get(lang) {
                return Ok(backend.clone());
            }
        }

        let backend = self.build_backend(lang);
        crate::veprintln!("[handler] loaded backend for '{}'", lang);

        let mut cache = self.cache.lock().unwrap();
        // Keep language switching bounded in a long-running session service.
        if cache.len() >= 8 {
            cache.clear();
        }
        Ok(cache
            .entry(lang.to_string())
            .or_insert_with(|| Arc::new(backend))
            .clone())
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
        let backend = self.get_or_load_backend(lang)?;
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        Ok(match &backend.spellchecker {
            Some(sc) => sc.is_correct(word),
            None => backend.dictionary.contains(word),
        })
    }

    pub fn suggest(&self, word: &str, max: usize, lang: &str) -> Result<Vec<String>, String> {
        let backend = self.get_or_load_backend(lang)?;
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

    pub fn complete(
        &self,
        word: &str,
        max: usize,
        lang: &str,
    ) -> Result<Vec<DictionaryResult>, String> {
        self.complete_with(
            &CompletionInput {
                input: word,
                ..Default::default()
            },
            max,
            lang,
        )
    }

    pub fn completion_deadline(&self) -> std::time::Duration {
        self.config.completion.response_deadline
    }

    pub fn validate_complete(&self, input: &CompletionInput<'_>, max: usize) -> Result<(), String> {
        if max > self.config.max_complete_results {
            return Err(format!(
                "requested max {max} exceeds Complete cap {}",
                self.config.max_complete_results
            ));
        }
        let invalid = |word: &str| {
            word.len() > 512
                || word.chars().count() > 128
                || word.chars().any(|ch| ch.is_control() || ch.is_whitespace())
        };
        if invalid(input.input)
            || input.context.len() > 16
            || input
                .context
                .iter()
                .any(|word| word.is_empty() || invalid(word))
        {
            return Err(
                "invalid completion input or context: at most 16 words, 128 characters per word"
                    .into(),
            );
        }
        Ok(())
    }

    pub fn complete_with(
        &self,
        input: &CompletionInput<'_>,
        max: usize,
        lang: &str,
    ) -> Result<Vec<DictionaryResult>, String> {
        self.validate_complete(input, max)?;
        if max == 0 {
            return Ok(Vec::new());
        }
        let backend = self.get_or_load_backend(lang)?;
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{lang}'"));
        }
        AndroidCompleter::new(backend.dictionary.as_ref())
            .with_predictor(backend.predictor.as_deref())
            .with_language(lang)
            .with_config(self.config.completion.clone())
            .complete_with(input, max)
            .map(|rows| {
                rows.into_iter()
                    .map(|c| DictionaryResult {
                        word: c.word,
                        confidence: c.score,
                    })
                    .collect()
            })
    }

    pub fn complete_request(
        &self,
        params: &CompleteParams,
        lang: &str,
    ) -> Result<Vec<DictionaryResult>, String> {
        let context: Vec<_> = params.context.iter().map(String::as_str).collect();
        self.complete_with(
            &CompletionInput {
                input: &params.word,
                context: &context,
                input_prep: params.options.input_prep,
                context_prep: params.options.context_prep,
                case_preference: params.options.case_preference,
            },
            params.max,
            lang,
        )
    }

    #[cfg(feature = "swipe")]
    pub fn recognize_swipe(
        &self,
        request: crate::swipe::SwipeRequest,
        lang: &str,
    ) -> Result<Vec<(String, f64)>, String> {
        if request.max_results() > self.config.max_complete_results {
            return Err(format!(
                "requested max {} exceeds swipe cap {}",
                request.max_results(),
                self.config.max_complete_results
            ));
        }
        let backend = self.get_or_load_backend(lang)?;
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        request.recognize(backend.dictionary.as_ref())
    }

    pub fn query(
        &self,
        queries: &[DictionaryQuery],
        lang: &str,
    ) -> Result<Vec<DictionaryResult>, String> {
        let backend = self.get_or_load_backend(lang)?;
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        Ok(backend.dictionary.query_prefixes(queries))
    }

    pub fn query_limited(
        &self,
        queries: &[DictionaryQuery],
        lang: &str,
        max: usize,
    ) -> Result<Vec<DictionaryResult>, String> {
        if max > self.config.max_query_results {
            return Err(format!(
                "requested max {} exceeds bounded-query cap {}",
                max, self.config.max_query_results
            ));
        }
        let backend = self.get_or_load_backend(lang)?;
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        Ok(backend.dictionary.query_limited(queries, max))
    }

    pub fn predict(
        &self,
        context: &[&str],
        max: usize,
        lang: &str,
    ) -> Result<Vec<Prediction>, String> {
        self.complete_with(
            &CompletionInput {
                context,
                ..Default::default()
            },
            max,
            lang,
        )
        .map(|rows| {
            rows.into_iter()
                .map(|r| Prediction {
                    word: r.word,
                    confidence: r.confidence,
                })
                .collect()
        })
    }

    pub fn frequency(&self, word: &str, lang: &str) -> Result<f64, String> {
        let backend = self.get_or_load_backend(lang)?;
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
        let backend = self.get_or_load_backend(lang)?;
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
        let backend = self.get_or_load_backend(lang)?;
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

            "complete" | "complete_with" => {
                let params: CompleteParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {e}")),
                };
                match self.complete_request(&params, lang) {
                    Ok(rows) => DaemonResponse::success(
                        id,
                        json!(
                            rows.into_iter()
                                .map(|r| json!({"word":r.word,"confidence":r.confidence}))
                                .collect::<Vec<_>>()
                        ),
                    ),
                    Err(e) => DaemonResponse::error(id, e),
                }
            }
            "query_limited" => {
                let params: LimitedQueryParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {e}")),
                };
                if params.query.prefixes.len() > 16
                    || params.query.suffixes.len() > 16
                    || params
                        .query
                        .prefixes
                        .iter()
                        .chain(&params.query.suffixes)
                        .chain(params.query.prefix.iter())
                        .chain(params.query.suffix.iter())
                        .any(|s| s.len() > 256)
                {
                    return DaemonResponse::error(id, "completion query is too large");
                }
                match self.query_limited(&params.query.into_queries(), lang, params.max) {
                    Ok(rows) => DaemonResponse::success(
                        id,
                        json!(
                            rows.into_iter()
                                .map(|r| json!({"word":r.word,"confidence":r.confidence}))
                                .collect::<Vec<_>>()
                        ),
                    ),
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

            "predict" | "predict_with" => {
                let params: PredictParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {}", e)),
                };
                match self.complete_request(
                    &CompleteParams {
                        word: String::new(),
                        context: params.context,
                        max: params.max,
                        options: params.options,
                    },
                    lang,
                ) {
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
        let predictor: Box<dyn crate::prediction::Predictor> = Box::new(
            SmoothedPredictor::new(std::sync::Arc::new(backend)).with_deltas(vec![0.4, 0.4, 0.2]),
        );

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

    #[test]
    fn handler_complete_uses_engine_and_enforces_cap() {
        let mut dict = FileDictionaryBackend::new();
        dict.add_word_mut("hello".to_string(), 120.0);
        dict.add_word_mut("help".to_string(), 149.0);
        dict.add_word_mut("helium".to_string(), 80.0);
        let handler = DaemonHandler::new(Box::new(dict), None, None, "en_US".into());

        let results = handler.complete("hel", 6, "en_US").unwrap();
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.word.starts_with("hel")));

        let err = handler.complete("hel", 1_001, "en_US").unwrap_err();
        assert!(err.contains("exceeds"), "unexpected error: {}", err);
        // At-cap requests pass the gate.
        assert!(handler.complete("hel", 1_000, "en_US").is_ok());
    }

    #[test]
    fn handler_query_limited_enforces_cap() {
        let dict = FileDictionaryBackend::new();
        let handler = DaemonHandler::new(Box::new(dict), None, None, "en_US".into());
        let query = DictionaryQuery {
            prefix: Some("hel".to_string()),
            suffix: None,
            min_length: None,
            max_length: None,
        };
        let err = handler
            .query_limited(&[query.clone()], "en_US", 200_001)
            .unwrap_err();
        assert!(err.contains("exceeds"), "unexpected error: {}", err);
        assert!(handler.query_limited(&[query], "en_US", 200_000).is_ok());
    }

    #[test]
    fn handler_caps_are_configurable() {
        let mut cfg = DaemonConfig::default_for("en_US");
        cfg.max_complete_results = 2;
        cfg.max_query_results = 5;
        let handler = DaemonHandler::with_config(cfg);
        // Cap checks run before any backend is loaded.
        let err = handler.complete("hel", 3, "en_US").unwrap_err();
        assert!(err.contains("exceeds"), "unexpected error: {}", err);
        let query = DictionaryQuery {
            prefix: Some("hel".to_string()),
            suffix: None,
            min_length: None,
            max_length: None,
        };
        let err = handler.query_limited(&[query], "en_US", 6).unwrap_err();
        assert!(err.contains("exceeds"), "unexpected error: {}", err);
    }
}
