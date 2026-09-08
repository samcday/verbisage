//! Optional whole-word swipe prototype. Geometry and trace share widget coordinates.
//! Drift Type owns gesture scoring; Patricia supplies a bounded borrowed snapshot.
use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use drift_type::{DriftType, FeatureExtraction, InputPoint, LanguageModel, RectKey, RectKeyLayout};

use crate::dictionary::{DictionaryBackend, DictionaryResult};

pub const MAX_POINTS: usize = 512;
pub const MAX_KEYS: usize = 64;
pub const MAX_WORD_BYTES: usize = 48;
pub const MAX_CANDIDATES: usize = 2048;
pub const MAX_TRIE_NODES: usize = 131072;
const MAX_COORDINATE: f64 = 16384.0;
const SEARCH_BUDGET: Duration = Duration::from_millis(500);

pub type TracePoint = (f64, f64, u32);
pub type KeyBounds = (String, f64, f64, f64, f64);

/// A validated, single-finger request. The frontend owns gesture cancellation and
/// must discard any response whose input context has changed while it was pending.
pub struct SwipeRequest {
    trace: Vec<TracePoint>,
    keys: Vec<KeyBounds>,
    max: usize,
}

impl SwipeRequest {
    pub fn new(trace: Vec<TracePoint>, keys: Vec<KeyBounds>, max: u32) -> Result<Self, String> {
        if !(2..=MAX_POINTS).contains(&trace.len()) || !(2..=MAX_KEYS).contains(&keys.len()) {
            return Err("swipe requires 2..512 points and 2..64 keys".into());
        }
        let mut labels = BTreeSet::new();
        for (label, left, top, width, height) in &keys {
            if label.len() != 1
                || !label.as_bytes()[0].is_ascii_lowercase()
                || !labels.insert(label)
            {
                return Err("swipe key labels must be unique lowercase ASCII letters".into());
            }
            if [*left, *top, *width, *height]
                .iter()
                .any(|v| !v.is_finite())
                || *left < 0.0
                || *top < 0.0
                || *width < 1.0
                || *height < 1.0
                || left + width > MAX_COORDINATE
                || top + height > MAX_COORDINATE
            {
                return Err("invalid swipe key rectangle".into());
            }
        }
        let mut previous_ms = 0;
        if trace[0].2 != 0 {
            return Err("swipe timestamps must start at zero".into());
        }
        for (x, y, millis) in &trace {
            if !x.is_finite()
                || !y.is_finite()
                || x.abs() > MAX_COORDINATE
                || y.abs() > MAX_COORDINATE
            {
                return Err("invalid swipe point coordinates".into());
            }
            if *millis < previous_ms || *millis > 10000 {
                return Err("swipe timestamps must be nondecreasing and at most 10000 ms".into());
            }
            previous_ms = *millis;
        }
        // Stationary traces have zero path length, which is undefined for Drift's
        // resampling. Keep elapsed times on the remaining genuine motion points.
        let mut motion = Vec::with_capacity(trace.len());
        for point in trace {
            if motion
                .last()
                .is_none_or(|last: &TracePoint| last.0 != point.0 || last.1 != point.1)
            {
                motion.push(point);
            }
        }
        let distance: f64 = motion
            .windows(2)
            .map(|pair| (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1))
            .sum();
        if motion.len() < 2 || distance < 1.0 {
            return Err("swipe trace has no usable motion".into());
        }
        let origin_x = keys.iter().map(|key| key.1).fold(f64::INFINITY, f64::min);
        let origin_y = keys.iter().map(|key| key.2).fold(f64::INFINITY, f64::min);
        let quantized: Vec<_> = motion
            .iter()
            .map(|(x, y, _)| ((x - origin_x) as f32, (y - origin_y) as f32))
            .collect();
        if !quantized.windows(2).any(|pair| pair[0] != pair[1]) {
            return Err("swipe trace has no usable motion after coordinate conversion".into());
        }
        Ok(Self {
            trace: motion,
            keys,
            max: max.min(100) as usize,
        })
    }

