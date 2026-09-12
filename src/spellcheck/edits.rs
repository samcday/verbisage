//! Shared one-edit generator. Callers own matching, budgets, and ranking.

use crate::spatial::cost::{
    INSERTION_COST, INSERTION_COST_FIRST_CHAR, INSERTION_COST_SAME_CHAR, OMISSION_COST,
    OMISSION_COST_FIRST_CHAR, OMISSION_COST_SAME_CHAR, TRANSPOSITION_COST, quality_to_cost,
};

/// A source of substitution candidates for a character.
///
/// The geometry-free [`LatinAlphabet`] is the default; layout- and touch-aware
/// sources live in [`crate::spatial`] and return only nearby keys, with a
/// weight derived from the spatial distance between the intended and
/// replacement keys.
pub trait EditSource {
    /// The characters that can be inserted.
    fn letters(&self) -> &[char];

    /// Candidate replacements for `ch`, each with a multiplicative weight in
    /// `0.0..=1.0` (higher is better). The input character itself must not be
    /// returned.
    ///
    /// `position` is the original input position the character is aligned to,
    /// or `None` for a character introduced by an earlier edit. Touch-aware
    /// sources use it to anchor on the actual touch point for that position.
    fn substitutions(&self, ch: char, position: Option<usize>) -> Vec<(char, f64)>;
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

    fn substitutions(&self, ch: char, _position: Option<usize>) -> Vec<(char, f64)> {
        self.letters()
            .iter()
            .copied()
            .filter(|candidate| *candidate != ch)
            .map(|candidate| (candidate, 0.5))
            .collect()
    }
}

/// Return false from the visitor to stop immediately (for example on timeout).
pub fn visit_edits(
    word: &str,
    source: &dyn EditSource,
    mut visit: impl FnMut(String, f64) -> bool,
) {
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
        for (ch, weight) in source.substitutions(chars[i], Some(i)) {
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

/// Like [`visit_edits`], but yields HeliBoard spatial edit *costs* (lower is
/// better) and tracks which original input position each character came from.
///
/// `alignment[i]` is the input index `word[i]` is aligned to, or `None` for a
/// character introduced by an earlier edit. The visitor receives the edited
/// string, its alignment and the cost of the edit, so touch sources stay
/// anchored on the original touch coordinates across multiple edits.
pub fn visit_edits_cost(
    word: &str,
    alignment: &[Option<usize>],
    source: &dyn EditSource,
    mut visit: impl FnMut(String, Vec<Option<usize>>, f64) -> bool,
) {
    let chars: Vec<char> = word.chars().collect();
    for i in 0..chars.len().saturating_sub(1) {
        let mut c = chars.clone();
        c.swap(i, i + 1);
        let mut a = alignment.to_vec();
        a.swap(i, i + 1);
        if !visit(c.into_iter().collect(), a, TRANSPOSITION_COST) {
            return;
        }
    }
    for i in 0..chars.len() {
        // A shorter candidate means an extra input character was typed, which
        // HeliBoard calls an insertion.
        let repeated = (i > 0 && chars[i - 1] == chars[i])
            || (i + 1 < chars.len() && chars[i + 1] == chars[i]);
        let cost = if repeated {
            INSERTION_COST_SAME_CHAR
        } else if i == 0 {
            INSERTION_COST_FIRST_CHAR
        } else {
            INSERTION_COST
        };
        let mut c = chars.clone();
        c.remove(i);
        let mut a = alignment.to_vec();
        if i < a.len() {
            a.remove(i);
        }
        if !visit(c.into_iter().collect(), a, cost) {
            return;
        }
    }
    for i in 0..=chars.len() {
        // A longer candidate means the dictionary has an extra character, which
        // HeliBoard calls an omission; doubling a neighbour is cheapest. The
        // inserted character has no original touch point.
        for &ch in source.letters() {
            let repeated = (i > 0 && chars[i - 1] == ch) || (i < chars.len() && chars[i] == ch);
            let cost = if repeated {
                OMISSION_COST_SAME_CHAR
            } else if i == 0 {
                OMISSION_COST_FIRST_CHAR
            } else {
                OMISSION_COST
            };
            let mut c = chars.clone();
            c.insert(i, ch);
            let mut a = alignment.to_vec();
            a.insert(i, None);
            if !visit(c.into_iter().collect(), a, cost) {
                return;
            }
        }
    }
    for i in 0..chars.len() {
        let position = alignment.get(i).copied().flatten();
        for (ch, weight) in source.substitutions(chars[i], position) {
            if ch != chars[i] {
                // Source weights are qualities derived from spatial cost.
                let mut c = chars.clone();
                c[i] = ch;
                if !visit(
                    c.into_iter().collect(),
                    alignment.to_vec(),
                    quality_to_cost(weight),
                ) {
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
        fn substitutions(&self, ch: char, _position: Option<usize>) -> Vec<(char, f64)> {
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
}
