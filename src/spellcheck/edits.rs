//! Shared one-edit generator. Callers own matching, budgets, and ranking.

use keyboard_layout::{Key, KeyboardLayout, RectKey, RectKeyLayout};

/// A source of substitution candidates for a character.
///
/// The geometry-free [`LatinAlphabet`] is the default. Layout-aware sources
/// (added later) return only nearby keys, with a weight derived from the
/// spatial distance between the intended and replacement keys.
pub trait EditSource {
    /// The characters that can be inserted.
    fn letters(&self) -> &[char];

    /// Candidate replacements for `ch`, each with a multiplicative weight in
    /// `0.0..=1.0` (higher is better). The input character itself must not be
    /// returned.
    fn substitutions(&self, ch: char) -> Vec<(char, f64)>;
}

pub struct LatinAlphabet;

const LATIN: [char; 26] = [
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z',
];

impl EditSource for LatinAlphabet {
    fn letters(&self) -> &[char] {
        &LATIN
    }

    fn substitutions(&self, ch: char) -> Vec<(char, f64)> {
        self.letters()
            .iter()
            .copied()
            .filter(|candidate| *candidate != ch)
            .map(|candidate| (candidate, 0.5))
            .collect()
    }
}

/// Layout-aware [`EditSource`]: substitutions are limited to keys within a
/// small radius of the typed key, weighted by normalised key distance.
pub struct LayoutEdits<'a> {
    layout: &'a RectKeyLayout,
    letters: Vec<char>,
    key_diameter: f64,
    radius: f64,
}

impl<'a> LayoutEdits<'a> {
    pub fn new(layout: &'a RectKeyLayout) -> Self {
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
        Self {
            layout,
            letters,
            key_diameter: f64::from(layout.median_key_diameter().max(f32::EPSILON)),
            radius: 1.2,
        }
    }
}

impl EditSource for LayoutEdits<'_> {
    fn letters(&self) -> &[char] {
        &self.letters
    }

    fn substitutions(&self, ch: char) -> Vec<(char, f64)> {
        let Some(origin) = self.layout.location_of(&ch.to_string()) else {
            return LatinAlphabet.substitutions(ch);
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

/// Return false from the visitor to stop immediately (for example on timeout).
pub fn visit_edits(word: &str, source: &dyn EditSource, mut visit: impl FnMut(String, f64) -> bool) {
    let chars: Vec<char> = word.chars().collect();
    for i in 0..chars.len().saturating_sub(1) {
        let mut c = chars.clone();
        c.swap(i, i + 1);
        if !visit(c.into_iter().collect(), 0.9) {
            return;
        }
    }
    for i in 0..chars.len() {
        let repeated = (i > 0 && chars[i - 1] == chars[i])
            || (i + 1 < chars.len() && chars[i + 1] == chars[i]);
        let mut c = chars.clone();
        c.remove(i);
        if !visit(c.into_iter().collect(), if repeated { 0.9 } else { 0.65 }) {
            return;
        }
    }
    for i in 0..=chars.len() {
        for &ch in source.letters() {
            let repeated = (i > 0 && chars[i - 1] == ch) || (i < chars.len() && chars[i] == ch);
            let mut c = chars.clone();
            c.insert(i, ch);
            if !visit(c.into_iter().collect(), if repeated { 0.9 } else { 0.65 }) {
                return;
            }
        }
    }
    for i in 0..chars.len() {
        for (ch, weight) in source.substitutions(chars[i]) {
            if ch != chars[i] {
                let mut c = chars.clone();
                c[i] = ch;
                if !visit(c.into_iter().collect(), weight) {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Extra;
    impl EditSource for Extra {
        fn letters(&self) -> &[char] {
            &['ä', 'e']
        }
        fn substitutions(&self, ch: char) -> Vec<(char, f64)> {
            self.letters()
                .iter()
                .copied()
                .filter(|candidate| *candidate != ch)
                .map(|candidate| (candidate, 0.5))
                .collect()
        }
    }

    #[test]
    fn preserves_edit_classes_and_accepts_non_latin_alphabets() {
        let mut rows = Vec::new();
        visit_edits("ha", &Extra, |w, cost| {
            rows.push((w, cost));
            true
        });
        assert!(rows.contains(&("ah".into(), 0.9)));
        assert!(rows.contains(&("hä".into(), 0.5)));
        let mut count = 0;
        visit_edits("example", &LatinAlphabet, |_, _| {
            count += 1;
            false
        });
        assert_eq!(count, 1);
    }

    #[test]
    fn layout_edits_are_proximity_weighted() {
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
        let edits = LayoutEdits::new(&layout);
        let weights: std::collections::HashMap<char, f64> =
            edits.substitutions('s').into_iter().collect();

        assert!(weights.contains_key(&'w'));
        assert!(weights.contains_key(&'d'));
        assert!(
            !weights.contains_key(&'p'),
            "a distant key must not be a substitution"
        );
        assert!(
            weights[&'w'] > weights[&'e'],
            "a nearer key must weigh more: {weights:?}"
        );
    }
}
