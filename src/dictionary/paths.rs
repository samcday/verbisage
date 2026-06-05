use std::path::PathBuf;

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

/// Extension list searched when resolving dictionary files for a language.
const DICT_EXTENSIONS: &[&str] = &["dic", "freq", "wordlist"];

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
        // ~user — not supported; pass through verbatim
        PathBuf::from(path)
    }
}

// ---------------------------------------------------------------------------
// Path-override semantics
// ---------------------------------------------------------------------------

/// How to treat a particular dictionary layer (system or user).
#[derive(Debug, Clone)]
pub enum PathOverride {
    /// No explicit override — look up files inside the standard directory.
    Default,
    /// Explicitly skip this layer (empty string was passed).
    Skip,
    /// Use this specific file path.
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
/// The resolution order is:
///
/// 1. If a system-file override is given, use only that file
///    (or skip the system layer entirely if the override was empty).
/// 2. Otherwise scan `system_dir` for `<language>.{dic,freq,wordlist}`.
/// 3. Same for the user layer (override first, then directory scan).
///
/// Files are returned **system-first, user-last** so that user entries
/// override system entries when merged.
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

    /// Set the system data directory (overrides the built-in default).
    pub fn with_system_dir(mut self, dir: PathBuf) -> Self {
        self.system_dir = dir;
        self
    }

    /// Set the user data directory (overrides the built-in default).
    pub fn with_user_dir(mut self, dir: PathBuf) -> Self {
        self.user_dir = dir;
        self
    }

    /// Resolve all dictionary files that should be loaded, in load order.
    ///
    /// **System files come first, user files come last**, so that user
    /// frequencies take precedence when a word appears in both layers.
    pub fn resolve_dict_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = Vec::new();

        // ── System layer ────────────────────────────────────────────────
        match &self.system_file_override {
            PathOverride::Skip => { /* skip entirely */ }
            PathOverride::File(p) => {
                files.push(expand_tilde(p.to_str().unwrap_or("")));
            }
            PathOverride::Default => {
                for ext in DICT_EXTENSIONS {
                    let f = self.system_dir.join(format!("{}.{}", self.language, ext));
                    if f.exists() {
                        files.push(f);
                    }
                }
            }
        }

        // ── User layer ──────────────────────────────────────────────────
        match &self.user_file_override {
            PathOverride::Skip => { /* skip entirely */ }
            PathOverride::File(p) => {
                files.push(expand_tilde(p.to_str().unwrap_or("")));
            }
            PathOverride::Default => {
                for ext in DICT_EXTENSIONS {
                    let f = self.user_dir.join(format!("{}.{}", self.language, ext));
                    if f.exists() {
                        files.push(f);
                    }
                }
            }
        }

        files
    }
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
    fn resolve_empty_for_nonexistent_language() {
        let lp = LanguagePaths::new("nonexistent_lang_xyz");
        let files = lp.resolve_dict_files();
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
    }
}
