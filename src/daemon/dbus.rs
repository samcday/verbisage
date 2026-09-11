use zbus::fdo::Error as FdoError;
use zbus::interface;

use crate::dictionary::DictionaryQuery;

use super::DaemonHandler;

/// D-Bus interface exposing dictionary, spell-check, and prediction methods.
///
/// Registered at the well-known name `org.verbisage.Dictionary`, object path
/// `/org/verbisage/Dictionary`, interface `org.verbisage.Dictionary1`.
pub struct VerbisageDbus {
    handler: std::sync::Arc<DaemonHandler>,
    completion_slot: std::sync::Arc<tokio::sync::Semaphore>,
    completion_runtime: tokio::runtime::Handle,
    #[cfg(feature = "swipe")]
    swipe_slot: std::sync::Arc<tokio::sync::Semaphore>,
    #[cfg(feature = "swipe")]
    swipe_runtime: Option<tokio::runtime::Handle>,
}

impl VerbisageDbus {
    pub fn new(handler: DaemonHandler) -> Self {
        Self {
            handler: std::sync::Arc::new(handler),
            completion_slot: std::sync::Arc::new(tokio::sync::Semaphore::new(2)),
            completion_runtime: completion_runtime(),
            #[cfg(feature = "swipe")]
            swipe_slot: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            #[cfg(feature = "swipe")]
            swipe_runtime: tokio::runtime::Handle::try_current().ok(),
        }
    }
}

// Synchronous/P2P library users may register this object outside Tokio.
// Reuse one small runtime in that case; daemon users keep their existing runtime.
fn completion_runtime() -> tokio::runtime::Handle {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    tokio::runtime::Handle::try_current().unwrap_or_else(|_| {
        RUNTIME
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_time()
                    .build()
                    .expect("create completion worker runtime")
            })
            .handle()
            .clone()
    })
}

impl VerbisageDbus {
    async fn completion_request(
        &self,
        params: super::protocol::CompleteParams,
        lang: String,
    ) -> Result<Vec<(String, f64)>, FdoError> {
        let context: Vec<_> = params.context.iter().map(String::as_str).collect();
        self.handler
            .validate_complete(
                &crate::completion::CompletionInput {
                    input: &params.word,
                    context: &context,
                    input_prep: params.options.input_prep,
                    context_prep: params.options.context_prep,
                    case_preference: params.options.case_preference,
                },
                params.max,
            )
            .map_err(FdoError::InvalidArgs)?;
        if params.max == 0 {
            return Ok(Vec::new());
        }
        let permit = self
            .completion_slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| FdoError::Failed("completion workers are busy".into()))?;
        let deadline = self.handler.completion_deadline();
        let handler = self.handler.clone();
        let worker = self.completion_runtime.spawn_blocking(move || {
            let _permit = permit;
            handler.complete_request(&params, &lang)
        });
        let rows = self
            .completion_runtime
            .spawn(async move { tokio::time::timeout(deadline, worker).await })
            .await
            .map_err(|_| FdoError::Failed("completion response task failed".into()))?
            .map_err(|_| FdoError::Failed("completion exceeded its response deadline".into()))?
            .map_err(|_| FdoError::Failed("completion worker failed".into()))?
            .map_err(log_and_err)?;
        Ok(rows.into_iter().map(|r| (r.word, r.confidence)).collect())
    }
}

