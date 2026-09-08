use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[cfg(feature = "patricia")]
pub mod patricia;

pub mod compact;
pub mod file;
pub mod paths;
pub mod subsequence;

#[cfg(feature = "hunspell")]
pub mod hunspell;
#[cfg(feature = "hunspell")]
pub use hunspell::HunspellDictionaryBackend;

#[cfg(feature = "marisa")]
pub mod marisa;
#[cfg(feature = "marisa")]
pub use marisa::MarisaDictionaryBackend;

pub use compact::CompactDictionary;
pub use file::FileDictionaryBackend;
pub use paths::LanguagePaths;

#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "sqlite")]
pub use sqlite::PresageSqliteBackend;

// ---------------------------------------------------------------------------
// Text conventions
// ---------------------------------------------------------------------------
//
// The library-wide internal text default is NFC. All text crossing backend
// API boundaries — queries, lookups, stored keys, returned words — is NFC.
// Caller input is assumed NFC; backends convert at ingest (text-built
// backends normalize words on load) and assume NFC on hot query paths
// without re-normalizing per call.
//
// Matching is exact (case-sensitive) by default. Backends perform no
// implicit case folding or other normalization; any normalization
// (lowercasing, transliteration, accent handling) is configured at backend
// construction time, never applied silently inside a query. Normalization
// pipelines, where used, start with an NFC stage.

// ---------------------------------------------------------------------------
// Query types
// ---------------------------------------------------------------------------

/// Constraint set for filtering dictionary entries.
///
/// Every field is optional — omitted fields are unconstrained. All string
/// comparison is exact and case-sensitive (see the text conventions above);
/// callers that want case-insensitive matching normalize both sides
/// explicitly before building a query.
#[derive(Debug, Clone)]
pub struct DictionaryQuery {
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
}

/// A single dictionary entry returned from a query.
#[derive(Debug, Clone, PartialEq)]
pub struct DictionaryResult {
    pub word: String,
    /// Normalized frequency in 0.0–1.0 (the word's share of the backend's
    /// unigram mass), or -1.0 when the backend has no frequency information
    /// for this entry. Backends keep native raw counts in storage and
    /// convert at this boundary.
    pub confidence: f64,
}

// ---------------------------------------------------------------------------
// Backend trait
// ---------------------------------------------------------------------------

/// Backend-agnostic dictionary query interface.
///
/// All implementors must be [`Send`] + [`Sync`] so that a single trait object
/// can be shared across threads (e.g. inside an `Arc`).
pub trait DictionaryBackend: Send + Sync {
    /// Whether no words are available. Loading failures return an empty file
    /// backend, which lets the daemon report unavailable language data.
    fn is_empty(&self) -> bool {
        false
    }

    /// Batch-query the dictionary against one or more constraint sets.
    ///
    /// The returned vector is sorted by descending confidence, then
    /// lexicographically by word.
    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult>;

    /// Return the highest ranked candidates, with a bounded output allocation.
    fn query_limited(&self, queries: &[DictionaryQuery], max: usize) -> Vec<DictionaryResult> {
        let mut results = self.query_prefixes(queries);
        results.truncate(max);
        results
    }

    /// Bounded candidate snapshot for the optional whole-word swipe prototype.
    #[cfg(feature = "swipe")]
    fn swipe_candidates(
        &self,
        _starts: &[String],
        _ends: &[String],
        _letters: &[u8],
        _deadline: std::time::Instant,
    ) -> Result<Vec<DictionaryResult>, String> {
        Err("swipe recognition requires a Patricia backend".into())
    }

    /// Retrieve the normalized frequency for a word: its 0.0–1.0 share of
    /// the backend's unigram mass, or -1.0 when the word is unknown or the
    /// backend carries no frequency data. The lookup itself is exact.
    fn get_frequency(&self, word: &str) -> f64;

    /// Return `true` when the word exists in the dictionary, compared
    /// exactly (no case folding or normalization).
    fn contains(&self, word: &str) -> bool;

    /// Whether this backend supports write operations.
    fn is_writable(&self) -> bool {
        false
    }

