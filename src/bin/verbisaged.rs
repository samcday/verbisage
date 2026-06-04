use std::path::PathBuf;

use clap::Parser;

use verbisage::daemon::{run, DaemonHandler};
use verbisage::dictionary::FileDictionaryBackend;
use verbisage::prediction::Predictor;
use verbisage::spellcheck::{DictionarySpellChecker, SpellChecker};

#[cfg(feature = "sqlite")]
use verbisage::dictionary::SqliteDictionaryBackend;
#[cfg(feature = "hunspell")]
use verbisage::spellcheck::HunspellSpellChecker;
#[cfg(feature = "sqlite")]
use verbisage::spellcheck::SqliteSpellChecker;

#[derive(Parser)]
#[command(
    name = "verbisaged",
    version,
    about = "Word-list daemon with spell-check, swipe-typing queries, and next-word prediction"
)]
struct Cli {
    /// Dictionary backend to use
    #[arg(long, short, default_value = "file", env = "VERBISAGE_BACKEND")]
    backend: String,

    // ── File backend ──────────────────────────────────────────────────────
    /// Path to word list (one word per line); used with --backend file
    #[arg(long, short, default_value = "/usr/share/dict/words")]
    path: PathBuf,

    // ── SQLite backend ────────────────────────────────────────────────────
    /// Table name for the SQLite backend
    #[arg(long, default_value = "words")]
    table: String,

    /// Word column name for the SQLite backend
    #[arg(long, default_value = "word")]
    word_col: String,

    /// Frequency column name for the SQLite backend
    #[arg(long, default_value = "frequency")]
    freq_col: String,

    // ── Hunspell backend ──────────────────────────────────────────────────
    /// Path to .aff file; used with --backend hunspell
    #[arg(long)]
    affix: Option<PathBuf>,

    /// Path to .dic file; used with --backend hunspell
    #[arg(long)]
    dict: Option<PathBuf>,

    /// Language tag (e.g. en_US); used with --backend hunspell when --affix/--dict are absent
    #[arg(long, default_value = "en_US")]
    tag: String,
}

fn main() {
    let cli = Cli::parse();

    match cli.backend.as_str() {
        "file" => start_file(&cli),
        "sqlite" => start_sqlite(&cli),
        "hunspell" => start_hunspell(&cli),
        other => {
            eprintln!("[verbisaged] unknown backend '{}'", other);
            eprintln!("  valid backends: file, sqlite, hunspell");
            std::process::exit(1);
        }
    }
}

fn start_file(cli: &Cli) {
    let dict = match FileDictionaryBackend::from_word_list(&cli.path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "[verbisaged] failed to load word list '{}': {}",
                cli.path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    let sc: Option<Box<dyn SpellChecker>> = Some(Box::new(DictionarySpellChecker::new(
        std::sync::Arc::new(dict.clone()),
    )));

    let handler = DaemonHandler::new(Box::new(dict), sc, None as Option<Box<dyn Predictor>>);
    run(handler);
}

#[cfg(feature = "sqlite")]
fn start_sqlite(cli: &Cli) {
    let dict = match SqliteDictionaryBackend::from_sqlite(
        &cli.path,
        &cli.table,
        &cli.word_col,
        &cli.freq_col,
    ) {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "[verbisaged] failed to open sqlite database '{}': {}",
                cli.path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    let sc: Option<Box<dyn SpellChecker>> = Some(Box::new(SqliteSpellChecker::new(
        std::sync::Arc::new(dict.clone()),
    )));

    let handler = DaemonHandler::new(Box::new(dict), sc, None as Option<Box<dyn Predictor>>);
    run(handler);
}

#[cfg(not(feature = "sqlite"))]
fn start_sqlite(_cli: &Cli) {
    eprintln!("[verbisaged] sqlite support not compiled in (enable feature 'sqlite')");
    std::process::exit(1);
}

#[cfg(feature = "hunspell")]
fn start_hunspell(cli: &Cli) {
    let checker: Box<dyn SpellChecker> = match (&cli.affix, &cli.dict) {
        (Some(aff), Some(dic)) => Box::new(
            HunspellSpellChecker::from_files(aff, dic).unwrap_or_else(|e| {
                eprintln!("[verbisaged] failed to load hunspell .aff/.dic: {}", e);
                std::process::exit(1);
            }),
        ),
        _ => Box::new(
            HunspellSpellChecker::from_tag(&cli.tag).unwrap_or_else(|e| {
                eprintln!(
                    "[verbisaged] failed to load hunspell dictionary '{}': {}",
                    cli.tag, e
                );
                std::process::exit(1);
            }),
        ),
    };

    let handler = DaemonHandler::new(
        Box::new(FileDictionaryBackend::new()),
        Some(checker),
        None as Option<Box<dyn Predictor>>,
    );
    run(handler);
}

#[cfg(not(feature = "hunspell"))]
fn start_hunspell(_cli: &Cli) {
    eprintln!("[verbisaged] hunspell support not compiled in (enable feature 'hunspell')");
    std::process::exit(1);
}
