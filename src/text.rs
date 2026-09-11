//! Explicit per-request text preparation. Stored words keep their casing.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Normalization {
    #[default]
    None,
    Nfc,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseFold {
    #[default]
    None,
    #[serde(alias = "ascii_lower")]
    AsciiLowercase,
    #[serde(alias = "unicode_lower")]
    UnicodeLowercase,
    Full,
    LangSpecific,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextPrep {
    pub normalization: Normalization,
    pub fold: CaseFold,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CasePreference {
    #[serde(alias = "case_insensitive")]
    Insensitive,
    #[default]
    PreferMatched,
}

/// Language-specific folders are registered independently of request types.
/// The initial language map is deliberately empty; no language policy is guessed.
pub type Folder = fn(&str) -> String;

#[derive(Clone, Default)]
pub struct LangDb {
    folders: BTreeMap<String, Folder>,
    languages: BTreeMap<String, String>,
}

impl LangDb {
    pub fn register(&mut self, name: impl Into<String>, folder: Folder) {
        self.folders.insert(name.into(), folder);
    }

    pub fn assign(&mut self, lang: &str, folder: &str) -> Result<(), String> {
        if !self.folders.contains_key(folder) {
            return Err(format!("unknown case folder {folder}"));
        }
        self.languages.insert(language_key(lang), folder.into());
        Ok(())
    }

    fn folder(&self, lang: Option<&str>) -> Option<Folder> {
        let key = language_key(lang?);
        let name = self.languages.get(&key).or_else(|| {
            key.split_once('-')
                .and_then(|(base, _)| self.languages.get(base))
        })?;
        self.folders.get(name).copied()
    }
}

fn language_key(lang: &str) -> String {
    lang.replace('_', "-").to_ascii_lowercase()
}

impl TextPrep {
    pub const CLI_DEFAULT: Self = Self {
        normalization: Normalization::Nfc,
        fold: CaseFold::LangSpecific,
    };

    pub fn apply(self, text: &str, languages: &LangDb, lang: Option<&str>) -> String {
        let normalized = match self.normalization {
            Normalization::None => text.to_owned(),
            Normalization::Nfc => text.nfc().collect(),
        };
        match self.fold {
            CaseFold::None => normalized,
            CaseFold::AsciiLowercase => normalized.to_ascii_lowercase(),
            CaseFold::UnicodeLowercase => normalized.to_lowercase(),
            CaseFold::Full => caseless::default_case_fold_str(&normalized),
            CaseFold::LangSpecific => languages
                .folder(lang)
                .map_or_else(|| normalized.to_lowercase(), |fold| fold(&normalized)),
        }
    }

    pub fn warning(self) -> Option<&'static str> {
        (self.normalization == Normalization::None && self.fold != CaseFold::None)
            .then_some("case folding without explicit NFC may miss canonically equivalent words")
    }
}

/// NFC is applied at text-store ingestion, never implicitly at exact lookup.
pub fn nfc(text: &str) -> String {
    text.nfc().collect()
}

pub const BOS_WIRE: &str = "<s>";
pub const BOS: &str = "\u{ffff}";

/// A keyboard-authored boundary discards older sentences. Empty is unknown
/// context, not an inferred beginning of a sentence.
pub fn prepare_context(
    context: &[&str],
    prep: TextPrep,
    languages: &LangDb,
    lang: Option<&str>,
) -> Vec<String> {
    let start = context
        .iter()
        .rposition(|s| *s == BOS_WIRE || *s == BOS)
        .unwrap_or(0);
    context[start..]
        .iter()
        .map(|s| {
            if *s == BOS_WIRE || *s == BOS {
                BOS.into()
            } else {
                prep.apply(s, languages, lang)
            }
        })
        .collect()
}

/// Stores without sentence-start rows ignore the marker and older context.
pub fn after_boundary<'a, 'b>(context: &'a [&'b str]) -> &'a [&'b str] {
    match context.iter().rposition(|s| *s == BOS || *s == BOS_WIRE) {
        Some(i) => &context[i + 1..],
        None => context,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_default_and_explicit_preparation() {
        let db = LangDb::default();
        assert_eq!(
            TextPrep::default().apply("E\u{301}Σİß", &db, None),
            "E\u{301}Σİß"
        );
        let prep = TextPrep {
            normalization: Normalization::Nfc,
            fold: CaseFold::Full,
        };
        // Default Unicode folding is non-Turkic: dotted I retains its dot.
        assert_eq!(prep.apply("E\u{301}Σςİß", &db, None), "éσσi\u{307}ss");
        assert_eq!(prep.apply("\u{1c90}", &db, None), "\u{10d0}");
        assert_eq!(
            TextPrep::CLI_DEFAULT.apply("HELLO", &db, Some("en_US")),
            "hello"
        );
    }

    #[test]
    fn registry_matrix_is_idempotent() {
        let db = LangDb::default();
        for fold in [
            CaseFold::None,
            CaseFold::AsciiLowercase,
            CaseFold::UnicodeLowercase,
            CaseFold::Full,
            CaseFold::LangSpecific,
        ] {
            for normalization in [Normalization::None, Normalization::Nfc] {
                let prep = TextPrep {
                    normalization,
                    fold,
                };
                for input in ["Straße", "İIı", "Σςσ", "CAFÉ", "E\u{301}", "Hello", ""] {
                    let once = prep.apply(input, &db, Some("en_US"));
                    assert_eq!(
                        prep.apply(&once, &db, Some("en_US")),
                        once,
                        "{prep:?}: {input}"
                    );
                }
            }
        }
    }

    #[test]
    fn language_map_is_explicit_and_extensible() {
        let mut db = LangDb::default();
        assert!(db.assign("de", "missing").is_err());
        db.register("example", |s| s.to_lowercase().replace('ä', "ae"));
        db.assign("de", "example").unwrap();
        assert_eq!(
            TextPrep::CLI_DEFAULT.apply("Ähre", &db, Some("de_DE")),
            "aehre"
        );
        assert_eq!(
            TextPrep::CLI_DEFAULT.apply("Ähre", &db, Some("en_US")),
            "ähre"
        );
        assert_eq!(
            TextPrep {
                fold: CaseFold::None,
                ..TextPrep::CLI_DEFAULT
            }
            .apply("Ähre", &db, Some("de")),
            "Ähre"
        );
    }

    #[test]
    fn sentence_boundary_is_explicit_and_never_inferred() {
        let db = LangDb::default();
        assert!(prepare_context(&[], TextPrep::default(), &db, None).is_empty());
        assert_eq!(
            prepare_context(
                &["old", BOS_WIRE, "Hello"],
                TextPrep::CLI_DEFAULT,
                &db,
                None
            ),
            [BOS, "hello"]
        );
        assert_eq!(after_boundary(&[BOS, "hello"]), ["hello"]);
    }
}
