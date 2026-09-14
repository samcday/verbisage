//! Whole-word swipe recognition over a registered shared layout.
//!
//! The trace and the registered key rectangles share the client's widget
//! coordinates, and the layout normalizes both exactly once. Drift Type owns
//! gesture scoring; Patricia supplies a bounded candidate snapshot whose words
//! are matched to the layout's labels in one canonical form and returned in
//! their stored spelling.
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use drift_type::{DriftType, FeatureExtraction, GestureFeature, InputPoint, LanguageModel};
use keyboard_layout::{Key, KeyboardLayout, Point, RectKeyLayout};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use crate::dictionary::DictionaryBackend;

pub const MAX_POINTS: usize = 512;
pub const MAX_WORD_BYTES: usize = 48;
pub const MAX_CANDIDATES: usize = 2048;
pub const MAX_TRIE_NODES: usize = 131072;
const MAX_COORDINATE: f64 = 16384.0;
const SEARCH_BUDGET: Duration = Duration::from_millis(500);

pub type TracePoint = (f64, f64, u32);

/// The one form in which layout labels and dictionary words are compared:
/// NFC. Applied to both sides, so a decomposed label meets a composed word.
/// Case is kept: which case variant sits on which key is the layout's own
/// business, and Unicode lowercasing does not identify the same physical key
/// (a Greek Σ lowercases to σ although the layout emits them from different
/// keys). A client that wants both cases recognized uploads both, as the
/// keyboard exports them. No accent is stripped and no full case folding
/// expands letters.
pub fn canonical(text: &str) -> String {
    text.nfc().collect::<String>()
}

/// The graphemes a registered layout can gesture, in canonical form, and the
/// labels it deliberately leaves out of gesture paths.
///
/// A label the layout maps (main or alternate, at most one grapheme) is
/// gesturable. A single-grapheme label present on a key that the layout
/// nevertheless does not map was excluded by the client's ignored labels:
/// words may contain it, but it is never required on the path. Anything else
/// is unknown, and a word needing it is not offered at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwipeVocabulary {
    mapped: HashSet<String>,
    ignored: HashSet<String>,
}

impl SwipeVocabulary {
    pub fn of(layout: &RectKeyLayout) -> Self {
        let mut mapped = HashSet::new();
        let mut ignored = HashSet::new();
        for key in layout.iter() {
            for label in key.all_labels() {
                let form = canonical(label);
                if form.graphemes(true).count() != 1 {
                    continue;
                }
                if layout.location_of(label).is_some() {
                    mapped.insert(form);
                } else {
                    ignored.insert(form);
                }
            }
        }
        ignored.retain(|form| !mapped.contains(form));
        Self { mapped, ignored }
    }

    /// A vocabulary from explicit labels, for tests of the candidate search.
    pub fn from_labels(mapped: &[&str], ignored: &[&str]) -> Self {
        Self {
            mapped: mapped.iter().map(|label| canonical(label)).collect(),
            ignored: ignored.iter().map(|label| canonical(label)).collect(),
        }
    }

    pub fn mapped_count(&self) -> usize {
        self.mapped.len()
    }

    pub fn is_mapped(&self, grapheme: &str) -> bool {
        self.mapped.contains(grapheme)
    }

    fn is_known(&self, grapheme: &str) -> bool {
        self.mapped.contains(grapheme) || self.ignored.contains(grapheme)
    }

    /// The scoring form of a stored word: its canonical form, when every
    /// grapheme is gesturable or ignored and at least two are gesturable.
    /// A word with any other grapheme has no complete path and is refused
    /// rather than scored on the part of it the layout can reach.
    pub fn scoring_form(&self, word: &str) -> Option<String> {
        let form = canonical(word);
        let mut gestured = 0usize;
        for grapheme in form.graphemes(true) {
            if self.mapped.contains(grapheme) {
                gestured += 1;
            } else if !self.ignored.contains(grapheme) {
                return None;
            }
        }
        (gestured >= 2).then_some(form)
    }

