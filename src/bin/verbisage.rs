use std::collections::HashMap;

use clap::{Parser, Subcommand};

use verbisage::backends::{BackendDef, BackendType};
use verbisage::cli::{ClientMode, SharedArgs, open_backend};
use verbisage::config::{default_config_path, load_config};
use verbisage::debug;
use verbisage::dictionary::paths::expand_tilde;
use verbisage::dictionary::{DictionaryQuery, DictionaryResult};
use verbisage::veprintln;

#[cfg(feature = "dbus")]
use verbisage::clients::DbusClient;

/// Resolve built-in backend type names (e.g. "file", "sqlite") in a chain
/// string to named backends. For each built-in type not already defined in
/// `cfg.backends`, a `default_<type>` entry is created with path information
/// from CLI overrides or build-time defaults, and the chain is rewritten to
/// reference it.
fn resolve_backend_chain(
    chain: &str,
    cfg: &mut verbisage::config::Config,
    shared: &SharedArgs,
) -> String {
    let segments: Vec<&str> = chain.split('+').collect();
    let system_dir = shared
        .system_data_dir
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| verbisage::dictionary::paths::SYSTEM_DATA_DIR.to_string());
    let user_dir = shared
        .user_data_dir
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| verbisage::dictionary::paths::USER_DATA_DIR_REL.to_string());
    let resolved: Vec<String> = segments
        .into_iter()
        .map(|seg| {
            let seg = seg.trim();
            if let Some(bt) = BackendType::from_name(seg) {
                let name = format!("default_{}", bt.as_name());
                let mut backends = cfg.backends.take().unwrap_or_default();
                // Create new or update existing with CLI overrides
                if !backends.contains_key(&name) {
                    backends.insert(
                        name.clone(),
                        BackendDef {
                            backend_type: bt,
                            path: None,
                            ngram_path: None,
                            system_dir: Some(system_dir.clone()),
                            user_dir: Some(user_dir.clone()),
                            system_patterns: None,
                            user_patterns: None,
                            format: None,
                            delimiter: None,
                            has_header: None,
                            word_index: None,
                            freq_index: None,
                            table: None,
                            word_col: None,
                            freq_col: None,
                            table_ngrams: None,
                            context_cols: None,
                            next_col: None,
                            enable_unigrams: None,
                            enable_ngrams: None,
                            embedded_correction_engine: None,
                        },
                    );
                } else {
                    // Update existing backend with CLI overrides
                    if let Some(backend) = backends.get_mut(&name) {
                        backend.system_dir = Some(system_dir.clone());
                        backend.user_dir = Some(user_dir.clone());
                    }
                }
                cfg.backends = Some(backends);
                name
            } else {
                seg.to_string()
            }
        })
        .collect();
    resolved.join("+")
}

// ── Shared args ────────────────────────────────────────────────────────────

#[derive(clap::Args, Clone)]
struct GlobalArgs {
    #[command(flatten)]
    shared: SharedArgs,
}

