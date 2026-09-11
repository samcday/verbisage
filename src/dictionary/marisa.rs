use std::path::Path;

use rsmarisa::{Agent, Trie};

use crate::dictionary::{DictionaryBackend, DictionaryQuery, DictionaryResult};

/// Dictionary backend backed by a MARISA trie (static, read-only).
pub struct MarisaDictionaryBackend {
    trie: Trie,
    writable: bool,
}

impl MarisaDictionaryBackend {
    pub fn from_file(path: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let s = path
            .to_str()
            .ok_or_else(|| format!("non-utf8 path: {}", path.display()))?;
        let mut trie = Trie::new();
        trie.load(s)?;
        Ok(Self {
            trie,
            writable: false,
        })
    }

    pub fn new() -> Self {
        Self {
            trie: Trie::new(),
            writable: false,
        }
    }
}

impl Default for MarisaDictionaryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DictionaryBackend for MarisaDictionaryBackend {
    fn search_words(
        &self,
        search: &super::search::WordSearch<'_>,
        deadline: std::time::Instant,
    ) -> Result<Vec<DictionaryResult>, String> {
        super::search::check_deadline(deadline)?;
        let mut results = Vec::new();
        let mut agent = Agent::new();
        agent.set_query_str("");
        while self.trie.predictive_search(&mut agent) {
            search.push(&mut results, agent.key().as_str(), -1.0, deadline)?;
        }
        super::search::check_deadline(deadline)?;
        Ok(results)
    }

    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        let mut results = Vec::new();

        for query in queries {
            let prefix = match &query.prefix {
                Some(p) => p.clone(),
                None => String::new(),
            };

            let min_len = query.min_length.unwrap_or(0);
            let max_len = query.max_length.unwrap_or(usize::MAX);

            let mut agent = Agent::new();
            agent.set_query_str(&prefix);

            while self.trie.predictive_search(&mut agent) {
                let word = agent.key().as_str().to_string();
                let word_len = word.chars().count();
                if word_len < min_len || word_len > max_len {
                    continue;
                }
                if let Some(ref suffix) = query.suffix {
                    if !word.ends_with(suffix) {
                        continue;
                    }
                }
                results.push(DictionaryResult {
                    word,
                    confidence: -1.0,
                });
            }
        }

        results.sort_by(|a, b| {
            b.confidence
                .total_cmp(&a.confidence)
                .then_with(|| a.word.cmp(&b.word))
        });

        results.dedup_by(|a, b| a.word == b.word);
        results
    }

    fn get_frequency(&self, _word: &str) -> f64 {
        -1.0
    }

    fn contains(&self, word: &str) -> bool {
        let mut agent = Agent::new();
        agent.set_query_str(word);
        self.trie.lookup(&mut agent)
    }

    fn is_writable(&self) -> bool {
        self.writable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsmarisa::Keyset;
    use tempfile::tempdir;

    fn build_test_trie(dir: &Path, words: &[&str]) {
        let mut keyset = Keyset::new();
        for w in words {
            keyset.push_back_str(w).unwrap();
        }
        let mut trie = Trie::new();
        trie.build(&mut keyset, 0);
        let path = dir.join("test.marisa");
        let s = path.to_str().unwrap();
        trie.save(s).unwrap();
    }

    #[test]
    fn contains_works() {
        let dir = tempdir().unwrap();
        build_test_trie(dir.path(), &["hello", "world", "help"]);
        let dict = MarisaDictionaryBackend::from_file(&dir.path().join("test.marisa")).unwrap();
        assert!(dict.contains("hello"));
        assert!(dict.contains("world"));
        assert!(!dict.contains("foo"));
    }

    #[test]
    fn prefix_query() {
        let dir = tempdir().unwrap();
        build_test_trie(dir.path(), &["hello", "help", "helm", "world"]);
        let dict = MarisaDictionaryBackend::from_file(&dir.path().join("test.marisa")).unwrap();
        let results = dict.query_prefixes(&[DictionaryQuery {
            prefix: Some("hel".into()),
            suffix: None,
            min_length: None,
            max_length: None,
        }]);
        assert_eq!(results.len(), 3);
    }
}