    /// The canonical form of a trie prefix when a word below it may still
    /// have a scoring form, or `None` when its subtree can be pruned.
    ///
    /// Only the complete graphemes decide: the last one may still gain a
    /// combining mark from a child node, and its lowercase form can depend on
    /// what follows it (a final sigma is not a medial one), so it is never
    /// judged here.
    pub fn prefix_form(&self, prefix: &str) -> Option<String> {
        let form = canonical(prefix);
        let graphemes: Vec<&str> = form.graphemes(true).collect();
        let complete = graphemes.len().saturating_sub(1);
        graphemes[..complete]
            .iter()
            .all(|grapheme| self.is_known(grapheme))
            .then_some(form)
    }
}

/// One stored word offered to the solver under its scoring form.
#[derive(Debug, Clone, PartialEq)]
pub struct SwipeCandidate {
    /// The spelling stored in the dictionary, returned to the caller.
    pub word: String,
    /// The canonical form the gesture is scored against.
    pub scoring: String,
    pub confidence: f64,
}

impl SwipeCandidate {
    /// Higher confidence first, then the stored spelling.
    pub fn rank(a: &Self, b: &Self) -> std::cmp::Ordering {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.word.cmp(&b.word))
    }
}

/// A registered key seen through the canonical form: the same centroid and
/// diameter, labels the solver's endpoint features can compare with scoring
/// forms.
struct CanonicalKey {
    main: Option<String>,
    alternates: Vec<String>,
    centroid: Point,
    diameter: f32,
}

impl Key for CanonicalKey {
    fn main_label(&self) -> Option<&str> {
        self.main.as_deref()
    }

    fn secondary_labels(&self) -> impl Iterator<Item = &str> {
        self.alternates.iter().map(String::as_str)
    }

    fn centroid(&self) -> Point {
        self.centroid
    }

    fn diameter(&self) -> f32 {
        self.diameter
    }
}

/// The registered layout with its labels in canonical form. Geometry,
/// normalization and the key diameter are the registered layout's own; only
/// how a grapheme finds its key changes, and it finds it through the main and
/// alternate labels exactly as the registered layout maps them.
struct CanonicalLayout<'a> {
    layout: &'a RectKeyLayout,
    keys: Vec<CanonicalKey>,
    positions: HashMap<String, Point>,
}

impl<'a> CanonicalLayout<'a> {
    fn new(layout: &'a RectKeyLayout, vocabulary: &SwipeVocabulary) -> Self {
        let gesturable = |label: &str| -> Option<String> {
            let form = canonical(label);
            (vocabulary.is_mapped(&form) && layout.location_of(label).is_some()).then_some(form)
        };
        let keys: Vec<CanonicalKey> = layout
            .iter()
            .map(|key| {
                let main = key.main_label().and_then(gesturable);
                let mut alternates: Vec<String> = Vec::new();
                for label in key.secondary_labels().filter_map(gesturable) {
                    if main.as_deref() != Some(label.as_str()) && !alternates.contains(&label) {
                        alternates.push(label);
                    }
                }
                CanonicalKey {
                    main,
                    alternates,
                    centroid: key.centroid(),
                    diameter: key.diameter(),
                }
            })
            .collect();
        // Main labels take precedence over alternates, as in the registered layout.
        let mut positions = HashMap::new();
        for key in &keys {
            if let Some(main) = &key.main {
                positions.entry(main.clone()).or_insert(key.centroid);
            }
        }
        for key in &keys {
            for alternate in &key.alternates {
                positions.entry(alternate.clone()).or_insert(key.centroid);
            }
        }
        Self {
            layout,
            keys,
            positions,
        }
    }
}

