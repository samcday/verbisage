//! Touch-point spatial model.
//!
//! Substitutions are anchored on the actual touch point for the input position
//! (normalised into layout space) rather than the key centre of the presumed
//! typed character, which is what HeliBoard's proximity traversal does.

use keyboard_layout::{KeyboardLayout, Point, RectKeyLayout};

use super::TouchPoint;
use super::physical::single_char_labels;
use crate::spellcheck::edits::{EditSource, LatinAlphabet};

pub struct TouchEdits<'a> {
    layout: &'a RectKeyLayout,
    points: &'a [TouchPoint],
    letters: Vec<char>,
    key_diameter: f64,
    radius: f64,
}

impl<'a> TouchEdits<'a> {
    pub fn new(layout: &'a RectKeyLayout, points: &'a [TouchPoint]) -> Self {
        Self {
            layout,
            points,
            letters: single_char_labels(layout),
            key_diameter: f64::from(layout.median_key_diameter().max(f32::EPSILON)),
            radius: 1.2,
        }
    }

    /// The normalised anchor for input position `index`: the touch point when
    /// one is available, otherwise the presumed typed key's centre.
    fn anchor(&self, ch: char, index: usize) -> Option<Point> {
        self.points
            .get(index)
            .map(|point| self.layout.normalise(Point::new(point.x, point.y)))
            .or_else(|| self.layout.location_of(&ch.to_string()))
    }
}

impl EditSource for TouchEdits<'_> {
    fn letters(&self) -> &[char] {
        &self.letters
    }

    fn substitutions(&self, ch: char, index: usize) -> Vec<(char, f64)> {
        let Some(origin) = self.anchor(ch, index) else {
            return LatinAlphabet.substitutions(ch, index);
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

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard_layout::RectKey;

    fn layout() -> RectKeyLayout {
        RectKeyLayout::new(
            vec![
                RectKey::from_rect(Some("a".into()), vec![], 0.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("b".into()), vec![], 10.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("c".into()), vec![], 20.0, 0.0, 10.0, 10.0),
            ],
            &[],
        )
    }

    #[test]
    fn substitutions_are_anchored_on_the_touch_point() {
        let layout = layout();
        // Touch at the centre of `b` while `a` was typed.
        let points = [TouchPoint::new(15.0, 5.0)];
        let edits = TouchEdits::new(&layout, &points);
        let weights: std::collections::HashMap<char, f64> =
            edits.substitutions('a', 0).into_iter().collect();

        assert!(
            weights[&'b'] > weights[&'c'],
            "the key under the touch should weigh most: {weights:?}"
        );
    }

    #[test]
    fn falls_back_to_the_typed_key_without_a_point() {
        let layout = layout();
        let edits = TouchEdits::new(&layout, &[]);
        let weights: std::collections::HashMap<char, f64> =
            edits.substitutions('a', 0).into_iter().collect();
        assert!(weights.contains_key(&'b'));
    }
}
