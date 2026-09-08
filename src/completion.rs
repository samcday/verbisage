//! Current-word ranking. Scores are relative heuristics, not probabilities.
use std::collections::BTreeMap;

use crate::dictionary::{DictionaryBackend, DictionaryQuery, DictionaryResult};

/// Merge intact prefixes with all usable one-edit candidates before truncating.
/// Intact prefixes share one weight, so max+1 rows suffice before removing
/// the exact input. The frontend owns that literal choice.
pub fn complete(backend: &dyn DictionaryBackend, input: &str, max: usize) -> Vec<DictionaryResult> {
    if max == 0 || input.is_empty() {
        return Vec::new();
    }
    let word = input.to_lowercase();
    let query = DictionaryQuery {
        prefix: Some(word.clone()),
        suffix: None,
        min_length: None,
        max_length: None,
    };
    // word -> (match weight, dictionary frequency)
    let mut candidates = BTreeMap::new();
    for result in backend.query_limited(&[query], max.min(100) + 1) {
        if result.word.to_lowercase() != word {
            candidates.insert(result.word, (1.0_f64, result.confidence));
        }
    }
    let chars: Vec<char> = word.chars().collect();
    // A valid word is deliberate input; very short fragments are too ambiguous
    // to correct. In both cases retain ordinary prefix completion.
    let mut letters = word.chars();
    let title: String = letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default();
    let known = backend.contains(&word)
        || backend.contains(&title)
        || backend.contains(&word.to_uppercase());
    if chars.len() >= 3 && !known {
        let mut add = |candidate: String, weight: f64| {
            if candidate == word {
                return;
            }
            if let Some(existing) = candidates.get_mut(&candidate) {
                existing.0 = existing.0.max(weight);
            } else if backend.contains(&candidate) {
                let frequency = backend.get_frequency(&candidate);
                candidates.insert(candidate, (weight, frequency));
            }
        };
        for i in 0..chars.len() - 1 {
            let mut edited = chars.clone();
            edited.swap(i, i + 1);
            add(edited.into_iter().collect(), 0.9);
        }
        for i in 0..chars.len() {
            let repeated = (i > 0 && chars[i - 1] == chars[i])
                || (i + 1 < chars.len() && chars[i + 1] == chars[i]);
            let mut edited = chars.clone();
            edited.remove(i);
            add(
                edited.into_iter().collect(),
                if repeated { 0.9 } else { 0.65 },
            );
        }
        for i in 0..=chars.len() {
            for ch in 'a'..='z' {
                let repeated = (i > 0 && chars[i - 1] == ch) || (i < chars.len() && chars[i] == ch);
                let mut edited = chars.clone();
                edited.insert(i, ch);
                add(
                    edited.into_iter().collect(),
                    if repeated { 0.9 } else { 0.65 },
                );
            }
        }
        for i in 0..chars.len() {
            for ch in 'a'..='z' {
                if chars[i] != ch {
                    let mut edited = chars.clone();
                    edited[i] = ch;
                    add(edited.into_iter().collect(), 0.5);
                }
            }
        }
    }
    let prior = |frequency: f64| {
        if frequency.is_finite() && frequency > 0.0 {
            frequency.ln_1p()
        } else {
            0.0
        }
    };
    let largest = candidates
        .values()
        .map(|(_, frequency)| prior(*frequency))
        .fold(0.0_f64, f64::max);
    let mut results: Vec<_> = candidates
        .into_iter()
        .map(|(word, (weight, frequency))| DictionaryResult {
            word,
            confidence: weight
                * if largest > 0.0 {
                    prior(frequency) / largest
                } else {
                    1.0
                },
        })
        .collect();
    results.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.word.cmp(&b.word))
    });
    results.truncate(max.min(100));
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::FileDictionaryBackend;

    fn dictionary(rows: &[(&str, f64)]) -> FileDictionaryBackend {
        let mut dictionary = FileDictionaryBackend::new();
        for (word, frequency) in rows {
            dictionary.add_word_mut((*word).into(), *frequency);
        }
        dictionary
    }

    #[test]
    fn typo_and_prefix_candidates_are_ranked_together_before_truncation() {
        let dictionary = dictionary(&[
            ("held", 158.0),
            ("help", 149.0),
            ("hero", 126.0),
            ("hello", 120.0),
            ("helots", 65.0),
            ("helot", 52.0),
        ]);
        let results = complete(&dictionary, "helo", 6);
        assert_eq!(results[0].word, "hello");
        assert!(results.iter().any(|r| r.word == "helots"));
        assert_eq!(complete(&dictionary, "helo", 1), results[..1]);
        assert_eq!(complete(&dictionary, "helo", 6), results);
        assert!(
            results
                .iter()
                .all(|r| r.confidence.is_finite() && (0.0..=1.0).contains(&r.confidence))
        );
    }

    #[test]
    fn valid_words_and_short_fragments_keep_prefixes() {
        let dictionary = dictionary(&[
            ("test", 100.0),
            ("testing", 80.0),
            ("best", 900.0),
            ("he", 80.0),
            ("help", 70.0),
            ("the", 1000.0),
        ]);
        assert_eq!(
            complete(&dictionary, "test", 6)
                .iter()
                .map(|r| r.word.as_str())
                .collect::<Vec<_>>(),
            ["testing"]
        );
        assert_eq!(
            complete(&dictionary, "he", 6)
                .iter()
                .map(|r| r.word.as_str())
                .collect::<Vec<_>>(),
            ["help"]
        );
        assert!(complete(&dictionary, "", 6).is_empty());
        assert!(complete(&dictionary, "he", 0).is_empty());
    }

    #[test]
    fn invalid_frequency_values_never_escape_as_nan_scores() {
        let dictionary = dictionary(&[
            ("test", f64::NAN),
            ("testing", f64::INFINITY),
            ("tester", -4.0),
            ("testable", 1.0),
        ]);
        let results = complete(&dictionary, "tes", 6);
        assert_eq!(results[0].word, "testable");
        assert!(
            results
                .iter()
                .all(|r| r.confidence.is_finite() && (0.0..=1.0).contains(&r.confidence))
        );
        assert_eq!(complete(&dictionary, "tes", 6), results);
    }

    #[test]
    fn generic_edits_and_unknown_frequencies_are_deterministic() {
        let dictionary = dictionary(&[("the", 0.0), ("ten", 0.0), ("letter", 0.0), ("élan", 0.0)]);
        assert_eq!(complete(&dictionary, "teh", 1)[0].word, "the");
        assert_eq!(complete(&dictionary, "lettter", 1)[0].word, "letter");
        assert_eq!(complete(&dictionary, "éllan", 1)[0].word, "élan");
    }
}
