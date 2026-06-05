use std::path::PathBuf;
use std::sync::Arc;

use crate::dictionary::paths::{LanguagePaths, expand_tilde};
use crate::dictionary::{DictionaryBackend, FileDictionaryBackend};
use crate::prediction::Predictor;
use crate::spellcheck::{DictionarySpellChecker, SpellChecker};

#[cfg(feature = "sqlite")]
use crate::backends::SharedSqliteConnection;
#[cfg(feature = "sqlite")]
use crate::dictionary::SqliteDictionaryBackend;
#[cfg(feature = "sqlite")]
use crate::prediction::sqlite::SqlitePredictor;
#[cfg(feature = "sqlite")]
use crate::spellcheck::SqliteSpellChecker;

#[cfg(feature = "hunspell")]
use crate::spellcheck::HunspellSpellChecker;

use super::chain::SegmentRole;
use super::merged::{MergedDictionary, MergedPredictor};
use super::{BackendType, Capability, ResolvedBackendDef};

// ---------------------------------------------------------------------------
// Per-backend build
// ---------------------------------------------------------------------------

/// Build a concrete set of backend objects for one resolved backend definition.
pub fn build_backend(
    def: &ResolvedBackendDef,
    lang: &str,
    lp: &LanguagePaths,
) -> (
    Box<dyn DictionaryBackend>,
    Option<Box<dyn SpellChecker>>,
    Option<Box<dyn Predictor>>,
) {
    match def.backend_type {
        BackendType::File => build_file(def, lang, lp),
        BackendType::Sqlite => build_sqlite(def, lang, lp),
        BackendType::Marisa => build_marisa(def, lang, lp),
        BackendType::Hunspell => build_hunspell(def, lang, lp),
    }
}

// ---------------------------------------------------------------------------
// File backend
// ---------------------------------------------------------------------------

fn build_file(
    def: &ResolvedBackendDef,
    lang: &str,
    lp: &LanguagePaths,
) -> (
    Box<dyn DictionaryBackend>,
    Option<Box<dyn SpellChecker>>,
    Option<Box<dyn Predictor>>,
) {
    let files = resolve_files(def, lang, lp);

    let dict = if files.is_empty() {
        FileDictionaryBackend::new()
    } else if def.delimiter.is_some() {
        // CSV mode — load with explicit delimiter
        FileDictionaryBackend::from_multiple_files(&files).unwrap_or_else(|e| {
            eprintln!("warning: failed to load file backend: {}", e);
            FileDictionaryBackend::new()
        })
    } else {
        // Flat / freq mode — auto-detect whitespace-separated or line-separated
        FileDictionaryBackend::from_multiple_files(&files).unwrap_or_else(|e| {
            eprintln!("warning: failed to load file backend: {}", e);
            FileDictionaryBackend::new()
        })
    };

    let sc: Box<dyn SpellChecker> = Box::new(DictionarySpellChecker::new(Arc::new(dict.clone())));
    (Box::new(dict), Some(sc), None)
}

// ---------------------------------------------------------------------------
// SQLite backend
// ---------------------------------------------------------------------------

