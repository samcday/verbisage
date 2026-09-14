//! Layout-only request behaviour for on-screen keyboard clients.
//!
//! These cover the frontend contract used by the paired Stevia branch: the
//! client registers the layer it is actually showing (so an active Shift layer
//! uploads capital labels) and sends the token with completion requests only.
//! Touch points are deliberately absent; see `CONTEXTUAL-API.md`.

use std::sync::Arc;

use keyboard_layout::{RectKey, RectKeyLayout};
use verbisage::completion::{
    AndroidCompleter, CompletionCandidate, CompletionEngine, CompletionInput,
};
use verbisage::dictionary::FileDictionaryBackend;
use verbisage::layout::{LayoutUpload, build_layout};
use verbisage::prediction::{Prediction, Predictor};
use verbisage::spatial::SpatialInput;
use verbisage::text::{CaseFold, Normalization, TextPrep};

/// A block of uniform keys, labelled exactly as the caller spells them.
fn layout_from(rows: &[&str]) -> Arc<RectKeyLayout> {
    layout_with_alternates(rows, &[])
}

/// As [`layout_from`], with extra alternate labels attached to a main label's
/// key, the way an on-screen keyboard exposes its long-press characters.
fn layout_with_alternates(rows: &[&str], alternates: &[(&str, &[&str])]) -> Arc<RectKeyLayout> {
    let mut keys = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        for (c, label) in row.chars().enumerate() {
            let label = label.to_string();
            let alt: Vec<String> = alternates
                .iter()
                .find(|(main, _)| *main == label)
                .map(|(_, alt)| alt.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default();
            keys.push(RectKey::from_rect(
                Some(label),
                alt,
                c as f32 * 10.0,
                r as f32 * 20.0,
                10.0,
                20.0,
            ));
        }
    }
    Arc::new(RectKeyLayout::new(keys, &[]))
}

fn qwerty_lower() -> Arc<RectKeyLayout> {
    layout_from(&["qwertyuiop", "asdfghjkl", "zxcvbnm"])
}

fn qwerty_upper() -> Arc<RectKeyLayout> {
    layout_from(&["QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM"])
}

/// The frozen real Stevia US normal-layer export, copied unchanged. See
/// `tests/fixtures/README.md` for its identity and generation provenance.
const EXPORTED_US_NORMAL: &str = include_str!("fixtures/layout-us-normal.json");

fn exported_us_normal() -> Arc<RectKeyLayout> {
    let upload: LayoutUpload =
        serde_json::from_str(EXPORTED_US_NORMAL).expect("frozen Stevia US normal-layer export");
    Arc::new(build_layout(&upload).expect("build exported US normal layer"))
}

/// The same export with the apostrophe removed from the period key's
/// long-press menu, so an apostrophe has no key at all.
fn exported_us_normal_without_apostrophe() -> Arc<RectKeyLayout> {
    let mut upload: LayoutUpload = serde_json::from_str(EXPORTED_US_NORMAL).expect("fixture");
    for key in upload.keys.as_mut().expect("keys") {
        key.alt_labels.retain(|label| label != "'");
    }
    Arc::new(build_layout(&upload).expect("build control layer"))
}

/// What the Stevia completer sends on every request.
fn keyboard_prep() -> TextPrep {
    TextPrep {
        normalization: Normalization::Nfc,
        fold: CaseFold::Full,
    }
}

fn dictionary(rows: &[(&str, f64)]) -> FileDictionaryBackend {
    let mut dictionary = FileDictionaryBackend::new();
    for &(word, count) in rows {
        dictionary.add_word_mut(word.into(), count);
    }
    dictionary
}

fn words(rows: &[CompletionCandidate]) -> Vec<&str> {
    rows.iter().map(|r| r.word.as_str()).collect()
}

