use std::collections::HashSet;

/// Generate all subsequence variants of `chars` up to length `max_len`.
///
/// Variants are emitted longest-first so callers see the most specific
/// matches before shorter (less discriminative) ones.  Stops early once
/// `limit` variants have been collected.
pub fn generate_subsequence_variants(chars: &[char], max_len: usize, limit: usize) -> Vec<String> {
    let mut variants = Vec::new();
    let mut seen = HashSet::new();
    let cap = limit.max(1);

    for target_len in (1..=max_len).rev() {
        collect_subsequences(
            chars,
            target_len,
            0,
            &mut Vec::new(),
            &mut variants,
            cap,
            &mut seen,
        );
        if variants.len() >= cap {
            break;
        }
    }

    variants
}

fn collect_subsequences(
    chars: &[char],
    target_len: usize,
    start: usize,
    buffer: &mut Vec<char>,
    variants: &mut Vec<String>,
    cap: usize,
    seen: &mut HashSet<String>,
) {
    if variants.len() >= cap {
        return;
    }
    if buffer.len() == target_len {
        let candidate: String = buffer.iter().collect();
        if seen.insert(candidate.clone()) {
            variants.push(candidate);
        }
        return;
    }

    let remaining = target_len - buffer.len();
    for idx in start..chars.len() {
        if chars.len() - idx < remaining {
            break;
        }
        buffer.push(chars[idx]);
        collect_subsequences(chars, target_len, idx + 1, buffer, variants, cap, seen);
        buffer.pop();
        if variants.len() >= cap {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_subsequence_generation() {
        let chars: Vec<char> = "hlo".chars().collect();
        let variants = generate_subsequence_variants(&chars, 3, 10);
        assert!(variants.contains(&"hlo".to_string()));
        assert!(variants.contains(&"hl".to_string()));
    }

    #[test]
    fn respects_limit() {
        let chars: Vec<char> = "abcde".chars().collect();
        let variants = generate_subsequence_variants(&chars, 5, 3);
        assert!(variants.len() <= 3);
    }
}
