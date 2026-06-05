use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use verbisage::daemon::{run, DaemonHandler};
use verbisage::dictionary::paths::{expand_tilde, LanguagePaths, PathOverride};
use verbisage::dictionary::{DictionaryQuery, FileDictionaryBackend};
use verbisage::prediction::Predictor;
use verbisage::spellcheck::{DictionarySpellChecker, SpellChecker};

#[cfg(feature = "sqlite")]
use verbisage::dictionary::SqliteDictionaryBackend;
#[cfg(feature = "hunspell")]
use verbisage::spellcheck::HunspellSpellChecker;
#[cfg(feature = "sqlite")]
use verbisage::spellcheck::SqliteSpellChecker;

// ── Enums ─────────────────────────────────────────────────────────────────

#[derive(ValueEnum, Clone)]
enum BackendKind {
    File,
    #[cfg(feature = "sqlite")]
    Sqlite,
    #[cfg(feature = "hunspell")]
    Hunspell,
}

#[derive(ValueEnum, Clone, Default)]
enum Mode {
    #[default]
    Daemon,
    Check,
    Correct,
    Predict,
    Query,
}

// ── CLI definition ────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "verbisaged",
    version,
    about = "Word-list daemon and CLI with spell-check, swipe-typing queries, and next-word prediction"
)]
struct Cli {
    /// Dictionary backend
    #[arg(long, short, default_value = "file", env = "VERBISAGE_BACKEND")]
    backend: BackendKind,

    /// Operation mode
    #[arg(long)]
    mode: Option<Mode>,

    // ── Backend options ──────────────────────────────────────────────────
    /// Path to dictionary data (word-list file, sqlite db, …).
    ///
    /// For the file backend, when this is *not* given, the language-based
    /// lookup via --system-data-dir / --user-data-dir is used instead.
    #[arg(long, short)]
    path: Option<PathBuf>,

    /// Language tag (e.g. en_US) – used for both the file dictionary
    /// directory lookup and the hunspell backend.
    #[arg(long, default_value = "en_US", env = "VERBISAGE_LANGUAGE")]
    language: String,

    /// System data directory (language-specific files are looked up here)
    #[arg(long)]
    system_data_dir: Option<PathBuf>,

    /// User data directory (language-specific files are looked up here;
    /// ~ is expanded)
    #[arg(long)]
    user_data_dir: Option<PathBuf>,

    /// System dictionary file override (empty string = skip this layer)
    #[arg(long)]
    system_dict: Option<String>,

    /// User dictionary file override (empty string = skip this layer;
    /// ~ is expanded)
    #[arg(long)]
    user_dict: Option<String>,

    /// Table name for the SQLite backend
    #[arg(long, default_value = "words")]
    table: String,

    /// Word column name for the SQLite backend
    #[arg(long, default_value = "word")]
    word_col: String,

    /// Frequency column name for the SQLite backend
    #[arg(long, default_value = "frequency")]
    freq_col: String,

    /// Path to .aff file; used with --backend hunspell
    #[arg(long)]
    affix: Option<PathBuf>,

    /// Path to .dic file; used with --backend hunspell
    #[arg(long)]
    dict: Option<PathBuf>,

    /// Language tag for hunspell when --affix/--dict are absent
    #[arg(long, default_value = "en_US")]
    tag: String,

    // ── CLI-mode options ─────────────────────────────────────────────────
    /// Word to check / correct / query against
    #[arg(long)]
    word: Option<String>,

    /// Context words (space-separated); used in predict mode
    #[arg(long)]
    context: Option<String>,

    /// Prefix filter(s); can be repeated
    #[arg(long)]
    prefix: Vec<String>,

    /// Suffix filter(s); can be repeated
    #[arg(long)]
    suffix: Vec<String>,

    /// Minimum word length; used in query mode
    #[arg(long)]
    min_len: Option<usize>,

    /// Maximum word length; used in query mode
    #[arg(long)]
    max_len: Option<usize>,
}

// ── Entry point ───────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();

    let mode = cli.mode.clone().unwrap_or_else(|| {
        if cli.word.is_some() || cli.context.is_some() {
            Mode::Check
        } else {
            Mode::Daemon
        }
    });

    match mode {
        Mode::Daemon => run_daemon(&cli),
        Mode::Check => run_check(&cli),
        Mode::Correct => run_correct(&cli),
        Mode::Predict => run_predict(&cli),
        Mode::Query => run_query(&cli),
    }
}

// ── Backend construction ──────────────────────────────────────────────────