/// Per-call preparation. CLI defaults are explicit NFC plus language folding.
#[derive(clap::Args)]
struct TextOptions {
    #[arg(long, default_value="nfc", value_parser=["none","nfc"])]
    normalize: String,
    #[arg(long, default_value="lang_specific", value_parser=["none","ascii_lowercase","unicode_lowercase","full","lang_specific"])]
    fold: String,
    #[arg(long, default_value="nfc", value_parser=["none","nfc"])]
    context_normalize: String,
    #[arg(long, default_value="lang_specific", value_parser=["none","ascii_lowercase","unicode_lowercase","full","lang_specific"])]
    context_fold: String,
    #[arg(long, default_value="prefer_matched", value_parser=["insensitive","prefer_matched"])]
    case_preference: String,
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
        /// Keyboard layout file (HeliBoard simple/JSON or Keyboard3 XML) or XKB layout name.
        #[arg(long)]
        layout: Option<String>,
    },

    /// Complete an unfinished word using committed context.
    Complete {
        #[arg(long)]
        word: String,
        #[arg(long)]
        context: Option<String>,
        #[arg(long, default_value_t = 10)]
        max: usize,
        /// Keyboard layout file (HeliBoard simple/JSON or Keyboard3 XML) or XKB layout name.
        #[arg(long)]
        layout: Option<String>,
        #[command(flatten)]
        text: TextOptions,
    },

    /// Predict the next word given a context.
    Predict {
        /// Context words (space-separated).
        #[arg(long)]
        context: Option<String>,
        /// Maximum number of suggestions.
        #[arg(long, default_value_t = 10)]
        max: usize,
        #[command(flatten)]
        text: TextOptions,
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

    /// Dump the effective runtime config (config file + CLI overrides + defaults) as TOML.
    ConfigDump,

    /// Add observations to the count of an n-gram sequence.
    NgramBump {
        /// N-gram words (space-separated).
        #[arg(long)]
        context: String,
        /// Number of observations to add.
        #[arg(long, default_value_t = 1)]
        count: u64,
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
        Command::Correct { word, layout } => {
            let rows = load_optional_layout(layout.as_deref());
            run_correct(&shared, named_backends, client_mode, &word, rows.as_ref())
        }
        Command::ConfigDump => run_config_dump(&cli.global.shared, config.as_ref()),
        Command::Predict { context, max, text } => run_complete(
            &shared,
            named_backends,
            client_mode,
            "",
            context.as_deref(),
            max,
            &text,
            None,
        ),
        Command::Complete {
            word,
            context,
            max,
            layout,
            text,
        } => {
            let rows = load_optional_layout(layout.as_deref());
            run_complete(
                &shared,
                named_backends,
                client_mode,
                &word,
                context.as_deref(),
                max,
                &text,
                rows.as_ref(),
            )
        }
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
            count,
            save_unknown,
        } => run_ngram_bump(
            &shared,
            named_backends,
            client_mode,
            &context,
            count,
            save_unknown,
        ),
    }
}

// ── Mode dispatchers ───────────────────────────────────────────────────────

// ── Layout loading ─────────────────────────────────────────────────────────

fn load_optional_layout(value: Option<&str>) -> Option<keyboard_layout::physical::RowLayout> {
    let value = value?;
    let path = std::path::Path::new(value);
    let is_file = path.is_file();
    veprintln!(
        "layout: loading {} '{}'",
        if is_file { "layout file" } else { "xkb layout" },
        value
    );
    let result = if is_file {
        load_layout_rows(path)
    } else {
        load_xkb_layout(value)
    };
    match result {
        Ok(rows) => {
            let keys: usize = rows.rows.iter().map(|row| row.keys.len()).sum();
            veprintln!(
                "layout: loaded {} rows, {keys} keys",
                rows.rows.len()
            );
            Some(rows)
        }
        Err(error) => {
            eprintln!("layout error: {error}");
            std::process::exit(1);
        }
    }
}

fn load_xkb_layout(name: &str) -> Result<keyboard_layout::physical::RowLayout, String> {
    use std::str::FromStr;
    let source = keyboard_layout::readers::xkb::XkbSource::from_str(name)
        .map_err(|error| error.to_string())?;
    veprintln!(
        "layout: xkb layout='{}' variant={:?} model={:?} rules='{}'",
        source.layout,
        source.variant,
        source.model,
        source.rules
    );
    let rows = keyboard_layout::readers::xkb::parse(&source).map_err(|error| error.to_string())?;
    veprintln!(
        "layout: xkb '{}' resolved ({} rows)",
        name,
        rows.rows.len()
    );
    Ok(rows)
}

fn load_layout_rows(
    path: &std::path::Path,
) -> Result<keyboard_layout::physical::RowLayout, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let trimmed = text.trim_start();
    let (format, parsed) = if trimmed.starts_with('<') {
        ("Keyboard3 XML", keyboard_layout::readers::unicode::parse(&text))
    } else if trimmed.starts_with('[') || trimmed.starts_with('{') {
        (
            "HeliBoard/FlorisBoard JSON",
            keyboard_layout::readers::heli_json::parse(&text),
        )
    } else {
        (
            "HeliBoard simple",
            keyboard_layout::readers::heli_simple::parse(&text),
        )
    };
    veprintln!(
        "layout: file '{}' detected as {format}",
        path.display()
    );
    parsed.map_err(|error| error.to_string())
}

