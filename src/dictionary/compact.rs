use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::{DictionaryBackend, DictionaryQuery, DictionaryResult, SharedQueryCache};

type WordIndex = u32;
type SharedError = Box<dyn Error + Send + Sync>;

/// Memory-compact word-list dictionary with index-based lookaside caches.
///
/// Strings are stored exactly once in a [`Vec<String>`].  All other data
/// structures (first-letter cache, length cache, reverse-index map) reference
/// entries by their `WordIndex`, avoiding string duplication.
pub struct CompactDictionary {
    words: Vec<String>,
    word_to_index: HashMap<String, WordIndex>,
    first_letter_cache: HashMap<char, Vec<WordIndex>>,
    length_cache: HashMap<usize, Vec<WordIndex>>,
    frequencies: Vec<f64>,
}

impl CompactDictionary {
    /// Load from a plain word list (one word per line).  Every word receives
    /// a default frequency of 1.0.
    pub fn from_word_file<P: AsRef<Path>>(path: P) -> Result<Self, SharedError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);

        let mut words = Vec::new();
        let mut word_to_index = HashMap::new();
        let mut first_letter_cache: HashMap<char, Vec<WordIndex>> = HashMap::new();
        let mut length_cache: HashMap<usize, Vec<WordIndex>> = HashMap::new();
        let mut frequencies = Vec::new();

        for line in reader.lines() {
            let line = line?;
            let word = line.trim();
            if word.is_empty() {
                continue;
            }

            let word = word.to_string();
            let index = words.len() as WordIndex;

            words.push(word.clone());
            frequencies.push(1.0);

            word_to_index.insert(word.clone(), index);

            if let Some(first) = word.chars().next() {
                first_letter_cache.entry(first).or_default().push(index);
            }

            length_cache
                .entry(word.len())
                .or_default()
                .push(index);
        }

        Ok(Self {
            words,
            word_to_index,
            first_letter_cache,
            length_cache,
            frequencies,
        })
    }

    /// Load from a frequency file (each line: `word <freq>`).
    pub fn from_frequency_file<P: AsRef<Path>>(path: P) -> Result<Self, SharedError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);

        let mut words = Vec::new();
        let mut word_to_index = HashMap::new();
        let mut first_letter_cache: HashMap<char, Vec<WordIndex>> = HashMap::new();
        let mut length_cache: HashMap<usize, Vec<WordIndex>> = HashMap::new();
        let mut frequencies = Vec::new();

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

            let index = words.len() as WordIndex;
            words.push(word.clone());
            frequencies.push(frequency);

            word_to_index.insert(word.clone(), index);

            if let Some(first) = word.chars().next() {
                first_letter_cache.entry(first).or_default().push(index);
            }

            length_cache
                .entry(word.len())
                .or_default()
                .push(index);
        }

        Ok(Self {
            words,
            word_to_index,
            first_letter_cache,
            length_cache,
            frequencies,
        })
    }

    /// Look up a word by its index.
    pub fn get_word(&self, index: WordIndex) -> Option<&str> {
        self.words.get(index as usize).map(|s| s.as_str())
    }

    /// Look up a frequency by its index.
    pub fn get_frequency_by_index(&self, index: WordIndex) -> Option<f64> {
        self.frequencies.get(index as usize).copied()
    }

    /// Return the index for a word, or `None`.
    pub fn get_word_index(&self, word: &str) -> Option<WordIndex> {
        self.word_to_index.get(word).copied()
    }

    /// All words starting with `prefix`.
    pub fn words_starting_with(&self, prefix: &str) -> Vec<&str> {
        if let Some(first) = prefix.chars().next() {
            if let Some(indexes) = self.first_letter_cache.get(&first) {
                return indexes
                    .iter()
                    .filter_map(|&idx| {
                        let w = &self.words[idx as usize];
                        if w.starts_with(prefix) {
                            Some(w.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    /// All words having exactly `length` bytes (not codepoints).
    pub fn words_with_length(&self, length: usize) -> Vec<&str> {
        if let Some(indexes) = self.length_cache.get(&length) {
            indexes
                .iter()
                .map(|&idx| self.words[idx as usize].as_str())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// All words whose length in bytes falls in `[min_len, max_len]`.
    pub fn words_with_length_range(&self, min_len: usize, max_len: usize) -> Vec<&str> {
        let mut result = Vec::new();
        for len in min_len..=max_len {
            if let Some(indexes) = self.length_cache.get(&len) {
                for &idx in indexes {
                    result.push(self.words[idx as usize].as_str());
                }
            }
        }
        result
    }

    /// Raw index slice for words starting with `first_letter`.
    pub fn indexes_by_first_letter(&self, first_letter: char) -> Option<&[WordIndex]> {
        self.first_letter_cache
            .get(&first_letter)
            .map(|v| v.as_slice())
    }

    /// Raw index slice for words with exactly `length` bytes.
    pub fn indexes_by_length(&self, length: usize) -> Option<&[WordIndex]> {
        self.length_cache.get(&length).map(|v| v.as_slice())
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn memory_stats(&self) -> MemoryStats {
        MemoryStats {
            word_count: self.words.len(),
            string_bytes: self.words.iter().map(|w| w.len()).sum(),
            index_entries: self.word_to_index.len(),
            first_letter_entries: self
                .first_letter_cache
                .values()
                .map(|v| v.len())
                .sum(),
            length_entries: self.length_cache.values().map(|v| v.len()).sum(),
        }
    }
}

impl DictionaryBackend for CompactDictionary {
    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        let mut all_results = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for (idx, word) in self.words.iter().enumerate() {
            let confidence = self.frequencies[idx];

            for query in queries {
                let min_len = query.min_length.unwrap_or(0);
                let max_len = query.max_length.unwrap_or(usize::MAX);

                if word.len() < min_len || word.len() > max_len {
                    continue;
                }

                if SharedQueryCache::result_matches_query(word, query) {
                    if seen.insert(word.clone()) {
                        all_results.push(DictionaryResult {
                            word: word.clone(),
                            confidence: if confidence > 0.0 {
                                confidence
                            } else {
                                -1.0
                            },
                        });
                    }
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
        self.word_to_index
            .get(word)
            .and_then(|&idx| self.frequencies.get(idx as usize).copied())
            .unwrap_or(0.5)
    }

    fn contains(&self, word: &str) -> bool {
        self.word_to_index.contains_key(word)
    }
}

// ---------------------------------------------------------------------------
// Memory statistics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub word_count: usize,
    pub string_bytes: usize,
    pub index_entries: usize,
    pub first_letter_entries: usize,
    pub length_entries: usize,
}

impl std::fmt::Display for MemoryStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Word count:              {}", self.word_count)?;
        writeln!(f, "String bytes:            {}", self.string_bytes)?;
        writeln!(f, "Index entries:           {}", self.index_entries)?;
        writeln!(
            f,
            "First-letter cache:      {}",
            self.first_letter_entries
        )?;
        writeln!(f, "Length cache:            {}", self.length_entries)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn basic_functionality() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_cdict_words.txt");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(f, "hello").unwrap();
            writeln!(f, "world").unwrap();
            writeln!(f, "help").unwrap();
            writeln!(f, "helium").unwrap();
        }

        let dict = CompactDictionary::from_word_file(&path).unwrap();
        assert_eq!(dict.len(), 4);
        assert!(dict.contains("hello"));
        assert!(!dict.contains("test"));

        let hel = dict.words_starting_with("hel");
        assert_eq!(hel.len(), 3);

        let len5 = dict.words_with_length(5);
        assert_eq!(len5.len(), 2);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn frequency_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_cdict_freq.txt");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(f, "hello 100.0").unwrap();
            writeln!(f, "world 50.0").unwrap();
            writeln!(f, "test").unwrap();
        }

        let dict = CompactDictionary::from_frequency_file(&path).unwrap();

        assert_eq!(
            dict.get_frequency_by_index(dict.get_word_index("hello").unwrap()),
            Some(100.0)
        );
        assert_eq!(
            dict.get_frequency_by_index(dict.get_word_index("test").unwrap()),
            Some(1.0)
        );

        let _ = std::fs::remove_file(&path);
    }
}