fn score_of(rows: &[CompletionCandidate], word: &str) -> f64 {
    rows.iter()
        .find(|row| row.word == word)
        .map(|row| row.score)
        .unwrap_or_else(|| panic!("{word} missing from {rows:?}"))
}

/// A language model with realistic, genuinely small next-word probabilities.
struct CorpusLikeModel(Vec<(&'static str, f64)>);

impl Predictor for CorpusLikeModel {
    fn predict_next(&self, _context: &[&str], max_suggestions: usize) -> Vec<Prediction> {
        let mut rows: Vec<Prediction> = self
            .0
            .iter()
            .map(|(word, p)| Prediction {
                word: (*word).to_string(),
                confidence: *p,
            })
            .collect();
        rows.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
        rows.truncate(max_suggestions);
        rows
    }

    fn candidate_score(&self, _context: &[&str], candidate: &str) -> Option<f64> {
        self.0
            .iter()
            .find(|(word, _)| *word == candidate)
            .map(|(_, p)| *p)
    }
}

/// An empty input asks for next-word candidates. Registering a layout must not
/// disturb that ranking: there is no typed position to compare against, so the
/// spatial branch charged every candidate the same first-completion cost and
/// collapsed realistic low probabilities into a lexical tie.
#[test]
fn empty_input_predictions_keep_contextual_order_with_a_registered_layout() {
    let dict = dictionary(&[
        ("later", 90.0),
        ("have", 80.0),
        ("know", 70.0),
        ("apple", 60.0),
    ]);
    // Deliberately low, as real corpus next-word probabilities are: all four
    // sit below the point where the additive spatial model saturates at zero.
    let model = CorpusLikeModel(vec![
        ("later", 0.031),
        ("have", 0.024),
        ("know", 0.011),
        ("apple", 0.004),
    ]);
    let engine = AndroidCompleter::new(&dict).with_predictor(Some(&model));

    let request = |spatial: SpatialInput| {
        engine
            .complete_with(
                &CompletionInput {
                    input: "",
                    context: &["see", "you"],
                    input_prep: keyboard_prep(),
                    context_prep: keyboard_prep(),
                    spatial,
                    ..Default::default()
                },
                6,
            )
            .unwrap()
    };

    let plain = request(SpatialInput::None);
    let with_layout = request(SpatialInput::Layout(qwerty_lower()));

    assert_eq!(
        words(&plain),
        ["later", "have", "know", "apple"],
        "the fixture must rank by probability without a layout"
    );
    assert_eq!(
        words(&with_layout),
        words(&plain),
        "a registered layout must not reorder next-word predictions"
    );
    for (layout, plain) in with_layout.iter().zip(&plain) {
        assert_eq!(
            layout.score, plain.score,
            "prediction scores must be identical: {layout:?} vs {plain:?}"
        );
    }
    assert!(
        with_layout.windows(2).all(|w| w[0].score > w[1].score),
        "predictions must stay strictly ordered, not collapse into a tie: {with_layout:?}"
    );
}

/// The client registers the layer it is really showing. With Shift active that
/// layer carries capital labels, and the request's own preparation has already
/// folded the input; correction must behave the same either way.
#[test]
fn an_active_shift_layer_corrects_like_the_unshifted_layer() {
    let dict = dictionary(&[("hello", 158.0), ("help", 120.0), ("helm", 40.0)]);
    let engine = AndroidCompleter::new(&dict);

    let request = |spatial: SpatialInput| {
        engine
            .complete_with(
                &CompletionInput {
                    input: "helo",
                    input_prep: keyboard_prep(),
                    context_prep: keyboard_prep(),
                    spatial,
                    ..Default::default()
                },
                6,
            )
            .unwrap()
    };

    let lower = request(SpatialInput::Layout(qwerty_lower()));
    let upper = request(SpatialInput::Layout(qwerty_upper()));

    assert_eq!(words(&lower)[0], "hello", "{lower:?}");
    assert_eq!(words(&upper), words(&lower), "Shift parity: {upper:?}");
    for (shifted, plain) in upper.iter().zip(&lower) {
        assert_eq!(shifted.score, plain.score, "{shifted:?} vs {plain:?}");
    }
}

/// Without the request's preparation the capital labels are taken as authored
/// and a folded input cannot reach any of them. This documents that the
/// preparation is what makes an active Shift layer usable, so a regression
/// shows up here rather than as a silent loss of corrections.
#[test]
fn unprepared_capital_labels_cannot_correct_folded_input() {
    let dict = dictionary(&[("hello", 158.0), ("help", 120.0)]);
    let engine = AndroidCompleter::new(&dict);
    let spatial = SpatialInput::Layout(qwerty_upper());

    let unprepared = engine
        .complete_with(
            &CompletionInput {
                input: "helo",
                spatial: spatial.clone(),
                ..Default::default()
            },
            6,
        )
        .unwrap();
    let prepared = engine
        .complete_with(
            &CompletionInput {
                input: "helo",
                input_prep: keyboard_prep(),
                context_prep: keyboard_prep(),
                spatial,
                ..Default::default()
            },
            6,
        )
        .unwrap();

    assert!(
        !words(&unprepared).contains(&"hello"),
        "no-op preparation keeps the authored capitals: {unprepared:?}"
    );
    assert!(
        words(&prepared).contains(&"hello"),
        "the request's own preparation reaches them: {prepared:?}"
    );
}

/// Alternate (long-press) labels place accented characters on their key, so a
/// word typed with one is corrected using the layout around that key instead
/// of falling back to a flat alphabet.
#[test]
fn alternate_labels_anchor_corrections_from_a_typed_accent() {
    // "hzllo" is the more frequent word, so only real proximity evidence can
    // put the neighbouring "hello" first.
    let dict = dictionary(&[("hello", 100.0), ("hzllo", 120.0)]);
    let engine = AndroidCompleter::new(&dict);
    let typed_accent = |layout: Arc<RectKeyLayout>| {
        engine
            .complete_with(
                &CompletionInput {
                    // What the keyboard sends after an é long-press slip.
                    input: "héllo",
                    input_prep: keyboard_prep(),
                    context_prep: keyboard_prep(),
                    spatial: SpatialInput::Layout(layout),
                    ..Default::default()
                },
                6,
            )
            .unwrap()
    };

    let plain = typed_accent(qwerty_lower());
    let with_alternates = typed_accent(layout_with_alternates(
        &["qwertyuiop", "asdfghjkl", "zxcvbnm"],
        &[("e", &["é", "è", "ê"])],
    ));

    assert_eq!(
        words(&with_alternates)[0],
        "hello",
        "the accent's own key must rank its neighbour first: {with_alternates:?}"
    );
    assert_eq!(
        words(&plain)[0],
        "hzllo",
        "without the alternate the accent has no position and frequency decides: {plain:?}"
    );
}

/// Alternate labels are positions and edit-alphabet members alike: a word
/// typed with a long-press character is corrected from the layout around that
/// key, and a neighbouring key's alternate can be the correction itself.
#[test]
fn alternate_labels_join_the_edit_alphabet() {
    let dict = dictionary(&[("café", 100.0)]);
    let engine = AndroidCompleter::new(&dict);
    let alternates = layout_with_alternates(
        &["qwertyuiop", "asdfghjkl", "zxcvbnm"],
        &[("e", &["é", "è", "ê"])],
    );
    let results = engine
        .complete_with(
            &CompletionInput {
                input: "cafe",
                input_prep: keyboard_prep(),
                context_prep: keyboard_prep(),
                spatial: SpatialInput::Layout(alternates),
                ..Default::default()
            },
            6,
        )
        .unwrap();

    assert_eq!(
        words(&results)[0],
        "café",
        "the declared alternate generates the accent correction: {results:?}"
    );
}

/// Layout-only corrections are priced by the edit that produced them, so each
/// kind of slip is charged once. All five candidates below are one edit from
/// the typed word and share a frequency, leaving the edit cost to decide.
#[test]
fn each_edit_kind_is_charged_once_under_a_layout() {
    let dict = dictionary(&[
        ("qweet", 100.0), // a missed repeated letter
        ("qwte", 100.0),  // two letters swapped
        ("qwer", 100.0),  // t mistyped as its neighbour r
        ("qwe", 100.0),   // an extra letter typed
        ("qwez", 100.0),  // t mistyped as a distant z
    ]);
    let engine = AndroidCompleter::new(&dict);
    let request = |spatial: SpatialInput| {
        engine
            .complete_with(
                &CompletionInput {
                    input: "qwet",
                    input_prep: keyboard_prep(),
                    context_prep: keyboard_prep(),
                    spatial,
                    ..Default::default()
                },
                6,
            )
            .unwrap()
    };

    let ranked = request(SpatialInput::Layout(qwerty_lower()));
    let rank = |word: &str| {
        words(&ranked)
            .iter()
            .position(|candidate| *candidate == word)
            .unwrap_or_else(|| panic!("{word} missing from {ranked:?}"))
    };

    // A missing letter costs one omission, not one mismatch per following
    // character: it must not sink below an unrelated neighbouring key.
    assert!(rank("qweet") < rank("qwer"), "{ranked:?}");
    assert!(rank("qwte") < rank("qwer"), "{ranked:?}");
    // Proximity still separates same-length candidates.
    assert!(rank("qwer") < rank("qwez"), "{ranked:?}");
    // An extra typed letter is a real slip, but a costlier one than a
    // neighbouring key, and still better than a distant key.
    assert!(rank("qwer") < rank("qwe"), "{ranked:?}");
    assert!(rank("qwe") < rank("qwez"), "{ranked:?}");

    // Without geometry the two substitutions are indistinguishable, so the
    // layout is what separates them.
    let flat = request(SpatialInput::None);
    assert_eq!(
        score_of(&flat, "qwer"),
        score_of(&flat, "qwez"),
        "the geometry-free alphabet must not prefer either neighbour: {flat:?}"
    );
    assert!(
        score_of(&ranked, "qwer") > score_of(&ranked, "qwez"),
        "the layout must: {ranked:?}"
    );
}


/// What a layout can and cannot settle on its own.
///
/// A dropped repeated letter and a slip onto the neighbouring key are both
/// cheap and comparable, so the language model decides between them; a slip
/// onto a distant key is not, and loses even when it is the likeliest word.
/// This is the shape of the real `helo` case, where the shipped dictionary
/// makes `help` more probable than `hello`, and it is why per-character touch
/// points (a separate change) are what can distinguish the first two.
#[test]
fn a_neighbouring_slip_and_a_dropped_letter_are_settled_by_the_language_model() {
    let ranked = |rows: &[(&str, f64)]| {
        let mut dict = FileDictionaryBackend::new();
        for (word, count) in rows {
            dict.add_word_mut((*word).into(), *count);
        }
        let engine = AndroidCompleter::new(&dict);
        let out = engine
            .complete_with(
                &CompletionInput {
                    input: "helo",
                    input_prep: keyboard_prep(),
                    context_prep: keyboard_prep(),
                    spatial: SpatialInput::Layout(qwerty_lower()),
                    ..Default::default()
                },
                6,
            )
            .unwrap();
        out.iter().map(|row| row.word.clone()).collect::<Vec<_>>()
    };

    // Equally probable: the cheaper edit wins.
    assert_eq!(
        ranked(&[("hello", 500.0), ("help", 500.0)])[0],
        "hello"
    );
    // As in the shipped dictionary, where help is the more probable word.
    assert_eq!(
        ranked(&[("hello", 471.0), ("help", 584.0)])[0],
        "help"
    );
    // A distant key is not a cheap slip, however probable the word is.
    let distant = ranked(&[("hello", 471.0), ("held", 620.0)]);
    assert_eq!(distant[0], "hello", "{distant:?}");
}


/// A layout whose alphabet is neither ASCII nor 26 keys must still correct,
/// using its own keys rather than a built-in Latin alphabet.
#[test]
fn a_non_latin_layout_corrects_within_its_own_alphabet() {
    let dict = dictionary(&[("привет", 158.0), ("привел", 40.0)]);
    let engine = AndroidCompleter::new(&dict);
    let layout = layout_from(&["йцукенгшщзхъ", "фывапролджэ", "ячсмитьбю"]);

    let results = engine
        .complete_with(
            &CompletionInput {
                // 'с' sits next to 'м' and 'и' on the bottom row; the typed
                // word is one substitution away from the dictionary entry.
                input: "присет",
                input_prep: keyboard_prep(),
                context_prep: keyboard_prep(),
                spatial: SpatialInput::Layout(layout),
                ..Default::default()
            },
            6,
        )
        .unwrap();

    assert_eq!(words(&results)[0], "привет", "{results:?}");
}

/// The real exported US normal layer places an apostrophe only on the period
/// key's long-press menu. Spatial correction resolves both the explicit
/// punctuation slip (`don.t`, a substitution on that key) and the missing
/// apostrophe (`dont`, an omission the same key supplies) to the stored
/// spelling. The two are distinct edits, so they are asserted separately.
#[test]
fn exported_us_normal_corrects_the_period_key_apostrophe_alternative() {
    let dict = dictionary(&[("don't", 220.0)]);
    let engine = AndroidCompleter::new(&dict);
    let request = |input: &str| {
        engine
            .complete_with(
                &CompletionInput {
                    input,
                    input_prep: keyboard_prep(),
                    context_prep: keyboard_prep(),
                    spatial: SpatialInput::Layout(exported_us_normal()),
                    ..Default::default()
                },
                6,
            )
            .unwrap()
    };

    // The tap landed on the period key's apostrophe alternate.
    let dotted = request("don.t");
    assert_eq!(
        words(&dotted).first(),
        Some(&"don't"),
        "the period slip corrects through the apostrophe alternate: {dotted:?}"
    );

    // A different case: the apostrophe is simply missing, not mistyped.
    let missing = request("dont");
    assert_eq!(
        words(&missing).first(),
        Some(&"don't"),
        "the omission is supplied by the period key's alternate: {missing:?}"
    );
}

/// Control: with the apostrophe removed from the period key, neither input can
/// reach the apostrophe spelling through this geometry. The layout is
/// otherwise intact and still corrects ordinary slips against its own keys, so
/// the control isolates the punctuation mapping rather than a broken layout.
#[test]
fn exported_us_normal_without_the_apostrophe_mapping_cannot_correct_to_it() {
    let dict = dictionary(&[("don't", 220.0), ("dot", 90.0)]);
    let engine = AndroidCompleter::new(&dict);
    let request = |input: &str| {
        engine
            .complete_with(
                &CompletionInput {
                    input,
                    input_prep: keyboard_prep(),
                    context_prep: keyboard_prep(),
                    spatial: SpatialInput::Layout(exported_us_normal_without_apostrophe()),
                    ..Default::default()
                },
                6,
            )
            .unwrap()
    };

    for input in ["don.t", "dont"] {
        let results = request(input);
        assert!(
            !words(&results).contains(&"don't"),
            "no apostrophe key means no apostrophe correction from {input:?}: {results:?}"
        );
    }
    // The control geometry still prices an ordinary neighbour slip against its
    // own keys, so removing the apostrophe did not disable correction itself.
    let ordinary = request("dgt");
    assert!(
        words(&ordinary).contains(&"dot"),
        "the control layout still corrects its own slips: {ordinary:?}"
    );
}
