use std::env;
use std::path::PathBuf;

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

fn main() {
    let args: Vec<String> = env::args().collect();
    let backend = parse_arg(&args, "--backend")
        .or_else(|| env::var("VERBISAGE_BACKEND").ok())
        .unwrap_or_else(|| "file".to_string());

    match backend.as_str() {
        "file" => start_file(&args),
        "sqlite" => start_sqlite(&args),
        "hunspell" => start_hunspell(&args),
        other => {
            eprintln!("[verbisaged] unknown backend '{}'", other);
            eprintln!("Usage: verbisaged --backend (file|sqlite|hunspell) [options]");
            std::process::exit(1);
        }
    }
}

fn start_file(args: &[String]) {
    let path = parse_arg(args, "--path").unwrap_or_else(|| "/usr/share/dict/words".to_string());

    let dict = match FileDictionaryBackend::from_word_list(PathBuf::from(&path)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[verbisaged] failed to load word list '{}': {}", path, e);
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
fn start_sqlite(args: &[String]) {
    let path = parse_arg(args, "--path").unwrap_or_else(|| "dictionary.db".to_string());
    let table = parse_arg(args, "--table").unwrap_or_else(|| "words".to_string());
    let word_col = parse_arg(args, "--word-col").unwrap_or_else(|| "word".to_string());
    let freq_col = parse_arg(args, "--freq-col").unwrap_or_else(|| "frequency".to_string());

    let dict = match SqliteDictionaryBackend::from_sqlite(
        PathBuf::from(&path),
        &table,
        &word_col,
        &freq_col,
    ) {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "[verbisaged] failed to open sqlite database '{}': {}",
                path, e
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
fn start_sqlite(_args: &[String]) {
    eprintln!("[verbisaged] sqlite support not compiled in (enable feature 'sqlite')");
    std::process::exit(1);
}

#[cfg(feature = "hunspell")]
fn start_hunspell(args: &[String]) {
    let aff = parse_arg(args, "--affix").unwrap_or_else(|| "".to_string());
    let dic = parse_arg(args, "--dict").unwrap_or_else(|| "".to_string());

    let checker: Box<dyn SpellChecker> = if !aff.is_empty() && !dic.is_empty() {
        Box::new(
            HunspellSpellChecker::from_files(PathBuf::from(&aff), PathBuf::from(&dic))
                .unwrap_or_else(|e| {
                    eprintln!("[verbisaged] failed to load hunspell .aff/.dic: {}", e);
                    std::process::exit(1);
                }),
        )
    } else {
        let tag = parse_arg(args, "--tag").unwrap_or_else(|| "en_US".to_string());
        Box::new(HunspellSpellChecker::from_tag(&tag).unwrap_or_else(|e| {
            eprintln!(
                "[verbisaged] failed to load hunspell dictionary '{}': {}",
                tag, e
            );
            std::process::exit(1);
        }))
    };

    let handler = DaemonHandler::new(
        // For hunspell mode we don't have a DictionaryBackend, so use an
        // empty file backend as a fallback.
        Box::new(FileDictionaryBackend::new()),
        Some(checker),
        None as Option<Box<dyn Predictor>>,
    );
    run(handler);
}

#[cfg(not(feature = "hunspell"))]
fn start_hunspell(_args: &[String]) {
    eprintln!("[verbisaged] hunspell support not compiled in (enable feature 'hunspell')");
    std::process::exit(1);
}

/// Simple argument parser: returns the value after `--name` or `None`.
fn parse_arg(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find_map(|w| {
        if w[0] == name {
            Some(w[1].clone())
        } else {
            None
        }
    })
}
