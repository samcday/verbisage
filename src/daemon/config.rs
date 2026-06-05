use std::path::PathBuf;

use clap::ValueEnum;

use crate::dictionary::paths::{LanguagePaths, PathOverride, expand_tilde};

#[derive(ValueEnum, Clone, Debug)]
pub enum BackendKind {
    File,
    #[cfg(feature = "sqlite")]
    Sqlite,
    #[cfg(feature = "hunspell")]
    Hunspell,
}

pub struct DaemonConfig {
    pub backend: BackendKind,
    pub language_paths: LanguagePaths,
    pub default_lang: String,
    pub sqlite_table: String,
    pub sqlite_word_col: String,
    pub sqlite_freq_col: String,
    pub hunspell_affix: Option<PathBuf>,
    pub hunspell_dict: Option<PathBuf>,
    pub eager_path: Option<PathBuf>,
    pub eager_system_dict: Option<String>,
    pub eager_user_dict: Option<String>,
}

/// Construct a `LanguagePaths` seeded with the CLI-provided data dirs and
/// path overrides (but *not* with a specific language — the caller supplies
/// that later).
fn base_language_paths(
    system_data_dir: Option<&PathBuf>,
    user_data_dir: Option<&PathBuf>,
    system_dict: Option<&str>,
    user_dict: Option<&str>,
) -> LanguagePaths {
    let mut lp = LanguagePaths::new("placeholder");
    if let Some(dir) = system_data_dir {
        lp = lp.with_system_dir(expand_tilde(dir.to_str().unwrap_or("")));
    }
    if let Some(dir) = user_data_dir {
        lp = lp.with_user_dir(expand_tilde(dir.to_str().unwrap_or("")));
    }
    lp.system_file_override = PathOverride::from_cli(system_dict);
    lp.user_file_override = PathOverride::from_cli(user_dict);
    lp
}

impl DaemonConfig {
    pub fn default_for(lang: &str) -> Self {
        Self {
            backend: BackendKind::File,
            language_paths: LanguagePaths::new(lang),
            default_lang: lang.to_string(),
            sqlite_table: String::new(),
            sqlite_word_col: String::new(),
            sqlite_freq_col: String::new(),
            hunspell_affix: None,
            hunspell_dict: None,
            eager_path: None,
            eager_system_dict: None,
            eager_user_dict: None,
        }
    }

    pub fn from_cli(
        backend: &BackendKind,
        language: Option<&str>,
        path: Option<&PathBuf>,
        system_data_dir: Option<&PathBuf>,
        user_data_dir: Option<&PathBuf>,
        system_dict: Option<&str>,
        user_dict: Option<&str>,
        table: &str,
        word_col: &str,
        freq_col: &str,
        affix: Option<&PathBuf>,
        dict: Option<&PathBuf>,
    ) -> Self {
        let default_lang = language.unwrap_or("en_US").to_string();
        let lp = base_language_paths(system_data_dir, user_data_dir, system_dict, user_dict);
        Self {
            backend: backend.clone(),
            language_paths: lp,
            default_lang,
            sqlite_table: table.to_string(),
            sqlite_word_col: word_col.to_string(),
            sqlite_freq_col: freq_col.to_string(),
            hunspell_affix: affix.cloned(),
            hunspell_dict: dict.cloned(),
            eager_path: path.cloned(),
            eager_system_dict: system_dict.map(String::from),
            eager_user_dict: user_dict.map(String::from),
        }
    }
}