fn open_backend(
    cli: &Cli,
) -> (
    Box<dyn verbisage::dictionary::DictionaryBackend>,
    Option<Box<dyn SpellChecker>>,
) {
    match &cli.backend {
        BackendKind::File => open_file_backend(cli),

        #[cfg(feature = "sqlite")]
        BackendKind::Sqlite => {
            let dict = SqliteDictionaryBackend::from_sqlite(
                &cli.path.as_deref().unwrap_or_else(|| {
                    eprintln!("--path is required for sqlite backend");
                    std::process::exit(1);
                }),
                &cli.table,
                &cli.word_col,
                &cli.freq_col,
            )
            .unwrap_or_else(|e| {
                eprintln!(
                    "failed to open sqlite database '{}': {}",
                    cli.path.as_ref().unwrap().display(),
                    e
                );
                std::process::exit(1);
            });
            let sc: Box<dyn SpellChecker> =
                Box::new(SqliteSpellChecker::new(std::sync::Arc::new(dict.clone())));
            (Box::new(dict), Some(sc))
        }

        #[cfg(feature = "hunspell")]
        BackendKind::Hunspell => {
            let checker: Box<dyn SpellChecker> = match (&cli.affix, &cli.dict) {
                (Some(aff), Some(dic)) => Box::new(
                    HunspellSpellChecker::from_files(aff, dic).unwrap_or_else(|e| {
                        eprintln!("failed to load hunspell .aff/.dic: {}", e);
                        std::process::exit(1);
                    }),
                ),
                _ => Box::new(
                    HunspellSpellChecker::from_tag(&cli.language).unwrap_or_else(|e| {
                        eprintln!(
                            "failed to load hunspell dictionary '{}': {}",
                            cli.language, e
                        );
                        std::process::exit(1);
                    }),
                ),
            };
            (Box::new(FileDictionaryBackend::new()), Some(checker))
        }
    }
}

/// Open the file backend, resolving paths via LanguagePaths.
fn open_file_backend(
    cli: &Cli,
) -> (
    Box<dyn verbisage::dictionary::DictionaryBackend>,
    Option<Box<dyn SpellChecker>>,
) {
    let files: Vec<PathBuf> = if let Some(path) = &cli.path {
        // Explicit --path given: use it directly (backward compat).
        vec![path.clone()]
    } else {
        // Build LanguagePaths from the CLI options.
        let mut lp = LanguagePaths::new(&cli.language);
        if let Some(dir) = &cli.system_data_dir {
            lp = lp.with_system_dir(expand_tilde(dir.to_str().unwrap_or("")));
        }
        if let Some(dir) = &cli.user_data_dir {
            lp = lp.with_user_dir(expand_tilde(dir.to_str().unwrap_or("")));
        }
        lp.system_file_override = PathOverride::from_cli(cli.system_dict.as_deref());
        lp.user_file_override = PathOverride::from_cli(cli.user_dict.as_deref());

        let found = lp.resolve_dict_files();
        if found.is_empty() {
            eprintln!("no dictionary files found for language '{}'", cli.language);
            eprintln!("  looked in:");
            eprintln!("    system: {}", lp.system_dir.display());
            eprintln!("    user:   {}", lp.user_dir.display());
            std::process::exit(1);
        }
        found
    };

    let dict = FileDictionaryBackend::from_multiple_files(&files).unwrap_or_else(|e| {
        eprintln!("failed to load dictionary files: {}", e);
        std::process::exit(1);
    });

    let sc: Box<dyn SpellChecker> = Box::new(DictionarySpellChecker::new(std::sync::Arc::new(
        dict.clone(),
    )));
    (Box::new(dict), Some(sc))
}

// ── Mode dispatchers ──────────────────────────────────────────────────────

fn run_daemon(cli: &Cli) {
    let (dict, sc) = open_backend(cli);
    let handler = DaemonHandler::new(dict, sc, None as Option<Box<dyn Predictor>>);
    run(handler);
}

fn run_check(cli: &Cli) {
    let word = cli
        .word
        .as_deref()
        .or_else(|| cli.context.as_deref())
        .unwrap_or_else(|| {
            eprintln!("usage: verbisaged --mode check --word <word>");
            std::process::exit(1);
        });
    let (_, sc) = open_backend(cli);

    let correct = sc.as_ref().map(|s| s.is_correct(word)).unwrap_or(false);

    if correct {
        println!("true");
    } else {
        println!("false");
        std::process::exit(1);
    }
}

fn run_correct(cli: &Cli) {
    let word = cli.word.as_deref().unwrap_or_else(|| {
        eprintln!("usage: verbisaged --mode correct --word <word>");
        std::process::exit(1);
    });
    let (_, sc) = open_backend(cli);

    let suggestions = sc.as_ref().map(|s| s.suggest(word)).unwrap_or_default();

    for s in &suggestions {
        println!("{}", s);
    }
    if suggestions.is_empty() {
        std::process::exit(1);
    }
}

fn run_predict(cli: &Cli) {
    let context = cli.context.as_deref().unwrap_or_else(|| {
        eprintln!("usage: verbisaged --mode predict --context <words...>");
        std::process::exit(1);
    });
    let _ctx: Vec<&str> = context.split_whitespace().collect();
    eprintln!("predict mode requires a Predictor backend (not yet wired)");
    std::process::exit(1);
}

fn run_query(cli: &Cli) {
    let prefixes = if cli.prefix.is_empty() {
        vec![None]
    } else {
        cli.prefix.iter().map(|p| Some(p.clone())).collect()
    };
    let suffixes = if cli.suffix.is_empty() {
        vec![None]
    } else {
        cli.suffix.iter().map(|s| Some(s.clone())).collect()
    };

    let queries: Vec<DictionaryQuery> = prefixes
        .iter()
        .flat_map(|p| {
            suffixes.iter().map(move |s| DictionaryQuery {
                prefix: p.clone(),
                suffix: s.clone(),
                min_length: cli.min_len,
                max_length: cli.max_len,
            })
        })
        .collect();

    let (dict, _) = open_backend(cli);
    let results = dict.query_prefixes(&queries);

    for r in &results {
        println!("{}  {}", r.word, r.confidence);
    }
    if results.is_empty() {
        std::process::exit(1);
    }
}
