use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::RwLock;

use super::subsequence::generate_subsequence_variants;
use super::{DictionaryBackend, DictionaryQuery, DictionaryResult, SharedQueryCache};

type SharedError = Box<dyn Error + Send + Sync>;

/// In-memory dictionary backed by a [`HashMap`] of words to frequencies.
///
/// Maintains three auxiliary structures for fast lookups:
///
/// * `words_sorted` — a sorted `Vec<String>` for binary search.
/// * `prefixes` — a `HashSet` of every prefix of every word.
/// * `length_buckets` — words grouped by `word.len()`.
///
/// Querying does a full scan of `words_sorted`, which is acceptable for
/// dictionaries under ~1 M entries.
pub struct FileDictionaryBackend {
    inner: RwLock<FileDictionaryInner>,
    writable: bool,
}

struct FileDictionaryInner {
    words: HashMap<String, f64>,
    total_frequency: f64,
    words_sorted: Vec<String>,
    prefixes: HashSet<String>,
    length_buckets: HashMap<usize, Vec<String>>,
}

impl Clone for FileDictionaryBackend {
    fn clone(&self) -> Self {
        let inner = self.inner.read().unwrap();
        Self {
            inner: RwLock::new(inner.clone()),
            writable: self.writable,
        }
    }
}

impl Clone for FileDictionaryInner {
    fn clone(&self) -> Self {
        Self {
            words: self.words.clone(),
            total_frequency: self.total_frequency,
            words_sorted: self.words_sorted.clone(),
            prefixes: self.prefixes.clone(),
            length_buckets: self.length_buckets.clone(),
        }
    }
}

