//! Shared one-edit generator. Callers own matching, budgets, and ranking.

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
}
