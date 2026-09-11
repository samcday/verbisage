//! Layout-only spatial model.
//!
//! Substitutions are limited to keys within a small radius of the typed key,
//! weighted by the normalised distance between key centres. Used for physical
//! keyboards, where there are no touch points.

use keyboard_layout::{Key, KeyboardLayout, RectKeyLayout};

use crate::spellcheck::edits::{EditSource, LatinAlphabet};

pub struct PhysicalEdits<'a> {
    layout: &'a RectKeyLayout,
    letters: Vec<char>,
    key_diameter: f64,
    radius: f64,
}

impl<'a> PhysicalEdits<'a> {
    pub fn new(layout: &'a RectKeyLayout) -> Self {
        Self {
            layout,
            letters: single_char_labels(layout),
            key_diameter: f64::from(layout.median_key_diameter().max(f32::EPSILON)),
            radius: 1.2,
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

    fn substitutions(&self, ch: char, _index: usize) -> Vec<(char, f64)> {
        let Some(origin) = self.layout.location_of(&ch.to_string()) else {
            return LatinAlphabet.substitutions(ch, 0);
        };
        let mut out = Vec::new();
        for candidate in &self.letters {
            if *candidate == ch {
                continue;
            }
            let Some(target) = self.layout.location_of(&candidate.to_string()) else {
                continue;
            };
            let normalized = f64::from(origin.distance(target)) / self.key_diameter;
            if normalized <= self.radius {
                out.push((*candidate, (1.0 - normalized).clamp(0.1, 0.9)));
            }
        }
        out
    }
}

/// Collect the single-character main labels of a layout.
pub(super) fn single_char_labels(layout: &RectKeyLayout) -> Vec<char> {
    let mut letters = Vec::new();
    for key in layout.iter() {
        let Some(label) = key.main_label() else {
            continue;
        };
        let mut chars = label.chars();
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
            edits.substitutions('s', 0).into_iter().collect();

        assert!(weights.contains_key(&'w'));
        assert!(weights.contains_key(&'d'));
        assert!(
            !weights.contains_key(&'p'),
            "a distant key must not be a substitution"
        );
        assert!(weights[&'w'] > weights[&'e']);
    }
}
