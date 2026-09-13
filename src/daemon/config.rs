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
    pub completion: crate::completion::CompletionConfig,
    /// Concurrent gesture recognitions. Each permit is one CPU worker, held
    /// until that work really exits, so this bounds abandoned work too.
    pub swipe_workers: usize,
}

/// Recognition workers when nothing overrides it.
pub const DEFAULT_SWIPE_WORKERS: usize = 2;

/// Reject unusable worker counts before constructing the recognition semaphore.
pub fn validate_swipe_workers(value: usize) -> Result<usize, String> {
    if value == 0 {
        return Err("swipe workers must be at least 1".into());
    }
    #[cfg(feature = "swipe")]
    if value > tokio::sync::Semaphore::MAX_PERMITS {
        return Err(format!(
            "swipe workers must not exceed {}",
            tokio::sync::Semaphore::MAX_PERMITS
        ));
    }
    Ok(value)
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
            completion,
            swipe_workers: DEFAULT_SWIPE_WORKERS,
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
            completion,
            swipe_workers: args.swipe_workers.unwrap_or(DEFAULT_SWIPE_WORKERS),
        }
    }
}

#[cfg(all(test, feature = "swipe"))]
mod tests {
    use super::validate_swipe_workers;
    use tokio::sync::Semaphore;

    #[test]
    fn swipe_workers_accept_semaphore_limit() {
        let workers = validate_swipe_workers(Semaphore::MAX_PERMITS).unwrap();
        assert_eq!(workers, Semaphore::MAX_PERMITS);
        assert_eq!(Semaphore::new(workers).available_permits(), workers);
    }

    #[test]
    fn swipe_workers_reject_above_semaphore_limit() {
        assert!(validate_swipe_workers(Semaphore::MAX_PERMITS + 1).is_err());
        assert!(validate_swipe_workers(usize::MAX).is_err());
    }
}
