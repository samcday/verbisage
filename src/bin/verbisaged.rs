use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use verbisage::daemon::{run, DaemonHandler};
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
    /// Line-delimited JSON protocol on stdin/stdout (full API)
    #[default]
    Daemon,
    /// Check whether a word is in the dictionary (exit 0 = yes, 1 = no)
    Check,
    /// Print spelling suggestions, one per line
    Correct,
    /// Predict the next word given a space-separated context
    Predict,
    /// Query the dictionary with prefix / suffix / length constraints
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
    /// Path to dictionary data (word-list file, sqlite db, …); used with
    /// --backend file | sqlite
    #[arg(long, short, default_value = "/usr/share/dict/words")]
    path: PathBuf,

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

    /// Language tag (e.g. en_US); used with --backend hunspell when
    /// --affix/--dict are absent
    #[arg(long, default_value = "en_US")]
    tag: String,

    // ── CLI-mode options ─────────────────────────────────────────────────
    /// Word to check / correct / query against (used in check, correct, query modes)
    #[arg(long)]
    word: Option<String>,

    /// Context words (space-separated); used in predict mode
    #[arg(long)]
    context: Option<String>,

    /// Prefix filter; used in query mode
    #[arg(long)]
    prefix: Option<String>,

    /// Suffix filter; used in query mode
    #[arg(long)]
    suffix: Option<String>,

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
        // When no --mode is given, infer from presence of other args:
        // if --word or --context is present, default to check; otherwise daemon.
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
        BackendKind::File => {
            let dict = FileDictionaryBackend::from_word_list(&cli.path).unwrap_or_else(|e| {
                eprintln!("failed to load word list '{}': {}", cli.path.display(), e);
                std::process::exit(1);
            });
            let sc: Box<dyn SpellChecker> = Box::new(DictionarySpellChecker::new(
                std::sync::Arc::new(dict.clone()),
            ));
            (Box::new(dict), Some(sc))
        }

        #[cfg(feature = "sqlite")]
        BackendKind::Sqlite => {
            let dict = SqliteDictionaryBackend::from_sqlite(
                &cli.path,
                &cli.table,
                &cli.word_col,
                &cli.freq_col,
            )
            .unwrap_or_else(|e| {
                eprintln!(
                    "failed to open sqlite database '{}': {}",
                    cli.path.display(),
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
                    HunspellSpellChecker::from_tag(&cli.tag).unwrap_or_else(|e| {
                        eprintln!("failed to load hunspell dictionary '{}': {}", cli.tag, e);
                        std::process::exit(1);
                    }),
                ),
            };
            (Box::new(FileDictionaryBackend::new()), Some(checker))
        }
    }
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
    // Predictor isn't wired through the daemon handler yet — placeholders.
    let ctx: Vec<&str> = context.split_whitespace().collect();
    let _ = ctx;

    // TODO: instantiate a Predictor from the backend and call
    //       predict_next().  For now emit a stub message.
    eprintln!("predict mode requires a Predictor backend (not yet wired)");
    std::process::exit(1);
}

fn run_query(cli: &Cli) {
    let query = DictionaryQuery {
        prefix: cli.prefix.clone(),
        suffix: cli.suffix.clone(),
        min_length: cli.min_len,
        max_length: cli.max_len,
    };

    let (dict, _) = open_backend(cli);
    let results = dict.query_prefixes(&[query]);

    for r in &results {
        println!("{}  {}", r.word, r.confidence);
    }
    if results.is_empty() {
        std::process::exit(1);
    }
}
