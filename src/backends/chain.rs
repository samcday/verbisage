use std::collections::{HashMap, HashSet};

use super::{Capability, ResolvedBackendDef};

// ---------------------------------------------------------------------------
// Chain parsing
// ---------------------------------------------------------------------------

/// Split a chain string on `+` into segment names.
/// Returns an error on empty segments (e.g. `"A+"` or `"+B"`).
pub fn parse_chain(s: &str) -> Result<Vec<String>, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("empty backend chain string".to_string());
    }
    let segments: Vec<&str> = trimmed.split('+').map(|s| s.trim()).collect();

    // Reject empty segments
    for seg in &segments {
        if seg.is_empty() {
            return Err(format!(
                "invalid backend chain '{}': empty segment (double '+' or leading/trailing '+')",
                s
            ));
        }
    }

    Ok(segments.into_iter().map(|s| s.to_string()).collect())
}

// ---------------------------------------------------------------------------
// Role assignment
// ---------------------------------------------------------------------------

/// The role assigned to a single segment in the chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SegmentRole {
    /// Backend provides dictionary membership.
    Dictionary,
    /// Backend provides unigram frequencies.
    Unigrams,
    /// Backend provides n-gram predictions.
    Ngrams,
}

/// Full role assignment for a chain.
#[derive(Debug, Clone)]
pub struct RoleAssignment {
    /// Segment name → its resolved def + assigned role
    pub segments: Vec<SegWithRole>,
    /// All resolved definitions referred to by the chain
    pub defs: HashMap<String, ResolvedBackendDef>,
}

#[derive(Debug, Clone)]
pub struct SegWithRole {
    pub name: String,
    pub role: SegmentRole,
    pub def: ResolvedBackendDef,
}

// ---------------------------------------------------------------------------
// Validation warnings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ChainWarning {
    /// A backend that should provide dict (or is assumed to) does not.
    MissingCapability {
        name: String,
        role: SegmentRole,
        needed: Capability,
    },
    /// A segment has capabilities beyond its assigned role that will be ignored.
    ExtraCapability {
        name: String,
        role: SegmentRole,
        extra: HashSet<Capability>,
    },
    /// Single segment with frequencies but no dict.
    FrequenciesWithoutDict(String),
    /// Redundant segment that provides nothing useful for its role.
    RedundantSegment(String, SegmentRole),
}

impl std::fmt::Display for ChainWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCapability { name, role, needed } => {
                write!(
                    f,
                    "backend '{}' assigned role {:?} but lacks capability {:?}",
                    name, role, needed
                )
            }
            Self::ExtraCapability { name, role, extra } => {
                let caps: Vec<String> = extra.iter().map(|c| format!("{:?}", c)).collect();
                write!(
                    f,
                    "backend '{}' has extra capabilities {:?} that will be ignored for role {:?}",
                    name, caps, role
                )
            }
            Self::FrequenciesWithoutDict(name) => {
                write!(
                    f,
                    "backend '{}' has frequency data but no dictionary membership",
                    name
                )
            }
            Self::RedundantSegment(name, role) => {
                write!(
                    f,
                    "backend '{}' provides no useful capabilities for role {:?}",
                    name, role
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

/// Parse a chain string and assign roles, returning resolved definitions,
/// warnings, and errors.
pub fn assign_roles(
    chain: &str,
    defs: &HashMap<String, ResolvedBackendDef>,
) -> Result<(RoleAssignment, Vec<ChainWarning>), String> {
    let names = parse_chain(chain)?;
    let mut warnings: Vec<ChainWarning> = Vec::new();
    let mut segments: Vec<SegWithRole> = Vec::new();

    match names.len() {
        1 => assign_single(&names[0], defs, &mut warnings, &mut segments)?,
        2 => assign_two(&names, defs, &mut warnings, &mut segments)?,
        3 => assign_three(&names, defs, &mut warnings, &mut segments)?,
        n => {
            return Err(format!("backend chain must have 1–3 segments, got {}", n));
        }
    }

    Ok((
        RoleAssignment {
            segments,
            defs: defs.clone(),
        },
        warnings,
    ))
}

fn lookup_def<'a>(
    name: &str,
    defs: &'a HashMap<String, ResolvedBackendDef>,
    _warnings: &mut Vec<ChainWarning>,
) -> Result<&'a ResolvedBackendDef, String> {
    defs.get(name).ok_or_else(|| {
        format!(
            "backend '{}' referenced in chain but not defined in [backends]",
            name
        )
    })
}