    pub fn recognize(self, backend: &dyn DictionaryBackend) -> Result<Vec<(String, f64)>, String> {
        if self.max == 0 {
            return Ok(Vec::new());
        }
        // RectKey's public API uses integer bounds. Round logical pixels once;
        // all input points remain floating point in the same coordinate system.
        // Drift's RectKey normalizer currently subtracts the layout origin from
        // width/height as well as positions. Translate both keys and touch points
        // to a zero-origin rectangle first so absolute widget placement cannot
        // change feature radii or scores.
        let origin_x = self
            .keys
            .iter()
            .map(|key| key.1)
            .fold(f64::INFINITY, f64::min);
        let origin_y = self
            .keys
            .iter()
            .map(|key| key.2)
            .fold(f64::INFINITY, f64::min);
        let keys = self
            .keys
            .iter()
            .map(|(label, left, top, width, height)| {
                RectKey::new(
                    Some(label.clone()),
                    Vec::new(),
                    (left - origin_x).round() as u16,
                    (top - origin_y).round() as u16,
                    width.round() as u16,
                    height.round() as u16,
                )
            })
            .collect();
        let layout = RectKeyLayout::new(keys, &[]);
        let mut points: Vec<_> = self
            .trace
            .iter()
            .map(|(x, y, millis)| {
                InputPoint::new((x - origin_x) as f32, (y - origin_y) as f32, *millis, 0)
            })
            .collect();
        points
            .dedup_by(|left, right| left.point.x == right.point.x && left.point.y == right.point.y);
        let dictionary = SwipeDictionary {
            backend,
            letters: self.keys.iter().map(|key| key.0.as_bytes()[0]).collect(),
            deadline: Instant::now() + SEARCH_BUDGET,
            snapshot: OnceLock::new(),
        };
        let solver = DriftType {
            candidates_per_dictionary_hint: MAX_CANDIDATES,
            ..DriftType::new()
        };
        let results = solver.resolve(&layout, &[&dictionary], &dictionary, "", &points, "");
        if let Some(Err(error)) = dictionary.snapshot.get() {
            return Err(error.clone());
        }
        let mut seen = BTreeSet::new();
        Ok(results
            .into_iter()
            .filter(|candidate| {
                candidate.source_id.is_some()
                    && candidate.score.is_finite()
                    && seen.insert(candidate.word.clone())
            })
            .take(self.max)
            .map(|candidate| {
                // Drift distances are lower-is-better. Expose a finite, monotonic
                // higher-is-better heuristic, without treating it as a probability.
                (
                    candidate.word,
                    1.0 / (1.0 + f64::from(candidate.score.max(0.0))),
                )
            })
            .collect())
    }
}

struct SwipeDictionary<'a> {
    backend: &'a dyn DictionaryBackend,
    letters: Vec<u8>,
    deadline: Instant,
    // The stable owned strings bridge Patricia's streaming entries to Drift's
    // borrowed dictionary interface. They live for this single request only.
    snapshot: OnceLock<Result<Vec<DictionaryResult>, String>>,
}

impl<'d> drift_type::Dictionary<'d> for SwipeDictionary<'_> {
    fn get_candidate_words(&'d self, features: &FeatureExtraction, _hint: usize) -> Vec<&'d str> {
        let snapshot = self.snapshot.get_or_init(|| {
            let labels = |feature: &drift_type::GestureFeature| -> Vec<String> {
                feature
                    .possible_labels
                    .iter()
                    .filter(|s| s.len() == 1 && self.letters.contains(&s.as_bytes()[0]))
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            };
            let Some(first) = features.features().first() else {
                return Ok(Vec::new());
            };
            let Some(last) = features.features().last() else {
                return Ok(Vec::new());
            };
            let (starts, ends) = (labels(first), labels(last));
            if starts.is_empty() || ends.is_empty() {
                return Ok(Vec::new());
            }
            self.backend
                .swipe_candidates(&starts, &ends, &self.letters, self.deadline)
        });
        match snapshot {
            Ok(entries) => entries.iter().map(|entry| entry.word.as_str()).collect(),
            Err(_) => Vec::new(),
        }
    }
}