fn build_inner_from_reader<R: BufRead>(
    reader: R,
    mode: FileReadMode,
) -> Result<FileDictionaryInner, SharedError> {
    let mut words = HashMap::new();
    let mut words_sorted = Vec::new();
    let mut prefixes = HashSet::new();
    let mut length_buckets: HashMap<usize, Vec<String>> = HashMap::new();

    let mut line_no = 0usize;
    for line in reader.lines() {
        line_no += 1;
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let FileReadMode::Delimited {
            delimiter,
            has_header,
            word_index,
            freq_index,
        } = mode
        {
            if has_header && line_no == 1 {
                continue;
            }
            let parts: Vec<&str> = trimmed.split(delimiter as char).collect();
            let word = parts
                .get(word_index)
                .ok_or_else(|| {
                    format!(
                        "line {}: missing word column at index {}",
                        line_no, word_index
                    )
                })?
                .trim()
                .to_string();
            if word.is_empty() {
                continue;
            }
            let frequency = match freq_index {
                Some(idx) => parts
                    .get(idx)
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .unwrap_or(1.0),
                None => 1.0,
            };
            insert_into(
                &mut words,
                &mut words_sorted,
                &mut prefixes,
                &mut length_buckets,
                word,
                frequency,
            );
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        let (word, frequency) = if parts.len() >= 2 {
            (parts[0].to_string(), parts[1].parse().unwrap_or(1.0))
        } else {
            (parts[0].to_string(), 1.0)
        };
        insert_into(
            &mut words,
            &mut words_sorted,
            &mut prefixes,
            &mut length_buckets,
            word,
            frequency,
        );
    }

    words_sorted.sort_unstable();
    words_sorted.dedup();
    let total_frequency = words
        .values()
        .copied()
        .filter(|f| f.is_finite() && *f > 0.0)
        .sum();
    Ok(FileDictionaryInner {
        words,
        total_frequency,
        words_sorted,
        prefixes,
        length_buckets,
    })
}

enum FileReadMode {
    /// Plain word list or frequency file (whitespace-separated: `word freq`)
    Flat,
    /// CSV/TSV with explicit column indexes
    Delimited {
        delimiter: u8,
        has_header: bool,
        word_index: usize,
        freq_index: Option<usize>,
    },
}

fn insert_into(
    words: &mut HashMap<String, f64>,
    words_sorted: &mut Vec<String>,
    prefixes: &mut HashSet<String>,
    length_buckets: &mut HashMap<usize, Vec<String>>,
    word: String,
    frequency: f64,
) {
    let word = crate::text::nfc(&word);
    let len = word.chars().count();
    words.insert(word.clone(), frequency);
    words_sorted.push(word.clone());

    let chars: Vec<char> = word.chars().collect();
    for i in 1..=chars.len() {
        prefixes.insert(chars[..i].iter().collect());
    }
    length_buckets.entry(len).or_default().push(word);
}

impl FileDictionaryBackend {
    /// Build from a plain word list (one word per line).
    pub fn from_word_list<P: AsRef<Path>>(path: P, writable: bool) -> Result<Self, SharedError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let inner = build_inner_from_reader(reader, FileReadMode::Flat)?;
        Ok(Self {
            inner: RwLock::new(inner),
            writable,
        })
    }

    /// Build from a frequency file (each line: `word <freq>`).
    pub fn from_frequency_file<P: AsRef<Path>>(
        path: P,
        writable: bool,
    ) -> Result<Self, SharedError> {
        Self::from_word_list(path, writable)
    }

    /// Create an empty dictionary.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(FileDictionaryInner {
                words: HashMap::new(),
                total_frequency: 0.0,
                words_sorted: Vec::new(),
                prefixes: HashSet::new(),
                length_buckets: HashMap::new(),
            }),
            writable: true,
        }
    }

    /// Create an empty dictionary with the given writability.
    pub fn with_writable(writable: bool) -> Self {
        Self {
            inner: RwLock::new(FileDictionaryInner {
                words: HashMap::new(),
                total_frequency: 0.0,
                words_sorted: Vec::new(),
                prefixes: HashSet::new(),
                length_buckets: HashMap::new(),
            }),
            writable,
        }
    }

    /// Merge another dictionary into this one (user frequencies win on conflict).
    pub fn merge(&self, other: &FileDictionaryBackend) {
        let other_inner = other.inner.read().unwrap();
        let mut inner = self.inner.write().unwrap();
        for (word, freq) in &other_inner.words {
            insert_word(&mut inner, word.clone(), *freq);
        }
    }

    /// Load and merge multiple frequency files into a single dictionary.
    ///
    /// Files are processed in order; later files override earlier ones for
    /// frequency when the same word appears in multiple sources.
    pub fn from_multiple_files<P: AsRef<Path>>(
        paths: &[P],
        writable: bool,
    ) -> Result<Self, SharedError> {
        let dict = Self::with_writable(writable);
        for path in paths {
            let other = Self::from_frequency_file(path, writable)?;
            dict.merge(&other);
        }
        Ok(dict)
    }

    /// Load from a delimited (CSV/TSV) file with configurable column indexes.
    ///
    /// - `delimiter`: byte to split on (e.g. `b','` for CSV, `b'\t'` for TSV).
    /// - `has_header`: if true, skip the first line.
    /// - `word_index`: zero-based column index for the word.
    /// - `freq_index`: optional zero-based column index for the frequency;
    ///   `None` means all entries get frequency 1.0.
    pub fn from_delimited_file<P: AsRef<Path>>(
        path: P,
        delimiter: u8,
        has_header: bool,
        word_index: usize,
        freq_index: Option<usize>,
        writable: bool,
    ) -> Result<Self, SharedError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let inner = build_inner_from_reader(
            reader,
            FileReadMode::Delimited {
                delimiter,
                has_header,
                word_index,
                freq_index,
            },
        )?;
        Ok(Self {
            inner: RwLock::new(inner),
            writable,
        })
    }

    /// Insert or update a word (mutable reference version).
    pub fn add_word_mut(&mut self, word: String, frequency: f64) {
        let mut inner = self.inner.write().unwrap();
        insert_word(&mut inner, word, frequency);
    }

    /// Return the number of entries.
    pub fn len(&self) -> usize {
        let inner = self.inner.read().unwrap();
        inner.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return the frequency for `word` or 0.0 if absent.
    pub fn frequency(&self, word: &str) -> f64 {
        let inner = self.inner.read().unwrap();
        inner.words.get(word).copied().unwrap_or(0.0)
    }

    /// Subsequence-based fuzzy matching, useful for spelling suggestions.
    pub fn subsequence_matches(
        &self,
        pattern: &str,
        _max_edit_distance: usize,
        limit: usize,
    ) -> Vec<String> {
        let inner = self.inner.read().unwrap();
        let chars: Vec<char> = pattern.chars().collect();
        let variants = generate_subsequence_variants(&chars, chars.len(), limit);
        let mut matches = Vec::new();

        for variant in variants {
            if let Ok(pos) = inner.words_sorted.binary_search(&variant) {
                matches.push(inner.words_sorted[pos].clone());
                if matches.len() >= limit {
                    break;
                }
            }
        }

        matches
    }
}