fn assign_single(
    name: &str,
    defs: &HashMap<String, ResolvedBackendDef>,
    warnings: &mut Vec<ChainWarning>,
    segments: &mut Vec<SegWithRole>,
) -> Result<(), String> {
    let def = lookup_def(name, defs, warnings)?;

    // Single segment inherits all capabilities
    let has_dict = def.capabilities.contains(&Capability::Dictionary);
    let has_freq = def.capabilities.contains(&Capability::Unigrams);

    if def.capabilities.is_empty() {
        return Err(format!("backend '{}' resolves to empty capabilities", name));
    }

    // Warn: frequencies without dict
    if has_freq && !has_dict {
        warnings.push(ChainWarning::FrequenciesWithoutDict(name.to_string()));
    }

    // For single segment, assign dict role (covers all capabilities)
    segments.push(SegWithRole {
        name: name.to_string(),
        role: SegmentRole::Dictionary,
        def: def.clone(),
    });

    Ok(())
}

fn assign_two(
    names: &[String],
    defs: &HashMap<String, ResolvedBackendDef>,
    warnings: &mut Vec<ChainWarning>,
    segments: &mut Vec<SegWithRole>,
) -> Result<(), String> {
    let seg0_def = lookup_def(&names[0], defs, warnings)?;
    let seg1_def = lookup_def(&names[1], defs, warnings)?;

    // seg0 = dictionary role
    if !seg0_def.capabilities.contains(&Capability::Dictionary) {
        warnings.push(ChainWarning::MissingCapability {
            name: names[0].clone(),
            role: SegmentRole::Dictionary,
            needed: Capability::Dictionary,
        });
    }
    let extra0: HashSet<Capability> = seg0_def
        .capabilities
        .difference(&HashSet::from([Capability::Dictionary]))
        .copied()
        .collect();
    if !extra0.is_empty() {
        warnings.push(ChainWarning::ExtraCapability {
            name: names[0].clone(),
            role: SegmentRole::Dictionary,
            extra: extra0,
        });
    }

    segments.push(SegWithRole {
        name: names[0].clone(),
        role: SegmentRole::Dictionary,
        def: seg0_def.clone(),
    });

    // seg1 = unigrams/ngrams (whichever it provides)
    let has_uni = seg1_def.capabilities.contains(&Capability::Unigrams);
    let has_ngram = seg1_def.capabilities.contains(&Capability::Ngrams);
    let has_dict = seg1_def.capabilities.contains(&Capability::Dictionary);

    if !has_uni && !has_ngram {
        warnings.push(ChainWarning::RedundantSegment(
            names[1].clone(),
            SegmentRole::Unigrams,
        ));
    }
    if has_dict && !has_uni && !has_ngram {
        warnings.push(ChainWarning::RedundantSegment(
            names[1].clone(),
            SegmentRole::Unigrams,
        ));
    } else if has_dict {
        // seg1 has dict — warn it's ignored, assign to unigrams
        warnings.push(ChainWarning::ExtraCapability {
            name: names[1].clone(),
            role: SegmentRole::Unigrams,
            extra: HashSet::from([Capability::Dictionary]),
        });
    }

    // Pick role: prefer unigrams if available, else ngrams
    let role = if has_uni {
        SegmentRole::Unigrams
    } else if has_ngram {
        SegmentRole::Ngrams
    } else {
        SegmentRole::Unigrams
    };

    segments.push(SegWithRole {
        name: names[1].clone(),
        role,
        def: seg1_def.clone(),
    });

    Ok(())
}

