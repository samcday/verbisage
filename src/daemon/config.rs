use std::collections::HashMap;

use crate::backends::BackendDef;
use crate::cli::SharedArgs;
use crate::dictionary::paths::LanguagePaths;

pub struct DaemonConfig {
    pub backend_chain: String,
    pub named_backends: HashMap<String, BackendDef>,
    pub language_paths: LanguagePaths,
    pub default_lang: String,
}

impl DaemonConfig {
    pub fn default_for(lang: &str) -> Self {
        Self {
            backend_chain: "file".into(),
            named_backends: HashMap::new(),
            language_paths: LanguagePaths::new(lang),
            default_lang: lang.to_string(),
        }
    }

    pub fn from_cli(args: &SharedArgs) -> Self {
        let default_lang = args.lang().to_string();
        let lp = crate::cli::base_language_paths(args);
        Self {
            backend_chain: args.backend.clone().unwrap_or_else(|| "file".into()),
            named_backends: HashMap::new(),
            language_paths: lp,
            default_lang,
        }
    }
}
