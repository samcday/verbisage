use std::fs;
use std::mem;
use std::path::Path;

use rsmarisa::{Agent, Trie};

use crate::dictionary::{DictionaryBackend, DictionaryQuery, DictionaryResult};
use crate::prediction::ngram_backend::NgramBackend;

/// N‑gram data access layer backed by a MARISA trie + companion counts file.
///
/// The trie stores n‑gram keys prefixed by order number:
/// - `"1 <word>"` for unigrams
/// - `"2 <w1> <w2>"` for bigrams
/// - `"3 <w1> <w2> <w3>"` for trigrams
///
/// The counts file is a flat file of u32 little-endian values, indexed
/// by trie key ID, with a 4-byte magic header.
pub struct MarisaNgramBackend {
    trie: Trie,
    counts: Vec<u32>,
    total_unigram_count: u64,
    max_order: usize,
}

impl MarisaNgramBackend {
    pub fn from_files(
        trie_path: &Path,
        counts_path: &Path,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let trie_path_str = trie_path
            .to_str()
            .ok_or_else(|| format!("non-utf8 path: {}", trie_path.display()))?;

        let mut trie = Trie::new();
        trie.load(trie_path_str)?;

        let counts_data = fs::read(counts_path)?;
        let count_bytes = &counts_data[4..];
        let count_u32_slice = unsafe {
            debug_assert!(count_bytes.as_ptr() as usize % mem::align_of::<u32>() == 0);
            std::slice::from_raw_parts(count_bytes.as_ptr() as *const u32, count_bytes.len() / 4)
        };
        let counts: Vec<u32> = count_u32_slice.to_vec();

        let mut total = 0u64;
        let mut max_order = 0usize;
        let mut agent = Agent::new();

        agent.set_query_str("");
        while trie.predictive_search(&mut agent) {
            let key = agent.key().as_str();
            let id = agent.key().id();

            if let Some(&c) = counts.get(id) {
                if c > 0 {
                    if key.starts_with("1 ") {
                        total += c as u64;
                    }
                    if let Some(prefix) = key.split_whitespace().next() {
                        if let Ok(order) = prefix.parse::<usize>() {
                            if order > max_order {
                                max_order = order;
                            }
                        }
                    }
                }
            }
        }

        Ok(Self {
            trie,
            counts,
            total_unigram_count: total,
            max_order,
        })
    }

    fn ngram_key(ngram: &[&str]) -> String {
        let order = ngram.len();
        format!("{} {}", order, ngram.join(" "))
    }
}

impl DictionaryBackend for MarisaNgramBackend {
    fn search_words(
        &self,
        search: &crate::dictionary::search::WordSearch<'_>,
        deadline: std::time::Instant,
    ) -> Result<Vec<DictionaryResult>, String> {
        crate::dictionary::search::check_deadline(deadline)?;
        let mut results = Vec::new();
        let mut agent = Agent::new();
        agent.set_query_str("1 ");
        while self.trie.predictive_search(&mut agent) {
            let key = agent.key().as_str();
            if let Some(word) = key.strip_prefix("1 ") {
                let count = self.counts.get(agent.key().id()).copied().unwrap_or(0);
                search.push(
                    &mut results,
                    word,
                    crate::dictionary::normalized_frequency(
                        f64::from(count),
                        self.total_unigram_count as f64,
                    ),
                    deadline,
                )?;
            }
            crate::dictionary::search::check_deadline(deadline)?;
        }
        Ok(results)
    }

    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        let mut results: Vec<DictionaryResult> = Vec::new();