    /// Add a word to the dictionary.
    ///
    /// `frequency` is in the backend's native count units and is normalized
    /// on read (see `get_frequency`). For backends that only track
    /// membership (MARISA, Hunspell), `frequency` is ignored.
    ///
    /// `allow_existing`: if true, overwrite when the word exists;
    /// if false, return Err when the word already exists.
    fn add_word(
        &self,
        _word: &str,
        _frequency: f64,
        _allow_existing: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err("not supported".into())
    }
}

impl<T: DictionaryBackend> DictionaryBackend for Arc<T> {
    fn is_empty(&self) -> bool {
        (**self).is_empty()
    }

    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        (**self).query_prefixes(queries)
    }

    fn query_limited(&self, queries: &[DictionaryQuery], max: usize) -> Vec<DictionaryResult> {
        (**self).query_limited(queries, max)
    }

    #[cfg(feature = "swipe")]
    fn swipe_candidates(
        &self,
        starts: &[String],
        ends: &[String],
        letters: &[u8],
        deadline: std::time::Instant,
    ) -> Result<Vec<DictionaryResult>, String> {
        (**self).swipe_candidates(starts, ends, letters, deadline)
    }

    fn get_frequency(&self, word: &str) -> f64 {
        (**self).get_frequency(word)
    }

    fn contains(&self, word: &str) -> bool {
        (**self).contains(word)
    }

    fn is_writable(&self) -> bool {
        (**self).is_writable()
    }

    fn add_word(
        &self,
        word: &str,
        frequency: f64,
        allow_existing: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        (**self).add_word(word, frequency, allow_existing)
    }
}

// ---------------------------------------------------------------------------
// Shared query cache
// ---------------------------------------------------------------------------

/// A cached query together with the results that were returned by the backend.
#[derive(Debug, Clone)]
pub struct SharedCachedQuery {
    pub queries: Vec<DictionaryQuery>,
    pub results: Vec<DictionaryResult>,
}

/// Thread-safe LRU-ish cache for dictionary queries.
///
/// The cache supports two lookup strategies:
///
/// 1. **Exact key match** — fastest path, O(1).
/// 2. **Containment match** — if a previously cached query is a superset of
///    the current query (wider prefix, wider suffix, looser length bounds),
///    the cached results are filtered down.  This avoids a full backend scan
///    when queries become progressively narrower.
pub struct SharedQueryCache {
    cache: Mutex<HashMap<u64, SharedCachedQuery>>,
    size_limit: AtomicUsize,
    _merged_count: AtomicUsize,
}

