use clap::{Parser, ValueEnum};

use verbisage::cli::{SharedArgs, open_backend};
use verbisage::debug;
use verbisage::dictionary::DictionaryQuery;

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
}

// ── Entry point ───────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();
    debug::set_verbose(cli.shared.verbose);

    let mode = cli.mode.clone().unwrap_or_else(|| {
        if cli.word.is_some() || cli.context.is_some() {
            Mode::Check
        } else {
            eprintln!("usage: verbisage <check|correct|predict|query> [options]");
            std::process::exit(1);
        }
    });

    match mode {
        Mode::Check => run_check(&cli),
        Mode::Correct => run_correct(&cli),
        Mode::Predict => run_predict(&cli),
        Mode::Query => run_query(&cli),
    }
}

// ── Mode dispatchers ───────────────────────────────────────────────────────

fn run_check(cli: &Cli) {
    let word = cli
        .word
        .as_deref()
        .or_else(|| cli.context.as_deref())
        .unwrap_or_else(|| {
            eprintln!("usage: verbisage check --word <word>");
            std::process::exit(1);
        });

    let lang = cli.shared.language.as_deref().unwrap_or("en_US");
    let correct = if cli.shared.dbus {
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
        let (_, sc) = open_backend(&cli.shared, lang);
        sc.as_ref().map(|s| s.is_correct(word)).unwrap_or(false)
    };

    if correct {
        println!("true");
    } else {
        println!("false");
        std::process::exit(1);
    }
}

fn run_correct(cli: &Cli) {
    let word = cli.word.as_deref().unwrap_or_else(|| {
        eprintln!("usage: verbisage correct --word <word>");
        std::process::exit(1);
    });

    let lang = cli.shared.language.as_deref().unwrap_or("en_US");
    let suggestions: Vec<String> = if cli.shared.dbus {
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
        let (_, sc) = open_backend(&cli.shared, lang);
        sc.as_ref().map(|s| s.suggest(word)).unwrap_or_default()
    };

    for s in &suggestions {
        println!("{}", s);
    }
    if suggestions.is_empty() {
        std::process::exit(1);
    }
}

fn run_predict(_cli: &Cli) {
    eprintln!("predict mode requires a Predictor backend (not yet wired)");
    std::process::exit(1);
}

fn run_query(cli: &Cli) {
    let min = cli.min_len.unwrap_or(0);
    let max = cli.max_len.unwrap_or(0);
    let lang = cli.shared.language.as_deref().unwrap_or("en_US");

    let results: Vec<(String, f64)> = if cli.shared.dbus {
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

        let (dict, _) = open_backend(&cli.shared, lang);
        dict.query_prefixes(&queries)
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