fn log_and_err(msg: String) -> FdoError {
    eprintln!("[dbus-server] error: {}", msg);
    FdoError::Failed(msg)
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    use crate::dictionary::{DictionaryBackend, DictionaryResult};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::time::Duration;

    struct BlockedDictionary {
        release: Arc<AtomicBool>,
        entered: Arc<AtomicUsize>,
    }
    impl DictionaryBackend for BlockedDictionary {
        fn query_prefixes(&self, _: &[DictionaryQuery]) -> Vec<DictionaryResult> {
            self.entered.fetch_add(1, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            vec![]
        }
        fn get_frequency(&self, _: &str) -> f64 {
            -1.0
        }
        fn contains(&self, _: &str) -> bool {
            false
        }
    }
    struct ReleaseOnDrop(Arc<AtomicBool>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timed_out_completion_keeps_both_worker_permits_until_exit() {
        let release = Arc::new(AtomicBool::new(false));
        let _release_on_failure = ReleaseOnDrop(release.clone());
        let entered = Arc::new(AtomicUsize::new(0));
        let service = Arc::new(VerbisageDbus::new(DaemonHandler::new(
            Box::new(BlockedDictionary {
                release: release.clone(),
                entered: entered.clone(),
            }),
            None,
            None,
            "en_US".into(),
        )));
        let mut requests = Vec::new();
        for _ in 0..2 {
            let service = service.clone();
            requests.push(tokio::spawn(async move {
                service.complete("h", 6, "en_US").await
            }));
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while entered.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            service
                .complete("h", 6, "en_US")
                .await
                .unwrap_err()
                .to_string()
                .contains("busy")
        );
        assert!(
            !tokio::time::timeout(
                Duration::from_millis(200),
                service.is_correct("hello", "en_US")
            )
            .await
            .unwrap()
            .unwrap()
        );
        assert!(
            service
                .complete("h", 1001, "en_US")
                .await
                .unwrap_err()
                .to_string()
                .contains("cap")
        );
        for request in requests {
            assert!(
                request
                    .await
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("deadline")
            );
        }
        assert!(
            service
                .complete("h", 6, "en_US")
                .await
                .unwrap_err()
                .to_string()
                .contains("busy")
        );
        assert_eq!(entered.load(Ordering::SeqCst), 2);
        release.store(true, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(1), async {
            while service.completion_slot.available_permits() != 2 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[interface(name = "org.verbisage.Dictionary1", introspection_docs = true)]
impl VerbisageDbus {
    /// Check whether a single word is recognised by the dictionary for the
    /// given language.
    ///
    /// @param word  Word to check.
    /// @param lang  BCP-47 / POSIX language tag (e.g. "en_US").
    /// @return      `true` if @word is recognised, `false` otherwise.
    #[zbus(out_args("result"))]
    async fn is_correct(&self, word: &str, lang: &str) -> Result<bool, FdoError> {
        crate::veprintln!("[dbus-server] IsCorrect({}, {})", word, lang);
        self.handler.is_correct(word, lang).map_err(log_and_err)
    }

    /// Return spelling suggestions (corrections) for a given word in the
    /// given language.
    ///
    /// @param word  Word to correct.
    /// @param max   Maximum number of suggestions to return.
    /// @param lang  Language tag.
    /// @return      Ordered list of spelling suggestions (may be empty).
    #[zbus(out_args("result"))]
    async fn suggest(&self, word: &str, max: u32, lang: &str) -> Result<Vec<String>, FdoError> {
        crate::veprintln!("[dbus-server] Suggest({}, {}, {})", word, max, lang);
        self.handler
            .suggest(word, max as usize, lang)
            .map_err(log_and_err)
    }

    /// Rank current-word prefixes and single-edit corrections together.
    /// Scores are relative heuristics, not probabilities. The caller keeps
    /// the exact typed literal as an independent choice. Known words and
    /// fragments shorter than three characters receive prefix matches only.
    /// Empty/whitespace input or max=0 returns []; max above the configured
    /// Complete cap (default 1_000) is rejected by the handler.
    #[zbus(out_args("result"))]
    async fn complete(
        &self,
        word: &str,
        max: u32,
        lang: &str,
    ) -> Result<Vec<(String, f64)>, FdoError> {
        // Legacy clients retain the empty-input convention. CompleteWith and
        // PredictWith explicitly request next-word candidates.
        self.handler
            .validate_complete(&crate::completion::CompletionInput::default(), max as usize)
            .map_err(FdoError::InvalidArgs)?;
        if word.len() > 512 || word.chars().count() > 128 || word.chars().any(char::is_control) {
            return Err(FdoError::InvalidArgs(
                "completion word is too large or contains control characters".into(),
            ));
        }
        if word.is_empty() || word.chars().any(char::is_whitespace) {
            return Ok(Vec::new());
        }
        self.completion_request(
            super::protocol::CompleteParams {
                word: word.into(),
                context: vec![],
                max: max as usize,
                options: Default::default(),
            },
            lang.into(),
        )
        .await
    }

    /// Complete using committed context and explicit per-request preparation.
    /// Each prep tuple is (normalization, fold); defaults are (none, none).
    #[zbus(out_args("result"))]
    async fn complete_with(
        &self,
        word: String,
        context: Vec<String>,
        max: u32,
        lang: String,
        input_prep: (String, String),
        context_prep: (String, String),
        case_preference: String,
    ) -> Result<Vec<(String, f64)>, FdoError> {
        let options = super::protocol::CompletionOptions {
            input_prep: crate::text::TextPrep::from_names(&input_prep.0, &input_prep.1)
                .map_err(FdoError::InvalidArgs)?,
            context_prep: crate::text::TextPrep::from_names(&context_prep.0, &context_prep.1)
                .map_err(FdoError::InvalidArgs)?,
            case_preference: serde_json::from_value(serde_json::json!(case_preference))
                .map_err(|e| FdoError::InvalidArgs(e.to_string()))?,
        };
        self.completion_request(
            super::protocol::CompleteParams {
                word,
                context,
                max: max as usize,
                options,
            },
            lang,
        )
        .await
    }

    /// Next-word prediction uses the same engine and text options as completion.
    #[zbus(out_args("result"))]
    async fn predict_with(
        &self,
        context: Vec<String>,
        max: u32,
        lang: String,
        context_prep: (String, String),
    ) -> Result<Vec<(String, f64)>, FdoError> {
        let prep = crate::text::TextPrep::from_names(&context_prep.0, &context_prep.1)
            .map_err(FdoError::InvalidArgs)?;
        self.completion_request(
            super::protocol::CompleteParams {
                word: String::new(),
                context,
                max: max as usize,
                options: super::protocol::CompletionOptions {
                    context_prep: prep,
                    ..Default::default()
                },
            },
            lang,
        )
        .await
    }

    /// Resolve a complete single-finger word gesture against supplied key bounds.
    /// Coordinates share widget logical units. Each point includes elapsed ms.
    /// The caller must discard stale replies and require explicit word selection.
    #[cfg(feature = "swipe")]
    #[zbus(out_args("result"))]
    async fn recognize_swipe(
        &self,
        trace: Vec<(f64, f64, u32)>,
        keys: Vec<(String, f64, f64, f64, f64)>,
        max: u32,
        lang: String,
    ) -> Result<Vec<(String, f64)>, FdoError> {
        let request =
            crate::swipe::SwipeRequest::new(trace, keys, max).map_err(FdoError::InvalidArgs)?;
        // zbus uses its own executor in the existing daemon configuration. Use
        // the Tokio handle captured when the daemon registered this interface;
        // neither worker creation nor the timer may assume a Tokio caller.
        let runtime = self
            .swipe_runtime
            .as_ref()
            .ok_or_else(|| FdoError::Failed("swipe worker runtime is unavailable".into()))?;
        let permit = self
            .swipe_slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| FdoError::Failed("swipe recognition is busy".into()))?;
        let handler = self.handler.clone();
        let worker = runtime.spawn_blocking(move || {
            // A timed-out worker retains its permit until it finishes: incoming
            // requests cannot build an unbounded queue of abandoned CPU work.
            let _permit = permit;
            handler.recognize_swipe(request, &lang)
        });
        runtime.spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(1), worker).await
        }).await
            .map_err(|_| FdoError::Failed("swipe response task failed".into()))?
            .map_err(|_| FdoError::Failed("swipe recognition exceeded its response deadline".into()))?
            .map_err(|_| FdoError::Failed("swipe recognition worker failed".into()))?
            .map_err(log_and_err)
    }

    /// Search the dictionary for words matching prefix and/or suffix
    /// constraints.  When both `prefixes` and `suffixes` are empty every
    /// entry is a candidate (subject to length bounds).
    ///
    /// @param prefixes  Required prefixes (empty = no prefix constraint).
    /// @param suffixes  Required suffixes (empty = no suffix constraint).
    /// @param min_len   Minimum word length (0 = no constraint).
    /// @param max_len   Maximum word length (0 = no constraint).
    /// @param lang      Language tag.
    /// @return          Array of (word, confidence) pairs matching the
    ///                  Cartesian product of @prefixes × @suffixes.
    #[zbus(out_args("result"))]
    async fn query(
        &self,
        prefixes: Vec<String>,
        suffixes: Vec<String>,
        min_len: u32,
        max_len: u32,
        lang: &str,
    ) -> Result<Vec<(String, f64)>, FdoError> {
        crate::veprintln!(
            "[dbus-server] Query({:?}, {:?}, {}, {}, {})",
            prefixes,
            suffixes,
            min_len,
            max_len,
            lang
        );
        let queries = dictionary_queries(prefixes, suffixes, min_len, max_len);
        self.handler
            .query(&queries, lang)
            .map(|results| {
                results
                    .into_iter()
                    .map(|r| (r.word, r.confidence))
                    .collect()
            })
            .map_err(log_and_err)
    }

    /// Query completion candidates with a bounded response. Zero max returns
    /// no candidates; max above the configured bounded-query cap (default
    /// 200_000) is rejected by the handler. Query remains available for
    /// clients that need the complete dictionary result set.
    #[zbus(out_args("result"))]
    async fn query_limited(
        &self,
        prefixes: Vec<String>,
        suffixes: Vec<String>,
        min_len: u32,
        max_len: u32,
        lang: &str,
        max: u32,
    ) -> Result<Vec<(String, f64)>, FdoError> {
        if prefixes.len() > 16
            || suffixes.len() > 16
            || prefixes.iter().chain(&suffixes).any(|s| s.len() > 256)
        {
            return Err(FdoError::InvalidArgs(
                "completion query is too large".into(),
            ));
        }
        if max == 0 {
            return Ok(Vec::new());
        }
        let queries = dictionary_queries(prefixes, suffixes, min_len, max_len);
        self.handler
            .query_limited(&queries, lang, max as usize)
            .map(|results| {
                results
                    .into_iter()
                    .map(|r| (r.word, r.confidence))
                    .collect()
            })
            .map_err(log_and_err)
    }

    /// Predict the next word(s) given a sequence of context words.
    ///
    /// @param context  Preceding words (space-split is the caller's
    ///                  responsibility).
    /// @param max      Maximum number of predictions to return.
    /// @param lang     Language tag.
    /// @return         Array of (word, confidence) predictions.
    #[zbus(out_args("result"))]
    async fn predict(
        &self,
        context: Vec<String>,
        max: u32,
        lang: &str,
    ) -> Result<Vec<(String, f64)>, FdoError> {
        self.completion_request(
            super::protocol::CompleteParams {
                word: String::new(),
                context,
                max: max as usize,
                options: Default::default(),
            },
            lang.into(),
        )
        .await
    }

    /// Return the frequency / rank of a word in the dictionary for the
    /// given language.
    ///
    /// @param word  Word to look up.
    /// @param lang  Language tag.
    /// @return      Frequency score (0.0 = unknown word).
    #[zbus(out_args("result"))]
    async fn frequency(&self, word: &str, lang: &str) -> Result<f64, FdoError> {
        crate::veprintln!("[dbus-server] Frequency({}, {})", word, lang);
        self.handler.frequency(word, lang).map_err(log_and_err)
    }

    /// Add a word to the dictionary for the given language.
    ///
    /// @param word          Word to add.
    /// @param frequency     Frequency score for the word.
    /// @param allow_existing If true, overwrite when the word already exists.
    /// @param lang          Language tag.
    #[zbus(out_args("result"))]
    async fn add_word(
        &self,
        word: &str,
        frequency: f64,
        allow_existing: bool,
        lang: &str,
    ) -> Result<bool, FdoError> {
        crate::veprintln!(
            "[dbus-server] AddWord({}, {}, {}, {})",
            word,
            frequency,
            allow_existing,
            lang
        );
        self.handler
            .add_word(word, frequency, allow_existing, lang)
            .map(|_| true)
            .map_err(log_and_err)
    }

    /// Increase the frequency of an n-gram for the given language.
    ///
    /// @param ngram        Full n-gram sequence (context + next word).
    /// @param delta        Amount to increase the frequency by.
    /// @param save_unknown If true, create the n-gram if it doesn't exist.
    /// @param lang         Language tag.
    #[zbus(out_args("result"))]
    async fn bump_ngram(
        &self,
        ngram: Vec<String>,
        delta: f64,
        save_unknown: bool,
        lang: &str,
    ) -> Result<bool, FdoError> {
        crate::veprintln!(
            "[dbus-server] BumpNgram({:?}, {}, {}, {})",
            ngram,
            delta,
            save_unknown,
            lang
        );
        self.handler
            .increase_ngram_frequency(&ngram, delta, save_unknown, lang)
            .map(|_| true)
            .map_err(log_and_err)
    }
}

/// Register on the session bus and serve forever.
pub async fn run(handler: DaemonHandler) -> zbus::Result<()> {
    let dbus_obj = VerbisageDbus::new(handler);
    crate::veprintln!("[daemon] connecting to dbus...");
    let _conn = zbus::connection::Builder::session()?
        .name("org.verbisage.Dictionary")?
        .serve_at("/org/verbisage/Dictionary", dbus_obj)?
        .build()
        .await?;
    eprintln!("[daemon] connected to dbus");
    std::future::pending::<()>().await;
    Ok(())
}

fn dictionary_queries(
    prefixes: Vec<String>,
    suffixes: Vec<String>,
    min_len: u32,
    max_len: u32,
) -> Vec<DictionaryQuery> {
    let prefix_opts: Vec<Option<String>> = if prefixes.is_empty() {
        vec![None]
    } else {
        prefixes.into_iter().map(Some).collect()
    };
    let suffix_opts: Vec<Option<String>> = if suffixes.is_empty() {
        vec![None]
    } else {
        suffixes.into_iter().map(Some).collect()
    };
    let queries: Vec<DictionaryQuery> = prefix_opts
        .into_iter()
        .flat_map(|p| {
            suffix_opts.iter().map(move |s| DictionaryQuery {
                prefix: p.clone(),
                suffix: s.clone(),
                min_length: if min_len == 0 {
                    None
                } else {
                    Some(min_len as usize)
                },
                max_length: if max_len == 0 {
                    None
                } else {
                    Some(max_len as usize)
                },
            })
        })
        .collect();
    queries
}

#[cfg(all(test, feature = "swipe"))]
mod swipe_tests {
    use super::*;
    use crate::dictionary::{DictionaryBackend, DictionaryQuery, DictionaryResult};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::time::Duration;

    struct BlockedDictionary {
        release: Arc<AtomicBool>,
        entered: Arc<AtomicUsize>,
    }
    impl DictionaryBackend for BlockedDictionary {
        fn query_prefixes(&self, _: &[DictionaryQuery]) -> Vec<DictionaryResult> {
            Vec::new()
        }
        fn get_frequency(&self, _: &str) -> f64 {
            0.0
        }
        fn contains(&self, _: &str) -> bool {
            false
        }
        fn swipe_candidates(
            &self,
            _: &[String],
            _: &[String],
            _: &[u8],
            _: std::time::Instant,
        ) -> Result<Vec<DictionaryResult>, String> {
            self.entered.fetch_add(1, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(Vec::new())
        }
    }
    struct ReleaseOnDrop(Arc<AtomicBool>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    fn arguments() -> (Vec<(f64, f64, u32)>, Vec<(String, f64, f64, f64, f64)>) {
        (
            vec![(10.0, 10.0, 0), (50.0, 10.0, 100)],
            vec![
                ("a".into(), 0.0, 0.0, 20.0, 20.0),
                ("b".into(), 40.0, 0.0, 20.0, 20.0),
            ],
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn swipe_deadline_keeps_one_worker_and_complete_responsive() {
        let release = Arc::new(AtomicBool::new(false));
        let _release_on_failure = ReleaseOnDrop(release.clone());
        let entered = Arc::new(AtomicUsize::new(0));
        let handler = DaemonHandler::new(
            Box::new(BlockedDictionary {
                release: release.clone(),
                entered: entered.clone(),
            }),
            None,
            None,
            "en_US".into(),
        );
        let dbus = Arc::new(VerbisageDbus::new(handler));
        let first = dbus.clone();
        let worker = tokio::spawn(async move {
            let (trace, keys) = arguments();
            first.recognize_swipe(trace, keys, 6, "en_US".into()).await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while entered.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("worker entered candidate search");
        let (trace, keys) = arguments();
        assert!(
            dbus.recognize_swipe(trace, keys, 6, "en_US".into())
                .await
                .unwrap_err()
                .to_string()
                .contains("busy")
        );
        let completed =
            tokio::time::timeout(Duration::from_millis(200), dbus.complete("hel", 6, "en_US"))
                .await
                .unwrap()
                .unwrap();
        assert!(completed.is_empty());
        assert!(
            worker
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("deadline")
        );
        let (trace, keys) = arguments();
        assert!(
            dbus.recognize_swipe(trace, keys, 6, "en_US".into())
                .await
                .unwrap_err()
                .to_string()
                .contains("busy")
        );
        assert_eq!(entered.load(Ordering::SeqCst), 1);
        release.store(true, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(1), async {
            while dbus.swipe_slot.available_permits() != 1 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
}
