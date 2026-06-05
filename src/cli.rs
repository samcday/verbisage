use std::path::PathBuf;

use clap::Args;

use crate::daemon::BackendKind;
use crate::dictionary::paths::{LanguagePaths, PathOverride, expand_tilde};
use crate::dictionary::{DictionaryBackend, FileDictionaryBackend};
use crate::spellcheck::{DictionarySpellChecker, SpellChecker};

#[cfg(feature = "sqlite")]
use crate::dictionary::SqliteDictionaryBackend;
#[cfg(feature = "hunspell")]
use crate::spellcheck::HunspellSpellChecker;
#[cfg(feature = "sqlite")]
use crate::spellcheck::SqliteSpellChecker;

/// Shared CLI arguments used by both daemon and one-shot binaries.
#[derive(Args, Clone)]
pub struct SharedArgs {
    /// Dictionary backend
    #[arg(long, short, default_value = "file", env = "VERBISAGE_BACKEND")]
    pub backend: BackendKind,

    /// Language tag (e.g. en_US) — used for directory lookup and hunspell.
    #[arg(long, env = "VERBISAGE_LANGUAGE")]
    pub language: Option<String>,

    /// Path to dictionary data (word-list file, sqlite db, …)
    #[arg(long, short)]
    pub path: Option<PathBuf>,

    /// System data directory
    #[arg(long)]
    pub system_data_dir: Option<PathBuf>,

    /// User data directory (~ is expanded)
    #[arg(long)]
    pub user_data_dir: Option<PathBuf>,

    /// System dictionary file override (empty = skip this layer)
    #[arg(long)]
    pub system_dict: Option<String>,

    /// User dictionary file override (empty = skip, ~ expanded)
    #[arg(long)]
    pub user_dict: Option<String>,

    /// Table name for the SQLite backend
    #[arg(long, default_value = "words")]
    pub table: String,

    /// Word column name for the SQLite backend
    #[arg(long, default_value = "word")]
    pub word_col: String,

    /// Frequency column name for the SQLite backend
    #[arg(long, default_value = "frequency")]
    pub freq_col: String,

    /// Path to .aff file; used with --backend hunspell
    #[arg(long)]
    pub affix: Option<PathBuf>,

    /// Path to .dic file; used with --backend hunspell
    #[arg(long)]
    pub dict: Option<PathBuf>,

    /// Use D-Bus transport (server in daemon mode, client in one-shot modes).
    #[arg(long)]
    pub dbus: bool,

    /// Enable verbose debug output to stderr.
    #[arg(long)]
    pub verbose: bool,
}

// ── Eager backend construction (used by one-shot modes) ────────────────────

/// Open a backend and optional spellchecker for the given language.
///
/// Exits the process on construction failure.
pub fn open_backend(
    args: &SharedArgs,
    lang: &str,
) -> (Box<dyn DictionaryBackend>, Option<Box<dyn SpellChecker>>) {
    match &args.backend {
        BackendKind::File => open_file_backend(args, lang),

        #[cfg(feature = "sqlite")]
        BackendKind::Sqlite => open_sqlite_backend(args, lang),

        #[cfg(feature = "hunspell")]
        BackendKind::Hunspell => {
            let checker: Box<dyn SpellChecker> = match (&args.affix, &args.dict) {
                (Some(aff), Some(dic)) => Box::new(
                    HunspellSpellChecker::from_files(aff, dic).unwrap_or_else(|e| {
                        eprintln!("failed to load hunspell .aff/.dic: {}", e);
                        std::process::exit(1);
                    }),
                ),
                _ => Box::new(HunspellSpellChecker::from_tag(lang).unwrap_or_else(|e| {
                    eprintln!("failed to load hunspell dictionary '{}': {}", lang, e);
                    std::process::exit(1);
                })),
            };
            (Box::new(FileDictionaryBackend::new()), Some(checker))
        }
    }
}

fn open_file_backend(
    args: &SharedArgs,
    lang: &str,
) -> (Box<dyn DictionaryBackend>, Option<Box<dyn SpellChecker>>) {
    let files: Vec<PathBuf> = if let Some(path) = &args.path {
        vec![path.clone()]
    } else {
        let mut lp = LanguagePaths::new(lang);
        if let Some(dir) = &args.system_data_dir {
            lp = lp.with_system_dir(expand_tilde(dir.to_str().unwrap_or("")));
        }
        if let Some(dir) = &args.user_data_dir {
            lp = lp.with_user_dir(expand_tilde(dir.to_str().unwrap_or("")));
        }
        lp.system_file_override = PathOverride::from_cli(args.system_dict.as_deref());
        lp.user_file_override = PathOverride::from_cli(args.user_dict.as_deref());

        lp.resolve_dict_files()
    };

    if files.is_empty() {
        return (Box::new(FileDictionaryBackend::new()), None);
    }

    let dict = FileDictionaryBackend::from_multiple_files(&files).unwrap_or_else(|e| {
        eprintln!("failed to load dictionary files: {}", e);
        std::process::exit(1);
    });

    let sc: Box<dyn SpellChecker> = Box::new(DictionarySpellChecker::new(std::sync::Arc::new(
        dict.clone(),
    )));
    (Box::new(dict), Some(sc))
}

#[cfg(feature = "sqlite")]
fn open_sqlite_backend(
    args: &SharedArgs,
    lang: &str,
) -> (Box<dyn DictionaryBackend>, Option<Box<dyn SpellChecker>>) {
    let db_path = if let Some(path) = &args.path {
        path.clone()
    } else {
        let mut lp = LanguagePaths::new(lang);
        if let Some(dir) = &args.system_data_dir {
            lp = lp.with_system_dir(expand_tilde(dir.to_str().unwrap_or("")));
        }
        if let Some(dir) = &args.user_data_dir {
            lp = lp.with_user_dir(expand_tilde(dir.to_str().unwrap_or("")));
        }
        lp.system_file_override = PathOverride::from_cli(args.system_dict.as_deref());
        lp.user_file_override = PathOverride::from_cli(args.user_dict.as_deref());

        match lp.resolve_sqlite_files().first() {
            Some(p) => p.clone(),
            None => return (Box::new(FileDictionaryBackend::new()), None),
        }
    };

    let dict =
        SqliteDictionaryBackend::from_sqlite(&db_path, &args.table, &args.word_col, &args.freq_col)
            .unwrap_or_else(|e| {
                eprintln!(
                    "failed to open sqlite database '{}': {}",
                    db_path.display(),
                    e
                );
                std::process::exit(1);
            });

    let sc: Box<dyn SpellChecker> =
        Box::new(SqliteSpellChecker::new(std::sync::Arc::new(dict.clone())));
    (Box::new(dict), Some(sc))
}

// ── Backend config helper (used by both daemon and one-shot) ───────────────

/// Build a `LanguagePaths` seeded with CLI-provided dirs and overrides (no
/// specific language — the caller sets that later).
pub fn base_language_paths(args: &SharedArgs) -> LanguagePaths {
    let mut lp = LanguagePaths::new("placeholder");
    if let Some(dir) = &args.system_data_dir {
        lp = lp.with_system_dir(expand_tilde(dir.to_str().unwrap_or("")));
    }
    if let Some(dir) = &args.user_data_dir {
        lp = lp.with_user_dir(expand_tilde(dir.to_str().unwrap_or("")));
    }
    lp.system_file_override = PathOverride::from_cli(args.system_dict.as_deref());
    lp.user_file_override = PathOverride::from_cli(args.user_dict.as_deref());
    lp
}
