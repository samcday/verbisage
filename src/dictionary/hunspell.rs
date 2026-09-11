use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::dictionary::{DictionaryBackend, DictionaryQuery, DictionaryResult, SharedQueryCache};

type SharedError = Box<dyn Error + Send + Sync>;

/// Dictionary backend built from a Hunspell `.dic` file.
///
/// Parses the standard Hunspell dictionary format:
/// - First line: entry count (ignored)
/// - Subsequent lines: `word` or `word/affix_flags`
///
/// The affix flags are stripped.  Only the bare word is stored.
pub struct HunspellDictionaryBackend {
    words: HashMap<String, f64>,
    words_sorted: Vec<String>,
    writable: bool,
}

impl HunspellDictionaryBackend {
    /// Load a `.dic` file and build the dictionary.
    ///
    /// Every word gets frequency 1.0 (Hunspell dictionaries don't carry
    /// frequency information).
    pub fn from_dic_file<P: AsRef<Path>>(path: P) -> Result<Self, SharedError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        // Skip the count line (first line).
        let _count_line = lines.next();

        let mut words = HashMap::new();
        let mut words_sorted = Vec::new();

        for line in lines {
            let line = line?;
            let word = line.split('/').next().unwrap_or(&line).trim();
            if word.is_empty() {
                continue;
            }

            let word = crate::text::nfc(word);
            words.insert(word.clone(), 1.0);
            words_sorted.push(word);
        }

        words_sorted.sort_unstable();
        words_sorted.dedup();

        Ok(Self {
            words,
            words_sorted,
            writable: false,
        })
    }

    /// Create an empty instance.
    pub fn new() -> Self {
        Self {
            words: HashMap::new(),
            words_sorted: Vec::new(),
            writable: false,
        }
    }
}

impl Default for HunspellDictionaryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DictionaryBackend for HunspellDictionaryBackend {
    fn search_words(
        &self,
        search: &super::search::WordSearch<'_>,
        deadline: std::time::Instant,
    ) -> Result<Vec<DictionaryResult>, String> {
        super::search::check_deadline(deadline)?;
        let mut results = Vec::new();
        for word in &self.words_sorted {
            search.push(&mut results, word, -1.0, deadline)?;
        }
        super::search::check_deadline(deadline)?;
        Ok(results)
    }

    fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        let mut all_results = Vec::new();

        for word in &self.words_sorted {
            for query in queries {
                let min_len = query.min_length.unwrap_or(0);
                let max_len = query.max_length.unwrap_or(usize::MAX);

                if word.chars().count() < min_len || word.chars().count() > max_len {
                    continue;
                }

                if SharedQueryCache::result_matches_query(word, query) {
                    all_results.push(DictionaryResult {
                        word: word.clone(),
                        confidence: -1.0,
                    });
                    break;
                }
            }
        }

        all_results.sort_by(|a, b| {
            b.confidence
                .total_cmp(&a.confidence)
                .then_with(|| a.word.cmp(&b.word))
        });

        all_results
    }

    fn get_frequency(&self, word: &str) -> f64 {
        {
            let _ = word;
            -1.0
        }
    }

    fn contains(&self, word: &str) -> bool {
        self.words.contains_key(word)
    }

    fn is_writable(&self) -> bool {
        self.writable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn parse_dic_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.dic");
        {
            let mut f = File::create(&path).unwrap();
            writeln!(f, "3").unwrap();
            writeln!(f, "hello").unwrap();
            writeln!(f, "world/A").unwrap();
            writeln!(f, "help").unwrap();
        }

        let dict = HunspellDictionaryBackend::from_dic_file(&path).unwrap();
        assert!(dict.contains("hello"));
        assert!(dict.contains("world"));
        assert!(dict.contains("help"));
        assert!(!dict.contains("foo"));

        let results = dict.query_prefixes(&[DictionaryQuery {
            prefix: Some("hel".into()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        assert_eq!(results.len(), 2);
    }
}
