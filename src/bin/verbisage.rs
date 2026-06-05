use std::collections::HashMap;

use clap::{Parser, Subcommand};

use verbisage::backends::BackendDef;
use verbisage::cli::{ClientMode, SharedArgs, open_backend};
use verbisage::config::{default_config_path, load_config};
use verbisage::debug;
use verbisage::dictionary::paths::expand_tilde;
use verbisage::dictionary::{DictionaryQuery, DictionaryResult};

#[cfg(feature = "dbus")]
use verbisage::clients::DbusClient;

// ── Shared args ────────────────────────────────────────────────────────────

#[derive(clap::Args, Clone)]
struct GlobalArgs {
    #[command(flatten)]
    shared: SharedArgs,
}

// ── Subcommands ────────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum Command {
    /// Check whether a word is spelled correctly.
    Check {
        /// Word to check.
        #[arg(long)]
        word: String,
    },

    /// Suggest corrections for a misspelled word.
    Correct {
        /// Misspelled word to correct.
        #[arg(long)]
        word: String,
    },

    /// Predict the next word given a context.
    Predict {
        /// Context words (space-separated).
        #[arg(long)]
        context: Option<String>,
        /// Maximum number of suggestions.
        #[arg(long, default_value_t = 10)]
        max: usize,
    },

    /// Query the dictionary by prefix/suffix.
    Query {
        /// Prefix filter(s); can be repeated.
        #[arg(long)]
        prefix: Vec<String>,
        /// Suffix filter(s); can be repeated.
        #[arg(long)]
        suffix: Vec<String>,
        /// Minimum word length.
        #[arg(long)]
        min_len: Option<usize>,
        /// Maximum word length.
        #[arg(long)]
        max_len: Option<usize>,
    },

    /// Add a word to the user dictionary.
    WordAdd {
        /// Word to add.
        #[arg(long)]
        word: String,
        /// Frequency value.
        #[arg(long, default_value_t = 1.0)]
        frequency: f64,
        /// Allow overwriting existing words.
        #[arg(long, default_value_t = true)]
        allow_existing: bool,
    },

    /// Bump the n-gram frequency for a sequence of words.
    NgramBump {
        /// N-gram words (space-separated).
        #[arg(long)]
        context: String,
        /// Frequency delta.
        #[arg(long, default_value_t = 1.0)]
        delta: f64,
        /// Save unknown n-grams.
        #[arg(long, default_value_t = true)]
        save_unknown: bool,
    },
}

// ── CLI definition ─────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "verbisage",
    version,
    about = "One-shot word-list CLI — check, correct, predict, query"
)]
struct Cli {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: Command,
}

// ── Entry point ────────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();
    debug::set_verbose(cli.global.shared.verbose);

    // Resolve config path: CLI override, then default.
    let config_path = cli
        .global
        .shared
        .config
        .clone()
        .map(|p| expand_tilde(p.to_str().unwrap_or("")))
        .or_else(|| Some(default_config_path()));
    let config = config_path.as_ref().and_then(load_config);

    let cli_mode = cli.global.shared.mode;
    let shared = cli.global.shared.clone().apply_defaults(config.as_ref());
    let named_backends = config.as_ref().and_then(|c| c.backends.as_ref());

    let client_mode = SharedArgs::resolve_mode(
        cli_mode,
        config
            .as_ref()
            .and_then(|c| c.client.as_ref())
            .and_then(|cl| cl.mode.as_deref()),
        ClientMode::Standalone,
    );

    match cli.command {
        Command::Check { word } => run_check(&shared, named_backends, client_mode, &word),
        Command::Correct { word } => run_correct(&shared, named_backends, client_mode, &word),
        Command::Predict { context, max } => run_predict(
            &shared,
            named_backends,
            client_mode,
            context.as_deref(),
            max,
        ),
        Command::Query {
            prefix,
            suffix,
            min_len,
            max_len,
        } => run_query(
            &shared,
            named_backends,
            client_mode,
            &prefix,
            &suffix,
            min_len,
            max_len,
        ),
        Command::WordAdd {
            word,
            frequency,
            allow_existing,
        } => run_word_add(
            &shared,
            named_backends,
            client_mode,
            &word,
            frequency,
            allow_existing,
        ),
        Command::NgramBump {
            context,
            delta,
            save_unknown,
        } => run_ngram_bump(&shared, client_mode, &context, delta, save_unknown),
    }
}

