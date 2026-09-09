use std::collections::HashMap;

use crate::backends::BackendDef;
use crate::cli::SharedArgs;
use crate::dictionary::paths::LanguagePaths;

pub struct DaemonConfig {
    pub backend_chain: String,
    pub named_backends: HashMap<String, BackendDef>,
    pub language_paths: LanguagePaths,
    pub default_lang: String,
    /// Cap on accepted `Complete` `max` values; oversized requests are rejected.
    pub max_complete_results: usize,
    /// Cap on accepted bounded-query `max` values; oversized requests are rejected.
    pub max_query_results: usize,
}

impl DaemonConfig {
    pub fn default_for(lang: &str) -> Self {
        let completion = crate::completion::CompletionConfig::default();
        Self {
            backend_chain: "file".into(),
            named_backends: HashMap::new(),
            language_paths: LanguagePaths::new(lang),
            default_lang: lang.to_string(),
            max_complete_results: completion.max_complete_results,
            max_query_results: completion.max_query_results,
        }
    }

    pub fn from_cli(args: &SharedArgs) -> Self {
        let default_lang = args.lang().to_string();
        let mut lp = crate::cli::base_language_paths(args);
        lp.language = default_lang.clone();
        let completion = crate::completion::CompletionConfig::default();
        Self {
            backend_chain: args.backend.clone().unwrap_or_else(|| "file".into()),
            named_backends: HashMap::new(),
            language_paths: lp,
            default_lang,
            max_complete_results: args
                .max_complete_results
                .unwrap_or(completion.max_complete_results),
            max_query_results: args
                .max_query_results
                .unwrap_or(completion.max_query_results),
        }
    }
}
