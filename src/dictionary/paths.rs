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
pub const USER_DATA_DIR_REL: &str = env_or!("VERBISAGE_USER_DIR", ".local/share/verbisage");

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
}

impl LanguagePaths {
    pub fn new(language: &str) -> Self {
        let home = || std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        Self {
            system_dir: PathBuf::from(SYSTEM_DATA_DIR),
            user_dir: PathBuf::from(home()).join(USER_DATA_DIR_REL),
            language: language.to_string(),
            system_file_override: PathOverride::Default,
            user_file_override: PathOverride::Default,
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

    /// Convenience: word-list dictionary files (file backend).
    ///
    /// Looks for `<lang>.dic`, `<lang>.freq`, and `<lang>.wordlist` in
    /// both system and user directories.  User files are returned after
    /// system files so their frequencies take precedence.
    pub fn resolve_dict_files(&self) -> Vec<PathBuf> {
        let patterns = &["{lang}.dic", "{lang}.freq", "{lang}.wordlist"];
        let mut files = self.resolve_system(patterns);
        files.extend(self.resolve_user(patterns));
        files
    }

    /// SQLite dictionary files (sqlite backend).
    ///
    /// System: `database_{lang}.db`  (read‑only reference)
    /// User:   `lm_{lang}.db`       (trainable language model)
    pub fn resolve_sqlite_files(&self) -> Vec<PathBuf> {
        let sys = self.resolve_system(&["database_{lang}.db"]);
        let usr = self.resolve_user(&["lm_{lang}.db"]);
        sys.into_iter().chain(usr).collect()
    }
}

// ── internal helper ───────────────────────────────────────────────────────

fn find_files(dir: &Path, language: &str, patterns: &[&str]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for pattern in patterns {
        let filename = pattern.replace("{lang}", language);
        let f = dir.join(&filename);
        if f.exists() {
            files.push(f);
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
}
