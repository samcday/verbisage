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
    /// Backends are loaded lazily on first request per language.
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
            eprintln!(
                "warning: no dictionary files found for '{}' (dirs: system={}, user={})",
                lang,
                lp.system_dir.display(),
                lp.user_dir.display(),
            );
            return CachedBackend {
                loaded: false,
                dictionary: Box::new(FileDictionaryBackend::new()),
                spellchecker: None,
                predictor: None,
            };
        }

        let dict = match FileDictionaryBackend::from_multiple_files(&files) {
            Ok(d) => d,
            Err(e) => {
                eprintln!(
                    "warning: failed to load dictionary files for '{}': {}",
                    lang, e,
                );
                return CachedBackend {
                    loaded: false,
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
            loaded: true,
            dictionary: Box::new(dict),
            spellchecker: Some(sc),
            predictor: None,
        }
    }

    #[cfg(feature = "sqlite")]
    fn build_sqlite_backend(lang: &str, config: &DaemonConfig) -> CachedBackend {
        use crate::spellcheck::SqliteSpellChecker;

        let lp = Self::lang_paths(lang, config);

        // Prefer user DB (writable) over system DB (read-only).
        // For every file that opens successfully we verify the expected table
        // actually exists — a valid but wrong‑schema file is treated as absent.
        let dict = lp
            .user_sqlite_file()
            .and_then(|path| Self::try_open_sqlite(path, &config, lang, true))
            .or_else(|| {
                lp.system_sqlite_file()
                    .and_then(|path| Self::try_open_sqlite(path, &config, lang, false))
            });

        match dict {
            Some(d) => {
                let sc: Box<dyn SpellChecker> =
                    Box::new(SqliteSpellChecker::new(std::sync::Arc::new(d.clone())));
                CachedBackend {
                    loaded: true,
                    dictionary: Box::new(d),
                    spellchecker: Some(sc),
                    predictor: None,
                }
            }
            None => {
                eprintln!(
                    "warning: no sqlite database found for '{}' (dirs: system={}, user={})",
                    lang,
                    lp.system_dir.display(),
                    lp.user_dir.display(),
                );
                CachedBackend {
                    loaded: false,
                    dictionary: Box::new(FileDictionaryBackend::new()),
                    spellchecker: None,
                    predictor: None,
                }
            }
        }
    }

    #[cfg(feature = "hunspell")]
    fn build_hunspell_backend(lang: &str, config: &DaemonConfig) -> CachedBackend {
        use crate::spellcheck::HunspellSpellChecker;

        let sc = match (&config.hunspell_affix, &config.hunspell_dict) {
            (Some(aff), Some(dic)) => match HunspellSpellChecker::from_files(aff, dic) {
                Ok(c) => Some(c),
                Err(e) => {
                    eprintln!(
                        "warning: failed to load hunspell files for '{}': {}",
                        lang, e,
                    );
                    None
                }
            },
            _ => match HunspellSpellChecker::from_tag(lang) {
                Ok(c) => Some(c),
                Err(e) => {
                    eprintln!(
                        "warning: failed to load hunspell dictionary for '{}': {}",
                        lang, e,
                    );
                    None
                }
            },
        };

        match sc {
            Some(checker) => CachedBackend {
                loaded: true,
                dictionary: Box::new(FileDictionaryBackend::new()),
                spellchecker: Some(Box::new(checker)),
                predictor: None,
            },
            None => CachedBackend {
                loaded: false,
                dictionary: Box::new(FileDictionaryBackend::new()),
                spellchecker: None,
                predictor: None,
            },
        }
    }

    /// Try to open a SQLite backend at `path`, verifying the expected table
    /// actually exists.  Returns `None` (with a warning on stderr) when the
    /// file can't be opened or the table is missing.
    #[cfg(feature = "sqlite")]
    fn try_open_sqlite(
        path: std::path::PathBuf,
        config: &DaemonConfig,
        lang: &str,
        writable: bool,
    ) -> Option<crate::dictionary::SqliteDictionaryBackend> {
        use crate::dictionary::SqliteDictionaryBackend;

        let result = if writable {
            SqliteDictionaryBackend::from_sqlite(
                &path,
                &config.sqlite_table,
                &config.sqlite_word_col,
                &config.sqlite_freq_col,
            )
        } else {
            SqliteDictionaryBackend::from_sqlite_readonly(
                &path,
                &config.sqlite_table,
                &config.sqlite_word_col,
                &config.sqlite_freq_col,
            )
        };

        match result {
            Ok(d) if d.table_exists() => Some(d),
            Ok(_) => {
                eprintln!(
                    "warning: {} sqlite db for '{}' exists but has no '{}' table: {}",
                    if writable { "user" } else { "system" },
                    lang,
                    config.sqlite_table,
                    path.display(),
                );
                None
            }
            Err(e) => {
                eprintln!(
                    "warning: failed to open {} sqlite db for '{}' ({}): {}",
                    if writable { "user" } else { "system" },
                    lang,
                    path.display(),
                    e,
                );
                None
            }
        }
    }

    fn lang_paths(lang: &str, config: &DaemonConfig) -> LanguagePaths {
        let mut lp = LanguagePaths::new(lang);
        lp.system_dir = config.language_paths.system_dir.clone();
        lp.user_dir = config.language_paths.user_dir.clone();
        lp.system_file_override = config.language_paths.system_file_override.clone();
        lp.user_file_override = config.language_paths.user_file_override.clone();
        lp.system_dict_patterns = config.language_paths.system_dict_patterns.clone();
        lp.user_dict_patterns = config.language_paths.user_dict_patterns.clone();
        lp.system_sqlite_patterns = config.language_paths.system_sqlite_patterns.clone();
        lp.user_sqlite_patterns = config.language_paths.user_sqlite_patterns.clone();
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

            _ => DaemonResponse::error(id, format!("unknown method: {}", req.method)),
        }
    }
}
