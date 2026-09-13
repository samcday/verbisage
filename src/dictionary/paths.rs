use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Build-time overridable defaults
// ---------------------------------------------------------------------------

macro_rules! env_or {
    ($name:expr, $default:expr) => {
        match option_env!($name) {
            Some(v) => v,
            None => $default,
        }
    };
}

/// System-wide data directory.
///
/// Override at build time by setting the `VERBISAGE_SYSTEM_DIR` environment
/// variable (Meson, `cargo build`, etc.):
///
/// ```sh
/// VERBISAGE_SYSTEM_DIR=/custom/path cargo build
/// ```
pub const SYSTEM_DATA_DIR: &str = env_or!("VERBISAGE_SYSTEM_DIR", "/usr/share/verbisage");

/// User data directory (relative to `$HOME`).
///
/// Override at build time via `VERBISAGE_USER_DIR`.
pub const USER_DATA_DIR_REL: &str = env_or!("VERBISAGE_USER_DIR", "~/.local/share/verbisage");

// ---------------------------------------------------------------------------
// Helper: tilde expansion
// ---------------------------------------------------------------------------

/// Expand a leading `~/` or bare `~` to `$HOME`.
///
/// `~user/` prefixes are not yet supported and are returned as-is.
pub fn expand_tilde(path: &str) -> PathBuf {
    if !path.starts_with('~') {
        return PathBuf::from(path);
    }

    let after = &path[1..];
    if after.is_empty() || after.starts_with('/') {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join(after.trim_start_matches('/'))
    } else {
        PathBuf::from(path)
    }
}

/// Expand a `Path` that may begin with `~` (see [`expand_tilde`]).
fn expand_dir(dir: &Path) -> PathBuf {
    expand_tilde(&dir.to_string_lossy())
}

// ---------------------------------------------------------------------------
// Path-override semantics
// ---------------------------------------------------------------------------

/// How to treat a particular dictionary layer (system or user).
#[derive(Debug, Clone)]
pub enum PathOverride {
    /// No explicit override — look up files via pattern matching.
    Default,
    /// Explicitly skip this layer (empty string was passed).
    Skip,
    /// Use this specific file path directly.
    File(PathBuf),
}

impl PathOverride {
    pub fn from_cli(value: Option<&str>) -> Self {
        match value {
            None => PathOverride::Default,
            Some("") => PathOverride::Skip,
            Some(p) => PathOverride::File(expand_tilde(p)),
        }
    }
}

// ---------------------------------------------------------------------------
// LanguagePaths
// ---------------------------------------------------------------------------

/// Resolved system + user dictionary paths for a given language.
///
/// Each backend provides its own filename patterns; `{lang}` inside a
/// pattern is replaced with the configured language tag.  For example:
///
/// | Backend | System pattern | User pattern |
/// |---------|----------------|--------------|
/// | file    | `{lang}.dic`   | `{lang}.dic` |
/// | sqlite  | `database_{lang}.db` | `lm_{lang}.db` |
///
/// Call [`resolve_system`](Self::resolve_system) and
/// [`resolve_user`](Self::resolve_user) with the appropriate patterns,
/// then combine the results.
#[derive(Debug, Clone)]
pub struct LanguagePaths {
    pub system_dir: PathBuf,
    pub user_dir: PathBuf,
    pub language: String,
    pub system_file_override: PathOverride,
    pub user_file_override: PathOverride,
    pub system_dict_patterns: Vec<String>,
    pub user_dict_patterns: Vec<String>,
    pub system_sqlite_patterns: Vec<String>,
    pub user_sqlite_patterns: Vec<String>,
    pub system_marisa_patterns: Vec<String>,
    pub user_marisa_patterns: Vec<String>,
    pub system_marisa_ngram_trie_patterns: Vec<String>,
    pub user_marisa_ngram_trie_patterns: Vec<String>,
    pub system_marisa_ngram_counts_patterns: Vec<String>,
    pub user_marisa_ngram_counts_patterns: Vec<String>,
}