fn insert_word(inner: &mut FileDictionaryInner, word: String, frequency: f64) {
    let word = crate::text::nfc(&word);
    let len = word.chars().count();
    let old = inner.words.insert(word.clone(), frequency).unwrap_or(0.0);
    if old.is_finite() && old > 0.0 {
        inner.total_frequency -= old;
    }
    if frequency.is_finite() && frequency > 0.0 {
        inner.total_frequency += frequency;
    }

    if let Err(pos) = inner.words_sorted.binary_search(&word) {
        inner.words_sorted.insert(pos, word.clone());
    }

    let chars: Vec<char> = word.chars().collect();
    for i in 1..=chars.len() {
        inner.prefixes.insert(chars[..i].iter().collect());
    }

    inner.length_buckets.entry(len).or_default().push(word);
}

impl DictionaryBackend for FileDictionaryBackend {
    fn is_empty(&self) -> bool {
        self.inner.read().unwrap().words.is_empty()
    }

    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        let inner = self.inner.read().unwrap();
        let mut all_results = Vec::new();

        for word in &inner.words_sorted {
            let confidence = inner.words.get(word).copied().unwrap_or(0.0);

            for query in queries {
                let min_len = query.min_length.unwrap_or(0);
                let max_len = query.max_length.unwrap_or(usize::MAX);

                if word.chars().count() < min_len || word.chars().count() > max_len {
                    continue;
                }

                if SharedQueryCache::result_matches_query(word, query) {
                    all_results.push(DictionaryResult {
                        word: word.clone(),
                        confidence: super::normalized_frequency(confidence, inner.total_frequency),
                    });
                    break;
                }
            }
        }

        all_results.sort_by(super::rank_results);

        all_results
    }

    fn get_frequency(&self, word: &str) -> f64 {
        let inner = self.inner.read().unwrap();
        inner.words.get(word).map_or(-1.0, |count| {
            super::normalized_frequency(*count, inner.total_frequency)
        })
    }

    fn contains(&self, word: &str) -> bool {
        let inner = self.inner.read().unwrap();
        inner.words.contains_key(word)
    }

    fn is_writable(&self) -> bool {
        self.writable
    }

    fn add_word(
        &self,
        word: &str,
        frequency: f64,
        allow_existing: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.writable {
            return Err("backend is not writable".into());
        }

        let mut inner = self.inner.write().unwrap();
        let key = crate::text::nfc(word);

        if !allow_existing && inner.words.contains_key(&key) {
            return Err(format!("word '{}' already exists", word).into());
        }

        insert_word(&mut inner, key, frequency);
        Ok(())
    }
}

impl Default for FileDictionaryBackend {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_add_and_query() {
        let mut dict = FileDictionaryBackend::new();
        dict.add_word_mut("hello".to_string(), 100.0);
        dict.add_word_mut("world".to_string(), 50.0);
        dict.add_word_mut("help".to_string(), 75.0);

        assert!(dict.contains("hello"));
        assert!(!dict.contains("HELLO"));
        assert!(!dict.contains("foo"));

        let results = dict.query_prefixes(&[DictionaryQuery {
            prefix: Some("hel".to_string()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn subsequence_matches_works() {
        let mut dict = FileDictionaryBackend::new();
        dict.add_word_mut("hello".to_string(), 1.0);
        dict.add_word_mut("hallo".to_string(), 1.0);
        dict.add_word_mut("halo".to_string(), 1.0);

        let matches = dict.subsequence_matches("hxallo", 5, 5);
        assert!(matches.contains(&"hallo".to_string()));
        assert!(!matches.is_empty());
    }

    #[test]
    fn respects_limit() {
        let mut dict = FileDictionaryBackend::new();
        dict.add_word_mut("alpha".to_string(), 1.0);
        dict.add_word_mut("beta".to_string(), 1.0);
        dict.add_word_mut("gamma".to_string(), 1.0);

        let matches = dict.subsequence_matches("alpha", 2, 1);
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn add_word_trait_method() {
        let dict = FileDictionaryBackend::new();
        assert!(dict.is_writable());

        dict.add_word("hello", 10.0, false).unwrap();
        assert!(dict.contains("hello"));
        assert_eq!(dict.frequency("hello"), 10.0);

        let err = dict.add_word("hello", 20.0, false);
        assert!(err.is_err());

        dict.add_word("hello", 20.0, true).unwrap();
        assert_eq!(dict.frequency("hello"), 20.0);
    }

    #[test]
    fn with_writable_false() {
        let dict = FileDictionaryBackend::with_writable(false);
        assert!(!dict.is_writable());
        let err = dict.add_word("test", 1.0, true);
        assert!(err.is_err());
    }
}
