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
    CompleteParams, DaemonRequest, DaemonResponse, ForgetLayoutParams, FrequencyParams,
    IsCorrectParams, LimitedQueryParams, NgramBumpParams, PredictParams, QueryParams,
    RegisterLayoutParams, SuggestParams, WordAddParams,
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
    layouts: Mutex<crate::layout::LayoutCache>,
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
            layouts: Mutex::new(crate::layout::LayoutCache::default()),
            default_lang,
        }
    }

    /// Create from config with an empty cache (used by daemon).
    pub fn with_config(config: DaemonConfig) -> Self {
        let default_lang = config.default_lang.clone();
        Self {
            config,
            cache: Mutex::new(HashMap::new()),
            layouts: Mutex::new(crate::layout::LayoutCache::default()),
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
        self.suggest_with(word, max, lang, None)
    }

    pub fn suggest_with(
        &self,
        word: &str,
        max: usize,
        lang: &str,
        layout: Option<Arc<keyboard_layout::RectKeyLayout>>,
    ) -> Result<Vec<String>, String> {
        let backend = self.get_or_load_backend(lang)?;
        if !backend.loaded {
            return Err(format!("no dictionary loaded for '{}'", lang));
        }
        let input = crate::spellcheck::SuggestionInput {
            word,
            context: &[],
            spatial: crate::spatial::SpatialInput::from_parts(layout, Vec::new()),
        };
        let mut suggestions = match &backend.spellchecker {
            Some(sc) => sc.suggest_with(&input, max),
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
        let layout = match &params.layout {
            Some(token) => Some(
                self.layout(token)
                    .ok_or_else(|| format!("unknown layout token '{token}'"))?,
            ),
            None => None,
        };
        self.complete_with(
            &CompletionInput {
                input: &params.word,
                context: &context,
                input_prep: params.options.input_prep,
                context_prep: params.options.context_prep,
                case_preference: params.options.case_preference,
                spatial: crate::spatial::SpatialInput::from_parts(layout, Vec::new()),
            },
            params.max,
            lang,
        )
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

    // ── Layout registry ───────────────────────────────────────────────────

    pub fn register_layout(&self, upload: &crate::layout::LayoutUpload) -> Result<String, String> {
        let mut layouts = self
            .layouts
            .lock()
            .map_err(|_| "layout cache poisoned".to_string())?;
        crate::layout::register(&mut layouts, upload)
    }

    pub fn forget_layout(&self, token: &str) -> Result<bool, String> {
        let mut layouts = self
            .layouts
            .lock()
            .map_err(|_| "layout cache poisoned".to_string())?;
        Ok(layouts.forget(token))
    }

    pub fn layout(&self, token: &str) -> Option<Arc<keyboard_layout::RectKeyLayout>> {
        self.layouts.lock().ok()?.get(token)
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
                let layout = if let Some(token) = &params.layout {
                    match self.layout(token) {
                        Some(layout) => Some(layout),
                        None => {
                            return DaemonResponse::error(
                                id,
                                format!("unknown layout token '{token}'"),
                            );
                        }
                    }
                } else {
                    None
                };
                match self.suggest_with(&params.word, params.max, lang, layout) {
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
                        layout: None,
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

            "register_layout" => {
                let params: RegisterLayoutParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {e}")),
                };
                match self.register_layout(&params.layout) {
                    Ok(token) => DaemonResponse::success(id, json!(token)),
                    Err(e) => DaemonResponse::error(id, e),
                }
            }

            "forget_layout" => {
                let params: ForgetLayoutParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => return DaemonResponse::error(id, format!("bad params: {e}")),
                };
                match self.forget_layout(&params.token) {
                    Ok(removed) => DaemonResponse::success(id, json!(removed)),
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

    #[test]
    fn handler_registers_and_forgets_layouts() {
        let handler = DaemonHandler::new(
            Box::new(FileDictionaryBackend::new()),
            None,
            None,
            "en_US".into(),
        );
        let register = DaemonRequest {
            id: Some(1),
            method: "register_layout".into(),
            params: json!({
                "layout": { "rows": {
                    "rows": [{ "keys": [{
                        "main": "q",
                        "secondary": [],
                        "width": 1.0,
                        "stretch": false,
                        "rect": null
                    }] }],
                    "ignored_labels": []
                } }
            }),
            lang: None,
        };
        let response = handler.handle(register);
        assert!(response.error.is_none(), "{:?}", response.error);
        let token = response
            .result
            .unwrap()
            .as_str()
            .expect("token is a string")
            .to_string();
        assert!(handler.layout(&token).is_some());

        let forget = DaemonRequest {
            id: Some(2),
            method: "forget_layout".into(),
            params: json!({ "token": token }),
            lang: None,
        };
        let response = handler.handle(forget);
        assert!(response.error.is_none(), "{:?}", response.error);
        assert_eq!(response.result.unwrap(), json!(true));
        assert!(handler.layout(&token).is_none());
    }

    #[test]
    fn complete_resolves_registered_layout_token() {
        let mut dict = FileDictionaryBackend::new();
        dict.add_word_mut("hello".into(), 100.0);
        let handler = DaemonHandler::new(Box::new(dict), None, None, "en_US".into());

        let register = DaemonRequest {
            id: Some(1),
            method: "register_layout".into(),
            params: json!({ "layout": { "rows": { "rows": [{ "keys": [
                { "main": "h", "secondary": [], "width": 1.0, "stretch": false, "rect": null },
                { "main": "e", "secondary": [], "width": 1.0, "stretch": false, "rect": null },
                { "main": "l", "secondary": [], "width": 1.0, "stretch": false, "rect": null },
                { "main": "o", "secondary": [], "width": 1.0, "stretch": false, "rect": null }
            ] }], "ignored_labels": [] } } }),
            lang: None,
        };
        let token = handler
            .handle(register)
            .result
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let response = handler.handle(DaemonRequest {
            id: Some(2),
            method: "complete_with".into(),
            params: json!({ "word": "helo", "context": [], "max": 6, "layout": token }),
            lang: None,
        });
        assert!(response.error.is_none(), "{:?}", response.error);

        let response = handler.handle(DaemonRequest {
            id: Some(3),
            method: "complete_with".into(),
            params: json!({ "word": "helo", "context": [], "max": 6, "layout": "deadbeef" }),
            lang: None,
        });
        assert!(
            response
                .error
                .unwrap_or_default()
                .contains("unknown layout token")
        );
    }
}