fn assign_three(
    names: &[String],
    defs: &HashMap<String, ResolvedBackendDef>,
    warnings: &mut Vec<ChainWarning>,
    segments: &mut Vec<SegWithRole>,
) -> Result<(), String> {
    let seg0_def = lookup_def(&names[0], defs, warnings)?;
    let seg1_def = lookup_def(&names[1], defs, warnings)?;
    let seg2_def = lookup_def(&names[2], defs, warnings)?;

    // seg0 = dictionary
    if !seg0_def.capabilities.contains(&Capability::Dictionary) {
        warnings.push(ChainWarning::MissingCapability {
            name: names[0].clone(),
            role: SegmentRole::Dictionary,
            needed: Capability::Dictionary,
        });
    }
    let extra0: HashSet<Capability> = seg0_def
        .capabilities
        .difference(&HashSet::from([Capability::Dictionary]))
        .copied()
        .collect();
    if !extra0.is_empty() {
        // For 3-seg, seg0 extra caps beyond dict are just ignored/warned
        warnings.push(ChainWarning::ExtraCapability {
            name: names[0].clone(),
            role: SegmentRole::Dictionary,
            extra: extra0,
        });
    }

    segments.push(SegWithRole {
        name: names[0].clone(),
        role: SegmentRole::Dictionary,
        def: seg0_def.clone(),
    });

    // seg1 = unigrams
    if !seg1_def.capabilities.contains(&Capability::Unigrams) {
        warnings.push(ChainWarning::MissingCapability {
            name: names[1].clone(),
            role: SegmentRole::Unigrams,
            needed: Capability::Unigrams,
        });
    }
    let extra1: HashSet<Capability> = seg1_def
        .capabilities
        .difference(&HashSet::from([Capability::Unigrams]))
        .copied()
        .collect();
    if !extra1.is_empty() {
        warnings.push(ChainWarning::ExtraCapability {
            name: names[1].clone(),
            role: SegmentRole::Unigrams,
            extra: extra1,
        });
    }

    segments.push(SegWithRole {
        name: names[1].clone(),
        role: SegmentRole::Unigrams,
        def: seg1_def.clone(),
    });

    // seg2 = ngrams
    if !seg2_def.capabilities.contains(&Capability::Ngrams) {
        warnings.push(ChainWarning::MissingCapability {
            name: names[2].clone(),
            role: SegmentRole::Ngrams,
            needed: Capability::Ngrams,
        });
    }
    let extra2: HashSet<Capability> = seg2_def
        .capabilities
        .difference(&HashSet::from([Capability::Ngrams]))
        .copied()
        .collect();
    if !extra2.is_empty() {
        warnings.push(ChainWarning::ExtraCapability {
            name: names[2].clone(),
            role: SegmentRole::Ngrams,
            extra: extra2,
        });
    }

    segments.push(SegWithRole {
        name: names[2].clone(),
        role: SegmentRole::Ngrams,
        def: seg2_def.clone(),
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::ResolvedBackendDef;

    fn dict_backend(name: &str) -> ResolvedBackendDef {
        let mut caps = HashSet::new();
        caps.insert(Capability::Dictionary);
        ResolvedBackendDef {
            name: name.to_string(),
            backend_type: crate::backends::BackendType::File,
            path: None,
            capabilities: caps,
            delimiter: None,
            has_header: false,
            word_index: None,
            freq_index: None,
            table: None,
            word_col: None,
            freq_col: None,
            table_ngrams: None,
            context_cols: Vec::new(),
            next_col: None,
            hunspell_affix: None,
            hunspell_dict: None,
        }
    }

    fn uni_backend(name: &str) -> ResolvedBackendDef {
        let mut caps = HashSet::new();
        caps.insert(Capability::Unigrams);
        ResolvedBackendDef {
            capabilities: caps,
            name: name.to_string(),
            ..dict_backend("")
        }
    }

    fn ngram_backend(name: &str) -> ResolvedBackendDef {
        let mut caps = HashSet::new();
        caps.insert(Capability::Ngrams);
        ResolvedBackendDef {
            capabilities: caps,
            name: name.to_string(),
            ..dict_backend("")
        }
    }

    fn full_backend(name: &str) -> ResolvedBackendDef {
        let mut caps = HashSet::new();
        caps.insert(Capability::Dictionary);
        caps.insert(Capability::Unigrams);
        caps.insert(Capability::Ngrams);
        ResolvedBackendDef {
            capabilities: caps,
            name: name.to_string(),
            ..dict_backend("")
        }
    }

    #[test]
    fn parse_simple_chain() {
        let segs = parse_chain("A").unwrap();
        assert_eq!(segs, vec!["A"]);
    }

    #[test]
    fn parse_three_seg() {
        let segs = parse_chain("A+B+C").unwrap();
        assert_eq!(segs, vec!["A", "B", "C"]);
    }

    #[test]
    fn parse_empty_rejected() {
        assert!(parse_chain("").is_err());
        assert!(parse_chain("A+").is_err());
        assert!(parse_chain("+B").is_err());
    }

    #[test]
    fn single_segment_dict() {
        let mut defs = HashMap::new();
        defs.insert("A".into(), dict_backend("A"));

        let (assign, warnings) = assign_roles("A", &defs).unwrap();
        assert_eq!(assign.segments.len(), 1);
        assert_eq!(assign.segments[0].role, SegmentRole::Dictionary);
        assert!(warnings.is_empty());
    }

    #[test]
    fn single_segment_full() {
        let mut defs = HashMap::new();
        defs.insert("A".into(), full_backend("A"));

        let (assign, warnings) = assign_roles("A", &defs).unwrap();
        assert_eq!(assign.segments.len(), 1);
        assert_eq!(assign.segments[0].role, SegmentRole::Dictionary);
        assert!(warnings.is_empty());
    }

    #[test]
    fn two_segment() {
        let mut defs = HashMap::new();
        defs.insert("dict".into(), dict_backend("dict"));
        defs.insert("lm".into(), ngram_backend("lm"));

        let (assign, _) = assign_roles("dict+lm", &defs).unwrap();
        assert_eq!(assign.segments.len(), 2);
        assert_eq!(assign.segments[0].role, SegmentRole::Dictionary);
        assert_eq!(assign.segments[1].role, SegmentRole::Ngrams);
    }

    #[test]
    fn three_segment() {
        let mut defs = HashMap::new();
        defs.insert("d".into(), dict_backend("d"));
        defs.insert("u".into(), uni_backend("u"));
        defs.insert("n".into(), ngram_backend("n"));

        let (assign, _) = assign_roles("d+u+n", &defs).unwrap();
        assert_eq!(assign.segments.len(), 3);
        assert_eq!(assign.segments[0].role, SegmentRole::Dictionary);
        assert_eq!(assign.segments[1].role, SegmentRole::Unigrams);
        assert_eq!(assign.segments[2].role, SegmentRole::Ngrams);
    }

    #[test]
    fn warn_missing_dict_in_seg0() {
        let mut defs = HashMap::new();
        defs.insert("u".into(), uni_backend("u"));
        defs.insert("n".into(), ngram_backend("n"));

        let (_, warnings) = assign_roles("u+n", &defs).unwrap();
        assert!(warnings.iter().any(|w| matches!(
            w,
            ChainWarning::MissingCapability {
                role: SegmentRole::Dictionary,
                ..
            }
        )));
    }

    #[test]
    fn warn_extra_capabilities() {
        let mut defs = HashMap::new();
        defs.insert("full".into(), full_backend("full"));
        defs.insert("n".into(), ngram_backend("n"));

        let (_, warnings) = assign_roles("full+n", &defs).unwrap();
        let has_extra = warnings.iter().any(|w| {
            matches!(
                w,
                ChainWarning::ExtraCapability {
                    role: SegmentRole::Dictionary,
                    ..
                }
            )
        });
        assert!(
            has_extra,
            "expected ExtraCapability warning for seg0 full backend"
        );
    }

    #[test]
    fn error_undefined_backend() {
        let defs = HashMap::new();
        let result = assign_roles("A", &defs);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not defined"));
    }

    #[test]
    fn error_too_many_segments() {
        let mut defs = HashMap::new();
        defs.insert("A".into(), dict_backend("A"));
        let result = assign_roles("A+B+C+D", &defs);
        assert!(result.is_err());
    }
}
