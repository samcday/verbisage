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

/// Collect the single-character main labels of a layout after applying the
/// caller's label preparation.
///
/// Generated edits are compared against prepared candidate spellings, so a
/// layout authored in its own spelling (an active Shift layer carries `Q`, not
/// `q`) must be prepared the same way as the input. A label that does not
/// prepare to exactly one character is skipped, which is the same rule the
/// unprepared collector applies to multi-character labels.
pub(super) fn single_char_labels_with(
    layout: &RectKeyLayout,
    prepare: &dyn Fn(&str) -> String,
) -> Vec<char> {
    let mut letters = Vec::new();
    for key in layout.iter() {
        let Some(label) = key.main_label() else {
            continue;
        };
        let prepared = prepare(label);
        let mut chars = prepared.chars();
        if let (Some(ch), None) = (chars.next(), chars.next())
            && !letters.contains(&ch)
        {
            letters.push(ch);
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
}