        for query in queries {
            let prefix = match &query.prefix {
                Some(p) => p.clone(),
                None => continue,
            };
            let min_len = query.min_length.unwrap_or(0);
            let max_len = query.max_length.unwrap_or(usize::MAX);

            let unigram_prefix = format!("1 {}", prefix);
            let mut agent = Agent::new();
            agent.set_query_str(&unigram_prefix);

            while self.trie.predictive_search(&mut agent) {
                let key = agent.key().as_str();
                if !key.starts_with("1 ") {
                    break;
                }
                let word = &key[2..];
                let word_len = word.len();
                if word_len < min_len || word_len > max_len {
                    continue;
                }
                if let Some(ref suffix) = query.suffix {
                    if !word.ends_with(suffix) {
                        continue;
                    }
                }
                let id = agent.key().id();
                let count = self.counts.get(id).copied().unwrap_or(0) as f64;
                let confidence = if self.total_unigram_count > 0 {
                    count / self.total_unigram_count as f64
                } else {
                    -1.0
                };
                results.push(DictionaryResult {
                    word: word.to_string(),
                    confidence,
                });
            }
        }

        results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.word.cmp(&b.word))
        });
        results
    }

    fn get_frequency(&self, word: &str) -> f64 {
        let key = format!("1 {}", word);
        let mut agent = Agent::new();
        agent.set_query_str(&key);
        if self.trie.lookup(&mut agent) {
            let id = agent.key().id();
            let count = self.counts.get(id).copied().unwrap_or(0) as f64;
            if self.total_unigram_count > 0 {
                count / self.total_unigram_count as f64
            } else {
                -1.0
            }
        } else {
            -1.0
        }
    }

    fn contains(&self, word: &str) -> bool {
        let key = format!("1 {}", word);
        let mut agent = Agent::new();
        agent.set_query_str(&key);
        self.trie.lookup(&mut agent)
    }

    fn is_writable(&self) -> bool {
        false
    }
}

impl NgramBackend for MarisaNgramBackend {
    fn max_order(&self) -> usize {
        self.max_order
    }

    fn unigram_total(&self) -> u64 {
        self.total_unigram_count
    }

    fn ngram_count(&self, ngram: &[&str]) -> u64 {
        let key = Self::ngram_key(ngram);
        let mut agent = Agent::new();
        agent.set_query_str(&key);
        if self.trie.lookup(&mut agent) {
            let id = agent.key().id();
            self.counts.get(id).copied().unwrap_or(0) as u64
        } else {
            0
        }
    }