impl KeyboardLayout for CanonicalLayout<'_> {
    fn location_of(&self, label: &str) -> Option<Point> {
        self.positions
            .get(label)
            .or_else(|| self.positions.get(&canonical(label)))
            .copied()
    }

    fn path_for(&self, word: &str) -> Vec<Point> {
        word.graphemes(true)
            .flat_map(|grapheme| self.location_of(grapheme))
            .collect()
    }

    fn median_key_diameter(&self) -> f32 {
        self.layout.median_key_diameter()
    }

    fn normalise(&self, point: Point) -> Point {
        self.layout.normalise(point)
    }

    fn all_keys(&self) -> Vec<&impl Key> {
        self.keys.iter().collect()
    }
}

/// A validated, single-finger request bound to one immutable registered
/// layout. The frontend owns gesture cancellation and must discard any
/// response whose input context has changed while it was pending.
#[derive(Debug)]
pub struct SwipeRequest {
    trace: Vec<TracePoint>,
    layout: Arc<RectKeyLayout>,
    vocabulary: SwipeVocabulary,
    max: usize,
}

impl SwipeRequest {
    pub fn max_results(&self) -> usize {
        self.max
    }

    /// The registered layout this request was resolved against.
    pub fn layout(&self) -> &Arc<RectKeyLayout> {
        &self.layout
    }

    pub fn vocabulary(&self) -> &SwipeVocabulary {
        &self.vocabulary
    }

    /// Validate a trace against the layout it will be recognized on. The
    /// points are in the layout's own widget coordinates and are not
    /// converted here: the layout normalizes them once during recognition,
    /// and the same normalization is applied here to check that real motion
    /// survives it.
    pub fn new(
        trace: Vec<TracePoint>,
        layout: Arc<RectKeyLayout>,
        max: u32,
    ) -> Result<Self, String> {
        if !(2..=MAX_POINTS).contains(&trace.len()) {
            return Err("swipe requires 2..512 points".into());
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

        let diameter = layout.median_key_diameter();
        if !diameter.is_finite() || diameter <= 0.0 {
            return Err("registered layout has no usable key geometry".into());
        }
        let vocabulary = SwipeVocabulary::of(&layout);
        if vocabulary.mapped_count() < 2 {
            return Err("registered layout has fewer than two gesturable labels".into());
        }
        let normalized: Vec<(f32, f32)> = motion
            .iter()
            .map(|(x, y, _)| {
                let point = layout.normalise(Point::new(*x as f32, *y as f32));
                (point.x, point.y)
            })
            .collect();
        if normalized
            .iter()
            .any(|(x, y)| !x.is_finite() || !y.is_finite())
        {
            return Err("swipe trace does not normalize to finite coordinates".into());
        }
        if !normalized.windows(2).any(|pair| pair[0] != pair[1]) {
            return Err("swipe trace has no usable motion after layout normalization".into());
        }
        Ok(Self {
            trace: motion,
            layout,
            vocabulary,
            max: max as usize,
        })
    }

    pub fn recognize(self, backend: &dyn DictionaryBackend) -> Result<Vec<(String, f64)>, String> {
        if self.max == 0 {
            return Ok(Vec::new());
        }
        let layout = CanonicalLayout::new(&self.layout, &self.vocabulary);
        let mut points: Vec<_> = self
            .trace
            .iter()
            .map(|(x, y, millis)| InputPoint::new(*x as f32, *y as f32, *millis, 0))
            .collect();
        points
            .dedup_by(|left, right| left.point.x == right.point.x && left.point.y == right.point.y);
        let dictionary = SwipeDictionary {
            backend,
            vocabulary: &self.vocabulary,
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
            .filter(|candidate| candidate.source_id.is_some() && candidate.score.is_finite())
            .filter_map(|candidate| {
                let word = dictionary.spelling(&candidate.word)?;
                // Drift distances are lower-is-better. Expose a finite, monotonic
                // higher-is-better heuristic, without treating it as a probability.
                seen.insert(word.clone())
                    .then(|| (word, 1.0 / (1.0 + f64::from(candidate.score.max(0.0)))))
            })
            .take(self.max)
            .collect())
    }
}

/// The candidate snapshot of one request: every stored word with a scoring
/// form, and for each scoring form the best stored spelling behind it.
struct Snapshot {
    candidates: Vec<SwipeCandidate>,
    best: HashMap<String, usize>,
}

impl Snapshot {
    fn new(candidates: Vec<SwipeCandidate>) -> Self {
        let mut best: HashMap<String, usize> = HashMap::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let entry = best.entry(candidate.scoring.clone()).or_insert(index);
            if candidates[*entry].confidence < candidate.confidence {
                *entry = index;
            }
        }
        Self { candidates, best }
    }
}