impl LanguagePaths {
    pub fn new(language: &str) -> Self {
        Self {
            system_dir: PathBuf::from(SYSTEM_DATA_DIR),
            user_dir: PathBuf::from(USER_DATA_DIR_REL),
            language: language.to_string(),
            system_file_override: PathOverride::Default,
            user_file_override: PathOverride::Default,
            system_dict_patterns: vec![
                "{lang}.dic".into(),
                "{lang}.freq".into(),
                "{lang}.wordlist".into(),
            ],
            user_dict_patterns: vec![
                "{lang}.dic".into(),
                "{lang}.freq".into(),
                "{lang}.wordlist".into(),
            ],
            system_sqlite_patterns: vec!["database_{lang}.db".into()],
            user_sqlite_patterns: vec!["lm_{lang}.db".into()],
            system_marisa_patterns: vec!["{lang}.marisa".into()],
            user_marisa_patterns: vec!["{lang}.marisa".into()],
            system_marisa_ngram_trie_patterns: vec!["database_{lang}/ngrams.trie".into()],
            user_marisa_ngram_trie_patterns: vec!["database_{lang}/ngrams.trie".into()],
            system_marisa_ngram_counts_patterns: vec!["database_{lang}/ngrams.counts".into()],
            user_marisa_ngram_counts_patterns: vec!["database_{lang}/ngrams.counts".into()],
        }
    }

    /// Override the system data directory.
    pub fn with_system_dir(mut self, dir: PathBuf) -> Self {
        self.system_dir = dir;
        self
    }

    /// Override the user data directory.
    pub fn with_user_dir(mut self, dir: PathBuf) -> Self {
        self.user_dir = dir;
        self
    }

    /// Apply non-`None` pattern overrides for all four categories.
    pub fn set_patterns(
        &mut self,
        system_dict: Option<&[String]>,
        user_dict: Option<&[String]>,
        system_sqlite: Option<&[String]>,
        user_sqlite: Option<&[String]>,
    ) {
        if let Some(p) = system_dict {
            self.system_dict_patterns = p.to_vec();
        }
        if let Some(p) = user_dict {
            self.user_dict_patterns = p.to_vec();
        }
        if let Some(p) = system_sqlite {
            self.system_sqlite_patterns = p.to_vec();
        }
        if let Some(p) = user_sqlite {
            self.user_sqlite_patterns = p.to_vec();
        }
    }

    // ── Pattern-based resolution ────────────────────────────────────────

    /// Resolve files in **system** directory matching any of `patterns`.
    ///
    /// Each pattern has `{lang}` replaced with [`self.language`].  An
    /// override (file / skip) bypasses the directory scan entirely.
    ///
    /// Returns an empty vec when nothing is found and no file override
    /// was given.
    pub fn resolve_system(&self, patterns: &[&str]) -> Vec<PathBuf> {
        match &self.system_file_override {
            PathOverride::Skip => return Vec::new(),
            PathOverride::File(p) => {
                return vec![expand_tilde(p.to_str().unwrap_or(""))];
            }
            PathOverride::Default => {}
        }
        find_files(&self.system_dir, &self.language, patterns)
    }

    /// Resolve files in **user** directory matching any of `patterns`.
    ///
    /// Same semantics as [`resolve_system`](Self::resolve_system).
    pub fn resolve_user(&self, patterns: &[&str]) -> Vec<PathBuf> {
        match &self.user_file_override {
            PathOverride::Skip => return Vec::new(),
            PathOverride::File(p) => {
                return vec![expand_tilde(p.to_str().unwrap_or(""))];
            }
            PathOverride::Default => {}
        }
        find_files(&self.user_dir, &self.language, patterns)
    }

    /// Resolve ALL candidate paths in **system** directory (including non-existing).
    pub fn resolve_system_all(&self, patterns: &[&str]) -> Vec<PathBuf> {
        match &self.system_file_override {
            PathOverride::Skip => return Vec::new(),
            PathOverride::File(p) => {
                return vec![expand_tilde(p.to_str().unwrap_or(""))];
            }
            PathOverride::Default => {}
        }
        find_all_candidates(&self.system_dir, &self.language, patterns)
    }

    /// Resolve ALL candidate paths in **user** directory (including non-existing).
    pub fn resolve_user_all(&self, patterns: &[&str]) -> Vec<PathBuf> {
        match &self.user_file_override {
            PathOverride::Skip => return Vec::new(),
            PathOverride::File(p) => {
                return vec![expand_tilde(p.to_str().unwrap_or(""))];
            }
            PathOverride::Default => {}
        }
        find_all_candidates(&self.user_dir, &self.language, patterns)
    }

    /// Convenience: word-list dictionary files (file backend).
    ///
    /// For each pattern, tries each [`language_fallbacks`] tag from most to
    /// least specific within a layer. The system layer is resolved before the
    /// user layer, so an explicit system dictionary still wins over a user
    /// dictionary for the same request.
    pub fn resolve_dict_files(&self) -> Vec<PathBuf> {
        let sys_strs: Vec<&str> = self
            .system_dict_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let usr_strs: Vec<&str> = self.user_dict_patterns.iter().map(|s| s.as_str()).collect();
        let mut files = self.resolve_system(&sys_strs);
        files.extend(self.resolve_user(&usr_strs));
        files
    }

