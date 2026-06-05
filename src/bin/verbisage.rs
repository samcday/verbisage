use std::collections::HashMap;

use clap::{Parser, ValueEnum};

use verbisage::backends::BackendDef;
use verbisage::cli::{ClientMode, SharedArgs, open_backend};
use verbisage::config::{default_config_path, load_config};
use verbisage::debug;
use verbisage::dictionary::paths::expand_tilde;
use verbisage::dictionary::{DictionaryQuery, DictionaryResult};

#[cfg(feature = "dbus")]
use verbisage::clients::DbusClient;

// ── Enums ─────────────────────────────────────────────────────────────────

#[derive(ValueEnum, Clone, Default)]
enum Mode {
    #[default]
    Check,
    Correct,
    Predict,
    Query,
    WordAdd,
    NgramBump,
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
    shared: SharedArgs,

    /// Operation mode (positional)
    mode: Option<Mode>,

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

    /// Frequency for word-add mode
    #[arg(long, default_value_t = 1.0)]
    frequency: f64,

    /// Whether to allow overwriting existing words (word-add mode)
    #[arg(long, default_value_t = true)]
    allow_existing: bool,

    /// Delta for ngram-bump mode
    #[arg(long, default_value_t = 1.0)]
    delta: f64,

    /// Whether to save unknown n-grams (ngram-bump mode)
    #[arg(long, default_value_t = true)]
    save_unknown: bool,
}

// ── Entry point ───────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();
    debug::set_verbose(cli.shared.verbose);

    // Resolve config path: CLI override, then default.
    let config_path = cli
        .shared
        .config
        .clone()
        .map(|p| expand_tilde(p.to_str().unwrap_or("")))
        .or_else(|| Some(default_config_path()));
    let config = config_path.as_ref().and_then(load_config);

    // Capture CLI --mode before apply_defaults consumes cli.shared.
    let cli_mode = cli.shared.mode;

    // Merge config into CLI and apply defaults.
    let shared = cli.shared.clone().apply_defaults(config.as_ref());
    let named_backends = config.as_ref().and_then(|c| c.backends.as_ref());

    let operation = cli.mode.clone().unwrap_or_else(|| {
        if cli.word.is_some() || cli.context.is_some() {
            Mode::Check
        } else {
            eprintln!("usage: verbisage <check|correct|predict|query> [options]");
            std::process::exit(1);
        }
    });

    let client_mode = SharedArgs::resolve_mode(
        cli_mode,
        config
            .as_ref()
            .and_then(|c| c.client.as_ref())
            .and_then(|cl| cl.mode.as_deref()),
        ClientMode::Standalone,
    );

    match operation {
        Mode::Check => run_check(&cli, &shared, named_backends, client_mode),
        Mode::Correct => run_correct(&cli, &shared, named_backends, client_mode),
        Mode::Predict => run_predict(&cli, &shared, named_backends, client_mode),
        Mode::Query => run_query(&cli, &shared, named_backends, client_mode),
        Mode::WordAdd => run_word_add(&cli, &shared, named_backends, client_mode),
        Mode::NgramBump => run_ngram_bump(&cli, &shared, client_mode),
    }
}

// ── Mode dispatchers ───────────────────────────────────────────────────────

