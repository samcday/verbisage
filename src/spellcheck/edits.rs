//! Shared one-edit generator. Callers own matching, budgets, and ranking.

/// A future layout can supply its alphabet without another edit generator.
pub trait Alphabet {
    fn letters(&self) -> &[char];
}

pub struct LatinAlphabet;
impl Alphabet for LatinAlphabet {
    fn letters(&self) -> &[char] {
        &[
            'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q',
            'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
        ]
    }
}

/// Return false from the visitor to stop immediately (for example on timeout).
pub fn visit_edits(
    word: &str,
    alphabet: &dyn Alphabet,
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
        for &ch in alphabet.letters() {
            let repeated = (i > 0 && chars[i - 1] == ch) || (i < chars.len() && chars[i] == ch);
            let mut c = chars.clone();
            c.insert(i, ch);
            if !visit(c.into_iter().collect(), if repeated { 0.9 } else { 0.65 }) {
                return;
            }
        }
    }
    for i in 0..chars.len() {
        for &ch in alphabet.letters() {
            if ch != chars[i] {
                let mut c = chars.clone();
                c[i] = ch;
                if !visit(c.into_iter().collect(), 0.5) {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_edit_classes_and_accepts_non_latin_alphabets() {
        struct Extra;
        impl Alphabet for Extra {
            fn letters(&self) -> &[char] {
                &['ä', 'e']
            }
        }
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
