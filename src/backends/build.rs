use std::path::PathBuf;
use std::sync::Arc;

use crate::dictionary::paths::{LanguagePaths, expand_tilde};
use crate::dictionary::{DictionaryBackend, FileDictionaryBackend, PresageSqliteBackend};
use crate::prediction::{Predictor, smoothed::SmoothedPredictor};
use crate::spellcheck::{DictionarySpellChecker, SpellChecker};

#[cfg(feature = "sqlite")]
use crate::spellcheck::SqliteSpellChecker;

#[cfg(feature = "hunspell")]
use crate::dictionary::HunspellDictionaryBackend;

#[cfg(feature = "hunspell")]
use crate::spellcheck::HunspellSpellChecker;

use super::chain::SegmentRole;
use super::merged::{MergedDictionary, MergedPredictor};
use super::{BackendType, Capability, ResolvedBackendDef};

/// Normalize a directory path string: expand tilde and ensure no trailing slash.
/// This ensures consistent behavior regardless of whether the user includes
/// a trailing slash in their config.
fn normalize_dir(dir: &str) -> PathBuf {
    let expanded = expand_tilde(dir);
    let path_str = expanded.to_string_lossy();
    // Strip trailing slashes (but keep root "/")
    let trimmed = path_str.trim_end_matches('/');
    PathBuf::from(if trimmed.is_empty() { "/" } else { trimmed })
}

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
        BackendType::Patricia => build_patricia(def, lang),
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

    if files.is_empty() {
        let tried = lp.resolve_dict_files_all()
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "warning: no file dictionary found for '{}' (tried: {})",
            lang,
            if tried.is_empty() {
                "(no paths resolved)".to_string()
            } else {
                tried
            }
        );
    }

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
    } else {
        for f in &files {
            eprintln!("info: loaded file dictionary: {}", f.display());
        }
        if let Some(delim) = &def.delimiter {
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
        }
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
    _def: &ResolvedBackendDef,
    lang: &str,
    lp: &LanguagePaths,
) -> (
    Box<dyn DictionaryBackend>,
    Option<Box<dyn SpellChecker>>,
    Option<Box<dyn Predictor>>,
) {
    let files = resolve_files(_def, lang, lp);
    let path = files.first().cloned();

    let path = match path {
        Some(p) => p,
        None => {
            eprintln!(
                "warning: no sqlite database found for '{}' (tried: {})",
                lang,
                files
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
            return (empty, None, None);
        }
    };

    match PresageSqliteBackend::open(&path, true) {
        Ok(backend) => {
            eprintln!("info: loaded sqlite database: {}", path.display());
            let backend_arc = Arc::new(backend);
            let dict: Box<dyn DictionaryBackend> = Box::new(backend_arc.clone());
            let sc: Box<dyn SpellChecker> = Box::new(SqliteSpellChecker::new(backend_arc.clone()));
            let predictor = SmoothedPredictor::new(backend_arc);

            (dict, Some(sc), Some(Box::new(predictor)))
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
    use std::sync::Arc;

    let trie_candidates = resolve_marisa_ngram_trie_files(def, lang, lp);
    let counts_candidates = resolve_marisa_ngram_counts_files(def, lang, lp);

    let trie_path = trie_candidates.iter().cloned().find(|p| p.exists());
    let trie_path = match trie_path {
        Some(p) => p,
        None => {
            eprintln!(
                "warning: no marisa ngram trie found for '{}' (tried: {})",
                lang,
                trie_candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return (Box::new(FileDictionaryBackend::new()), None, None);
        }
    };

    let counts_path = counts_candidates.iter().cloned().find(|p| p.exists());
    let counts_path = match counts_path {
        Some(p) => p,
        None => {
            eprintln!(
                "warning: no marisa ngram counts file found for '{}' (tried: {})",
                lang,
                counts_candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return (Box::new(FileDictionaryBackend::new()), None, None);
        }
    };

    let backend =
        match crate::prediction::marisa::MarisaNgramBackend::from_files(&trie_path, &counts_path) {
            Ok(b) => {
                eprintln!(
                    "info: loaded marisa ngram backend: {} + {}",
                    trie_path.display(),
                    counts_path.display()
                );
                Arc::new(b)
            }
            Err(e) => {
                eprintln!(
                    "warning: failed to load marisa ngram backend '{}': {}",
                    trie_path.display(),
                    e
                );
                return (Box::new(FileDictionaryBackend::new()), None, None);
            }
        };

    let dict: Box<dyn DictionaryBackend> = Box::new(backend.clone());

    let spellchecker = {
        let checker = crate::spellcheck::DictionarySpellChecker::new(backend.clone());
        Some(Box::new(checker) as Box<dyn SpellChecker>)
    };

    let predictor = if def.capabilities.contains(&Capability::Ngrams) {
        let ngram_backend: std::sync::Arc<dyn crate::prediction::ngram_backend::NgramBackend> =
            std::sync::Arc::new(backend);
        Some(
            Box::new(crate::prediction::smoothed::SmoothedPredictor::new(
                ngram_backend,
            )) as Box<dyn crate::prediction::Predictor>,
        )
    } else {
        None
    };

    (dict, spellchecker, predictor)
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
            Ok(d) => {
                eprintln!("info: loaded hunspell dictionary: {}", path.display());
                Box::new(d) as Box<dyn DictionaryBackend>
            }
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
        (Some(aff), Some(dic)) => {
            match HunspellSpellChecker::from_files(&aff, &dic, def.embedded_correction_engine) {
                Ok(c) => {
                    eprintln!(
                        "info: loaded hunspell spellchecker: {} + {}",
                        aff.display(),
                        dic.display()
                    );
                    Some(Box::new(c) as Box<dyn SpellChecker>)
                }
                Err(e) => {
                    eprintln!("warning: failed to load hunspell from files: {}", e);
                    None
                }
            }
        }
        _ => match HunspellSpellChecker::from_tag(lang, def.embedded_correction_engine) {
            Ok(c) => {
                eprintln!("info: loaded hunspell spellchecker for tag '{}'", lang);
                Some(Box::new(c) as Box<dyn SpellChecker>)
            }
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
        let aff = PathBuf::from(dir).join(format!("{}.aff", lang));
        let dic = PathBuf::from(dir).join(format!("{}.dic", lang));
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
// Marisa ngram path resolution
// ---------------------------------------------------------------------------

#[cfg(feature = "marisa")]
fn resolve_marisa_ngram_trie_files(
    def: &ResolvedBackendDef,
    lang: &str,
    lp: &LanguagePaths,
) -> Vec<PathBuf> {
    let mut results = Vec::new();

    if let Some(p) = &def.ngram_path {
        let expanded = p.replace("{lang}", lang);
        let path = expand_tilde(&expanded);
        if !results.contains(&path) {
            results.push(path);
        }
    }

    if let Some(ref dict_path) = def.path {
        let expanded = dict_path.replace("{lang}", lang);
        let base = PathBuf::from(expand_tilde(&expanded));
        let dir = if base.is_dir() {
            base
        } else if let Some(parent) = base.parent() {
            parent.to_path_buf()
        } else {
            PathBuf::new()
        };
        if !dir.as_os_str().is_empty() {
            let candidate = dir.join("ngrams.trie");
            if !results.contains(&candidate) {
                results.push(candidate);
            }
        }
    }

    for p in lp.resolve_all_marisa_ngram_trie_files() {
        if !results.contains(&p) {
            results.push(p);
        }
    }

    results
}

#[cfg(feature = "marisa")]
fn resolve_marisa_ngram_counts_files(
    def: &ResolvedBackendDef,
    lang: &str,
    lp: &LanguagePaths,
) -> Vec<PathBuf> {
    let mut results = Vec::new();

    if let Some(p) = &def.ngram_path {
        let expanded = p.replace("{lang}", lang).replace(".trie", ".counts");
        let path = expand_tilde(&expanded);
        if !results.contains(&path) {
            results.push(path);
        }
    }

    if let Some(ref dict_path) = def.path {
        let expanded = dict_path.replace("{lang}", lang);
        let base = PathBuf::from(expand_tilde(&expanded));
        let dir = if base.is_dir() {
            base
        } else if let Some(parent) = base.parent() {
            parent.to_path_buf()
        } else {
            PathBuf::new()
        };
        if !dir.as_os_str().is_empty() {
            let candidate = dir.join("ngrams.counts");
            if !results.contains(&candidate) {
                results.push(candidate);
            }
        }
    }

    for p in lp.resolve_all_marisa_ngram_counts_files() {
        if !results.contains(&p) {
            results.push(p);
        }
    }

    results
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
        BackendType::Hunspell | BackendType::Patricia => Vec::new(),
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
///
/// `cli_system_dir` and `cli_user_dir` are CLI overrides (e.g. `--system-data-dir`).
/// When set, they take precedence over per-backend config values.
pub fn compose_chain(
    assignment: &super::chain::RoleAssignment,
    lang: &str,
    lp: &LanguagePaths,
    cli_system_dir: Option<&str>,
    cli_user_dir: Option<&str>,
) -> ComposedBackend {
    let mut dict_backends: Vec<Box<dyn DictionaryBackend>> = Vec::new();
    let mut spellcheckers: Vec<Option<Box<dyn SpellChecker>>> = Vec::new();
    let mut predictors: Vec<Box<dyn Predictor>> = Vec::new();

    for seg in &assignment.segments {
        let mut seg_lp = lp.clone();
        // CLI overrides take precedence over per-backend config values
        if cli_system_dir.is_none() {
            if let Some(ref dir) = seg.def.system_dir {
                seg_lp = seg_lp.with_system_dir(normalize_dir(dir));
            }
        }
        if cli_user_dir.is_none() {
            if let Some(ref dir) = seg.def.user_dir {
                seg_lp = seg_lp.with_user_dir(normalize_dir(dir));
            }
        }
        // Per-backend pattern overrides always apply (no CLI equivalent yet)
        if seg.def.system_patterns.is_some() || seg.def.user_patterns.is_some() {
            seg_lp.set_patterns(
                seg.def.system_patterns.as_deref(),
                seg.def.user_patterns.as_deref(),
                seg.def.system_patterns.as_deref(),
                seg.def.user_patterns.as_deref(),
            );
        }
        let (dict, sc, pred) = build_backend(&seg.def, lang, &seg_lp);
        match seg.role {
            SegmentRole::Dictionary | SegmentRole::Unigrams => {
                dict_backends.push(dict);
                spellcheckers.push(sc);
                if let Some(p) = pred {
                    predictors.push(p);
                }
            }
            SegmentRole::Ngrams => {
                // Ngrams segments also provide dict (word membership + frequencies)
                // and may carry a spellchecker. The n-gram backend itself contains
                // unigram counts, so dict collection is needed for the suggester.
                dict_backends.push(dict);
                spellcheckers.push(sc);
                if let Some(p) = pred {
                    predictors.push(p);
                }
            }
        }
    }

    let loaded = dict_backends.iter().any(|dict| !dict.is_empty()) || !predictors.is_empty();

    let merged_dict: Box<dyn DictionaryBackend> = if dict_backends.is_empty() {
        Box::new(FileDictionaryBackend::new())
    } else if dict_backends.len() == 1 {
        dict_backends.into_iter().next().unwrap()
    } else {
        eprintln!("info: merged {} dictionary backends", dict_backends.len());
        Box::new(MergedDictionary::new(dict_backends))
    };

    let spellchecker = spellcheckers.into_iter().flatten().next();

    let predictor: Option<Box<dyn Predictor>> = if predictors.is_empty() {
        None
    } else if predictors.len() == 1 {
        Some(predictors.into_iter().next().unwrap())
    } else {
        eprintln!("info: merged {} predictor backends", predictors.len());
        Some(Box::new(MergedPredictor::new(predictors)) as Box<dyn Predictor>)
    };

    // Wire n-gram backend to spellchecker if possible
    if let Some(ref sc) = spellchecker {
        if sc.can_use_ngram_backend() {
            if let Some(ref pred) = predictor {
                if let Some(ngram) = pred.ngram_backend() {
                    sc.set_ngram_backend(ngram);
                    eprintln!("info: attached n-gram backend to spellchecker");
                }
            }
        }
    }

    ComposedBackend {
        loaded,
        dictionary: merged_dict,
        spellchecker,
        predictor,
    }
}


#[cfg(feature = "patricia")]
fn build_patricia(
    def: &ResolvedBackendDef,
    lang: &str,
) -> (Box<dyn DictionaryBackend>, Option<Box<dyn SpellChecker>>, Option<Box<dyn Predictor>>) {
    let path = def.path.as_ref().map(|p| expand_tilde(&p.replace("{lang}", lang)))
        .unwrap_or_else(|| PathBuf::from(def.system_dir.as_deref()
            .unwrap_or("/usr/share/android-patricia-dictionaries")).join(format!("{lang}.dict")));
    match crate::dictionary::patricia::PatriciaDictionaryBackend::open(&path) {
        Ok(backend) => {
            let backend = Arc::new(backend);
            (Box::new(backend.clone()), Some(Box::new(backend.clone())), Some(Box::new(backend)))
        }
        Err(error) => {
            eprintln!("warning: cannot load Patricia dictionary '{}': {error}", path.display());
            (Box::new(FileDictionaryBackend::new()), None, None)
        }
    }
}

#[cfg(not(feature = "patricia"))]
fn build_patricia(
    _def: &ResolvedBackendDef,
    _lang: &str,
) -> (Box<dyn DictionaryBackend>, Option<Box<dyn SpellChecker>>, Option<Box<dyn Predictor>>) {
    eprintln!("warning: patricia feature not enabled");
    (Box::new(FileDictionaryBackend::new()), None, None)
}