fn run_check(
    cli: &Cli,
    shared: &SharedArgs,
    named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
) {
    let word = cli
        .word
        .as_deref()
        .or(cli.context.as_deref())
        .unwrap_or_else(|| {
            eprintln!("usage: verbisage check --word <word>");
            std::process::exit(1);
        });

    let lang = shared.lang();
    let correct = if client_mode == ClientMode::Dbus {
        #[cfg(feature = "dbus")]
        {
            match DbusClient::new() {
                Ok(client) => match client.is_correct(word, lang) {
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
        {
            let _ = (word, lang);
            eprintln!("dbus feature not enabled; rebuild with --features dbus");
            std::process::exit(1);
        }
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
    cli: &Cli,
    shared: &SharedArgs,
    named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
) {
    let word = cli.word.as_deref().unwrap_or_else(|| {
        eprintln!("usage: verbisage correct --word <word>");
        std::process::exit(1);
    });

    let lang = shared.lang();
    let suggestions: Vec<String> = if client_mode == ClientMode::Dbus {
        #[cfg(feature = "dbus")]
        {
            match DbusClient::new() {
                Ok(client) => match client.suggest(word, 10, lang) {
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
        {
            let _ = (word, lang);
            eprintln!("dbus feature not enabled; rebuild with --features dbus");
            std::process::exit(1);
        }
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
    cli: &Cli,
    shared: &SharedArgs,
    _named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
) {
    let context: Vec<String> = cli
        .context
        .as_deref()
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default();

    let max = 10;
    let lang = shared.lang();

    let predictions: Vec<(String, f64)> = if client_mode == ClientMode::Dbus {
        #[cfg(feature = "dbus")]
        {
            match DbusClient::new() {
                Ok(client) => match client.predict(context, max, lang) {
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
        {
            let _ = (context, max, lang);
            eprintln!("dbus feature not enabled; rebuild with --features dbus");
            std::process::exit(1);
        }
    } else {
        eprintln!("warning: predict mode needs a Predictor backend (see --dbus)");
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
    cli: &Cli,
    shared: &SharedArgs,
    named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
) {
    let min = cli.min_len.unwrap_or(0);
    let max = cli.max_len.unwrap_or(0);
    let lang = shared.lang();

    let results: Vec<(String, f64)> = if client_mode == ClientMode::Dbus {
        #[cfg(feature = "dbus")]
        {
            match DbusClient::new() {
                Ok(client) => {
                    match client.query(&cli.prefix, &cli.suffix, min as u32, max as u32, lang) {
                        Ok(v) => v,
                        Err(e) => {
                            eprintln!("dbus call failed: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("dbus connection failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        #[cfg(not(feature = "dbus"))]
        {
            let _ = (min, max, lang);
            eprintln!("dbus feature not enabled; rebuild with --features dbus");
            std::process::exit(1);
        }
    } else {
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

        let (dict, sc) = open_backend(shared, lang, named_backends);
        let results: Vec<DictionaryResult> = dict.query_prefixes(&queries);
        if results.is_empty() && sc.is_none() {
            eprintln!("warning: no dictionary loaded for '{}'", lang);
        }
        results
            .into_iter()
            .map(|r| (r.word, r.confidence))
            .collect()
    };

    for (word, confidence) in &results {
        println!("{}  {}", word, confidence);
    }
    if results.is_empty() {
        std::process::exit(1);
    }
}

fn run_word_add(
    cli: &Cli,
    shared: &SharedArgs,
    named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
) {
    let word = cli.word.as_deref().unwrap_or_else(|| {
        eprintln!("usage: verbisage word-add --word <word>");
        std::process::exit(1);
    });

    let lang = shared.lang();

    if client_mode == ClientMode::Dbus {
        #[cfg(feature = "dbus")]
        {
            match DbusClient::new() {
                Ok(client) => {
                    match client.add_word(word, cli.frequency, cli.allow_existing, lang) {
                        Ok(_) => println!("true"),
                        Err(e) => {
                            eprintln!("dbus call failed: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("dbus connection failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        #[cfg(not(feature = "dbus"))]
        {
            let _ = (word, lang);
            eprintln!("dbus feature not enabled; rebuild with --features dbus");
            std::process::exit(1);
        }
    } else {
        let (dict, _sc) = open_backend(shared, lang, named_backends);
        match dict.add_word(word, cli.frequency, cli.allow_existing) {
            Ok(()) => println!("true"),
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
    }
}

fn run_ngram_bump(cli: &Cli, shared: &SharedArgs, client_mode: ClientMode) {
    let ngram: Vec<String> = cli
        .context
        .as_deref()
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_else(|| {
            eprintln!("usage: verbisage ngram-bump --context \"w1 w2 w3\"");
            std::process::exit(1);
        });

    let lang = shared.lang();

    if client_mode == ClientMode::Dbus {
        #[cfg(feature = "dbus")]
        {
            match DbusClient::new() {
                Ok(client) => match client.bump_ngram(ngram, cli.delta, cli.save_unknown, lang) {
                    Ok(_) => println!("true"),
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
        {
            let _ = (ngram, lang);
            eprintln!("dbus feature not enabled; rebuild with --features dbus");
            std::process::exit(1);
        }
    } else {
        eprintln!("warning: ngram-bump requires dbus mode for predictor access");
        std::process::exit(1);
    }
}
