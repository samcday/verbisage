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
/// word typed with one is measured from that key instead of taking the
/// unknown-character penalty at that position.
#[test]
fn alternate_labels_place_a_typed_accent_on_its_key() {
    let dict = dictionary(&[("café", 100.0), ("cafes", 100.0)]);
    let engine = AndroidCompleter::new(&dict);
    let accented = |layout: Arc<RectKeyLayout>| {
        engine
            .complete_with(
                &CompletionInput {
                    // What the keyboard sends after the é long-press.
                    input: "café",
                    input_prep: keyboard_prep(),
                    context_prep: keyboard_prep(),
                    spatial: SpatialInput::Layout(layout),
                    ..Default::default()
                },
                6,
            )
            .unwrap()
    };

    let plain = accented(qwerty_lower());
    let with_alternates = accented(layout_with_alternates(
        &["qwertyuiop", "asdfghjkl", "zxcvbnm"],
        &[("e", &["é", "è", "ê"])],
    ));

    assert!(
        score_of(&with_alternates, "café") > score_of(&plain, "café"),
        "a key-less accent must not penalise the word it was typed into: \
         {with_alternates:?} vs {plain:?}"
    );
}

/// Known limitation, asserted so it stays visible: alternate labels are only
/// positions. The edit alphabet is built from single-character main labels, so
/// uploading é as an alternate does not make `cafe -> café` a correction.
/// Generating accent substitutions is a separate change to the edit source.
#[test]
fn alternate_labels_do_not_join_the_edit_alphabet() {
    let dict = dictionary(&[("café", 100.0)]);
    let engine = AndroidCompleter::new(&dict);
    let results = engine
        .complete_with(
            &CompletionInput {
                input: "cafe",
                input_prep: keyboard_prep(),
                context_prep: keyboard_prep(),
                spatial: SpatialInput::Layout(layout_with_alternates(
                    &["qwertyuiop", "asdfghjkl", "zxcvbnm"],
                    &[("e", &["é", "è", "ê"])],
                )),
                ..Default::default()
            },
            6,
        )
        .unwrap();

    assert!(
        !words(&results).contains(&"café"),
        "documented limitation changed; update CONTEXTUAL-API.md too: {results:?}"
    );
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
