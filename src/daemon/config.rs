use std::path::PathBuf;

use clap::ValueEnum;

use crate::cli::SharedArgs;
use crate::dictionary::paths::LanguagePaths;

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

    pub fn from_cli(args: &SharedArgs) -> Self {
        let default_lang = args.language.as_deref().unwrap_or("en_US").to_string();
        let lp = crate::cli::base_language_paths(args);
        Self {
            backend: args.backend.clone(),
            language_paths: lp,
            default_lang,
            sqlite_table: args.table.clone(),
            sqlite_word_col: args.word_col.clone(),
            sqlite_freq_col: args.freq_col.clone(),
            hunspell_affix: args.affix.clone(),
            hunspell_dict: args.dict.clone(),
            eager_path: args.path.clone(),
            eager_system_dict: args.system_dict.clone(),
            eager_user_dict: args.user_dict.clone(),
        }
    }
}
