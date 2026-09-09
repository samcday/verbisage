use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::backends::BackendDef;

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct SqliteConfig {
    pub table: Option<String>,
    pub word_col: Option<String>,
    pub freq_col: Option<String>,
}

/// Client-side configuration (`[client]` section).
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct ClientConfig {
    /// Client operation mode: "standalone" (default) or "dbus".
    pub mode: Option<String>,
}

/// Daemon-side configuration (`[daemon]` section).
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct DaemonConfigSection {
    /// Daemon transport mode: "stdio" (default) or "dbus".
    pub mode: Option<String>,
    /// Cap on accepted `Complete` `max` values (default 1_000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_complete_results: Option<usize>,
    /// Cap on accepted bounded-query `max` values (default 200_000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_query_results: Option<usize>,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sqlite: Option<SqliteConfig>,
    /// Named backend definitions (`[backends.<name>]` sections).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backends: Option<HashMap<String, BackendDef>>,
    /// Client-side configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<ClientConfig>,
    /// Daemon-side configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonConfigSection>,
}

pub fn default_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(home).join(".config"));
    let user_config = config_dir.join("verbisage/config.toml");
    if user_config.exists() {
        user_config
    } else {
        PathBuf::from("/etc/verbisage/config.toml")
    }
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
