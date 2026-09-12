//! Shared spatial model for layout- and touch-aware ranking.
//!
//! [`SpatialInput`] is the single runtime-selectable type consumed by both the
//! completion engines and the spellchecker. It is built at the transport edge
//! from a registered layout token and the optional per-character touch points:
//! none -> [`SpatialInput::None`], token -> [`SpatialInput::Layout`], token +
//! points -> [`SpatialInput::Touch`].

pub mod cost;
pub mod physical;
pub mod touch;

pub use cost::{
    DISTANCE_WEIGHT_LANGUAGE, DISTANCE_WEIGHT_LENGTH,
    NORMALIZED_SPATIAL_DISTANCE_THRESHOLD_FOR_EDIT, TYPING_MAX_OUTPUT_SCORE_PER_INPUT,
};
pub use physical::PhysicalEdits;
pub use touch::TouchEdits;

use std::sync::Arc;

use keyboard_layout::{KeyboardLayout, Point, RectKeyLayout};

use crate::spellcheck::edits::{EditSource, LatinAlphabet};

/// One touch point for a typed character, in the caller's coordinate system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPoint {
    pub x: f32,
    pub y: f32,
}

impl TouchPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// The spatial context for a request.
#[derive(Debug, Clone, Default)]
pub enum SpatialInput {
    #[default]
    None,
    Layout(Arc<RectKeyLayout>),
    Touch {
        layout: Arc<RectKeyLayout>,
        points: Vec<TouchPoint>,
    },
}

impl SpatialInput {
    /// Build from the transport's (layout token, points) pair. Points without a
    /// layout are ignored.
    pub fn from_parts(layout: Option<Arc<RectKeyLayout>>, points: Vec<TouchPoint>) -> Self {
        match layout {
            None => Self::None,
            Some(layout) if points.is_empty() => Self::Layout(layout),
            Some(layout) => Self::Touch { layout, points },
        }
    }

    pub fn layout(&self) -> Option<&Arc<RectKeyLayout>> {
        match self {
            Self::None => None,
            Self::Layout(layout) | Self::Touch { layout, .. } => Some(layout),
        }
    }

