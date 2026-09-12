//! HeliBoard-derived scoring constants and shared cost/score helpers.
//!
//! This is the single source of truth for the spatial + language cost model
//! shared by prediction (n-gram ranking), completion
//! ([`AndroidCompleter`](crate::completion::AndroidCompleter)) and correction
//! ([`DictionarySpellChecker`](crate::spellcheck::DictionarySpellChecker)).
//!
//! Values mirror HeliBoard's `ScoringParams`; edit costs are additive spatial
//! units, and language cost is applied once per candidate as an improbability
//! weighted by [`DISTANCE_WEIGHT_LANGUAGE`].

/// Typed/spatial edit costs (HeliBoard `ScoringParams`).
pub const DISTANCE_WEIGHT_LENGTH: f64 = 0.1524;
pub const DISTANCE_WEIGHT_LANGUAGE: f64 = 1.1214;
pub const TYPING_MAX_OUTPUT_SCORE_PER_INPUT: f64 = 0.1;

/// A point-to-key length saturates here (in most-common-key-width units).
pub const MAX_SPATIAL_DISTANCE: f64 = 1.0;
/// Keys within this many key widths are "proximity" (near) keys.
pub const SEARCH_DISTANCE: f64 = 1.2;
/// Stop exploring error corrections once accumulated spatial cost per input
/// character exceeds this.
pub const NORMALIZED_SPATIAL_DISTANCE_THRESHOLD_FOR_EDIT: f64 = 0.095;

pub const PROXIMITY_COST: f64 = 0.0694;
pub const FIRST_CHAR_PROXIMITY_COST: f64 = 0.072;
pub const FIRST_PROXIMITY_COST: f64 = 0.07788;
pub const SUBSTITUTION_COST: f64 = 0.3806;
pub const ADDITIONAL_PROXIMITY_COST: f64 = 0.37972;
pub const OMISSION_COST: f64 = 0.467;
pub const OMISSION_COST_SAME_CHAR: f64 = 0.345;
pub const OMISSION_COST_FIRST_CHAR: f64 = 0.5256;
pub const INTENTIONAL_OMISSION_COST: f64 = 0.1;
pub const INSERTION_COST: f64 = 0.7248;
pub const INSERTION_COST_SAME_CHAR: f64 = 0.5508;
pub const INSERTION_COST_PROXIMITY_CHAR: f64 = 0.674;
pub const INSERTION_COST_FIRST_CHAR: f64 = 0.639;
pub const TRANSPOSITION_COST: f64 = 0.5608;
pub const TERMINAL_INSERTION_COST: f64 = 0.8128;
pub const COST_FIRST_COMPLETION: f64 = 0.4836;
pub const COST_COMPLETION: f64 = 0.00624;
pub const EXACT_MATCH_PROMOTION: f64 = 1.1;

/// Largest edit cost used when folding a cost into a `0.0..=1.0` quality.
pub const MAX_EDIT_COST: f64 = INSERTION_COST + DISTANCE_WEIGHT_LENGTH * 1.2;

/// HeliBoard's sweet-spot factor for a normalized squared key distance.
///
/// Gives a dead zone close to the key centre so small touch errors are free,
/// then ramps the factor up to `1.2` for distant keys.
pub fn sweet_spot_factor(normalized_squared_distance: f64) -> f64 {
    const A: f64 = 0.0;
    const B: f64 = 0.24;
    const C: f64 = 1.20;
    const R0: f64 = 0.0;
    const R1: f64 = 0.25;
    const R2: f64 = 1.0;
    let x = normalized_squared_distance;
    if x < R0 {
        A
    } else if x < R1 {
        (A * (R1 - x) + B * (x - R0)) / (R1 - R0)
    } else if x < R2 {
        (B * (R2 - x) + C * (x - R1)) / (R2 - R1)
    } else {
        C
    }
}

/// Spatial cost contribution of a key distance, in HeliBoard cost units.
pub fn key_distance_cost(normalized_squared_distance: f64) -> f64 {
    DISTANCE_WEIGHT_LENGTH
        * sweet_spot_factor(normalized_squared_distance.min(MAX_SPATIAL_DISTANCE))
}

/// Fold an edit cost into a `0.0..=1.0` quality where higher is better.
pub const fn cost_to_quality(cost: f64) -> f64 {
    let quality = 1.0 - cost / MAX_EDIT_COST;
    if quality < 0.0 {
        0.0
    } else if quality > 1.0 {
        1.0
    } else {
        quality
    }
}

/// Invert [`cost_to_quality`].
pub fn quality_to_cost(quality: f64) -> f64 {
    (1.0 - quality.clamp(0.0, 1.0)) * MAX_EDIT_COST
}

/// HeliBoard's hard-coded en_US additional proximity characters: each vowel is
/// also considered "near" the other vowels, independently of key geometry.
pub fn additional_proximity(ch: char) -> &'static [char] {
    match ch {
        'a' => &['e', 'i', 'o', 'u'],
        'e' => &['a', 'i', 'o', 'u'],
        'i' => &['a', 'e', 'o', 'u'],
        'o' => &['a', 'e', 'i', 'u'],
        'u' => &['a', 'e', 'i', 'o'],
        _ => &[],
    }
}

/// HeliBoard's final additive score: one accumulated spatial cost plus the
/// language-model improbability, normalised by a budget that grows with input
/// length. Higher is better, clamped to `0.0..=1.0`.
pub fn combined_score(spatial_cost: f64, language_improbability: f64, input_len: usize) -> f64 {
    let max_distance =
        DISTANCE_WEIGHT_LANGUAGE + input_len as f64 * TYPING_MAX_OUTPUT_SCORE_PER_INPUT;
    (1.0 - (spatial_cost + language_improbability * DISTANCE_WEIGHT_LANGUAGE) / max_distance)
        .clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweet_spot_has_a_dead_zone_and_saturates() {
        assert_eq!(sweet_spot_factor(0.0), 0.0);
        // At the sweet spot the factor is B.
        assert!((sweet_spot_factor(0.25) - 0.24).abs() < 1e-9);
        assert_eq!(sweet_spot_factor(10.0), 1.2);
    }

    #[test]
    fn near_keys_cost_less_than_far_keys() {
        let near = PROXIMITY_COST + key_distance_cost(0.05);
        let far = SUBSTITUTION_COST + key_distance_cost(0.9);
        assert!(near < far, "near={near} far={far}");
        assert!(cost_to_quality(near) > cost_to_quality(far));
    }

    #[test]
    fn combined_score_prefers_likely_words() {
        let likely = combined_score(0.1, 0.0, 5);
        let unlikely = combined_score(0.1, 1.0, 5);
        assert!(likely > unlikely);
    }
}