fn rect_layout(
    rows: &keyboard_layout::physical::RowLayout,
) -> std::sync::Arc<keyboard_layout::RectKeyLayout> {
    std::sync::Arc::new(rows.to_rect_key_layout(&keyboard_layout::physical::RowMetrics::default()))
}

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
        let (_, sc, _) = open_backend(shared, lang, named_backends);
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
    layout: Option<&keyboard_layout::physical::RowLayout>,
) {
    let lang = shared.lang();
    let suggestions: Vec<String> = if client_mode == ClientMode::Dbus {
        let token = layout.and_then(dbus_register_layout);
        dbus_suggest(word, 10, lang, token.as_deref())
    } else {
        let (_, sc, _) = open_backend(shared, lang, named_backends);
        match &sc {
            Some(checker) => {
                let input = verbisage::spellcheck::SuggestionInput {
                    word,
                    context: &[],
                    spatial: verbisage::spatial::SpatialInput::from_parts(
                        layout.map(rect_layout),
                        Vec::new(),
                    ),
                };
                checker.suggest_with(&input, 10)
            }
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
        eprintln!("no suggestions found for '{}'", word);
        std::process::exit(1);
    }
}

fn run_complete(
    shared: &SharedArgs,
    named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
    word: &str,
    context: Option<&str>,
    max: usize,
    text: &TextOptions,
    layout: Option<&keyboard_layout::physical::RowLayout>,
) {
    use verbisage::completion::{AndroidCompleter, CompletionEngine, CompletionInput};
    use verbisage::text::TextPrep;
    let context: Vec<_> = context.unwrap_or("").split_whitespace().collect();
    let input = CompletionInput {
        input: word,
        context: &context,
        input_prep: TextPrep::from_names(&text.normalize, &text.fold).unwrap(),
        context_prep: TextPrep::from_names(&text.context_normalize, &text.context_fold).unwrap(),
        case_preference: serde_json::from_value(serde_json::json!(text.case_preference)).unwrap(),
        spatial: verbisage::spatial::SpatialInput::from_parts(layout.map(rect_layout), Vec::new()),
    };
    for prep in [input.input_prep, input.context_prep] {
        if let Some(warning) = prep.warning() {
            eprintln!("warning: {warning}");
        }
    }
    let rows: Vec<(String, f64)> = if client_mode == ClientMode::Dbus {
        let token = layout.and_then(dbus_register_layout);
        dbus_complete_with(&input, max, shared.lang(), token.as_deref())
    } else {
        let (dict, _, predictor) = open_backend(shared, shared.lang(), named_backends);
        match AndroidCompleter::new(dict.as_ref())
            .with_predictor(predictor.as_deref())
            .with_language(shared.lang())
            .complete_with(&input, max)
        {
            Ok(rows) => rows.into_iter().map(|r| (r.word, r.score)).collect(),
            Err(error) => {
                eprintln!("error: {error}");
                std::process::exit(1);
            }
        }
    };
    for (word, score) in &rows {
        println!("{word}  {score}");
    }
    if rows.is_empty() && max != 0 {
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

        let (dict, sc, _) = open_backend(shared, lang, named_backends);
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
        let (dict, _, _) = open_backend(shared, lang, named_backends);
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
    shared: &SharedArgs,
    named_backends: Option<&HashMap<String, BackendDef>>,
    client_mode: ClientMode,
    context: &str,
    count: u64,
    save_unknown: bool,
) {
    let ngram: Vec<String> = context.split_whitespace().map(String::from).collect();
    let lang = shared.lang();

    if client_mode == ClientMode::Dbus {
        dbus_bump_ngram(ngram, count, save_unknown, lang);
    } else {
        let (_, _, predictor) = open_backend(shared, lang, named_backends);
        match predictor {
            Some(pred) => {
                let ngram_refs: Vec<&str> = ngram.iter().map(|s| s.as_str()).collect();
                match pred.increase_ngram_count(&ngram_refs, count, save_unknown) {
                    Ok(()) => println!("true"),
                    Err(e) => {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            None => {
                eprintln!("error: no predictor backend available for the given chain");
                std::process::exit(1)
            }
        }
    }
}

// ── Config dump ────────────────────────────────────────────────────────────

fn run_config_dump(shared: &SharedArgs, config: Option<&verbisage::config::Config>) {
    let merged = merge_config(shared, config);
    let toml = toml::to_string_pretty(&merged).unwrap();
    println!("{}", toml);
}

/// Merge config file + CLI overrides + defaults into a single Config struct.
fn merge_config(
    shared: &SharedArgs,
    file_config: Option<&verbisage::config::Config>,
) -> verbisage::config::Config {
    let mut cfg = file_config.cloned().unwrap_or_default();

    // Backend chain: CLI > config > default, then resolve built-in types to named backends
    let chain = shared
        .backend
        .clone()
        .or_else(|| cfg.backend.clone())
        .unwrap_or_else(|| "file".into());
    cfg.backend = Some(resolve_backend_chain(&chain, &mut cfg, shared));

    // Apply CLI path overrides to ALL backends (for config-dump accuracy)
    if let Some(ref sys_dir) = shared.system_data_dir {
        let sys = sys_dir.to_string_lossy().to_string();
        for backend in cfg.backends.iter_mut().flat_map(|m| m.values_mut()) {
            backend.system_dir = Some(sys.clone());
        }
    }
    if let Some(ref usr_dir) = shared.user_data_dir {
        let usr = usr_dir.to_string_lossy().to_string();
        for backend in cfg.backends.iter_mut().flat_map(|m| m.values_mut()) {
            backend.user_dir = Some(usr.clone());
        }
    }

    // Language: CLI > config > default
    cfg.language_default = shared
        .language
        .clone()
        .or_else(|| cfg.language_default.clone())
        .or(Some("en_US".into()));

    // SQLite: CLI overrides > config
    if shared.table.is_some() || shared.word_col.is_some() || shared.freq_col.is_some() {
        let mut sqlite = cfg.sqlite.take().unwrap_or_default();
        sqlite.table = shared
            .table
            .clone()
            .or_else(|| sqlite.table.clone())
            .or(Some("words".into()));
        sqlite.word_col = shared
            .word_col
            .clone()
            .or_else(|| sqlite.word_col.clone())
            .or(Some("word".into()));
        sqlite.freq_col = shared
            .freq_col
            .clone()
            .or_else(|| sqlite.freq_col.clone())
            .or(Some("frequency".into()));
        cfg.sqlite = Some(sqlite);
    }

    // Client mode: CLI > config
    if shared.mode.is_some() {
        let mut client = cfg.client.take().unwrap_or_default();
        client.mode = shared.mode.map(|m| match m {
            ClientMode::Standalone => "standalone".into(),
            ClientMode::Dbus => "dbus".into(),
            ClientMode::Stdio => "stdio".into(),
        });
        cfg.client = Some(client);
    }

    cfg
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
fn dbus_register_layout(rows: &keyboard_layout::physical::RowLayout) -> Option<String> {
    let upload = verbisage::layout::LayoutUpload {
        keys: None,
        rows: Some(rows.clone()),
        ignored_labels: Vec::new(),
    };
    Some(dbus_call(|client| Ok(client.register_layout(&upload)?)))
}

#[cfg(not(feature = "dbus"))]
fn dbus_register_layout(_rows: &keyboard_layout::physical::RowLayout) -> Option<String> {
    eprintln!("dbus feature not enabled; rebuild with --features dbus");
    std::process::exit(1);
}

#[cfg(feature = "dbus")]
fn dbus_suggest(word: &str, max: usize, lang: &str, layout: Option<&str>) -> Vec<String> {
    dbus_call(|client| {
        Ok(match layout {
            Some(token) => client.suggest_layout(word, max as u32, lang, token, &[])?,
            None => client.suggest(word, max as u32, lang)?,
        })
    })
}

#[cfg(not(feature = "dbus"))]
fn dbus_suggest(_word: &str, _max: usize, _lang: &str, _layout: Option<&str>) -> Vec<String> {
    dbus_call(|| Err("dbus not enabled".into()))
}

#[cfg(feature = "dbus")]
fn dbus_complete_with(
    input: &verbisage::completion::CompletionInput<'_>,
    max: usize,
    lang: &str,
    layout: Option<&str>,
) -> Vec<(String, f64)> {
    dbus_call(|client| {
        let max = u32::try_from(max).map_err(|_| "requested max exceeds D-Bus integer range")?;
        Ok(match layout {
            Some(token) => client.complete_with_layout(input, max, lang, token, &[])?,
            None => client.complete_with(input, max, lang)?,
        })
    })
}
#[cfg(not(feature = "dbus"))]
fn dbus_complete_with(
    _input: &verbisage::completion::CompletionInput<'_>,
    _max: usize,
    _lang: &str,
    _layout: Option<&str>,
) -> Vec<(String, f64)> {
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
fn dbus_bump_ngram(ngram: Vec<String>, count: u64, save_unknown: bool, lang: &str) {
    match DbusClient::new() {
        Ok(client) => match client.bump_ngram(ngram, count, save_unknown, lang) {
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
fn dbus_bump_ngram(_ngram: Vec<String>, _count: u64, _save_unknown: bool, _lang: &str) {
    dbus_call(|| Err("dbus not enabled".into()))
}