struct SwipeDictionary<'a> {
    backend: &'a dyn DictionaryBackend,
    vocabulary: &'a SwipeVocabulary,
    deadline: Instant,
    // The stable owned strings bridge Patricia's streaming entries to Drift's
    // borrowed dictionary interface. They live for this single request only.
    snapshot: OnceLock<Result<Snapshot, String>>,
}

impl SwipeDictionary<'_> {
    fn spelling(&self, scoring: &str) -> Option<String> {
        let snapshot = self.snapshot.get()?.as_ref().ok()?;
        let index = *snapshot.best.get(scoring)?;
        Some(snapshot.candidates[index].word.clone())
    }
}

impl<'d> drift_type::Dictionary<'d> for SwipeDictionary<'_> {
    fn get_candidate_words(&'d self, features: &FeatureExtraction, _hint: usize) -> Vec<&'d str> {
        let snapshot = self.snapshot.get_or_init(|| {
            let labels = |feature: &GestureFeature| -> Vec<String> {
                feature
                    .possible_labels
                    .iter()
                    .map(|label| canonical(label))
                    .filter(|label| self.vocabulary.is_mapped(label))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            };
            let Some(first) = features.features().first() else {
                return Ok(Snapshot::new(Vec::new()));
            };
            let Some(last) = features.features().last() else {
                return Ok(Snapshot::new(Vec::new()));
            };
            let (starts, ends) = (labels(first), labels(last));
            if starts.is_empty() || ends.is_empty() {
                return Ok(Snapshot::new(Vec::new()));
            }
            self.backend
                .swipe_candidates(self.vocabulary, &starts, &ends, self.deadline)
                .map(Snapshot::new)
        });
        match snapshot {
            Ok(snapshot) => snapshot
                .best
                .values()
                .map(|index| snapshot.candidates[*index].scoring.as_str())
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

impl LanguageModel for SwipeDictionary<'_> {
    fn log_probability(&self, word: &str, _source_id: Option<usize>) -> f32 {
        self.snapshot
            .get()
            .and_then(|result| result.as_ref().ok())
            .and_then(|snapshot| {
                snapshot
                    .best
                    .get(word)
                    .map(|index| &snapshot.candidates[*index])
            })
            .map_or(0.0, |candidate| candidate.confidence as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::patricia::PatriciaDictionaryBackend;
    use crate::layout::{KeyBox, LayoutUpload, build_layout};

    fn upload(scale: f64, offset: (f64, f64)) -> LayoutUpload {
        let keys = ["qwertyuiop", "asdfghjkl", "zxcvbnm"]
            .iter()
            .enumerate()
            .flat_map(|(row, letters)| {
                letters.chars().enumerate().map(move |(col, label)| KeyBox {
                    label: label.to_string(),
                    alt_labels: Vec::new(),
                    left: ((col as f64 * 40.0 + row as f64 * 20.0) * scale + offset.0) as f32,
                    top: (row as f64 * 50.0 * scale + offset.1) as f32,
                    width: (36.0 * scale) as f32,
                    height: (46.0 * scale) as f32,
                })
            })
            .collect();
        LayoutUpload {
            keys: Some(keys),
            rows: None,
            ignored_labels: Vec::new(),
        }
    }

    fn layout(upload: &LayoutUpload) -> Arc<RectKeyLayout> {
        Arc::new(build_layout(upload).unwrap())
    }

    fn trace(word: &str, upload: &LayoutUpload) -> Vec<TracePoint> {
        let keys = upload.keys.as_ref().unwrap();
        let mut points = Vec::new();
        for label in word.chars() {
            let key = keys
                .iter()
                .find(|key| key.label == label.to_string())
                .unwrap();
            let target = (
                f64::from(key.left + key.width / 2.0),
                f64::from(key.top + key.height / 2.0),
            );
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
    fn canonical_form_composes_without_changing_case_or_stripping_accents() {
        assert_eq!(canonical("E\u{301}COLE"), "ÉCOLE");
        assert_eq!(canonical("cafe\u{301}"), "café");
        assert_eq!(canonical("Straße"), "Straße");
        assert_eq!(canonical("ΟΔΟΣ"), "ΟΔΟΣ");
        // Case variants stay distinct: which key emits which is the layout's
        // own business, and Unicode lowercasing would merge different keys.
        assert_ne!(canonical("Σ"), canonical("σ"));
        assert_ne!(canonical("É"), canonical("é"));
    }

    #[test]
    fn vocabulary_refuses_unmapped_graphemes_and_keeps_ignored_ones_optional() {
        let vocabulary = SwipeVocabulary::from_labels(&["c", "a", "f", "e", "é"], &["'"]);
        assert_eq!(vocabulary.scoring_form("café").as_deref(), Some("café"));
        assert_eq!(
            vocabulary.scoring_form("cafe\u{301}").as_deref(),
            Some("café")
        );
        // Case is not inferred: relatives the layout never declared are
        // refused even though their lowercase twins can gesture.
        assert!(vocabulary.scoring_form("CAFE").is_none());
        assert!(vocabulary.scoring_form("cafÉ").is_none());
        assert_eq!(vocabulary.scoring_form("caf'e").as_deref(), Some("caf'e"));
        assert!(
            vocabulary.scoring_form("cafés").is_none(),
            "s is not on the layout"
        );
        assert!(
            vocabulary.scoring_form("a").is_none(),
            "one gesturable grapheme is no path"
        );
        assert!(vocabulary.scoring_form("a'").is_none());
        // A decomposed accent still under construction is never pruned away.
        assert!(vocabulary.prefix_form("cafe").is_some());
        assert!(vocabulary.prefix_form("cafe\u{301}").is_some());
        assert!(
            vocabulary.prefix_form("cafx").is_some(),
            "the final grapheme is undecided"
        );
        assert!(
            vocabulary.prefix_form("cafxe").is_none(),
            "a complete unknown grapheme prunes"
        );
    }

    #[test]
    fn vocabulary_of_a_layout_separates_mapped_from_ignored_labels() {
        let upload = LayoutUpload {
            keys: Some(vec![
                KeyBox {
                    label: "E".into(),
                    alt_labels: vec!["É".into(), "é".into()],
                    left: 0.0,
                    top: 0.0,
                    width: 10.0,
                    height: 10.0,
                },
                KeyBox {
                    label: "'".into(),
                    alt_labels: vec!["m".into()],
                    left: 10.0,
                    top: 0.0,
                    width: 10.0,
                    height: 10.0,
                },
            ]),
            rows: None,
            ignored_labels: vec!["'".into()],
        };
        let vocabulary = SwipeVocabulary::of(&build_layout(&upload).unwrap());
        // Every label the keys carry is gesturable exactly as uploaded.
        assert!(vocabulary.is_mapped("E"));
        assert!(vocabulary.is_mapped("É"));
        assert!(vocabulary.is_mapped("é"));
        assert!(vocabulary.is_mapped("m"));
        assert!(!vocabulary.is_mapped("'"));
        // The upload declares no standalone lowercase `e`, and none is
        // invented from the labels it does carry.
        assert!(!vocabulary.is_mapped("e"));
        assert_eq!(vocabulary.scoring_form("m'É").as_deref(), Some("m'É"));
        assert_eq!(vocabulary.mapped_count(), 4);
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
        let base = upload(1.0, (0.0, 0.0));
        let registered = layout(&base);
        for word in ["cat", "dog", "hello", "world"] {
            let points = trace(word, &base);
            let request = SwipeRequest::new(points.clone(), registered.clone(), 6).unwrap();
            assert!(Arc::ptr_eq(request.layout(), &registered));
            let results = request.recognize(&backend).unwrap();
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
                SwipeRequest::new(points, registered.clone(), 6)
                    .unwrap()
                    .recognize(&backend)
                    .unwrap()
            );
        }
        let original = SwipeRequest::new(trace("cat", &base), registered.clone(), 6)
            .unwrap()
            .recognize(&backend)
            .unwrap();
        let translated_upload = upload(1.0, (100.0, 80.0));
        let translated = SwipeRequest::new(
            trace("cat", &translated_upload),
            layout(&translated_upload),
            6,
        )
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
        let scaled_upload = upload(2.0, (100.0, 80.0));
        let results = SwipeRequest::new(trace("cat", &scaled_upload), layout(&scaled_upload), 1)
            .unwrap()
            .recognize(&backend)
            .unwrap();
        assert_eq!(results[0].0, "cat");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn validation_rejects_malformed_or_stationary_requests() {
        let base = upload(1.0, (0.0, 0.0));
        let registered = layout(&base);
        let p = trace("cat", &base);
        assert!(SwipeRequest::new(vec![], registered.clone(), 6).is_err());
        assert!(SwipeRequest::new(vec![p[0]; 513], registered.clone(), 6).is_err());
        assert!(SwipeRequest::new(vec![p[0]; 3], registered.clone(), 6).is_err());
        let mut bad = p.clone();
        bad[1].0 = f64::NAN;
        assert!(SwipeRequest::new(bad, registered.clone(), 6).is_err());
        let mut bad = p.clone();
        bad[1].2 = 10001;
        assert!(SwipeRequest::new(bad, registered.clone(), 6).is_err());
        let mut bad = p.clone();
        bad[2].2 = 0;
        assert!(SwipeRequest::new(bad, registered.clone(), 6).is_err());
        // Sub-pixel jitter far from a huge layout: motion before conversion,
        // none after it.
        let huge = LayoutUpload {
            keys: Some(vec![
                KeyBox {
                    label: "a".into(),
                    alt_labels: Vec::new(),
                    left: 16000.0,
                    top: 16000.0,
                    width: 100.0,
                    height: 100.0,
                },
                KeyBox {
                    label: "b".into(),
                    alt_labels: Vec::new(),
                    left: 16100.0,
                    top: 16000.0,
                    width: 100.0,
                    height: 100.0,
                },
            ]),
            rows: None,
            ignored_labels: Vec::new(),
        };
        let subpixel_loop = (0..512)
            .map(|index| {
                let offset = if index % 2 == 0 { 0.0009 } else { -0.0009 };
                (-16000.0 + offset, -16000.0 + offset, index)
            })
            .collect();
        assert!(SwipeRequest::new(subpixel_loop, layout(&huge), 6).is_err());
        // A layout with a single gesturable label offers no path.
        let lone = LayoutUpload {
            keys: Some(vec![KeyBox {
                label: "a".into(),
                alt_labels: Vec::new(),
                left: 0.0,
                top: 0.0,
                width: 10.0,
                height: 10.0,
            }]),
            rows: None,
            ignored_labels: Vec::new(),
        };
        assert!(
            SwipeRequest::new(p, layout(&lone), 6)
                .unwrap_err()
                .contains("gesturable")
        );
    }
}