// ── Mode dispatchers ───────────────────────────────────────────────────────

fn run_check(
    shared: &SharedArgs,
    named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
    word: &str,
) {
    let lang = shared.lang();
    let correct = if client_mode == ClientMode::Dbus {
        dbus_is_correct(word, lang)
    } else {
        let (_, sc) = open_backend(shared, lang, named_backends);
        match &sc {
            Some(s) => s.is_correct(word),
            None => {
                eprintln!("warning: no dictionary loaded for '{}'", lang);
                false
            }
        }
    };

    if correct {
        println!("true");
    } else {
        println!("false");
        std::process::exit(1);
    }
}

fn run_correct(
    shared: &SharedArgs,
    named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
    word: &str,
) {
    let lang = shared.lang();
    let suggestions: Vec<String> = if client_mode == ClientMode::Dbus {
        dbus_suggest(word, 10, lang)
    } else {
        let (_, sc) = open_backend(shared, lang, named_backends);
        match &sc {
            Some(s) => s.suggest(word),
            None => {
                eprintln!("warning: no dictionary loaded for '{}'", lang);
                Vec::new()
            }
        }
    };

    for s in &suggestions {
        println!("{}", s);
    }
    if suggestions.is_empty() {
        std::process::exit(1);
    }
}

fn run_predict(
    shared: &SharedArgs,
    _named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
    context: Option<&str>,
    max: usize,
) {
    let context_words: Vec<String> = context
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default();

    let lang = shared.lang();
    let predictions: Vec<(String, f64)> = if client_mode == ClientMode::Dbus {
        dbus_predict(context_words, max, lang)
    } else {
        eprintln!("warning: predict mode needs a Predictor backend (see --mode dbus)");
        Vec::new()
    };

    for (word, confidence) in &predictions {
        println!("{}  {}", word, confidence);
    }
    if predictions.is_empty() {
        std::process::exit(1);
    }
}

fn run_query(
    shared: &SharedArgs,
    named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
    prefix: &[String],
    suffix: &[String],
    min_len: Option<usize>,
    max_len: Option<usize>,
) {
    let lang = shared.lang();

    let results: Vec<DictionaryResult> = if client_mode == ClientMode::Dbus {
        let dbus_results = dbus_query(
            prefix,
            suffix,
            min_len.unwrap_or(0),
            max_len.unwrap_or(0),
            lang,
        );
        dbus_results
            .into_iter()
            .map(|(w, c)| DictionaryResult {
                word: w,
                confidence: c,
            })
            .collect()
    } else {
        let prefixes = if prefix.is_empty() {
            vec![None]
        } else {
            prefix.iter().map(|p| Some(p.clone())).collect()
        };
        let suffixes = if suffix.is_empty() {
            vec![None]
        } else {
            suffix.iter().map(|s| Some(s.clone())).collect()
        };

        let queries: Vec<DictionaryQuery> = prefixes
            .iter()
            .flat_map(|p| {
                suffixes.iter().map(move |s| DictionaryQuery {
                    prefix: p.clone(),
                    suffix: s.clone(),
                    min_length: min_len,
                    max_length: max_len,
                })
            })
            .collect();

        let (dict, sc) = open_backend(shared, lang, named_backends);
        let results = dict.query_prefixes(&queries);
        if results.is_empty() && sc.is_none() {
            eprintln!("warning: no dictionary loaded for '{}'", lang);
        }
        results
    };

    for r in &results {
        println!("{}  {}", r.word, r.confidence);
    }
    if results.is_empty() {
        std::process::exit(1);
    }
}

fn run_word_add(
    shared: &SharedArgs,
    named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
    word: &str,
    frequency: f64,
    allow_existing: bool,
) {
    let lang = shared.lang();

    if client_mode == ClientMode::Dbus {
        dbus_add_word(word, frequency, allow_existing, lang);
    } else {
        let (dict, _sc) = open_backend(shared, lang, named_backends);
        match dict.add_word(word, frequency, allow_existing) {
            Ok(()) => println!("true"),
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
    }
}

fn run_ngram_bump(
    _shared: &SharedArgs,
    client_mode: ClientMode,
    context: &str,
    delta: f64,
    save_unknown: bool,
) {
    let ngram: Vec<String> = context.split_whitespace().map(String::from).collect();
    let lang = _shared.lang();

    if client_mode == ClientMode::Dbus {
        dbus_bump_ngram(ngram, delta, save_unknown, lang);
    } else {
        eprintln!("warning: ngram-bump requires dbus mode for predictor access");
        std::process::exit(1);
    }
}

// ── D-Bus helpers ──────────────────────────────────────────────────────────

#[cfg(feature = "dbus")]
fn dbus_call<F, R>(f: F) -> R
where
    F: FnOnce(&DbusClient) -> Result<R, Box<dyn std::error::Error>>,
{
    match DbusClient::new() {
        Ok(client) => match f(&client) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("dbus call failed: {}", e);
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("dbus connection failed: {}", e);
            std::process::exit(1);
        }
    }
}