#[cfg(feature = "sqlite")]
fn build_sqlite(
    def: &ResolvedBackendDef,
    lang: &str,
    lp: &LanguagePaths,
) -> (
    Box<dyn DictionaryBackend>,
    Option<Box<dyn SpellChecker>>,
    Option<Box<dyn Predictor>>,
) {
    let files = resolve_files(def, lang, lp);
    let path = files.into_iter().next();

    let has_dict = def.capabilities.contains(&Capability::Dictionary)
        || def.capabilities.contains(&Capability::Unigrams);
    let has_ngrams = def.capabilities.contains(&Capability::Ngrams);

    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("warning: no sqlite database found for '{}'", lang);
            let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
            return (empty, None, None);
        }
    };

    if has_dict && has_ngrams {
        // Share connection between dict and predictor
        match SharedSqliteConnection::open(&path) {
            Ok(shared) => {
                let table = def.table.as_deref().unwrap_or("words");
                let word_col = def.word_col.as_deref().unwrap_or("word");
                let freq_col = def.freq_col.as_deref().unwrap_or("frequency");
                let dict =
                    SqliteDictionaryBackend::from_shared(shared.clone(), table, word_col, freq_col);

                let sc: Box<dyn SpellChecker> =
                    Box::new(SqliteSpellChecker::new(Arc::new(dict.clone())));

                let table_ngrams = def.table_ngrams.as_deref().unwrap_or("ngrams");
                let next_col = def.next_col.as_deref().unwrap_or("next");
                let context_cols = if def.context_cols.is_empty() {
                    vec!["prev".to_string()]
                } else {
                    def.context_cols.clone()
                };
                let predictor =
                    SqlitePredictor::new(shared, table_ngrams, &context_cols, next_col, freq_col);

                (Box::new(dict), Some(sc), Some(Box::new(predictor)))
            }
            Err(e) => {
                eprintln!(
                    "warning: failed to open sqlite db '{}': {}",
                    path.display(),
                    e
                );
                let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
                (empty, None, None)
            }
        }
    } else if has_dict {
        // Dict only (or dict + unigrams)
        let _writable = true; // user file — TODO: determine writability
        let result = SqliteDictionaryBackend::from_sqlite(
            &path,
            def.table.as_deref().unwrap_or("words"),
            def.word_col.as_deref().unwrap_or("word"),
            def.freq_col.as_deref().unwrap_or("frequency"),
        );
        match result {
            Ok(dict) => {
                let sc: Box<dyn SpellChecker> =
                    Box::new(SqliteSpellChecker::new(Arc::new(dict.clone())));
                (Box::new(dict), Some(sc), None)
            }
            Err(e) => {
                eprintln!(
                    "warning: failed to open sqlite db '{}': {}",
                    path.display(),
                    e
                );
                let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
                (empty, None, None)
            }
        }
    } else if has_ngrams {
        // Ngrams only
        let table_ngrams = def.table_ngrams.as_deref().unwrap_or("ngrams");
        let next_col = def.next_col.as_deref().unwrap_or("next");
        let freq_col = def.freq_col.as_deref().unwrap_or("frequency");
        let context_cols = if def.context_cols.is_empty() {
            vec!["prev".to_string()]
        } else {
            def.context_cols.clone()
        };
        match SqlitePredictor::from_path(&path, table_ngrams, &context_cols[0], next_col, freq_col)
        {
            Ok(predictor) => {
                let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
                (empty, None, Some(Box::new(predictor)))
            }
            Err(e) => {
                eprintln!(
                    "warning: failed to open ngram predictor '{}': {}",
                    path.display(),
                    e
                );
                let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
                (empty, None, None)
            }
        }
    } else {
        let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
        (empty, None, None)
    }
}

#[cfg(not(feature = "sqlite"))]
fn build_sqlite(
    _def: &ResolvedBackendDef,
    _lang: &str,
    _lp: &LanguagePaths,
) -> (
    Box<dyn DictionaryBackend>,
    Option<Box<dyn SpellChecker>>,
    Option<Box<dyn Predictor>>,
) {
    eprintln!("warning: sqlite feature not enabled");
    (Box::new(FileDictionaryBackend::new()), None, None)
}

// ---------------------------------------------------------------------------
// Marisa backend (placeholder — uses FileDictionaryBackend)
// ---------------------------------------------------------------------------

fn build_marisa(
    def: &ResolvedBackendDef,
    lang: &str,
    lp: &LanguagePaths,
) -> (
    Box<dyn DictionaryBackend>,
    Option<Box<dyn SpellChecker>>,
    Option<Box<dyn Predictor>>,
) {
    let files = resolve_files(def, lang, lp);

    let dict = if files.is_empty() {
        FileDictionaryBackend::new()
    } else {
        // Placeholder: MarisaDictionaryBackend not yet implemented.
        // For now, load as flat file.
        FileDictionaryBackend::from_multiple_files(&files).unwrap_or_else(|e| {
            eprintln!("warning: failed to load marisa file: {}", e);
            FileDictionaryBackend::new()
        })
    };

    let sc: Box<dyn SpellChecker> = Box::new(DictionarySpellChecker::new(Arc::new(dict.clone())));
    (Box::new(dict), Some(sc), None)
}

// ---------------------------------------------------------------------------
// Hunspell backend
// ---------------------------------------------------------------------------

