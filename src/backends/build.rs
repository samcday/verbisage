use std::path::PathBuf;
use std::sync::Arc;

use crate::dictionary::paths::{LanguagePaths, expand_tilde};
use crate::dictionary::{DictionaryBackend, FileDictionaryBackend};
use crate::prediction::{Predictor, smoothed::SmoothedPredictor};
use crate::spellcheck::{DictionarySpellChecker, SpellChecker};

#[cfg(feature = "sqlite")]
use crate::backends::SharedSqliteConnection;
#[cfg(feature = "sqlite")]
use crate::dictionary::SqliteDictionaryBackend;
#[cfg(feature = "sqlite")]
use crate::prediction::sqlite::SqliteNgramBackend;
#[cfg(feature = "sqlite")]
use crate::spellcheck::SqliteSpellChecker;

#[cfg(feature = "hunspell")]
use crate::dictionary::HunspellDictionaryBackend;
#[cfg(feature = "marisa")]
use crate::dictionary::MarisaDictionaryBackend;
#[cfg(feature = "marisa")]
use crate::prediction::marisa::MarisaNgramBackend;
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

    let user_dir = lp.user_dir.as_os_str().to_str().unwrap_or("").to_string();
    let user_files: Vec<_> = if user_dir.is_empty() {
        Vec::new()
    } else {
        files
            .iter()
            .filter(|f| {
                f.to_str()
                    .map(|p| p.starts_with(&user_dir))
                    .unwrap_or(false)
            })
            .collect()
    };
    let writable = user_files.len() == files.len() && !files.is_empty();

    let dict: FileDictionaryBackend = if files.is_empty() {
        FileDictionaryBackend::new()
    } else if let Some(delim) = &def.delimiter {
        if let Some(word_index) = def.word_index {
            // Delimited mode (CSV, TSV, etc.) with explicit column indexes.
            let delim_byte = delim.as_bytes().first().copied().unwrap_or(b',');
            let merged = FileDictionaryBackend::new();
            for f in &files {
                match FileDictionaryBackend::from_delimited_file(
                    f,
                    delim_byte,
                    def.has_header,
                    word_index,
                    def.freq_index,
                    writable,
                ) {
                    Ok(other) => merged.merge(&other),
                    Err(e) => eprintln!("warning: failed to load '{}': {}", f.display(), e),
                }
            }
            merged
        } else {
            // Delimiter set but no word_index — treat as flat word-per-line.
            eprintln!("warning: delimiter set but no word_index; falling back to flat mode");
            FileDictionaryBackend::from_multiple_files(&files, writable).unwrap_or_else(|e| {
                eprintln!("warning: failed to load file backend: {}", e);
                FileDictionaryBackend::new()
            })
        }
    } else {
        // Flat / freq mode — auto-detect whitespace-separated or line-separated
        FileDictionaryBackend::from_multiple_files(&files, writable).unwrap_or_else(|e| {
            eprintln!("warning: failed to load file backend: {}", e);
            FileDictionaryBackend::new()
        })
    };

    let dict_arc: Arc<FileDictionaryBackend> = Arc::new(dict);
    let sc: Box<dyn SpellChecker> = Box::new(DictionarySpellChecker::new(dict_arc.clone()));
    let dict_box: Box<dyn DictionaryBackend> = Box::new(dict_arc);
    (dict_box, Some(sc), None)
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
                let table_ngrams = def.table_ngrams.as_deref().unwrap_or("ngrams");
                let next_col = def.next_col.as_deref().unwrap_or("next");
                let freq_col = def.freq_col.as_deref().unwrap_or("frequency");
                let context_cols = if def.context_cols.is_empty() {
                    vec!["prev".to_string()]
                } else {
                    def.context_cols.clone()
                };
                let max_order = context_cols.len();
                let writable = true;

                let dict = if def.table.is_some() {
                    // Separate dictionary table
                    let table = def.table.as_deref().unwrap_or("words");
                    let word_col = def.word_col.as_deref().unwrap_or("word");
                    SqliteDictionaryBackend::from_shared(
                        shared.clone(),
                        table,
                        word_col,
                        freq_col,
                        writable,
                    )
                } else {
                    // Shared-table mode: dict reads ngram table's unigram rows
                    SqliteDictionaryBackend::from_ngram_unigrams(
                        shared.clone(),
                        table_ngrams,
                        &context_cols,
                        next_col,
                        freq_col,
                        writable,
                    )
                };

                let sc: Box<dyn SpellChecker> =
                    Box::new(SqliteSpellChecker::new(Arc::new(dict.clone())));

                let backend = SqliteNgramBackend::new(
                    shared,
                    table_ngrams,
                    &context_cols,
                    next_col,
                    freq_col,
                    max_order,
                    writable,
                );
                let predictor = SmoothedPredictor::new(Box::new(backend));

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
            true,
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
        let max_order = context_cols.len();
        match SqliteNgramBackend::from_path(
            &path,
            table_ngrams,
            &context_cols,
            next_col,
            freq_col,
            max_order,
            true,
        ) {
            Ok(backend) => {
                let predictor = SmoothedPredictor::new(Box::new(backend));
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

#[cfg(feature = "marisa")]
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

    let dict: Box<dyn DictionaryBackend> = match files.into_iter().next() {
        Some(path) => match MarisaDictionaryBackend::from_file(&path) {
            Ok(d) => Box::new(d),
            Err(e) => {
                eprintln!(
                    "warning: failed to load marisa file '{}': {}",
                    path.display(),
                    e
                );
                Box::new(FileDictionaryBackend::new())
            }
        },
        None => {
            eprintln!("warning: no marisa file found for '{}'", lang);
            Box::new(FileDictionaryBackend::new())
        }
    };

    // Build n-gram predictor if needed
    let predictor = if def.capabilities.contains(&Capability::Ngrams) {
        build_marisa_predictor(def, lang, lp)
    } else {
        None
    };

    (dict, None, predictor)
}

#[cfg(feature = "marisa")]
fn build_marisa_predictor(
    def: &ResolvedBackendDef,
    lang: &str,
    lp: &LanguagePaths,
) -> Option<Box<dyn Predictor>> {
    #[allow(unused_variables)]
    let ngram_path = def.ngram_path.as_deref();

    // Resolve ngram trie file
    let trie_file = if let Some(p) = ngram_path {
        let expanded = p.replace("{lang}", lang);
        let p = expand_tilde(&expanded);
        if p.exists() {
            Some(p)
        } else {
            eprintln!(
                "warning: marisa ngram trie path '{}' not found",
                p.display()
            );
            None
        }
    } else {
        // Check explicit dict path's directory for companion files
        if let Some(ref dict_path) = def.path {
            let expanded = dict_path.replace("{lang}", lang);
            let dir = PathBuf::from(expand_tilde(&expanded))
                .parent()
                .map(|p| p.to_path_buf());
            if let Some(d) = dir {
                let candidate = d.join("ngrams.trie");
                if candidate.exists() {
                    Some(candidate)
                } else {
                    // Fall through to LanguagePaths resolution
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
        .or_else(|| {
            // Try LanguagePaths resolution
            let tries = lp.resolve_marisa_ngram_trie_files();
            tries.into_iter().next()
        })
    };

    let trie_file = match trie_file {
        Some(p) => p,
        None => {
            eprintln!(
                "warning: no marisa ngram trie found for ngram prediction ('{}')",
                lang
            );
            return None;
        }
    };

    // Resolve companion counts file
    let counts_file = if let Some(p) = ngram_path {
        let counts_path = PathBuf::from(p.replace("{lang}", lang).replace(".trie", ".counts"));
        if counts_path.exists() {
            counts_path
        } else {
            let dir = trie_file.parent().unwrap();
            dir.join("ngrams.counts")
        }
    } else {
        let dir = trie_file.parent().unwrap();
        let candidate = dir.join("ngrams.counts");
        if candidate.exists() {
            candidate
        } else {
            let counts = lp.resolve_marisa_ngram_counts_files();
            counts.into_iter().next().unwrap_or_else(|| {
                eprintln!("warning: no marisa ngram counts found for '{}'", lang);
                candidate // will fail with a useful error below
            })
        }
    };

    match MarisaNgramBackend::from_files(&trie_file, &counts_file) {
        Ok(backend) => {
            let predictor = SmoothedPredictor::new(Box::new(backend));
            Some(Box::new(predictor) as Box<dyn Predictor>)
        }
        Err(e) => {
            eprintln!(
                "warning: failed to load marisa ngram predictor (trie={}, counts={}): {}",
                trie_file.display(),
                counts_file.display(),
                e
            );
            None
        }
    }
}

#[cfg(not(feature = "marisa"))]
fn build_marisa(
    _def: &ResolvedBackendDef,
    _lang: &str,
    _lp: &LanguagePaths,
) -> (
    Box<dyn DictionaryBackend>,
    Option<Box<dyn SpellChecker>>,
    Option<Box<dyn Predictor>>,
) {
    let _ = _def;
    let _ = _lang;
    let _ = _lp;
    eprintln!("warning: marisa feature not enabled");
    (Box::new(FileDictionaryBackend::new()), None, None)
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
    // Determine the .dic and .aff file paths.
    let (aff_path, dic_path) = find_hunspell_files(def, lang);

    let dict = match &dic_path {
        Some(path) => match HunspellDictionaryBackend::from_dic_file(path) {
            Ok(d) => Box::new(d) as Box<dyn DictionaryBackend>,
            Err(e) => {
                eprintln!(
                    "warning: failed to load hunspell .dic '{}': {}",
                    path.display(),
                    e
                );
                Box::new(FileDictionaryBackend::new()) as Box<dyn DictionaryBackend>
            }
        },
        None => Box::new(FileDictionaryBackend::new()) as Box<dyn DictionaryBackend>,
    };

    let sc = match (aff_path, dic_path) {
        (Some(aff), Some(dic)) => match HunspellSpellChecker::from_files(&aff, &dic) {
            Ok(c) => Some(Box::new(c) as Box<dyn SpellChecker>),
            Err(e) => {
                eprintln!("warning: failed to load hunspell from files: {}", e);
                None
            }
        },
        _ => match HunspellSpellChecker::from_tag(lang) {
            Ok(c) => Some(Box::new(c) as Box<dyn SpellChecker>),
            Err(e) => {
                eprintln!(
                    "warning: hunspell dictionary not found for '{}': {}",
                    lang, e
                );
                None
            }
        },
    };

    (dict, sc, None)
}

/// Search for Hunspell `.aff` and `.dic` files, preferring explicit paths
/// set on the backend def, then falling back to system directories.
#[cfg(feature = "hunspell")]
fn find_hunspell_files(
    _def: &ResolvedBackendDef,
    lang: &str,
) -> (Option<PathBuf>, Option<PathBuf>) {
    // Check explicit path — treat path as base name (without extension).
    // For now, always fall back to system search.
    let dirs = [
        "/usr/share/hunspell",
        "/usr/share/myspell",
        "/usr/share/myspell/dicts",
    ];

    for dir in &dirs {
        let aff = PathBuf::from(format!("{}/{}.aff", dir, lang));
        let dic = PathBuf::from(format!("{}/{}.dic", dir, lang));
        if aff.exists() && dic.exists() {
            return (Some(aff), Some(dic));
        }
    }

    (None, None)
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
