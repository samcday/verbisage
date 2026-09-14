//! Layout-only spatial model.
//!
//! Substitutions cover every key on the layout, weighted by the normalised
//! distance between key centres: nearby keys are cheaper, distant keys are
//! still offered so geometry never loses a candidate. Costs follow HeliBoard's
//! proximity/substitution split; vowel families are added as additional
//! proximity characters. Used for physical keyboards, where there are no touch
//! points.

use std::collections::HashMap;

use keyboard_layout::{Key, KeyboardLayout, RectKeyLayout};

use super::cost::{
    ADDITIONAL_PROXIMITY_COST, MAX_SPATIAL_DISTANCE, PROXIMITY_COST, SEARCH_DISTANCE,
    SUBSTITUTION_COST, additional_proximity, cost_to_quality, key_distance_cost,
};
use crate::spellcheck::edits::{EditSource, LatinAlphabet};

pub struct PhysicalEdits<'a> {
    layout: &'a RectKeyLayout,
    letters: Vec<char>,
    key_diameter: f64,
}

impl<'a> PhysicalEdits<'a> {
    pub fn new(layout: &'a RectKeyLayout) -> Self {
        Self::new_with(layout, &|label: &str| label.to_string())
    }

    /// Like [`PhysicalEdits::new`], but prepares the layout's labels with the
    /// request's own preparation so edits match prepared candidates.
    pub fn new_with(layout: &'a RectKeyLayout, prepare: &dyn Fn(&str) -> String) -> Self {
        Self {
            layout,
            letters: single_char_labels_with(layout, prepare),
            key_diameter: f64::from(layout.median_key_diameter().max(f32::EPSILON)),
        }
    }

    pub fn layout(&self) -> &RectKeyLayout {
        self.layout
    }

    pub fn key_diameter(&self) -> f64 {
        self.key_diameter
    }
}

impl EditSource for PhysicalEdits<'_> {
    fn letters(&self) -> &[char] {
        &self.letters
    }

    fn substitutions(&self, ch: char, _position: Option<usize>) -> Vec<(char, f64)> {
        let Some(origin) = self.layout.location_of(&ch.to_string()) else {
            return LatinAlphabet.substitutions(ch, None);
        };
        let mut weights: HashMap<char, f64> = HashMap::new();
        for candidate in &self.letters {
            if *candidate == ch {
                continue;
            }
            let Some(target) = self.layout.location_of(&candidate.to_string()) else {
                continue;
            };
            let distance = f64::from(origin.distance(target)) / self.key_diameter;
            let normalized = (distance * distance).min(MAX_SPATIAL_DISTANCE);
            let class = if distance <= SEARCH_DISTANCE {
                PROXIMITY_COST
            } else {
                SUBSTITUTION_COST
            };
            weights.insert(
                *candidate,
                cost_to_quality(class + key_distance_cost(normalized)),
            );
        }
        for extra in additional_proximity(ch) {
            if *extra != ch {
                weights
                    .entry(*extra)
                    .or_insert_with(|| cost_to_quality(ADDITIONAL_PROXIMITY_COST));
            }
        }
        weights.into_iter().collect()
    }
}

/// Collect the single-character labels of a layout after applying the
/// caller's label preparation.
///
/// Main and alternate labels are both sources of typed characters: a layout
/// may offer a character nowhere but in a long-press menu (an apostrophe that
/// only hangs off the period key, say), and correcting a tap against that
/// character needs its key. Generated edits are compared against prepared
/// candidate spellings, so a layout authored in its own spelling (an active
/// Shift layer carries `Q`, not `q`) must be prepared the same way as the
/// input. A label that does not prepare to exactly one character is skipped.
pub(super) fn single_char_labels_with(
    layout: &RectKeyLayout,
    prepare: &dyn Fn(&str) -> String,
) -> Vec<char> {
    let mut letters = Vec::new();
    for key in layout.iter() {
        for label in key.all_labels() {
            let prepared = prepare(label);
            let mut chars = prepared.chars();
            if let (Some(ch), None) = (chars.next(), chars.next())
                && !letters.contains(&ch)
            {
                letters.push(ch);
            }
        }
    }
    letters
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard_layout::RectKey;

    #[test]
    fn substitutions_are_proximity_weighted() {
        let keys = vec![
            RectKey::from_rect(Some("q".into()), vec![], 0.0, 0.0, 10.0, 10.0),
            RectKey::from_rect(Some("w".into()), vec![], 10.0, 0.0, 10.0, 10.0),
            RectKey::from_rect(Some("e".into()), vec![], 20.0, 0.0, 10.0, 10.0),
            RectKey::from_rect(Some("p".into()), vec![], 90.0, 0.0, 10.0, 10.0),
            RectKey::from_rect(Some("a".into()), vec![], 0.0, 10.0, 10.0, 10.0),
            RectKey::from_rect(Some("s".into()), vec![], 10.0, 10.0, 10.0, 10.0),
            RectKey::from_rect(Some("d".into()), vec![], 20.0, 10.0, 10.0, 10.0),
        ];
        let layout = RectKeyLayout::new(keys, &[]);
        let edits = PhysicalEdits::new(&layout);
        let weights: std::collections::HashMap<char, f64> =
            edits.substitutions('s', Some(0)).into_iter().collect();

        assert!(weights.contains_key(&'w'));
        assert!(weights.contains_key(&'d'));
        assert!(
            weights[&'p'] < weights[&'w'],
            "a distant key must still be offered but rank below a nearby one"
        );
        assert!(weights[&'w'] > weights[&'e']);
    }

    #[test]
    fn alternate_labels_are_tap_candidates_and_multichar_labels_are_not() {
        // The apostrophe hangs only off the period key's long-press menu, yet
        // a slip against it still gets a correction anchored at that key.
        let keys = vec![
            RectKey::from_rect(Some("a".into()), vec![], 0.0, 0.0, 10.0, 10.0),
            RectKey::from_rect(Some("s".into()), vec![], 10.0, 0.0, 10.0, 10.0),
            RectKey::from_rect(
                Some(".".into()),
                vec!["'".into()],
                20.0,
                0.0,
                10.0,
                10.0,
            ),
        ];
        let layout = RectKeyLayout::new_exact(keys, &[]);
        let edits = PhysicalEdits::new(&layout);
        assert!(
            edits.letters().contains(&'\''),
            "an alternate-only label is still typed: {:?}",
            edits.letters()
        );
        let weights: std::collections::HashMap<char, f64> =
            edits.substitutions('a', Some(0)).into_iter().collect();
        assert!(
            weights.contains_key(&'\''),
            "the alternate edits like a main label, from its own key: {weights:?}"
        );

        // A label that does not prepare to exactly one character contributes
        // nothing, mains and alternates alike.
        let keys = vec![
            RectKey::from_rect(Some("ab".into()), vec![], 0.0, 0.0, 10.0, 10.0),
            RectKey::from_rect(Some("c".into()), vec!["de".into()], 10.0, 0.0, 10.0, 10.0),
        ];
        let layout = RectKeyLayout::new_exact(keys, &[]);
        let edits = PhysicalEdits::new(&layout);
        assert_eq!(edits.letters(), ['c']);
    }
}