#[cfg(feature = "hunspell")]
fn build_hunspell(
    def: &ResolvedBackendDef,
    lang: &str,
    _lp: &LanguagePaths,
) -> (
    Box<dyn DictionaryBackend>,
    Option<Box<dyn SpellChecker>>,
    Option<Box<dyn Predictor>>,
) {
    let sc = match &def.hunspell_affix {
        Some(_) => {
            // Would need both affix and dict paths — not yet wired
            HunspellSpellChecker::from_tag(lang).ok()
        }
        None => HunspellSpellChecker::from_tag(lang).ok(),
    };

    match sc {
        Some(checker) => {
            // TODO: HunspellDictionaryBackend implementing DictionaryBackend
            let dict: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
            (dict, Some(Box::new(checker)), None)
        }
        None => {
            eprintln!("warning: hunspell dictionary not found for '{}'", lang);
            (Box::new(FileDictionaryBackend::new()), None, None)
        }
    }
}

#[cfg(not(feature = "hunspell"))]
fn build_hunspell(
    _def: &ResolvedBackendDef,
    _lang: &str,
    _lp: &LanguagePaths,
) -> (
    Box<dyn DictionaryBackend>,
    Option<Box<dyn SpellChecker>>,
    Option<Box<dyn Predictor>>,
) {
    eprintln!("warning: hunspell feature not enabled");
    (Box::new(FileDictionaryBackend::new()), None, None)
}

// ---------------------------------------------------------------------------
// Path resolution helper
// ---------------------------------------------------------------------------

fn resolve_files(def: &ResolvedBackendDef, lang: &str, lp: &LanguagePaths) -> Vec<PathBuf> {
    if let Some(ref explicit_path) = def.path {
        let expanded = explicit_path.replace("{lang}", lang);
        return vec![expand_tilde(&expanded)];
    }

    match def.backend_type {
        BackendType::File => lp.resolve_dict_files(),
        BackendType::Sqlite => lp.resolve_sqlite_files(),
        BackendType::Marisa => lp.resolve_marisa_files(),
        BackendType::Hunspell => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Top-level: build an entire chain role assignment into a CachedBackend
// ---------------------------------------------------------------------------

/// The assembled result of composing a chain of backends.
pub struct ComposedBackend {
    pub loaded: bool,
    pub dictionary: Box<dyn DictionaryBackend>,
    pub spellchecker: Option<Box<dyn SpellChecker>>,
    pub predictor: Option<Box<dyn Predictor>>,
}

/// Compose a chain role assignment into a single backend bundle.
pub fn compose_chain(
    assignment: &super::chain::RoleAssignment,
    lang: &str,
    lp: &LanguagePaths,
) -> ComposedBackend {
    let mut dict_backends: Vec<Box<dyn DictionaryBackend>> = Vec::new();
    let mut spellcheckers: Vec<Option<Box<dyn SpellChecker>>> = Vec::new();
    let mut predictors: Vec<Box<dyn Predictor>> = Vec::new();

    for seg in &assignment.segments {
        let (dict, sc, pred) = build_backend(&seg.def, lang, lp);
        match seg.role {
            SegmentRole::Dictionary | SegmentRole::Unigrams => {
                dict_backends.push(dict);
                spellcheckers.push(sc);
            }
            SegmentRole::Ngrams => {
                if let Some(p) = pred {
                    predictors.push(p);
                }
            }
        }
    }

    let loaded = !dict_backends.is_empty() || !predictors.is_empty();

    let merged_dict: Box<dyn DictionaryBackend> = if dict_backends.is_empty() {
        Box::new(FileDictionaryBackend::new())
    } else if dict_backends.len() == 1 {
        dict_backends.into_iter().next().unwrap()
    } else {
        Box::new(MergedDictionary::new(dict_backends))
    };

    let spellchecker = spellcheckers.into_iter().flatten().next();

    let predictor: Option<Box<dyn Predictor>> = if predictors.is_empty() {
        None
    } else if predictors.len() == 1 {
        Some(predictors.into_iter().next().unwrap())
    } else {
        Some(Box::new(MergedPredictor::new(predictors)) as Box<dyn Predictor>)
    };

    ComposedBackend {
        loaded,
        dictionary: merged_dict,
        spellchecker,
        predictor,
    }
}
