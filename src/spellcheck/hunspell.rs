use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::dictionary::HunspellDictionaryBackend;
use crate::prediction::ngram_backend::NgramBackend;
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
    /// zspell dictionary for is_correct and embedded suggestions.
    zdict: Arc<zspell::Dictionary>,
    /// HunspellDictionaryBackend for generic suggester (when not using embedded).
    dict_backend: Arc<HunspellDictionaryBackend>,
    /// Optional n-gram backend for context-aware suggestion scoring.
    ngram_backend: Arc<Mutex<Option<Arc<dyn NgramBackend>>>>,
    /// When true, use zspell's built-in suggestion engine.
    embedded_correction_engine: bool,
}

impl HunspellSpellChecker {
    /// Load a Hunspell dictionary from `.aff` and `.dic` file paths.
    ///
    /// `embedded_correction_engine`: when true, use zspell's built-in
    /// suggestion engine; when false, use the generic edit-distance suggester.
    pub fn from_files<P: AsRef<Path>>(
        aff_path: P,
        dic_path: P,
        embedded_correction_engine: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let aff = std::fs::read_to_string(aff_path.as_ref())?;
        let dic = std::fs::read_to_string(dic_path.as_ref())?;

        let zdict = zspell::builder().config_str(&aff).dict_str(&dic).build()?;
        let dict_backend = HunspellDictionaryBackend::from_dic_file(dic_path)?;

        Ok(Self {
            zdict: Arc::new(zdict),
            dict_backend: Arc::new(dict_backend),
            ngram_backend: Arc::new(Mutex::new(None)),
            embedded_correction_engine,
        })
    }

    /// Load a Hunspell dictionary by language tag (e.g. `"en_US"`).
    ///
    /// Searches common system directories:
    /// - `/usr/share/hunspell/`
    /// - `/usr/share/myspell/`
    /// - `/usr/share/myspell/dicts/`
    ///
    /// `embedded_correction_engine`: when true, use zspell's built-in
    /// suggestion engine; when false, use the generic edit-distance suggester.
    pub fn from_tag(
        tag: &str,
        embedded_correction_engine: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let dirs = [
            "/usr/share/hunspell",
            "/usr/share/myspell",
            "/usr/share/myspell/dicts",
        ];

        for dir in &dirs {
            let aff_path = PathBuf::from(dir).join(format!("{}.aff", tag));
            let dic_path = PathBuf::from(dir).join(format!("{}.dic", tag));

            if aff_path.exists() && dic_path.exists() {
                return Self::from_files(
                    aff_path.to_str().unwrap(),
                    dic_path.to_str().unwrap(),
                    embedded_correction_engine,
                );
            }
        }

        Err(format!("hunspell dictionary for '{}' not found", tag).into())
    }
}

impl SpellChecker for HunspellSpellChecker {
    fn is_correct(&self, word: &str) -> bool {
        self.zdict.check(word)
    }

    fn suggest(&self, word: &str, context: &[&str]) -> Vec<String> {
        if self.embedded_correction_engine {
            self.zdict
                .entry(word)
                .suggest()
                .map(|v| v.into_iter().map(|s| s.to_string()).collect())
                .unwrap_or_default()
        } else {
            let ngram_opt = self.ngram_backend.lock().unwrap().clone();
            let ngram_ref = ngram_opt.as_ref().map(|nb| nb.as_ref());
            crate::spellcheck::suggest::suggest_edits(
                &*self.dict_backend,
                ngram_ref,
                word,
                context,
                10,
            )
        }
    }

    fn can_use_ngram_backend(&self) -> bool {
        !self.embedded_correction_engine
    }

    fn has_ngram_backend(&self) -> bool {
        self.ngram_backend.lock().unwrap().is_some()
    }

    fn set_ngram_backend(&self, backend: std::sync::Arc<dyn NgramBackend>) {
        if !self.embedded_correction_engine {
            let mut guard = self.ngram_backend.lock().unwrap();
            *guard = Some(backend);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_by_tag() {
        let checker = HunspellSpellChecker::from_tag("en_US", true).unwrap();
        assert!(checker.is_correct("hello"));
        assert!(!checker.is_correct("helo"));
        let suggestions = checker.suggest("helo", &[]);
        assert!(suggestions.contains(&"hello".to_string()));
    }
}