    /// Resolve ALL candidate dictionary paths (including non-existing).
    pub fn resolve_dict_files_all(&self) -> Vec<PathBuf> {
        let sys_strs: Vec<&str> = self
            .system_dict_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let usr_strs: Vec<&str> = self.user_dict_patterns.iter().map(|s| s.as_str()).collect();
        let mut files = self.resolve_system_all(&sys_strs);
        files.extend(self.resolve_user_all(&usr_strs));
        files
    }

    /// Marisa n-gram trie files (for prediction).
    pub fn resolve_marisa_ngram_trie_files(&self) -> Vec<PathBuf> {
        let sys: Vec<&str> = self
            .system_marisa_ngram_trie_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let usr: Vec<&str> = self
            .user_marisa_ngram_trie_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let sys = self.resolve_system(&sys);
        let usr = self.resolve_user(&usr);
        sys.into_iter().chain(usr).collect()
    }

    /// Marisa n-gram counts files (companion to the trie).
    pub fn resolve_marisa_ngram_counts_files(&self) -> Vec<PathBuf> {
        let sys: Vec<&str> = self
            .system_marisa_ngram_counts_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let usr: Vec<&str> = self
            .user_marisa_ngram_counts_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let sys = self.resolve_system(&sys);
        let usr = self.resolve_user(&usr);
        sys.into_iter().chain(usr).collect()
    }

    /// Marisa n-gram trie files — ALL candidates (including non-existing).
    pub fn resolve_all_marisa_ngram_trie_files(&self) -> Vec<PathBuf> {
        let sys: Vec<&str> = self
            .system_marisa_ngram_trie_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let usr: Vec<&str> = self
            .user_marisa_ngram_trie_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let sys = self.resolve_system_all(&sys);
        let usr = self.resolve_user_all(&usr);
        sys.into_iter().chain(usr).collect()
    }

    /// Marisa n-gram counts files — ALL candidates (including non-existing).
    pub fn resolve_all_marisa_ngram_counts_files(&self) -> Vec<PathBuf> {
        let sys: Vec<&str> = self
            .system_marisa_ngram_counts_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let usr: Vec<&str> = self
            .user_marisa_ngram_counts_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let sys = self.resolve_system_all(&sys);
        let usr = self.resolve_user_all(&usr);
        sys.into_iter().chain(usr).collect()
    }

    /// Marisa trie files.
    pub fn resolve_marisa_files(&self) -> Vec<PathBuf> {
        let sys_strs: Vec<&str> = self
            .system_marisa_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let usr_strs: Vec<&str> = self
            .user_marisa_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let sys = self.resolve_system(&sys_strs);
        let usr = self.resolve_user(&usr_strs);
        sys.into_iter().chain(usr).collect()
    }

    /// SQLite dictionary files (sqlite backend).
    pub fn resolve_sqlite_files(&self) -> Vec<PathBuf> {
        let sys_strs: Vec<&str> = self
            .system_sqlite_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let usr_strs: Vec<&str> = self
            .user_sqlite_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let sys = self.resolve_system(&sys_strs);
        let usr = self.resolve_user(&usr_strs);
        sys.into_iter().chain(usr).collect()
    }

    /// Resolve only the **system** sqlite file (read-only backend).
    pub fn system_sqlite_file(&self) -> Option<PathBuf> {
        let strs: Vec<&str> = self
            .system_sqlite_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        self.resolve_system(&strs).into_iter().next()
    }

    /// Resolve only the **user** sqlite file (read-write backend).
    /// Returns the first existing file, if any.
    pub fn user_sqlite_file(&self) -> Option<PathBuf> {
        let strs: Vec<&str> = self
            .user_sqlite_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        self.resolve_user(&strs).into_iter().next()
    }

    /// Intended user sqlite file path, whether it exists or not.
    /// Uses the first pattern so the caller can create it lazily.
    pub fn user_sqlite_path(&self) -> Option<PathBuf> {
        let fallbacks = language_fallbacks(&self.language);
        let pattern = self.user_sqlite_patterns.first()?;
        let lang = fallbacks.first().unwrap_or(&self.language);
        Some(expand_dir(&self.user_dir).join(pattern.replace("{lang}", lang)))
    }
}

// ── internal helpers ──────────────────────────────────────────────────────

