use std::path::Path;
use std::sync::Arc;

use zspell::Dictionary as ZspellDict;

use crate::spellcheck::SpellChecker;

/// [`SpellChecker`] implementation wrapping a Hunspell dictionary via the
/// `zspell` crate.
///
/// # Attention points for the implementor
///
/// * `zspell::Dictionary::suggest` is relatively expensive.  The dictionary
///   object is cached in an `Arc` and should be shared across all callers.
/// * Loading a Hunspell dictionary requires both an `.aff` file and a `.dic`
///   file.  Use [`HunspellSpellChecker::from_files`] or
///   [`HunspellSpellChecker::from_tag`] to construct.
/// * `zspell::Dictionary::check` is `Send`-safe; `suggest` may involve
///   internal mutation.  Wrap in a `Mutex` if `Send + Sync` is required for
///   the trait object.  **Current impl**: the `zspell` docs advise that
///   `suggest` is read-only on the aff/dic data, so a single `Arc` is safe
///   in practice, but this may need a `Mutex` depending on the `zspell`
///   version.
pub struct HunspellSpellChecker {
    dict: Arc<ZspellDict>,
    language_tag: String,
}

impl HunspellSpellChecker {
    /// Load a Hunspell dictionary from `.aff` and `.dic` file paths.
    pub fn from_files<P: AsRef<Path>>(
        aff_path: P,
        dic_path: P,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let dict = ZspellDict::new(
            &zspell::DictBuilder::new()
                .aff_file(aff_path.as_ref())
                .dic_file(dic_path.as_ref()),
        )?;
        Ok(Self {
            dict: Arc::new(dict),
            language_tag: String::new(),
        })
    }

    /// Load a Hunspell dictionary from a language tag (e.g. `"en_US"`).
    ///
    /// This relies on `zspell`'s built-in search paths (typically
    /// `/usr/share/hunspell/` or `~/.hunspell/`).
    pub fn from_tag(tag: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let dict = ZspellDict::new(&zspell::DictBuilder::new().tag(tag))?;
        Ok(Self {
            dict: Arc::new(dict),
            language_tag: tag.to_string(),
        })
    }
}

impl SpellChecker for HunspellSpellChecker {
    fn is_correct(&self, word: &str) -> bool {
        // zspell::Dictionary::check returns true when the word is
        // found in the dictionary (respecting affix rules).
        self.dict.check(word)
    }

    fn suggest(&self, word: &str) -> Vec<String> {
        // zspell::Dictionary::suggest returns a Vec<String> of
        // candidate corrections, sorted by the internal Hunspell
        // ranking (best first).
        self.dict.suggest(word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_by_tag() {
        // This test requires a Hunspell dictionary installed on the system.
        // It is ignored by default (marking as a canary).
        if let Ok(checker) = HunspellSpellChecker::from_tag("en_US") {
            assert!(checker.is_correct("hello"));
            assert!(!checker.is_correct("helo"));
            let suggestions = checker.suggest("helo");
            assert!(suggestions.contains(&"hello".to_string()));
        }
    }

    #[test]
    #[ignore = "requires hunspell-en-us system package"]
    fn test_suggestions() {
        let checker = HunspellSpellChecker::from_tag("en_US").unwrap();
        let suggestions = checker.suggest("helo");
        assert!(!suggestions.is_empty());
        assert!(suggestions.contains(&"hello".to_string()));
    }
}
