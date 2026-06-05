use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::backends::BackendDef;

/// Per-layer pattern overrides for dictionary files.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct LayerPatterns {
    pub dict: Option<Vec<String>>,
    pub sqlite: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct PathConfig {
    pub system_dir: Option<String>,
    pub user_dir: Option<String>,
    pub system: Option<LayerPatterns>,
    pub user: Option<LayerPatterns>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct SqliteConfig {
    pub table: Option<String>,
    pub word_col: Option<String>,
    pub freq_col: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub language_default: Option<String>,
    #[serde(default)]
    pub paths: Option<PathConfig>,
    #[serde(default)]
    pub sqlite: Option<SqliteConfig>,
    /// Named backend definitions (`[backends.<name>]` sections).
    #[serde(default)]
    pub backends: Option<HashMap<String, BackendDef>>,
}

pub fn default_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
    PathBuf::from(home).join(".config/verbisage/daemon.conf")
}

pub fn load_config(path: &PathBuf) -> Option<Config> {
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("error: failed to read config '{}': {}", path.display(), e);
        std::process::exit(1);
    });
    toml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("error: failed to parse config '{}': {}", path.display(), e);
        std::process::exit(1);
    })
}
