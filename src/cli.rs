use std::path::PathBuf;

use clap::{Args, ValueEnum};

use crate::backends::resolve_chain_with_backcompat;
use crate::config::Config;
use crate::dictionary::DictionaryBackend;
use crate::dictionary::paths::{LanguagePaths, PathOverride, expand_tilde};
use crate::spellcheck::SpellChecker;

/// Transport / client mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ClientMode {
    /// One-shot, local backend (client only).
    Standalone,
    /// Serve/connect over D-Bus.
    Dbus,
    /// Serve over stdio JSON (daemon only).
    Stdio,
}

/// Pattern overrides carried from config file (not CLI args).
#[derive(Clone, Default)]
pub struct PatternOverrides {
    pub system_dict: Option<Vec<String>>,
    pub user_dict: Option<Vec<String>>,
    pub system_sqlite: Option<Vec<String>>,
    pub user_sqlite: Option<Vec<String>>,
}

/// Shared CLI arguments used by both daemon and one-shot binaries.
#[derive(Args, Clone)]
pub struct SharedArgs {
    /// Backend chain (e.g. "file", "sqlite", "dict+lm", "A+B+C")
    #[arg(long, short, env = "VERBISAGE_BACKEND")]
    pub backend: Option<String>,

    /// Language tag (e.g. en_US) — used for directory lookup and hunspell.
    #[arg(long, env = "VERBISAGE_LANGUAGE")]
    pub language: Option<String>,

    /// Path to config file
    #[arg(long)]
    pub config: Option<PathBuf>,

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
    #[arg(long)]
    pub table: Option<String>,

    /// Word column name for the SQLite backend
    #[arg(long)]
    pub word_col: Option<String>,

    /// Frequency column name for the SQLite backend
    #[arg(long)]
    pub freq_col: Option<String>,

    /// Path to .aff file; used with --backend hunspell
    #[arg(long)]
    pub affix: Option<PathBuf>,

    /// Path to .dic file; used with --backend hunspell
    #[arg(long)]
    pub dict: Option<PathBuf>,

    /// Operation mode: "standalone" (client, local backend), "dbus" (client or daemon, D-Bus transport), "stdio" (daemon, stdio JSON server).
    #[arg(long, value_enum)]
    pub mode: Option<ClientMode>,

    /// Enable verbose debug output to stderr.
    #[arg(long)]
    pub verbose: bool,

    /// Pattern overrides — set from config file, not a CLI argument.
    #[arg(skip)]
    pub patterns: PatternOverrides,
}

// ── Merge config + CLI + defaults ─────────────────────────────────────────

impl SharedArgs {
    /// Merge config-file values into CLI-parsed args, applying final defaults
    /// for any fields that are still `None`.
    pub fn apply_defaults(mut self, config: Option<&Config>) -> Self {
        let cfg = config;

        // ── backend ──────────────────────────────────────────────────────
        if self.backend.is_none() {
            self.backend = cfg
                .and_then(|c| c.backend.as_deref())
                .map(|s| s.to_string())
                .or(Some("file".into()));
        }

        // ── language / language_default ──────────────────────────────────
        if self.language.is_none() {
            self.language = cfg
                .and_then(|c| c.language_default.clone())
                .or(Some("en_US".into()));
        }

        // ── table ────────────────────────────────────────────────────────
        if self.table.is_none() {
            self.table = cfg
                .and_then(|c| c.sqlite.as_ref())
                .and_then(|s| s.table.clone())
                .or(Some("words".into()));
        }

        // ── word_col ─────────────────────────────────────────────────────
        if self.word_col.is_none() {
            self.word_col = cfg
                .and_then(|c| c.sqlite.as_ref())
                .and_then(|s| s.word_col.clone())
                .or(Some("word".into()));
        }

        // ── freq_col ─────────────────────────────────────────────────────
        if self.freq_col.is_none() {
            self.freq_col = cfg
                .and_then(|c| c.sqlite.as_ref())
                .and_then(|s| s.freq_col.clone())
                .or(Some("frequency".into()));
        }

        // ── system_data_dir ──────────────────────────────────────────────
        if self.system_data_dir.is_none() {
            self.system_data_dir = Some(expand_tilde(crate::dictionary::paths::SYSTEM_DATA_DIR));
        }

        // ── user_data_dir ────────────────────────────────────────────────
        if self.user_data_dir.is_none() {
            self.user_data_dir = Some(expand_tilde(crate::dictionary::paths::USER_DATA_DIR_REL));
        }

        // ── tilde expansion for all path fields ──────────────────────────
        if let Some(dir) = &mut self.system_data_dir {
            *dir = expand_tilde(dir.to_str().unwrap_or(""));
        }
        if let Some(dir) = &mut self.user_data_dir {
            *dir = expand_tilde(dir.to_str().unwrap_or(""));
        }
        if let Some(aff) = &mut self.affix {
            *aff = expand_tilde(aff.to_str().unwrap_or(""));
        }
        if let Some(d) = &mut self.dict {
            *d = expand_tilde(d.to_str().unwrap_or(""));
        }

        self
    }