impl SharedQueryCache {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            size_limit: AtomicUsize::new(0),
            _merged_count: AtomicUsize::new(0),
        }
    }

    /// Set the maximum number of cached entries (0 = disabled).
    pub fn set_limit(&self, limit: usize) {
        self.size_limit.store(limit, Ordering::Relaxed);
        if limit == 0 {
            if let Ok(mut cache) = self.cache.lock() {
                cache.clear();
            }
        }
    }

    pub fn limit(&self) -> usize {
        self.size_limit.load(Ordering::Relaxed)
    }

    pub fn clear(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// Check if a cached query is a superset of the new query.
    ///
    /// When this returns `true`, the cached results can be filtered to answer
    /// the new query without consulting the backend.
    pub fn query_contains_containment(
        cached: &DictionaryQuery,
        new_query: &DictionaryQuery,
    ) -> bool {
        match (&cached.prefix, &new_query.prefix) {
            (None, None) => {}
            (None, Some(_)) => return false,
            (Some(_), None) => return false,
            (Some(cached_prefix), Some(new_prefix)) => {
                if !new_prefix.starts_with(cached_prefix) {
                    return false;
                }
            }
        }

        match (&cached.suffix, &new_query.suffix) {
            (None, None) => {}
            (None, Some(_)) => return false,
            (Some(_), None) => return false,
            (Some(cached_suffix), Some(new_suffix)) => {
                if !new_suffix.ends_with(cached_suffix) {
                    return false;
                }
            }
        }

        let cached_min = cached.min_length.unwrap_or(0);
        let cached_max = cached.max_length.unwrap_or(usize::MAX);
        let new_min = new_query.min_length.unwrap_or(0);
        let new_max = new_query.max_length.unwrap_or(usize::MAX);

        if cached_min > new_min {
            return false;
        }
        if cached_max < new_max {
            return false;
        }
        if cached_min > cached_max {
            return false;
        }

        true
    }

    /// Verify that a single word satisfies every constraint in `query`.
    pub fn result_matches_query(word: &str, query: &DictionaryQuery) -> bool {
        let word_len = word.len();
        let lower = word.to_lowercase();

        if let Some(prefix) = &query.prefix {
            if !lower.starts_with(&prefix.to_lowercase()) {
                return false;
            }
        }

        if let Some(suffix) = &query.suffix {
            if !lower.ends_with(&suffix.to_lowercase()) {
                return false;
            }
        }

        if let Some(min_len) = query.min_length {
            if word_len < min_len {
                return false;
            }
        }

        if let Some(max_len) = query.max_length {
            if word_len > max_len {
                return false;
            }
        }

        true
    }

    /// Return cached results or compute new ones.
    pub fn get_or_compute<F>(
        &self,
        queries: &[DictionaryQuery],
        compute_fn: F,
    ) -> Vec<DictionaryResult>
    where
        F: FnOnce(&[DictionaryQuery]) -> Vec<DictionaryResult>,
    {
        let cache = self.cache.lock().unwrap();
        let limit = self.size_limit.load(Ordering::Relaxed);

        if limit == 0 {
            drop(cache);
            return compute_fn(queries);
        }

        let key = Self::hash_queries(queries);

        if let Some(cached) = cache.get(&key) {
            return cached.results.clone();
        }

        for cached_query in cache.values() {
            if Self::can_answer_via_containment(cached_query, queries) {
                self._merged_count.fetch_add(1, Ordering::Relaxed);
                return Self::answer_from_cache(cached_query, queries);
            }
        }
        drop(cache);

        let results = compute_fn(queries);

        if limit > 0 {
            if let Ok(mut cache) = self.cache.lock() {
                let cached_query = SharedCachedQuery {
                    queries: queries.to_vec(),
                    results: results.clone(),
                };
                cache.insert(Self::hash_queries(queries), cached_query);

                if cache.len() > limit {
                    if let Some(first_key) = cache.keys().next().copied() {
                        cache.remove(&first_key);
                    }
                }
            }
        }

        results
    }

    // -- private helpers ---------------------------------------------------

    fn hash_queries(queries: &[DictionaryQuery]) -> u64 {
        let mut hasher = DefaultHasher::new();
        for query in queries {
            query.prefix.hash(&mut hasher);
            query.suffix.hash(&mut hasher);
            query.min_length.hash(&mut hasher);
            query.max_length.hash(&mut hasher);
        }
        hasher.finish()
    }

    fn can_answer_via_containment(
        cached: &SharedCachedQuery,
        new_queries: &[DictionaryQuery],
    ) -> bool {
        for new_query in new_queries {
            let mut found = false;
            for cached_query in &cached.queries {
                if Self::query_contains_containment(cached_query, new_query) {
                    found = true;
                    break;
                }
            }
            if !found {
                return false;
            }
        }
        true
    }

    fn answer_from_cache(
        cached: &SharedCachedQuery,
        new_queries: &[DictionaryQuery],
    ) -> Vec<DictionaryResult> {
        let mut filtered = Vec::new();

        for cached_component in &cached.queries {
            for new_component in new_queries {
                if Self::query_contains_containment(cached_component, new_component) {
                    for result in &cached.results {
                        if Self::result_matches_query(&result.word, new_component) {
                            filtered.push(result.clone());
                        }
                    }
                }
            }
        }

        filtered.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        filtered.dedup_by(|a, b| a.word == b.word);
        filtered
    }
}

impl Default for SharedQueryCache {
    fn default() -> Self {
        Self::new()
    }
}
