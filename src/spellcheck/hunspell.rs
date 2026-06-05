use std::path::Path;
use std::sync::Arc;

use crate::spellcheck::SpellChecker;

/// [`SpellChecker`] implementation wrapping a Hunspell dictionary via the
/// `zspell` crate.
///
/// # Attention points for the implementor
///
/// * `zspell::Dictionary::entry().suggest()` is relatively expensive.
///   Cache the dictionary in an `Arc` and share across callers.
/// * Loading requires both `.aff` and `.dic` files.  Pass their paths to
///   [`HunspellSpellChecker::from_files`].
/// * The `zspell::DictBuilder` accepts dictionary data as string slices,
///   not file paths — file I/O is handled internally.
/// * Both `check` and `entry`/`suggest` take `&self`, so the `Arc` is
///   sufficient; no `Mutex` is required.
pub struct HunspellSpellChecker {
    dict: Arc<zspell::Dictionary>,
    _language_tag: String,
}

impl HunspellSpellChecker {
    /// Load a Hunspell dictionary from `.aff` and `.dic` file paths.
    pub fn from_files<P: AsRef<Path>>(
        aff_path: P,
        dic_path: P,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let aff = std::fs::read_to_string(aff_path.as_ref())?;
        let dic = std::fs::read_to_string(dic_path.as_ref())?;

        let dict = zspell::builder().config_str(&aff).dict_str(&dic).build()?;

        Ok(Self {
            dict: Arc::new(dict),
            _language_tag: String::new(),
        })
    }

    /// Load a Hunspell dictionary by language tag (e.g. `"en_US"`).
    ///
    /// Searches common system directories:
    /// - `/usr/share/hunspell/`
    /// - `/usr/share/myspell/`
    /// - `/usr/share/myspell/dicts/`
    pub fn from_tag(tag: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let dirs = [
            "/usr/share/hunspell",
            "/usr/share/myspell",
            "/usr/share/myspell/dicts",
        ];

        for dir in &dirs {
            let aff_path = format!("{}/{}.aff", dir, tag);
            let dic_path = format!("{}/{}.dic", dir, tag);

            if Path::new(&aff_path).exists() && Path::new(&dic_path).exists() {
                return Self::from_files(&aff_path, &dic_path);
            }
        }

        Err(format!("hunspell dictionary for '{}' not found", tag).into())
    }
}

impl SpellChecker for HunspellSpellChecker {
    fn is_correct(&self, word: &str) -> bool {
        self.dict.check(word)
    }

    fn suggest(&self, word: &str) -> Vec<String> {
        self.dict
            .entry(word)
            .suggest()
            .map(|v| v.into_iter().map(|s| s.to_string()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_by_tag() {
        let checker = HunspellSpellChecker::from_tag("en_US").unwrap();
        assert!(checker.is_correct("hello"));
        assert!(!checker.is_correct("helo"));
        let suggestions = checker.suggest("helo");
        assert!(suggestions.contains(&"hello".to_string()));
    }
}
