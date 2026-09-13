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
        #[cfg(feature = "swipe")]
        // Configured, never clamped: a caller that asked for three workers gets
        // three. Zero is rejected where the configuration is resolved, and
        // would fail requests as busy rather than deadlock if it reached here.
        let swipe_workers = handler.swipe_workers();
        Self {
            handler: std::sync::Arc::new(handler),
            // Completion capacity is separate so recognition cannot starve it.
            completion_slot: std::sync::Arc::new(tokio::sync::Semaphore::new(2)),
            completion_runtime: completion_runtime(),
            #[cfg(feature = "swipe")]
            swipe_slot: std::sync::Arc::new(tokio::sync::Semaphore::new(swipe_workers)),
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
                    spatial: crate::spatial::SpatialInput::None,
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

/// Narrow D-Bus f64 touch coordinates to the f32 geometry space.
///
/// Non-finite values and values outside the f32 range are invalid arguments,
/// not values to clamp or let saturate. Rejecting them before the handler is
/// entered keeps a bad call from becoming a late generic failure, including
/// when `max` is zero and no backend would otherwise be reached.
fn checked_touch_points(
    points: &[(f64, f64)],
) -> Result<Vec<crate::spatial::TouchPoint>, FdoError> {
    let limit = f32::MAX as f64;
    let invalid = || {
        FdoError::InvalidArgs(
            "touch point coordinates must be finite and representable as f32".into(),
        )
    };

    points
        .iter()
        .map(|&(x, y)| {
            if !x.is_finite() || !y.is_finite() || x.abs() > limit || y.abs() > limit {
                return Err(invalid());
            }
            let point = crate::spatial::TouchPoint::new(x as f32, y as f32);
            if !point.x.is_finite() || !point.y.is_finite() {
                return Err(invalid());
            }
            Ok(point)
        })
        .collect()
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

    fn empty_service() -> VerbisageDbus {
        VerbisageDbus::new(DaemonHandler::new(
            Box::new(crate::dictionary::FileDictionaryBackend::new()),
            None,
            None,
            "en_US".into(),
        ))
    }

    #[tokio::test]
    async fn non_finite_or_unrepresentable_touch_points_are_invalid_args() {
        let service = empty_service();
        let invalid = [
            (f64::NAN, 0.0),
            (0.0, f64::NAN),
            (f64::INFINITY, 0.0),
            (0.0, f64::NEG_INFINITY),
            (f64::MAX, 0.0),
            (-1.0e39, 0.0),
        ];

        for (x, y) in invalid {
            let error = service
                .complete_with(
                    "h".into(),
                    vec![],
                    0,
                    "en_US".into(),
                    ("nfc".into(), "full".into()),
                    ("nfc".into(), "full".into()),
                    "insensitive".into(),
                    String::new(),
                    vec![(x, y)],
                )
                .await
                .unwrap_err();
            assert!(
                matches!(error, FdoError::InvalidArgs(_)),
                "complete_with({x}, {y}) was not InvalidArgs: {error}"
            );

            let error = service
                .suggest("h", 6, "en_US", "", vec![(x, y)])
                .await
                .unwrap_err();
            assert!(
                matches!(error, FdoError::InvalidArgs(_)),
                "suggest({x}, {y}) was not InvalidArgs: {error}"
            );
        }
    }

    #[tokio::test]
    async fn representable_fractional_and_negative_touch_points_are_accepted() {
        let service = empty_service();

        let rows = service
            .complete_with(
                "h".into(),
                vec![],
                0,
                "en_US".into(),
                ("nfc".into(), "full".into()),
                ("nfc".into(), "full".into()),
                "insensitive".into(),
                String::new(),
                vec![(-12.5, 0.25), (3.0, -4.75)],
            )
            .await
            .unwrap();
        assert!(rows.is_empty());

        let suggestions = service
            .suggest("h", 6, "en_US", "", vec![(-12.5, 0.25)])
            .await
            .unwrap();
        assert!(suggestions.is_empty());
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
    async fn suggest(
        &self,
        word: &str,
        max: u32,
        lang: &str,
        layout: &str,
        points: Vec<(f64, f64)>,
    ) -> Result<Vec<String>, FdoError> {
        crate::veprintln!("[dbus-server] Suggest({}, {}, {}, {})", word, max, lang, layout);
        let layout = if layout.is_empty() {
            None
        } else {
            Some(self.handler.layout(layout).ok_or_else(|| {
                FdoError::InvalidArgs(format!("unknown layout token '{layout}'"))
            })?)
        };
        let points = checked_touch_points(&points)?;
        self.handler
            .suggest_with(word, max as usize, lang, layout, points)
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
                layout: None,
                points: Vec::new(),
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
        layout: String,
        points: Vec<(f64, f64)>,
    ) -> Result<Vec<(String, f64)>, FdoError> {
        let options = super::protocol::CompletionOptions {
            input_prep: crate::text::TextPrep::from_names(&input_prep.0, &input_prep.1)
                .map_err(FdoError::InvalidArgs)?,
            context_prep: crate::text::TextPrep::from_names(&context_prep.0, &context_prep.1)
                .map_err(FdoError::InvalidArgs)?,
            case_preference: serde_json::from_value(serde_json::json!(case_preference))
                .map_err(|e| FdoError::InvalidArgs(e.to_string()))?,
        };
        // Validate and narrow before the request, so a bad coordinate is an
        // argument error even when max is zero and the early return would
        // otherwise answer with an empty result.
        let points = checked_touch_points(&points)?;
        self.completion_request(
            super::protocol::CompleteParams {
                word,
                context,
                max: max as usize,
                options,
                layout: (!layout.is_empty()).then_some(layout),
                points: points.into_iter().map(|point| [point.x, point.y]).collect(),
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
                layout: None,
                points: Vec::new(),
            },
            lang,
        )
        .await
    }

    /// Resolve a complete single-finger word gesture against a registered
    /// layout. `layout` is a token from `RegisterLayout`: the same registry
    /// and the same immutable layout completion uses. Trace points are in
    /// that layout's own widget coordinates, each with its elapsed ms, and are
    /// normalized once by the layout. Main and alternate labels locate a
    /// word's graphemes in one canonical form (NFC, lowercase); results keep
    /// the stored spelling. An unknown or empty token is an explicit error.
    /// A `max` of zero asks for nothing and gets nothing at once: no token is
    /// resolved, no trace validated and no worker taken, so it cannot fail
    /// busy. The caller must discard stale replies and require explicit word
    /// selection.
    #[cfg(feature = "swipe")]
    #[zbus(out_args("result"))]
    async fn recognize_swipe(
        &self,
        trace: Vec<(f64, f64, u32)>,
        layout: &str,
        max: u32,
        lang: String,
    ) -> Result<Vec<(String, f64)>, FdoError> {
        crate::veprintln!(
            "[dbus-server] RecognizeSwipe({} points, {}, {}, {})",
            trace.len(),
            layout,
            max,
            lang
        );
        if max == 0 {
            return Ok(Vec::new());
        }
        if layout.is_empty() {
            return Err(FdoError::InvalidArgs(
                "swipe recognition requires a registered layout token".into(),
            ));
        }
        // Resolved here, before any worker is dispatched: the request keeps
        // this immutable layout, so forgetting or evicting the token afterwards
        // cannot change a recognition already accepted.
        let layout = self
            .handler
            .layout(layout)
            .ok_or_else(|| FdoError::InvalidArgs(format!("unknown layout token '{layout}'")))?;
        let request =
            crate::swipe::SwipeRequest::new(trace, layout, max).map_err(FdoError::InvalidArgs)?;
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
                layout: None,
                points: Vec::new(),
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

    /// Increase the count of an n-gram for the given language.
    ///
    /// @param ngram        Full n-gram sequence (context + next word).
    /// @param count        Number of observations to add.
    /// @param save_unknown If true, create the n-gram if it doesn't exist.
    /// @param lang         Language tag.
    #[zbus(out_args("result"))]
    async fn bump_ngram(
        &self,
        ngram: Vec<String>,
        count: u64,
        save_unknown: bool,
        lang: &str,
    ) -> Result<bool, FdoError> {
        crate::veprintln!(
            "[dbus-server] BumpNgram({:?}, {}, {}, {})",
            ngram,
            count,
            save_unknown,
            lang
        );
        self.handler
            .increase_ngram_count(&ngram, count, save_unknown, lang)
            .map(|_| true)
            .map_err(log_and_err)
    }

    /// Register a keyboard layout and return its content-hash token. The
    /// layout is a JSON-encoded `LayoutUpload`.
    #[zbus(out_args("result"))]
    async fn register_layout(&self, layout: &str) -> Result<String, FdoError> {
        crate::veprintln!("[dbus-server] RegisterLayout(...)");
        // Bound the raw string before parsing: a malformed or oversized upload
        // must not reach serde_json or the registry.
        if layout.len() > crate::layout::MAX_LAYOUT_UPLOAD_BYTES {
            return Err(FdoError::InvalidArgs(format!(
                "layout upload exceeds {} bytes",
                crate::layout::MAX_LAYOUT_UPLOAD_BYTES
            )));
        }
        let upload: crate::layout::LayoutUpload = serde_json::from_str(layout)
            .map_err(|error| FdoError::InvalidArgs(error.to_string()))?;
        crate::layout::validate_upload(&upload).map_err(FdoError::InvalidArgs)?;
        self.handler.register_layout(&upload).map_err(log_and_err)
    }

    /// Forget a previously registered keyboard layout.
    #[zbus(out_args("result"))]
    async fn forget_layout(&self, token: &str) -> Result<bool, FdoError> {
        crate::veprintln!("[dbus-server] ForgetLayout({})", token);
        self.handler.forget_layout(token).map_err(log_and_err)
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
            _: &crate::swipe::SwipeVocabulary,
            _: &[String],
            _: &[String],
            _: std::time::Instant,
        ) -> Result<Vec<crate::swipe::SwipeCandidate>, String> {
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
    fn trace() -> Vec<(f64, f64, u32)> {
        vec![(10.0, 10.0, 0), (50.0, 10.0, 100)]
    }
    /// Two keys registered through the shared registry; requests carry the
    /// token, never the geometry.
    async fn registered(service: &VerbisageDbus) -> String {
        let upload = serde_json::json!({ "keys": [
            { "label": "a", "left": 0.0, "top": 0.0, "width": 20.0, "height": 20.0 },
            { "label": "b", "left": 40.0, "top": 0.0, "width": 20.0, "height": 20.0 },
        ] });
        service.register_layout(&upload.to_string()).await.unwrap()
    }

    /// Exactly `workers` recognitions may occupy CPU workers at once; the next
    /// is refused as busy, completion stays responsive meanwhile, and every
    /// permit is retained until the abandoned CPU work actually exits.
    async fn configured_workers_case(workers: usize) {
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
        )
        .with_swipe_workers(workers);
        let dbus = Arc::new(VerbisageDbus::new(handler));
        assert_eq!(dbus.swipe_slot.available_permits(), workers);
        let token = registered(&dbus).await;

        let mut running = Vec::new();
        for _ in 0..workers {
            let service = dbus.clone();
            let token = token.clone();
            running.push(tokio::spawn(async move {
                service
                    .recognize_swipe(trace(), &token, 6, "en_US".into())
                    .await
            }));
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while entered.load(Ordering::SeqCst) < workers {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("every configured worker entered candidate search");

        // One more than configured is backpressure, not a queue.
        assert!(
            dbus.recognize_swipe(trace(), &token, 6, "en_US".into())
                .await
                .unwrap_err()
                .to_string()
                .contains("busy")
        );

        // Completion capacity is separate from recognition capacity.
        let completed =
            tokio::time::timeout(Duration::from_millis(200), dbus.complete("hel", 6, "en_US"))
                .await
                .expect("completion answered while recognition is saturated")
                .unwrap();
        assert!(completed.is_empty());

        for job in running {
            assert!(
                job.await
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("deadline")
            );
        }

        // The responses gave up, but the CPU work has not: the permits are
        // still held, so abandoned work cannot pile up behind them.
        assert_eq!(dbus.swipe_slot.available_permits(), 0);
        assert!(
            dbus.recognize_swipe(trace(), &token, 6, "en_US".into())
                .await
                .unwrap_err()
                .to_string()
                .contains("busy")
        );
        assert_eq!(entered.load(Ordering::SeqCst), workers);

        release.store(true, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(2), async {
            while dbus.swipe_slot.available_permits() != workers {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("permits returned once the work exited");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_configured_worker() {
        configured_workers_case(1).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_configured_workers_are_the_default() {
        assert_eq!(
            crate::daemon::DaemonConfig::default_for("en_US").swipe_workers,
            crate::daemon::DEFAULT_SWIPE_WORKERS
        );
        assert_eq!(crate::daemon::DEFAULT_SWIPE_WORKERS, 2);
        configured_workers_case(2).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn three_configured_workers_are_honoured_not_clamped() {
        configured_workers_case(3).await;
    }

    /// A request for no results is answered at once while every worker is
    /// occupied: it takes no permit, resolves no token and validates no trace,
    /// and a request that does want results is still refused as busy.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zero_max_is_answered_without_a_worker() {
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
        )
        .with_swipe_workers(1);
        let dbus = Arc::new(VerbisageDbus::new(handler));
        let token = registered(&dbus).await;
        let occupant = dbus.clone();
        let occupant_token = token.clone();
        let worker = tokio::spawn(async move {
            occupant
                .recognize_swipe(trace(), &occupant_token, 6, "en_US".into())
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while entered.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the occupant entered candidate search");
        assert_eq!(dbus.swipe_slot.available_permits(), 0);

        assert_eq!(
            dbus.recognize_swipe(trace(), &token, 0, "en_US".into())
                .await
                .unwrap(),
            Vec::new()
        );
        // No token or trace is looked at either: nothing is asked for.
        assert_eq!(
            dbus.recognize_swipe(trace(), "deadbeef", 0, "en_US".into())
                .await
                .unwrap(),
            Vec::new()
        );
        assert_eq!(
            dbus.recognize_swipe(Vec::new(), &token, 0, "en_US".into())
                .await
                .unwrap(),
            Vec::new()
        );
        assert_eq!(dbus.swipe_slot.available_permits(), 0);
        assert_eq!(entered.load(Ordering::SeqCst), 1);
        assert!(
            dbus.recognize_swipe(trace(), &token, 1, "en_US".into())
                .await
                .unwrap_err()
                .to_string()
                .contains("busy")
        );

        // Released well within its deadline, the occupant finishes normally
        // with the blocked dictionary's (empty) answer and returns its permit.
        release.store(true, Ordering::SeqCst);
        assert_eq!(worker.await.unwrap().unwrap(), Vec::new());
        tokio::time::timeout(Duration::from_secs(1), async {
            while dbus.swipe_slot.available_permits() != 1 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn zero_workers_is_rejected() {
        assert!(crate::daemon::validate_swipe_workers(0).is_err());
        assert_eq!(crate::daemon::validate_swipe_workers(3).unwrap(), 3);
    }

    /// The original single-worker case, now pinned to that configuration since
    /// the default is two.
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
        )
        .with_swipe_workers(1);
        let dbus = Arc::new(VerbisageDbus::new(handler));
        let token = registered(&dbus).await;
        let first = dbus.clone();
        let first_token = token.clone();
        let worker = tokio::spawn(async move {
            first
                .recognize_swipe(trace(), &first_token, 6, "en_US".into())
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while entered.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("worker entered candidate search");
        assert!(
            dbus.recognize_swipe(trace(), &token, 6, "en_US".into())
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
        assert!(
            dbus.recognize_swipe(trace(), &token, 6, "en_US".into())
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

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn empty_service() -> VerbisageDbus {
        VerbisageDbus::new(DaemonHandler::new(
            Box::new(crate::dictionary::FileDictionaryBackend::new()),
            None,
            None,
            "en_US".into(),
        ))
    }

    #[tokio::test]
    async fn oversized_layout_upload_is_rejected_before_parsing() {
        let service = empty_service();
        // Deliberately invalid JSON: the raw bound must reject it first.
        let oversized = "?".repeat(crate::layout::MAX_LAYOUT_UPLOAD_BYTES + 1);

        let error = service.register_layout(&oversized).await.unwrap_err();
        assert!(matches!(error, FdoError::InvalidArgs(_)), "{error}");
        assert!(error.to_string().contains("exceeds"), "{error}");
    }

    #[tokio::test]
    async fn structurally_oversized_layout_upload_is_invalid_args() {
        let service = empty_service();
        let upload = serde_json::json!({
            "keys": (0..crate::layout::MAX_LAYOUT_KEYS + 1)
                .map(|index| serde_json::json!({
                    "label": format!("k{index}"),
                    "left": 0.0,
                    "top": 0.0,
                    "width": 1.0,
                    "height": 1.0,
                }))
                .collect::<Vec<_>>(),
        });

        let error = service
            .register_layout(&upload.to_string())
            .await
            .unwrap_err();
        assert!(matches!(error, FdoError::InvalidArgs(_)), "{error}");
        assert!(error.to_string().contains("too many keys"), "{error}");
    }

    #[tokio::test]
    async fn valid_layout_registration_still_returns_a_token() {
        let service = empty_service();
        let upload = serde_json::json!({
            "keys": [{
                "label": "a",
                "left": 0.0,
                "top": 0.0,
                "width": 10.0,
                "height": 20.0,
            }],
        });

        let token = service.register_layout(&upload.to_string()).await.unwrap();
        assert!(!token.is_empty());
    }
}
