use std::fs;
use std::mem;
use std::path::Path;

use rsmarisa::{Agent, Trie};

use crate::prediction::{Prediction, Predictor};

/// Predictor backed by a MARISA trie with companion counts file.
///
/// The trie stores n-gram keys prefixed by order number:
/// - `"1 <word>"` for unigrams
/// - `"2 <w1> <w2>"` for bigrams
/// - `"3 <w1> <w2> <w3>"` for trigrams
///
/// The counts file is a flat file of u32 little-endian values, indexed
/// by trie key ID, with a 4-byte magic header.
pub struct MarisaPredictor {
    trie: Trie,
    /// Flat array of u32 counts, indexed by trie key ID.
    counts: Vec<u32>,
    /// Total sum of all unigram counts (for confidence normalisation).
    total_unigram_count: f64,
}

impl MarisaPredictor {
    /// Load from companion `.trie` and `.counts` files.
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
        // Skip 4-byte magic header
        let count_bytes = &counts_data[4..];
        let count_u32_slice = unsafe {
            // Align check: count_bytes starts at offset 4, which is always
            // 4-byte aligned when the file is read into a Vec<u8>.
            debug_assert!(count_bytes.as_ptr() as usize % mem::align_of::<u32>() == 0);
            std::slice::from_raw_parts(count_bytes.as_ptr() as *const u32, count_bytes.len() / 4)
        };

        let counts: Vec<u32> = count_u32_slice.to_vec();

        // Compute total unigram mass for confidence normalisation
        let mut total = 0u64;
        let mut agent = Agent::new();
        agent.set_query_str("1 ");
        while trie.predictive_search(&mut agent) {
            let id = agent.key().id();
            if let Some(&c) = counts.get(id) {
                total += c as u64;
            }
        }
        let total_unigram_count = total as f64;

        Ok(Self {
            trie,
            counts,
            total_unigram_count,
        })
    }

    /// Prefix for unigrams ("1 "), bigrams ("2 "), trigrams ("3 ").
    fn prefix_for_context(context: &[&str]) -> String {
        match context.len() {
            0 => "1 ".to_string(),
            1 => format!("2 {} ", context[0]),
            2 => format!("3 {} {} ", context[0], context[1]),
            _ => format!("3 {} {} ", context[0], context[1]),
        }
    }
}

impl Predictor for MarisaPredictor {
    fn predict_next(&self, context: &[&str], max_suggestions: usize) -> Vec<Prediction> {
        if max_suggestions == 0 {
            return Vec::new();
        }

        let prefix = Self::prefix_for_context(context);

        let mut agent = Agent::new();
        agent.set_query_str(&prefix);

        // Collect all completions with their counts
        let mut results: Vec<(String, u32)> = Vec::new();
        while self.trie.predictive_search(&mut agent) {
            let key = agent.key().as_str();
            let id = agent.key().id();

            // Check that key actually starts with our prefix
            if !key.starts_with(&prefix) {
                continue;
            }

            // Extract the continuation: the word(s) after the context words
            let rest = &key[prefix.len()..];
            let next_word = rest.split_whitespace().next().unwrap_or(rest).to_string();
            if next_word.is_empty() {
                continue;
            }

            let count = self.counts.get(id).copied().unwrap_or(0);
            if count > 0 {
                results.push((next_word, count));
            }
        }

        // Aggregate counts for same next_word (different suffixes match same next word)
        let mut aggregated: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        for (word, count) in results {
            *aggregated.entry(word).or_insert(0) += count as u64;
        }

        // Sort by count descending, then word ascending
        let mut sorted: Vec<(String, u64)> = aggregated.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        // Compute normaliser: for unigram context use total, for others use
        // the sum of all matched counts
        let normaliser = if context.is_empty() {
            self.total_unigram_count.max(1.0)
        } else {
            let total: u64 = sorted.iter().map(|(_, c)| c).sum();
            (total as f64).max(1.0)
        };

        sorted
            .into_iter()
            .take(max_suggestions)
            .map(|(word, count)| Prediction {
                word,
                confidence: (count as f64) / normaliser,
            })
            .collect()
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

        // Write counts file: 4-byte magic + u32 per key
        // Counts must be indexed by the trie's internal key ID (not insertion order).
        // Use lookup to find each key's ID from the built trie.
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
    fn predict_unigrams() {
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
        let p = MarisaPredictor::from_files(
            &dir.path().join("ngrams.trie"),
            &dir.path().join("ngrams.counts"),
        )
        .unwrap();

        let results = p.predict_next(&[], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].word, "der");
        assert_eq!(results[1].word, "die");
        assert!(results[0].confidence > 0.0 && results[0].confidence <= 1.0);
    }

    #[test]
    fn predict_bigrams() {
        let dir = tempdir().unwrap();
        build_marisa_ngrams(
            dir.path(),
            &[
                ("2 der stadt", 500),
                ("2 der fluss", 300),
                ("2 der berg", 100),
                ("2 die frau", 200),
            ],
        );
        let p = MarisaPredictor::from_files(
            &dir.path().join("ngrams.trie"),
            &dir.path().join("ngrams.counts"),
        )
        .unwrap();

        let results = p.predict_next(&["der"], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].word, "stadt");
        assert_eq!(results[1].word, "fluss");
    }

    #[test]
    fn predict_trigrams() {
        let dir = tempdir().unwrap();
        build_marisa_ngrams(
            dir.path(),
            &[
                ("3 der stadt park", 100),
                ("3 der stadt bahnhof", 200),
                ("3 der stadt zentrum", 50),
            ],
        );
        let p = MarisaPredictor::from_files(
            &dir.path().join("ngrams.trie"),
            &dir.path().join("ngrams.counts"),
        )
        .unwrap();

        let results = p.predict_next(&["der", "stadt"], 3);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].word, "bahnhof");
        assert_eq!(results[1].word, "park");
        assert_eq!(results[2].word, "zentrum");
    }

    #[test]
    fn empty_context_returns_unigrams() {
        let dir = tempdir().unwrap();
        build_marisa_ngrams(
            dir.path(),
            &[
                ("1 der", 100),
                ("2 der stadt", 50), // bigram should NOT be returned for empty context
            ],
        );
        let p = MarisaPredictor::from_files(
            &dir.path().join("ngrams.trie"),
            &dir.path().join("ngrams.counts"),
        )
        .unwrap();

        let results = p.predict_next(&[], 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].word, "der");
    }

    #[test]
    fn max_suggestions_respected() {
        let dir = tempdir().unwrap();
        build_marisa_ngrams(
            dir.path(),
            &[
                ("2 der stadt", 100),
                ("2 der fluss", 90),
                ("2 der berg", 80),
            ],
        );
        let p = MarisaPredictor::from_files(
            &dir.path().join("ngrams.trie"),
            &dir.path().join("ngrams.counts"),
        )
        .unwrap();

        assert_eq!(p.predict_next(&["der"], 0).len(), 0);
        assert_eq!(p.predict_next(&["der"], 1).len(), 1);
        assert_eq!(p.predict_next(&["der"], 2).len(), 2);
        assert_eq!(p.predict_next(&["der"], 10).len(), 3);
    }
}
