pub mod build;
pub mod chain;
pub mod merged;

#[cfg(feature = "sqlite")]
pub mod shared_sqlite;
#[cfg(feature = "sqlite")]
pub use shared_sqlite::SharedSqliteConnection;

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Backend types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub enum BackendType {
    #[serde(rename = "file")]
    File,
    #[serde(rename = "sqlite")]
    Sqlite,
    #[serde(rename = "marisa")]
    Marisa,
    #[serde(rename = "hunspell")]
    Hunspell,
}

// ---------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    Dictionary,
    Unigrams,
    Ngrams,
}

impl Capability {
    pub fn all() -> &'static [Capability] {
        &[
            Capability::Dictionary,
            Capability::Unigrams,
            Capability::Ngrams,
        ]
    }
}

// ---------------------------------------------------------------------------
// Format types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileFormat {
    Flat,
    Freq,
    CsvUnigrams,
    Custom,
}

impl FileFormat {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "flat" => Some(Self::Flat),
            "freq" => Some(Self::Freq),
            "csv_unigrams" => Some(Self::CsvUnigrams),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SqliteFormat {
    PresageWords,
    PresageUnigrams,
    PresageNgrams,
    Presage,
    Custom,
}

impl SqliteFormat {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "presage_words" => Some(Self::PresageWords),
            "presage_unigrams" => Some(Self::PresageUnigrams),
            "presage_ngrams" => Some(Self::PresageNgrams),
            "presage" => Some(Self::Presage),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Preset defaults
// ---------------------------------------------------------------------------

/// Defaults implied by a file format preset.
pub struct FilePreset {
    pub delimiter: Option<String>,
    pub has_header: bool,
    pub word_index: Option<usize>,
    pub freq_index: Option<usize>,
    pub capabilities: Vec<Capability>,
}

impl FilePreset {
    pub fn for_format(fmt: FileFormat) -> Self {
        match fmt {
            FileFormat::Flat => Self {
                delimiter: None,
                has_header: false,
                word_index: None,
                freq_index: None,
                capabilities: vec![Capability::Dictionary],
            },
            FileFormat::Freq => Self {
                delimiter: None,
                has_header: false,
                word_index: Some(0),
                freq_index: Some(1),
                capabilities: vec![Capability::Dictionary, Capability::Unigrams],
            },
            FileFormat::CsvUnigrams => Self {
                delimiter: None,
                has_header: false,
                word_index: Some(0),
                freq_index: Some(1),
                capabilities: vec![Capability::Dictionary, Capability::Unigrams],
            },
            FileFormat::Custom => Self {
                delimiter: None,
                has_header: false,
                word_index: None,
                freq_index: None,
                capabilities: Vec::new(),
            },
        }
    }
}

/// Defaults implied by a sqlite format preset.
pub struct SqlitePreset {
    pub table: Option<String>,
    pub word_col: Option<String>,
    pub freq_col: Option<String>,
    pub table_ngrams: Option<String>,
    pub context_cols: Option<Vec<String>>,
    pub next_col: Option<String>,
    pub capabilities: Vec<Capability>,
}

impl SqlitePreset {
    pub fn for_format(fmt: SqliteFormat) -> Self {
        match fmt {
            SqliteFormat::PresageWords => Self {
                table: Some("words".into()),
                word_col: Some("word".into()),
                freq_col: Some("frequency".into()),
                table_ngrams: None,
                context_cols: None,
                next_col: None,
                capabilities: vec![Capability::Dictionary, Capability::Unigrams],
            },
            SqliteFormat::PresageUnigrams => Self {
                table: Some("unigrams".into()),
                word_col: Some("word".into()),
                freq_col: Some("frequency".into()),
                table_ngrams: None,
                context_cols: None,
                next_col: None,
                capabilities: vec![Capability::Unigrams],
            },
            SqliteFormat::PresageNgrams => Self {
                table: None,
                word_col: None,
                freq_col: None,
                table_ngrams: Some("ngrams".into()),
                context_cols: Some(vec!["prev".into()]),
                next_col: Some("next".into()),
                capabilities: vec![Capability::Ngrams],
            },
            SqliteFormat::Presage => Self {
                table: Some("words".into()),
                word_col: Some("word".into()),
                freq_col: Some("frequency".into()),
                table_ngrams: Some("ngrams".into()),
                context_cols: Some(vec!["prev".into()]),
                next_col: Some("next".into()),
                capabilities: vec![
                    Capability::Dictionary,
                    Capability::Unigrams,
                    Capability::Ngrams,
                ],
            },
            SqliteFormat::Custom => Self {
                table: None,
                word_col: None,
                freq_col: None,
                table_ngrams: None,
                context_cols: None,
                next_col: None,
                capabilities: Vec::new(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// BackendDef — the main config struct
// ---------------------------------------------------------------------------

/// Per-backend definition from TOML `[backends.<name>]` sections.
#[derive(Debug, Clone, Deserialize)]
pub struct BackendDef {
    #[serde(rename = "type")]
    pub backend_type: BackendType,

    /// Explicit file path (`{lang}` expanded).  Bypasses LanguagePaths pattern scan.
    #[serde(default)]
    pub path: Option<String>,

    // ── Format / preset ──────────────────────────────────────────────────
    /// Format preset name (e.g. "flat", "presage", "custom").
    /// Interpreted differently for each BackendType.
    #[serde(default)]
    pub format: Option<String>,

    // ── File-backend fields ──────────────────────────────────────────────
    #[serde(default)]
    pub delimiter: Option<String>,
    #[serde(default)]
    pub has_header: Option<bool>,
    #[serde(default)]
    pub word_index: Option<usize>,
    #[serde(default)]
    pub freq_index: Option<usize>,

    // ── Sqlite-backend fields ────────────────────────────────────────────
    #[serde(default)]
    pub table: Option<String>,
    #[serde(default)]
    pub word_col: Option<String>,
    #[serde(default)]
    pub freq_col: Option<String>,
    #[serde(default)]
    pub table_ngrams: Option<String>,
    #[serde(default)]
    pub context_cols: Option<Vec<String>>,
    #[serde(default)]
    pub next_col: Option<String>,

    // ── Capability overrides ─────────────────────────────────────────────
    #[serde(default)]
    pub enable_unigrams: Option<bool>,
    #[serde(default)]
    pub enable_ngrams: Option<bool>,
}

/// Resolved backend with merged preset defaults and validated capabilities.
#[derive(Debug, Clone)]
pub struct ResolvedBackendDef {
    pub name: String,
    pub backend_type: BackendType,
    pub path: Option<String>,
    pub capabilities: HashSet<Capability>,

    // File fields
    pub delimiter: Option<String>,
    pub has_header: bool,
    pub word_index: Option<usize>,
    pub freq_index: Option<usize>,

    // Sqlite fields
    pub table: Option<String>,
    pub word_col: Option<String>,
    pub freq_col: Option<String>,
    pub table_ngrams: Option<String>,
    pub context_cols: Vec<String>,
    pub next_col: Option<String>,

    // Hunspell
    pub hunspell_affix: Option<String>,
    pub hunspell_dict: Option<String>,
}

// ---------------------------------------------------------------------------
// Validation errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum BackendConfigError {
    UnknownFormat(String, BackendType),
    MissingDelimiterWithIndex,
    MissingRequiredField(&'static str, Capability),
    EmptyCapabilities(String),
    UnsupportedType(&'static str),
    DuplicateBackendName(String),
}

impl std::fmt::Display for BackendConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownFormat(fmt, ty) => {
                write!(f, "unknown format '{}' for backend type {:?}", fmt, ty)
            }
            Self::MissingDelimiterWithIndex => {
                write!(
                    f,
                    "delimiter must be set when word_index or freq_index is specified"
                )
            }
            Self::MissingRequiredField(field, cap) => {
                write!(
                    f,
                    "missing required field '{}' for capability {:?}",
                    field, cap
                )
            }
            Self::EmptyCapabilities(name) => {
                write!(f, "backend '{}' resolves to empty capabilities", name)
            }
            Self::UnsupportedType(msg) => write!(f, "{}", msg),
            Self::DuplicateBackendName(name) => {
                write!(f, "duplicate backend name '{}'", name)
            }
        }
    }
}

impl std::error::Error for BackendConfigError {}

// ---------------------------------------------------------------------------
// Resolve a BackendDef (preset + user overrides) → ResolvedBackendDef
// ---------------------------------------------------------------------------

pub fn resolve_backend_def(
    name: &str,
    def: &BackendDef,
) -> Result<ResolvedBackendDef, BackendConfigError> {
    match def.backend_type {
        BackendType::File => resolve_file_backend(name, def),
        BackendType::Sqlite => resolve_sqlite_backend(name, def),
        BackendType::Marisa => resolve_marisa_backend(name, def),
        BackendType::Hunspell => resolve_hunspell_backend(name, def),
    }
}

fn resolve_file_backend(
    name: &str,
    def: &BackendDef,
) -> Result<ResolvedBackendDef, BackendConfigError> {
    let fmt = match &def.format {
        Some(f) => FileFormat::from_str(f)
            .ok_or_else(|| BackendConfigError::UnknownFormat(f.clone(), BackendType::File))?,
        None => FileFormat::Flat,
    };

    let preset = FilePreset::for_format(fmt);

    let delimiter = def.delimiter.clone().or(preset.delimiter);
    let has_header = def.has_header.unwrap_or(preset.has_header);
    let word_index = def.word_index.or(preset.word_index);
    let freq_index = def.freq_index.or(preset.freq_index);

    // Validation: delimiter=None + any index set → error
    if delimiter.is_none() && (word_index.is_some() || freq_index.is_some()) {
        return Err(BackendConfigError::MissingDelimiterWithIndex);
    }

    // Start with preset capabilities, override with explicit enable_* flags
    let mut capabilities: HashSet<Capability> = preset.capabilities.into_iter().collect();
    if let Some(en) = def.enable_unigrams {
        if en {
            capabilities.insert(Capability::Unigrams);
        } else {
            capabilities.remove(&Capability::Unigrams);
        }
    }
    if let Some(en) = def.enable_ngrams {
        // file backend doesn't support ngrams natively, but we allow the flag
        // (it'll be ignored downstream)
        if en {
            capabilities.insert(Capability::Ngrams);
        } else {
            capabilities.remove(&Capability::Ngrams);
        }
    }

    // Ngrams not really supported for file
    capabilities.remove(&Capability::Ngrams);

    // Validate required fields for active capabilities
    if capabilities.contains(&Capability::Unigrams) && delimiter.is_none() && word_index.is_none() {
        // freq format implies whitespace split with word_index=0, freq_index=1
        // so this is fine
    }

    // Validate: unigrams enabled but no freq_index set for CSV mode
    if capabilities.contains(&Capability::Unigrams) && delimiter.is_some() && freq_index.is_none() {
        return Err(BackendConfigError::MissingRequiredField(
            "freq_index",
            Capability::Unigrams,
        ));
    }

    if capabilities.is_empty() {
        return Err(BackendConfigError::EmptyCapabilities(name.to_string()));
    }

    Ok(ResolvedBackendDef {
        name: name.to_string(),
        backend_type: BackendType::File,
        path: def.path.clone(),
        capabilities,
        delimiter,
        has_header,
        word_index,
        freq_index,
        table: None,
        word_col: None,
        freq_col: None,
        table_ngrams: None,
        context_cols: Vec::new(),
        next_col: None,
        hunspell_affix: None,
        hunspell_dict: None,
    })
}

fn resolve_sqlite_backend(
    name: &str,
    def: &BackendDef,
) -> Result<ResolvedBackendDef, BackendConfigError> {
    let fmt = match &def.format {
        Some(f) => SqliteFormat::from_str(f)
            .ok_or_else(|| BackendConfigError::UnknownFormat(f.clone(), BackendType::Sqlite))?,
        None => SqliteFormat::Custom,
    };

    let preset = SqlitePreset::for_format(fmt);

    let table = def.table.clone().or(preset.table);
    let word_col = def.word_col.clone().or(preset.word_col);
    let freq_col = def.freq_col.clone().or(preset.freq_col);
    let table_ngrams = def.table_ngrams.clone().or(preset.table_ngrams);
    let context_cols = def
        .context_cols
        .clone()
        .or(preset.context_cols)
        .unwrap_or_default();
    let next_col = def.next_col.clone().or(preset.next_col);

    // Start with preset capabilities
    let mut capabilities: HashSet<Capability> = preset.capabilities.into_iter().collect();
    if let Some(en) = def.enable_unigrams {
        if en {
            capabilities.insert(Capability::Unigrams);
        } else {
            capabilities.remove(&Capability::Unigrams);
        }
    }
    if let Some(en) = def.enable_ngrams {
        if en {
            capabilities.insert(Capability::Ngrams);
        } else {
            capabilities.remove(&Capability::Ngrams);
        }
    }

    // Validate required fields
    if capabilities.contains(&Capability::Dictionary)
        || capabilities.contains(&Capability::Unigrams)
    {
        if table.is_none() {
            return Err(BackendConfigError::MissingRequiredField(
                "table",
                Capability::Dictionary,
            ));
        }
        if word_col.is_none() {
            return Err(BackendConfigError::MissingRequiredField(
                "word_col",
                Capability::Dictionary,
            ));
        }
        if freq_col.is_none() {
            return Err(BackendConfigError::MissingRequiredField(
                "freq_col",
                Capability::Dictionary,
            ));
        }
    }

    if capabilities.contains(&Capability::Ngrams) {
        if table_ngrams.is_none() {
            return Err(BackendConfigError::MissingRequiredField(
                "table_ngrams",
                Capability::Ngrams,
            ));
        }
        if context_cols.is_empty() {
            return Err(BackendConfigError::MissingRequiredField(
                "context_cols",
                Capability::Ngrams,
            ));
        }
        if next_col.is_none() {
            return Err(BackendConfigError::MissingRequiredField(
                "next_col",
                Capability::Ngrams,
            ));
        }
    }

    if capabilities.is_empty() {
        return Err(BackendConfigError::EmptyCapabilities(name.to_string()));
    }

    Ok(ResolvedBackendDef {
        name: name.to_string(),
        backend_type: BackendType::Sqlite,
        path: def.path.clone(),
        capabilities,
        delimiter: None,
        has_header: false,
        word_index: None,
        freq_index: None,
        table,
        word_col,
        freq_col,
        table_ngrams,
        context_cols,
        next_col,
        hunspell_affix: None,
        hunspell_dict: None,
    })
}

fn resolve_marisa_backend(
    name: &str,
    def: &BackendDef,
) -> Result<ResolvedBackendDef, BackendConfigError> {
    let mut capabilities = HashSet::new();
    capabilities.insert(Capability::Dictionary);

    if let Some(en) = def.enable_unigrams {
        if en {
            capabilities.insert(Capability::Unigrams);
        }
    }

    if capabilities.is_empty() {
        return Err(BackendConfigError::EmptyCapabilities(name.to_string()));
    }

    Ok(ResolvedBackendDef {
        name: name.to_string(),
        backend_type: BackendType::Marisa,
        path: def.path.clone(),
        capabilities,
        delimiter: None,
        has_header: false,
        word_index: None,
        freq_index: None,
        table: None,
        word_col: None,
        freq_col: None,
        table_ngrams: None,
        context_cols: Vec::new(),
        next_col: None,
        hunspell_affix: None,
        hunspell_dict: None,
    })
}

fn resolve_hunspell_backend(
    name: &str,
    def: &BackendDef,
) -> Result<ResolvedBackendDef, BackendConfigError> {
    let mut capabilities = HashSet::new();
    capabilities.insert(Capability::Dictionary);

    if let Some(en) = def.enable_unigrams {
        if en {
            capabilities.insert(Capability::Unigrams);
        }
    }

    if capabilities.is_empty() {
        return Err(BackendConfigError::EmptyCapabilities(name.to_string()));
    }

    Ok(ResolvedBackendDef {
        name: name.to_string(),
        backend_type: BackendType::Hunspell,
        path: def.path.clone(),
        capabilities,
        delimiter: None,
        has_header: false,
        word_index: None,
        freq_index: None,
        table: None,
        word_col: None,
        freq_col: None,
        table_ngrams: None,
        context_cols: Vec::new(),
        next_col: None,
        hunspell_affix: None,
        hunspell_dict: None,
    })
}

// ---------------------------------------------------------------------------
// Convenience: resolve all backends in a config map
// ---------------------------------------------------------------------------

pub fn resolve_all_backends(
    defs: &HashMap<String, BackendDef>,
) -> Result<HashMap<String, ResolvedBackendDef>, BackendConfigError> {
    let mut resolved = HashMap::new();
    for (name, def) in defs {
        let r = resolve_backend_def(name, def)?;
        resolved.insert(name.clone(), r);
    }
    Ok(resolved)
}