impl LanguageModel for SwipeDictionary<'_> {
    fn log_probability(&self, word: &str, _source_id: Option<usize>) -> f32 {
        self.snapshot
            .get()
            .and_then(|result| result.as_ref().ok())
            .and_then(|entries| entries.iter().find(|entry| entry.word == word))
            .map_or(0.0, |entry| entry.confidence as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::patricia::PatriciaDictionaryBackend;

    fn keys() -> Vec<KeyBounds> {
        ["qwertyuiop", "asdfghjkl", "zxcvbnm"]
            .iter()
            .enumerate()
            .flat_map(|(row, letters)| {
                letters.chars().enumerate().map(move |(col, label)| {
                    (
                        label.to_string(),
                        col as f64 * 40.0 + row as f64 * 20.0,
                        row as f64 * 50.0,
                        36.0,
                        46.0,
                    )
                })
            })
            .collect()
    }

    fn trace(word: &str, keys: &[KeyBounds]) -> Vec<TracePoint> {
        let mut points = Vec::new();
        for label in word.chars() {
            let key = keys.iter().find(|key| key.0 == label.to_string()).unwrap();
            let target = (key.1 + key.3 / 2.0, key.2 + key.4 / 2.0);
            if let Some((x, y, _)) = points.last().copied() {
                if target == (x, y) {
                    continue;
                }
                for step in 1..=6 {
                    points.push((
                        x + (target.0 - x) * step as f64 / 6.0,
                        y + (target.1 - y) * step as f64 / 6.0,
                        points.len() as u32 * 10,
                    ));
                }
            } else {
                points.push((target.0, target.1, 0));
            }
        }
        points
    }

    #[test]
    fn real_drift_scores_patricia_words_and_geometry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("en_US.dict");
        let mut dict = patricia_dict::Dictionary::create_empty_v403(&path, "en_US").unwrap();
        for (word, score) in [
            ("cat", 180),
            ("cart", 160),
            ("cut", 200),
            ("dog", 150),
            ("hello", 190),
            ("world", 200),
        ] {
            dict.append(word, score).unwrap();
        }
        drop(dict);
        let backend = PatriciaDictionaryBackend::open(&path).unwrap();
        let keys = keys();
        for word in ["cat", "dog", "hello", "world"] {
            let points = trace(word, &keys);
            let results = SwipeRequest::new(points.clone(), keys.clone(), 6)
                .unwrap()
                .recognize(&backend)
                .unwrap();
            assert_eq!(
                results.first().map(|r| r.0.as_str()),
                Some(word),
                "{word}: {results:?}"
            );
            assert!(
                results
                    .iter()
                    .all(|r| r.1.is_finite() && (0.0..=1.0).contains(&r.1))
            );
            assert_eq!(
                results,
                SwipeRequest::new(points, keys.clone(), 6)
                    .unwrap()
                    .recognize(&backend)
                    .unwrap()
            );
        }
        let original = SwipeRequest::new(trace("cat", &keys), keys.clone(), 6)
            .unwrap()
            .recognize(&backend)
            .unwrap();
        let translated_keys: Vec<_> = keys
            .iter()
            .map(|(s, x, y, w, h)| (s.clone(), x + 100.0, y + 80.0, *w, *h))
            .collect();
        let translated = SwipeRequest::new(trace("cat", &translated_keys), translated_keys, 6)
            .unwrap()
            .recognize(&backend)
            .unwrap();
        assert_eq!(
            original.iter().map(|r| &r.0).collect::<Vec<_>>(),
            translated.iter().map(|r| &r.0).collect::<Vec<_>>()
        );
        for (left, right) in original.iter().zip(&translated) {
            assert!(
                (left.1 - right.1).abs() < 0.00001,
                "translation changed score: {left:?} {right:?}"
            );
        }
        let shifted_keys: Vec<_> = keys
            .iter()
            .map(|(s, x, y, w, h)| (s.clone(), x * 2.0 + 100.0, y * 2.0 + 80.0, w * 2.0, h * 2.0))
            .collect();
        let shifted = trace("cat", &shifted_keys);
        let results = SwipeRequest::new(shifted, shifted_keys, 1)
            .unwrap()
            .recognize(&backend)
            .unwrap();
        assert_eq!(results[0].0, "cat");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn validation_rejects_malformed_or_stationary_requests() {
        let k = keys();
        let p = trace("cat", &k);
        assert!(SwipeRequest::new(vec![], k.clone(), 6).is_err());
        assert!(SwipeRequest::new(vec![p[0]; 513], k.clone(), 6).is_err());
        assert!(SwipeRequest::new(vec![p[0]; 3], k.clone(), 6).is_err());
        let mut bad = p.clone();
        bad[1].0 = f64::NAN;
        assert!(SwipeRequest::new(bad, k.clone(), 6).is_err());
        let mut bad = p.clone();
        bad[1].2 = 10001;
        assert!(SwipeRequest::new(bad, k.clone(), 6).is_err());
        let mut bad = p.clone();
        bad[2].2 = 0;
        assert!(SwipeRequest::new(bad, k.clone(), 6).is_err());
        let mut bad = k.clone();
        bad[0].3 = 0.0;
        assert!(SwipeRequest::new(p.clone(), bad, 6).is_err());
        let mut bad = k.clone();
        bad[0].0 = "the".into();
        assert!(SwipeRequest::new(p.clone(), bad, 6).is_err());
        let large_keys = vec![
            ("a".into(), 16000.0, 16000.0, 100.0, 100.0),
            ("b".into(), 16100.0, 16000.0, 100.0, 100.0),
        ];
        let subpixel_loop = (0..512)
            .map(|index| {
                let offset = if index % 2 == 0 { 0.0009 } else { -0.0009 };
                (-16000.0 + offset, -16000.0 + offset, index)
            })
            .collect();
        assert!(SwipeRequest::new(subpixel_loop, large_keys, 6).is_err());
        let mut bad = k.clone();
        bad[1].0 = bad[0].0.clone();
        assert!(SwipeRequest::new(p, bad, 6).is_err());
    }
}