    pub fn points(&self) -> Option<&[TouchPoint]> {
        match self {
            Self::Touch { points, .. } => Some(points),
            _ => None,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// The edit source for this context.
    pub fn edit_source(&self) -> EditSources<'_> {
        match self {
            Self::None => EditSources::Latin(LatinAlphabet),
            Self::Layout(layout) => EditSources::Physical(PhysicalEdits::new(layout)),
            Self::Touch { layout, points } => EditSources::Touch(TouchEdits::new(layout, points)),
        }
    }

    fn key_diameter(layout: &RectKeyLayout) -> f64 {
        f64::from(layout.median_key_diameter().max(f32::EPSILON))
    }

    /// Normalised distance from the input position's anchor to `ch`'s key
    /// centre. Only meaningful for [`SpatialInput::Touch`].
    pub fn char_distance(&self, index: usize, ch: char) -> Option<f64> {
        let layout = self.layout()?;
        let point = self.points()?.get(index)?;
        let origin = layout.normalise(Point::new(point.x, point.y));
        let target = layout.location_of(&ch.to_string())?;
        Some(f64::from(origin.distance(target)) / Self::key_diameter(layout))
    }

    /// Mean normalised distance from the touch points to the key centres of the
    /// typed input characters (the touch-accuracy gate metric).
    pub fn input_distance(&self, input: &str) -> Option<f64> {
        let layout = self.layout()?;
        let points = self.points()?;
        let diameter = Self::key_diameter(layout);
        let chars: Vec<char> = input.chars().collect();
        if chars.is_empty() {
            return None;
        }
        let mut total = 0.0;
        let mut counted = 0usize;
        for (index, ch) in chars.iter().enumerate() {
            let Some(point) = points.get(index) else {
                continue;
            };
            let Some(target) = layout.location_of(&ch.to_string()) else {
                continue;
            };
            let origin = layout.normalise(Point::new(point.x, point.y));
            total += f64::from(origin.distance(target)) / diameter;
            counted += 1;
        }
        (counted > 0).then(|| total / counted as f64)
    }

    /// Mean normalised spatial distance between the input and a candidate,
    /// aligned by position, or `None` when there is no spatial context.
    pub fn word_distance(&self, input: &str, candidate: &str) -> Option<f64> {
        let layout = self.layout()?;
        let key_diameter = Self::key_diameter(layout);
        let input_chars: Vec<char> = input.chars().collect();
        let candidate_chars: Vec<char> = candidate.chars().collect();
        let aligned = input_chars.len().min(candidate_chars.len());
        if aligned == 0 {
            return None;
        }

        let mut total = 0.0;
        for index in 0..aligned {
            let target = layout.location_of(&candidate_chars[index].to_string());
            let origin = match self {
                Self::Touch { points, .. } => points
                    .get(index)
                    .map(|point| layout.normalise(Point::new(point.x, point.y))),
                Self::Layout(_) => layout.location_of(&input_chars[index].to_string()),
                Self::None => None,
            };
            match (origin, target) {
                (Some(origin), Some(target)) => {
                    total += f64::from(origin.distance(target)) / key_diameter;
                }
                _ => total += 1.0,
            }
        }

        total += input_chars.len().abs_diff(candidate_chars.len()) as f64;
        let denominator = input_chars.len().max(candidate_chars.len()).max(1) as f64;
        Some((total / denominator).min(1.0))
    }
}

/// Ergonomic [`EditSource`] over the selected spatial model.
pub enum EditSources<'a> {
    Latin(LatinAlphabet),
    Physical(PhysicalEdits<'a>),
    Touch(TouchEdits<'a>),
}

impl EditSource for EditSources<'_> {
    fn letters(&self) -> &[char] {
        match self {
            Self::Latin(source) => source.letters(),
            Self::Physical(source) => source.letters(),
            Self::Touch(source) => source.letters(),
        }
    }

    fn substitutions(&self, ch: char, position: Option<usize>) -> Vec<(char, f64)> {
        match self {
            Self::Latin(source) => source.substitutions(ch, position),
            Self::Physical(source) => source.substitutions(ch, position),
            Self::Touch(source) => source.substitutions(ch, position),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard_layout::RectKey;

    fn layout() -> Arc<RectKeyLayout> {
        Arc::new(RectKeyLayout::new(
            vec![
                RectKey::from_rect(Some("a".into()), vec![], 0.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("b".into()), vec![], 10.0, 0.0, 10.0, 10.0),
                RectKey::from_rect(Some("c".into()), vec![], 20.0, 0.0, 10.0, 10.0),
            ],
            &[],
        ))
    }

    #[test]
    fn selects_the_model_from_the_available_data() {
        assert!(SpatialInput::from_parts(None, vec![]).is_none());
        assert!(matches!(
            SpatialInput::from_parts(Some(layout()), vec![]),
            SpatialInput::Layout(_)
        ));
        assert!(matches!(
            SpatialInput::from_parts(Some(layout()), vec![TouchPoint::new(15.0, 5.0)]),
            SpatialInput::Touch { .. }
        ));
        // Points without a layout degrade to None.
        assert!(SpatialInput::from_parts(None, vec![TouchPoint::new(15.0, 5.0)]).is_none());
    }

    #[test]
    fn touch_distance_is_smaller_near_the_touched_key() {
        let spatial = SpatialInput::from_parts(Some(layout()), vec![TouchPoint::new(15.0, 5.0)]);
        let near = spatial.char_distance(0, 'b').unwrap();
        let far = spatial.char_distance(0, 'a').unwrap();
        assert!(near < far, "near={near} far={far}");
    }

    #[test]
    fn word_distance_compares_touch_to_candidate_letters() {
        let spatial = SpatialInput::from_parts(Some(layout()), vec![TouchPoint::new(15.0, 5.0)]);
        let near = spatial.word_distance("a", "b").unwrap();
        let far = spatial.word_distance("a", "c").unwrap();
        assert!(near < far, "near={near} far={far}");
    }
}