    /// Resolve transport mode: CLI `--mode` > config section value > default.
    pub fn resolve_mode(
        cli_mode: Option<ClientMode>,
        config_mode: Option<&str>,
        default_mode: ClientMode,
    ) -> ClientMode {
        if let Some(m) = cli_mode {
            return m;
        }
        if let Some(s) = config_mode {
            match s {
                "standalone" => return ClientMode::Standalone,
                "dbus" => return ClientMode::Dbus,
                "stdio" => return ClientMode::Stdio,
                _ => {
                    eprintln!("warning: invalid mode '{}', using default", s);
                }
            }
        }
        default_mode
    }

    /// Convenience: get the resolved language (or default).
    pub fn lang(&self) -> &str {
        self.language.as_deref().unwrap_or("en_US")
    }
}

// ── Eager backend construction (used by one-shot modes) ────────────────────

/// Open a backend and optional spellchecker for the given language.
///
/// Exits the process on construction failure.
pub fn open_backend(
    args: &SharedArgs,
    lang: &str,
    named_backends: Option<&std::collections::HashMap<String, crate::backends::BackendDef>>,
) -> (Box<dyn DictionaryBackend>, Option<Box<dyn SpellChecker>>) {
    let chain = args.backend.as_deref().unwrap_or("file");

    let (assignment, warnings) = resolve_chain_with_backcompat(chain, named_backends)
        .unwrap_or_else(|e| {
            eprintln!("error: failed to resolve backend chain '{}': {}", chain, e);
            std::process::exit(1);
        });

    for w in &warnings {
        eprintln!("warning: {}", w);
    }

    let lp = build_language_paths(args, lang);
    let composed = crate::backends::build::compose_chain(&assignment, lang, &lp);

    (composed.dictionary, composed.spellchecker)
}

// ── Backend config helper (used by both daemon and one-shot) ───────────────

/// Build a `LanguagePaths` seeded with CLI-provided dirs and overrides (no
/// specific language — the caller sets that later).
pub fn base_language_paths(args: &SharedArgs) -> LanguagePaths {
    build_language_paths(args, "placeholder")
}

fn build_language_paths(args: &SharedArgs, lang: &str) -> LanguagePaths {
    let mut lp = LanguagePaths::new(lang);
    if let Some(dir) = &args.system_data_dir {
        lp = lp.with_system_dir(expand_tilde(dir.to_str().unwrap_or("")));
    }
    if let Some(dir) = &args.user_data_dir {
        lp = lp.with_user_dir(expand_tilde(dir.to_str().unwrap_or("")));
    }
    lp.system_file_override = PathOverride::from_cli(args.system_dict.as_deref());
    lp.user_file_override = PathOverride::from_cli(args.user_dict.as_deref());
    lp.set_patterns(
        args.patterns.system_dict.as_deref(),
        args.patterns.user_dict.as_deref(),
        args.patterns.system_sqlite.as_deref(),
        args.patterns.user_sqlite.as_deref(),
    );
    lp
}
