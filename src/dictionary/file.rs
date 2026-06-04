use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

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
/// dictionaries under ~1 M entries.
#[derive(Clone)]
pub struct FileDictionaryBackend {
    words: HashMap<String, f64>,
    words_sorted: Vec<String>,
    prefixes: HashSet<String>,
    length_buckets: HashMap<usize, Vec<String>>,
}

impl FileDictionaryBackend {
    /// Build from a plain word list (one word per line).
    pub fn from_word_list<P: AsRef<Path>>(path: P) -> Result<Self, SharedError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);

        let mut words = HashMap::new();
        let mut words_sorted = Vec::new();
        let mut prefixes = HashSet::new();
        let mut length_buckets = HashMap::new();

        for line in reader.lines() {
            let line = line?;
            let word = line.trim();
            if word.is_empty() {
                continue;
            }

            let word = word.to_string();
            let len = word.len();

            words.insert(word.clone(), 1.0);
            words_sorted.push(word.clone());

            let chars: Vec<char> = word.chars().collect();
            for i in 1..=chars.len() {
                prefixes.insert(chars[..i].iter().collect());
            }

            length_buckets.entry(len).or_default().push(word.clone());
        }

        words_sorted.sort_unstable();

        Ok(Self {
            words,
            words_sorted,
            prefixes,
            length_buckets,
        })
    }

    /// Build from a frequency file (each line: `word <freq>`).
    pub fn from_frequency_file<P: AsRef<Path>>(path: P) -> Result<Self, SharedError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);

        let mut words = HashMap::new();
        let mut words_sorted = Vec::new();
        let mut prefixes = HashSet::new();
        let mut length_buckets = HashMap::new();

        for line in reader.lines() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.split_whitespace().collect();
            let (word, frequency) = if parts.len() >= 2 {
                (parts[0].to_string(), parts[1].parse().unwrap_or(1.0))
            } else {
                (parts[0].to_string(), 1.0)
            };

            let len = word.len();
            words.insert(word.clone(), frequency);
            words_sorted.push(word.clone());

            let chars: Vec<char> = word.chars().collect();
            for i in 1..=chars.len() {
                prefixes.insert(chars[..i].iter().collect());
            }

            length_buckets.entry(len).or_default().push(word.clone());
        }

        words_sorted.sort_unstable();

        Ok(Self {
            words,
            words_sorted,
            prefixes,
            length_buckets,
        })
    }

    /// Create an empty dictionary.
    pub fn new() -> Self {
        Self {
            words: HashMap::new(),
            words_sorted: Vec::new(),
            prefixes: HashSet::new(),
            length_buckets: HashMap::new(),
        }
    }

    /// Insert or update a word.
    pub fn add_word(&mut self, word: String, frequency: f64) {
        let len = word.len();
        self.words.insert(word.clone(), frequency);

        if let Err(pos) = self.words_sorted.binary_search(&word) {
            self.words_sorted.insert(pos, word.clone());
        }

        let chars: Vec<char> = word.chars().collect();
        for i in 1..=chars.len() {
            self.prefixes.insert(chars[..i].iter().collect());
        }

        self.length_buckets.entry(len).or_default().push(word);
    }

    /// Return the number of entries.
    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Return the frequency for `word` or 0.0 if absent.
    pub fn frequency(&self, word: &str) -> f64 {
        self.words.get(&word.to_lowercase()).copied().unwrap_or(0.0)
    }

    /// Subsequence-based fuzzy matching, useful for spelling suggestions.
    pub fn subsequence_matches(
        &self,
        pattern: &str,
        _max_edit_distance: usize,
        limit: usize,
    ) -> Vec<String> {
        let chars: Vec<char> = pattern.chars().collect();
        let variants = generate_subsequence_variants(&chars, chars.len(), limit);
        let mut matches = Vec::new();

        for variant in variants {
            if let Ok(pos) = self.words_sorted.binary_search(&variant) {
                matches.push(self.words_sorted[pos].clone());
                if matches.len() >= limit {
                    break;
                }
            }
        }

        matches
    }
}

impl DictionaryBackend for FileDictionaryBackend {
    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        let mut all_results = Vec::new();

        for word in &self.words_sorted {
            let confidence = self.words.get(word).copied().unwrap_or(0.0);

            for query in queries {
                let min_len = query.min_length.unwrap_or(0);
                let max_len = query.max_length.unwrap_or(usize::MAX);

                if word.len() < min_len || word.len() > max_len {
                    continue;
                }

                if SharedQueryCache::result_matches_query(word, query) {
                    all_results.push(DictionaryResult {
                        word: word.clone(),
                        confidence: if confidence > 0.0 {
                            confidence
                        } else {
                            -1.0
                        },
                    });
                    break;
                }
            }
        }

        all_results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.word.cmp(&b.word))
        });

        all_results
    }

    fn get_frequency(&self, word: &str) -> f64 {
        self.frequency(word)
    }

    fn contains(&self, word: &str) -> bool {
        self.words.contains_key(&word.to_lowercase())
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
        dict.add_word("hello".to_string(), 100.0);
        dict.add_word("world".to_string(), 50.0);
        dict.add_word("help".to_string(), 75.0);

        assert!(dict.contains("hello"));
        assert!(dict.contains("HELLO"));
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
        dict.add_word("hello".to_string(), 1.0);
        dict.add_word("hallo".to_string(), 1.0);
        dict.add_word("halo".to_string(), 1.0);

        let matches = dict.subsequence_matches("hxallo", 5, 5);
        assert!(matches.contains(&"hallo".to_string()));
        assert!(!matches.is_empty());
    }

    #[test]
    fn respects_limit() {
        let mut dict = FileDictionaryBackend::new();
        dict.add_word("alpha".to_string(), 1.0);
        dict.add_word("beta".to_string(), 1.0);
        dict.add_word("gamma".to_string(), 1.0);

        let matches = dict.subsequence_matches("alpha", 2, 1);
        assert_eq!(matches.len(), 1);
    }
}