#[cfg(not(feature = "dbus"))]
fn dbus_call<F, R>(_f: F) -> R
where
    F: FnOnce() -> Result<R, Box<dyn std::error::Error>>,
{
    eprintln!("dbus feature not enabled; rebuild with --features dbus");
    std::process::exit(1);
}

#[cfg(feature = "dbus")]
fn dbus_is_correct(word: &str, lang: &str) -> bool {
    dbus_call(|client| Ok(client.is_correct(word, lang)?))
}

#[cfg(not(feature = "dbus"))]
fn dbus_is_correct(_word: &str, _lang: &str) -> bool {
    dbus_call(|| Err("dbus not enabled".into()))
}

#[cfg(feature = "dbus")]
fn dbus_suggest(word: &str, max: usize, lang: &str) -> Vec<String> {
    dbus_call(|client| Ok(client.suggest(word, max as u32, lang)?))
}

#[cfg(not(feature = "dbus"))]
fn dbus_suggest(_word: &str, _max: usize, _lang: &str) -> Vec<String> {
    dbus_call(|| Err("dbus not enabled".into()))
}

#[cfg(feature = "dbus")]
fn dbus_predict(context: Vec<String>, max: usize, lang: &str) -> Vec<(String, f64)> {
    dbus_call(|client| Ok(client.predict(context, max as u32, lang)?))
}

#[cfg(not(feature = "dbus"))]
fn dbus_predict(_context: Vec<String>, _max: usize, _lang: &str) -> Vec<(String, f64)> {
    dbus_call(|| Err("dbus not enabled".into()))
}

#[cfg(feature = "dbus")]
fn dbus_query(
    prefix: &[String],
    suffix: &[String],
    min_len: usize,
    max_len: usize,
    lang: &str,
) -> Vec<(String, f64)> {
    dbus_call(|client| Ok(client.query(prefix, suffix, min_len as u32, max_len as u32, lang)?))
}

#[cfg(not(feature = "dbus"))]
fn dbus_query(
    _prefix: &[String],
    _suffix: &[String],
    _min_len: usize,
    _max_len: usize,
    _lang: &str,
) -> Vec<(String, f64)> {
    dbus_call(|| Err("dbus not enabled".into()))
}

#[cfg(feature = "dbus")]
fn dbus_add_word(word: &str, frequency: f64, allow_existing: bool, lang: &str) {
    match DbusClient::new() {
        Ok(client) => match client.add_word(word, frequency, allow_existing, lang) {
            Ok(true) => println!("true"),
            Ok(false) => {
                eprintln!("word add failed");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("dbus call failed: {}", e);
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("dbus connection failed: {}", e);
            std::process::exit(1);
        }
    }
}

#[cfg(not(feature = "dbus"))]
fn dbus_add_word(_word: &str, _frequency: f64, _allow_existing: bool, _lang: &str) {
    dbus_call(|| Err("dbus not enabled".into()))
}

#[cfg(feature = "dbus")]
fn dbus_bump_ngram(ngram: Vec<String>, delta: f64, save_unknown: bool, lang: &str) {
    match DbusClient::new() {
        Ok(client) => match client.bump_ngram(ngram, delta, save_unknown, lang) {
            Ok(true) => println!("true"),
            Ok(false) => {
                eprintln!("ngram bump failed");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("dbus call failed: {}", e);
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("dbus connection failed: {}", e);
            std::process::exit(1);
        }
    }
}

#[cfg(not(feature = "dbus"))]
fn dbus_bump_ngram(_ngram: Vec<String>, _delta: f64, _save_unknown: bool, _lang: &str) {
    dbus_call(|| Err("dbus not enabled".into()))
}
