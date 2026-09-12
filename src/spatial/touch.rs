//! Touch-point spatial model.
//!
//! Substitutions are anchored on the actual touch point for the input position
//! (normalised into layout space) rather than the key centre of the presumed
//! typed character, which is what HeliBoard's proximity traversal does.

use std::collections::HashMap;

use keyboard_layout::{KeyboardLayout, Point, RectKeyLayout};

use super::TouchPoint;
use super::cost::{
    ADDITIONAL_PROXIMITY_COST, MAX_SPATIAL_DISTANCE, PROXIMITY_COST, SEARCH_DISTANCE,
    SUBSTITUTION_COST, additional_proximity, cost_to_quality, key_distance_cost,
};
use super::physical::single_char_labels;
use crate::spellcheck::edits::{EditSource, LatinAlphabet};

pub struct TouchEdits<'a> {
    layout: &'a RectKeyLayout,
    points: &'a [TouchPoint],
    letters: Vec<char>,
    key_diameter: f64,
}

impl<'a> TouchEdits<'a> {
    pub fn new(layout: &'a RectKeyLayout, points: &'a [TouchPoint]) -> Self {
        Self {
            layout,
            points,
            letters: single_char_labels(layout),
            key_diameter: f64::from(layout.median_key_diameter().max(f32::EPSILON)),
        }
    }

    /// The normalised anchor for a character: the touch point of the original
    /// input position it is aligned to when available, otherwise the presumed
    /// typed key's centre.
    fn anchor(&self, ch: char, position: Option<usize>) -> Option<Point> {
        position
            .and_then(|index| self.points.get(index))
            .map(|point| self.layout.normalise(Point::new(point.x, point.y)))
            .or_else(|| self.layout.location_of(&ch.to_string()))
    }
}

impl EditSource for TouchEdits<'_> {
    fn letters(&self) -> &[char] {
        &self.letters
    }

    fn substitutions(&self, ch: char, position: Option<usize>) -> Vec<(char, f64)> {
        let Some(origin) = self.anchor(ch, position) else {
            return LatinAlphabet.substitutions(ch, position);
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
            edits.substitutions('a', Some(0)).into_iter().collect();

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
            edits.substitutions('a', Some(0)).into_iter().collect();
        assert!(weights.contains_key(&'b'));
    }

    #[test]
    fn substitutions_follow_the_aligned_position_not_the_string_index() {
        let layout = layout();
        // Touches were recorded for the original input: position 0 near `a`,
        // position 1 near `c`.
        let points = [TouchPoint::new(5.0, 5.0), TouchPoint::new(25.0, 5.0)];
        let edits = TouchEdits::new(&layout, &points);

        let near_a: std::collections::HashMap<char, f64> =
            edits.substitutions('b', Some(0)).into_iter().collect();
        let near_c: std::collections::HashMap<char, f64> =
            edits.substitutions('b', Some(1)).into_iter().collect();
        assert!(
            near_a[&'a'] > near_a[&'c'],
            "position 0 should anchor near `a`: {near_a:?}"
        );
        assert!(
            near_c[&'c'] > near_c[&'a'],
            "position 1 should anchor near `c`: {near_c:?}"
        );

        // An inserted character has no original touch point and falls back to
        // the presumed key centre, so it anchors on `b`'s centre rather than
        // the touch recorded for position 0.
        let inserted: std::collections::HashMap<char, f64> =
            edits.substitutions('b', None).into_iter().collect();
        assert!(
            near_a[&'a'] > inserted[&'a'] && near_a[&'c'] < inserted[&'c'],
            "`None` should fall back to the key centre: \
             touched={near_a:?} inserted={inserted:?}"
        );
    }
}