    fn candidates(&self, context: &[&str], max_candidates: usize) -> Vec<(String, u64)> {
        let order = context.len() + 1;
        if order == 1 {
            let mut results: Vec<(String, u64)> = Vec::new();
            let mut agent = Agent::new();
            agent.set_query_str("1 ");
            while self.trie.predictive_search(&mut agent) {
                let key = agent.key().as_str();
                if !key.starts_with("1 ") {
                    break;
                }
                let id = agent.key().id();
                let count = self.counts.get(id).copied().unwrap_or(0) as u64;
                if count > 0 {
                    let word = &key[2..];
                    if !word.is_empty() {
                        results.push((word.to_string(), count));
                    }
                }
            }
            results.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            results.truncate(max_candidates);
            return results;
        }

        let prefix = format!("{} {} ", order, context.join(" "));
        let mut results: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        let mut agent = Agent::new();
        agent.set_query_str(&prefix);

        while self.trie.predictive_search(&mut agent) {
            let key = agent.key().as_str();
            if !key.starts_with(&prefix) {
                break;
            }
            let id = agent.key().id();
            let count = self.counts.get(id).copied().unwrap_or(0) as u64;
            if count > 0 {
                let rest = &key[prefix.len()..];
                let next_word = rest.split_whitespace().next().unwrap_or(rest);
                if !next_word.is_empty() {
                    *results.entry(next_word.to_string()).or_insert(0) += count;
                }
            }
        }

        let mut sorted: Vec<(String, u64)> = results.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        sorted.truncate(max_candidates);
        sorted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsmarisa::Keyset;
    use std::io::Write;
    use std::path::Path;
    use tempfile::tempdir;

    fn build_marisa_ngrams(dir: &Path, ngrams: &[(&str, u32)]) {
        let mut keyset = Keyset::new();
        for (key, _) in ngrams {
            keyset.push_back_str(key).unwrap();
        }
        let mut trie = Trie::new();
        trie.build(&mut keyset, 0);

        let trie_path = dir.join("ngrams.trie");
        let s = trie_path.to_str().unwrap();
        trie.save(s).unwrap();

        let counts_path = dir.join("ngrams.counts");
        let mut f = fs::File::create(&counts_path).unwrap();
        f.write_all(&0x0098a15au32.to_le_bytes()).unwrap();

        let mut counts = vec![0u32; trie.num_keys()];
        for (key, count) in ngrams {
            let mut agent = Agent::new();
            agent.set_query_str(key);
            if trie.lookup(&mut agent) {
                let id = agent.key().id();
                counts[id] = *count;
            }
        }
        for c in &counts {
            f.write_all(&c.to_le_bytes()).unwrap();
        }
    }

    #[test]
    fn unigram_total_sums_correctly() {
        let dir = tempdir().unwrap();
        build_marisa_ngrams(
            dir.path(),
            &[
                ("1 der", 1000),
                ("1 die", 800),
                ("1 und", 600),
                ("2 der stadt", 500),
            ],
        );
        let backend = MarisaNgramBackend::from_files(
            &dir.path().join("ngrams.trie"),
            &dir.path().join("ngrams.counts"),
        )
        .unwrap();
        assert_eq!(backend.unigram_total(), 2400);
    }

    #[test]
    fn ngram_count_exact_lookup() {
        let dir = tempdir().unwrap();
        build_marisa_ngrams(
            dir.path(),
            &[
                ("1 der", 1000),
                ("2 der stadt", 500),
                ("3 der stadt park", 100),
            ],
        );
        let backend = MarisaNgramBackend::from_files(
            &dir.path().join("ngrams.trie"),
            &dir.path().join("ngrams.counts"),
        )
        .unwrap();
        assert_eq!(backend.ngram_count(&["der"]), 1000);
        assert_eq!(backend.ngram_count(&["der", "stadt"]), 500);
        assert_eq!(backend.ngram_count(&["der", "stadt", "park"]), 100);
        assert_eq!(backend.ngram_count(&["der", "fluss"]), 0);
    }

    #[test]
    fn candidates_unigram() {
        let dir = tempdir().unwrap();
        build_marisa_ngrams(
            dir.path(),
            &[
                ("1 der", 1000),
                ("1 die", 800),
                ("1 und", 600),
                ("1 in", 400),
            ],
        );
        let backend = MarisaNgramBackend::from_files(
            &dir.path().join("ngrams.trie"),
            &dir.path().join("ngrams.counts"),
        )
        .unwrap();
        let results = backend.candidates(&[], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], ("der".to_string(), 1000));
        assert_eq!(results[1], ("die".to_string(), 800));
    }

    #[test]
    fn candidates_bigram() {
        let dir = tempdir().unwrap();
        build_marisa_ngrams(
            dir.path(),
            &[
                ("2 der stadt", 500),
                ("2 der fluss", 300),
                ("2 der berg", 100),
            ],
        );
        let backend = MarisaNgramBackend::from_files(
            &dir.path().join("ngrams.trie"),
            &dir.path().join("ngrams.counts"),
        )
        .unwrap();
        let results = backend.candidates(&["der"], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], ("stadt".to_string(), 500));
        assert_eq!(results[1], ("fluss".to_string(), 300));
    }

    #[test]
    fn max_order_detected() {
        let dir = tempdir().unwrap();
        build_marisa_ngrams(
            dir.path(),
            &[
                ("1 der", 100),
                ("2 der stadt", 50),
                ("3 der stadt park", 10),
            ],
        );
        let backend = MarisaNgramBackend::from_files(
            &dir.path().join("ngrams.trie"),
            &dir.path().join("ngrams.counts"),
        )
        .unwrap();
        assert_eq!(backend.max_order(), 3);
    }
}