/// Generate language tags to try, from most to least specific.
///
/// The caller's tag is preserved exactly and first. Variant subtags are then
/// dropped from the right (`-` separates them), and afterwards
/// underscore-separated components are dropped the same way. A full
/// regional/variant tag therefore reaches its regional form and then its base
/// language without changing the caller's spelling:
///
/// | Tag | Fallbacks |
/// |---|---|
/// | `fr_FR-br` | `fr_FR-br`, `fr_FR`, `fr` |
/// | `en_US` | `en_US`, `en` |
/// | `zh-Hant-TW` | `zh-Hant-TW`, `zh-Hant`, `zh` |
/// | `pt_BR` | `pt_BR`, `pt` |
/// | `de` | `de` |
///
/// Explicitly fixed paths do not use this chain; those stay fixed.
pub(crate) fn language_fallbacks(tag: &str) -> Vec<String> {
    let mut tags = vec![tag.to_string()];
    let mut current = tag;

    while let Some(index) = current.rfind('-') {
        current = &current[..index];
        if current.is_empty() {
            break;
        }
        if tags.last().map(String::as_str) != Some(current) {
            tags.push(current.to_string());
        }
    }
    while let Some(index) = current.rfind('_') {
        if index == 0 {
            break;
        }
        current = &current[..index];
        if tags.last().map(String::as_str) != Some(current) {
            tags.push(current.to_string());
        }
    }
    tags
}

/// Equivalent spellings of one tag, exact first.
///
/// The integration accepts both the POSIX (`fr_FR`) and the BCP-47
/// (`fr-FR`) region separator, so a dictionary written in one form stays
/// usable when the caller selected the other. Only the separator is swapped;
/// case and component order are preserved, and no components are dropped.
/// Fixed paths and layer overrides do not use these aliases.
pub(crate) fn language_spellings(tag: &str) -> Vec<String> {
    let mut spellings = vec![tag.to_string()];
    if tag.contains('_') {
        spellings.push(tag.replace('_', "-"));
    } else if tag.contains('-') {
        spellings.push(tag.replace('-', "_"));
    }
    spellings
}

/// Search `dir` for files matching any `pattern`, trying each language
/// fallback in order and, within one tag, its exact spelling before the
/// equivalent separator alias. Returns the first match per pattern (most
/// specific language wins). Only returns existing files.
fn find_files(dir: &Path, language: &str, patterns: &[&str]) -> Vec<PathBuf> {
    let dir = expand_dir(dir);
    let fallbacks = language_fallbacks(language);
    let mut files = Vec::new();

    for pattern in patterns {
        let mut found = None;
        'spellings: for lang in &fallbacks {
            for spelling in language_spellings(lang) {
                let f = dir.join(pattern.replace("{lang}", &spelling));
                if f.exists() {
                    found = Some(f);
                    break 'spellings;
                }
            }
        }
        if let Some(f) = found {
            files.push(f);
        }
    }

    files
}

