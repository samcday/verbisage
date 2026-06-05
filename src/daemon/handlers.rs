use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::dictionary::paths::LanguagePaths;
use crate::dictionary::{
    DictionaryBackend, DictionaryQuery, DictionaryResult, FileDictionaryBackend,
};
use crate::prediction::{Prediction, Predictor};
use crate::spellcheck::{DictionarySpellChecker, SpellChecker};

use super::config::{BackendKind, DaemonConfig};
use super::protocol::{
    DaemonRequest, DaemonResponse, FrequencyParams, IsCorrectParams, PredictParams, QueryParams,
    SuggestParams,
};

struct CachedBackend {
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
    /// When `eager_path` is set, the default language backend is loaded at
    /// startup instead of lazily.
    pub fn with_config(config: DaemonConfig) -> Self {
        let default_lang = config.default_lang.clone();
        let mut cache = HashMap::new();

        if let Some(path) = &config.eager_path {
            if let Ok(dict) = FileDictionaryBackend::from_multiple_files(&[path.clone()]) {
                let sc: Box<dyn SpellChecker> = Box::new(DictionarySpellChecker::new(
                    std::sync::Arc::new(dict.clone()),
                ));
                cache.insert(
                    default_lang.clone(),
                    Arc::new(CachedBackend {
                        dictionary: Box::new(dict),
                        spellchecker: Some(sc),
                        predictor: None,
                    }),
                );
            }
        }

        Self {
            config,
            cache: Mutex::new(cache),
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

        let mut cache = self.cache.lock().unwrap();
        cache
            .entry(lang.to_string())
            .or_insert_with(|| Arc::new(backend))
            .clone()
    }

    fn build_backend(&self, lang: &str) -> CachedBackend {
        match self.config.backend {
            BackendKind::File => Self::build_file_backend(lang, &self.config),
            #[cfg(feature = "sqlite")]
            BackendKind::Sqlite => Self::build_sqlite_backend(lang, &self.config),
            #[cfg(feature = "hunspell")]
            BackendKind::Hunspell => Self::build_hunspell_backend(lang, &self.config),
        }
    }

    fn build_file_backend(lang: &str, config: &DaemonConfig) -> CachedBackend {
        let lp = Self::lang_paths(lang, config);
        let files = lp.resolve_dict_files();

        if files.is_empty() {
            return CachedBackend {
                dictionary: Box::new(FileDictionaryBackend::new()),
                spellchecker: None,
                predictor: None,
            };
        }

        let dict = match FileDictionaryBackend::from_multiple_files(&files) {
            Ok(d) => d,
            Err(_) => {
                return CachedBackend {
                    dictionary: Box::new(FileDictionaryBackend::new()),
                    spellchecker: None,
                    predictor: None,
                };
            }
        };

        let sc: Box<dyn SpellChecker> = Box::new(DictionarySpellChecker::new(std::sync::Arc::new(
            dict.clone(),
        )));
        CachedBackend {
            dictionary: Box::new(dict),
            spellchecker: Some(sc),
            predictor: None,
        }
    }

    #[cfg(feature = "sqlite")]
    fn build_sqlite_backend(lang: &str, config: &DaemonConfig) -> CachedBackend {
        use crate::dictionary::SqliteDictionaryBackend;
        use crate::spellcheck::SqliteSpellChecker;

        let lp = Self::lang_paths(lang, config);
        let files = lp.resolve_sqlite_files();

        if let Some(path) = files.first() {
            if let Ok(dict) = SqliteDictionaryBackend::from_sqlite(
                path,
                &config.sqlite_table,
                &config.sqlite_word_col,
                &config.sqlite_freq_col,
            ) {
                let sc: Box<dyn SpellChecker> =
                    Box::new(SqliteSpellChecker::new(std::sync::Arc::new(dict.clone())));
                return CachedBackend {
                    dictionary: Box::new(dict),
                    spellchecker: Some(sc),
                    predictor: None,
                };
            }
        }

        CachedBackend {
            dictionary: Box::new(FileDictionaryBackend::new()),
            spellchecker: None,
            predictor: None,
        }
    }

    #[cfg(feature = "hunspell")]
    fn build_hunspell_backend(lang: &str, config: &DaemonConfig) -> CachedBackend {
        use crate::spellcheck::HunspellSpellChecker;

        let sc = match (&config.hunspell_affix, &config.hunspell_dict) {
            (Some(aff), Some(dic)) => HunspellSpellChecker::from_files(aff, dic).ok(),
            _ => HunspellSpellChecker::from_tag(lang).ok(),
        };

        match sc {
            Some(checker) => CachedBackend {
                dictionary: Box::new(FileDictionaryBackend::new()),
                spellchecker: Some(Box::new(checker)),
                predictor: None,
            },
            None => CachedBackend {
                dictionary: Box::new(FileDictionaryBackend::new()),
                spellchecker: None,
                predictor: None,
            },
        }
    }

    fn lang_paths(lang: &str, config: &DaemonConfig) -> LanguagePaths {
        let mut lp = LanguagePaths::new(lang);
        lp.system_dir = config.language_paths.system_dir.clone();
        lp.user_dir = config.language_paths.user_dir.clone();
        lp.system_file_override = config.language_paths.system_file_override.clone();
        lp.user_file_override = config.language_paths.user_file_override.clone();
        lp
    }

    // ── Typed API ─────────────────────────────────────────────────────────

    pub fn is_correct(&self, word: &str, lang: &str) -> bool {
        let backend = self.get_or_load_backend(lang);
        match &backend.spellchecker {
            Some(sc) => sc.is_correct(word),
            None => backend.dictionary.contains(word),
        }
    }

    pub fn suggest(&self, word: &str, max: usize, lang: &str) -> Vec<String> {
        let backend = self.get_or_load_backend(lang);
        let mut suggestions = match &backend.spellchecker {
            Some(sc) => sc.suggest(word),
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
        suggestions
    }

    pub fn query(&self, queries: &[DictionaryQuery], lang: &str) -> Vec<DictionaryResult> {
        let backend = self.get_or_load_backend(lang);
        backend.dictionary.query_prefixes(queries)
    }

    pub fn predict(&self, context: &[&str], max: usize, lang: &str) -> Vec<Prediction> {
        let backend = self.get_or_load_backend(lang);
        match &backend.predictor {
            Some(pred) => pred.predict_next(context, max),
            None => Vec::new(),
        }
    }

    pub fn frequency(&self, word: &str, lang: &str) -> f64 {
        let backend = self.get_or_load_backend(lang);
        backend.dictionary.get_frequency(word)
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
                let queries: Vec<DictionaryQuery> = params.into_queries();
                let results = self.query(&queries, lang);
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