/// Generate all candidate paths for `dir` matching any `pattern`, trying each
/// language fallback in order and its equivalent spellings. Returns ALL
/// candidates regardless of existence.
fn find_all_candidates(dir: &Path, language: &str, patterns: &[&str]) -> Vec<PathBuf> {
    let dir = expand_dir(dir);
    let fallbacks = language_fallbacks(language);
    let mut files = Vec::new();

    for pattern in patterns {
        for lang in &fallbacks {
            for spelling in language_spellings(lang) {
                files.push(dir.join(pattern.replace("{lang}", &spelling)));
            }
        }
    }

    files
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expansion_home() {
        let p = expand_tilde("~/foo/bar");
        assert!(p.to_str().unwrap().contains("/foo/bar"));
        assert!(!p.to_str().unwrap().contains('~'));
    }

    #[test]
    fn tilde_expansion_bare() {
        let p = expand_tilde("~");
        assert!(!p.to_str().unwrap().contains('~'));
    }

    #[test]
    fn plain_path_unchanged() {
        let p = expand_tilde("/usr/share/dict/words");
        assert_eq!(p.to_str().unwrap(), "/usr/share/dict/words");
    }

    #[test]
    fn default_dirs_keep_the_literal_tilde_but_expand_on_use() {
        // Display/config surfaces keep the literal `~`.
        let lp = LanguagePaths::new("en_US");
        assert!(
            lp.user_dir.to_string_lossy().starts_with('~'),
            "default user dir should stay verbose: {:?}",
            lp.user_dir
        );
        // Resolution expands it.
        let expanded = expand_dir(&lp.user_dir);
        assert!(!expanded.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn resolve_empty_for_nonexistent_patterns() {
        let lp = LanguagePaths::new("en_US");
        let files = lp.resolve_system(&["nonexistent_{lang}.xyz"]);
        assert!(files.is_empty());
    }

    #[test]
    fn override_skip_produces_no_files() {
        let lp = LanguagePaths {
            system_file_override: PathOverride::Skip,
            user_file_override: PathOverride::Skip,
            ..LanguagePaths::new("en_US")
        };
        assert!(lp.resolve_dict_files().is_empty());
        assert!(lp.resolve_sqlite_files().is_empty());
    }

    #[test]
    fn lang_substitution() {
        assert_eq!(
            "database_en_US.db",
            "database_{lang}.db".replace("{lang}", "en_US")
        );
        assert_eq!("lm_de.db", "lm_{lang}.db".replace("{lang}", "de"));
    }

    #[test]
    fn language_fallbacks_full_tag() {
        let tags = language_fallbacks("en_US");
        assert_eq!(tags, vec!["en_US", "en"]);
    }

    #[test]
    fn language_fallbacks_base_only() {
        let tags = language_fallbacks("de");
        assert_eq!(tags, vec!["de"]);
    }

    #[test]
    fn language_fallbacks_triple() {
        let tags = language_fallbacks("pt_BR");
        assert_eq!(tags, vec!["pt_BR", "pt"]);
    }

    #[test]
    fn language_fallbacks_variant_then_region_then_base() {
        assert_eq!(
            language_fallbacks("fr_FR-br"),
            vec!["fr_FR-br", "fr_FR", "fr"]
        );
        assert_eq!(
            language_fallbacks("zh-Hant-TW"),
            vec!["zh-Hant-TW", "zh-Hant", "zh"]
        );
        assert_eq!(language_fallbacks("fr-FR"), vec!["fr-FR", "fr"]);
        assert_eq!(language_fallbacks("fr"), vec!["fr"]);
    }

    #[test]
    fn find_files_prefers_region_then_base() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("fr.dic"), "base").unwrap();
        std::fs::write(temp.path().join("fr_FR.dic"), "region").unwrap();
        std::fs::write(temp.path().join("fr_FR-br.dic"), "exact").unwrap();
        let lp = LanguagePaths::new("fr_FR-br").with_system_dir(temp.path().to_path_buf());

        assert_eq!(
            lp.resolve_system(&["{lang}.dic"]),
            vec![temp.path().join("fr_FR-br.dic")]
        );
        std::fs::remove_file(temp.path().join("fr_FR-br.dic")).unwrap();
        assert_eq!(
            lp.resolve_system(&["{lang}.dic"]),
            vec![temp.path().join("fr_FR.dic")]
        );
        std::fs::remove_file(temp.path().join("fr_FR.dic")).unwrap();
        assert_eq!(
            lp.resolve_system(&["{lang}.dic"]),
            vec![temp.path().join("fr.dic")]
        );
        std::fs::remove_file(temp.path().join("fr.dic")).unwrap();
        assert!(lp.resolve_system(&["{lang}.dic"]).is_empty());
    }

    #[test]
    fn language_spellings_keep_case_and_components() {
        assert_eq!(
            language_spellings("fr_FR-br"),
            vec!["fr_FR-br", "fr-FR-br"]
        );
        assert_eq!(language_spellings("pt-PT"), vec!["pt-PT", "pt_PT"]);
        assert_eq!(language_spellings("en"), vec!["en"]);
    }

    #[test]
    fn find_files_tries_the_equivalent_separator() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("pt_PT.dic"), "posix").unwrap();
        let lp = LanguagePaths::new("pt-PT").with_system_dir(temp.path().to_path_buf());
        assert_eq!(
            lp.resolve_system(&["{lang}.dic"]),
            vec![temp.path().join("pt_PT.dic")]
        );

        // The exact spelling is still preferred when both exist.
        std::fs::write(temp.path().join("pt-PT.dic"), "bcp").unwrap();
        assert_eq!(
            lp.resolve_system(&["{lang}.dic"]),
            vec![temp.path().join("pt-PT.dic")]
        );
    }

    #[test]
    fn fixed_file_override_does_not_fall_back() {
        let temp = tempfile::tempdir().unwrap();
        let fixed = temp.path().join("fixed-wordlist.dic");
        std::fs::write(&fixed, "fixed").unwrap();
        let lp = LanguagePaths {
            system_file_override: PathOverride::File(fixed.clone()),
            user_file_override: PathOverride::Skip,
            ..LanguagePaths::new("fr_FR-br")
        };

        assert_eq!(lp.resolve_dict_files(), vec![fixed]);
    }
}
