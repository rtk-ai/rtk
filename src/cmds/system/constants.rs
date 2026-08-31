use crate::core::config::{self, FilterConfig};

pub const NOISE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "__pycache__",
    ".next",
    "dist",
    "build",
    ".cache",
    ".turbo",
    ".vercel",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    ".venv",
    "venv",
    "env", // Python legacy virtualenv dir — noise. .env (dotenv) is intentionally NOT here: agents must see it.
    "coverage",
    ".nyc_output",
    ".DS_Store",
    "Thumbs.db",
    ".idea",
    ".vscode",
    ".vs",
    "*.egg-info",
    ".eggs",
];

#[derive(Debug, Clone)]
pub(crate) struct IgnorePatterns {
    pub(crate) noise: Vec<String>,
    pub(crate) dirs: Vec<String>,
    pub(crate) files: Vec<String>,
}

impl IgnorePatterns {
    pub(crate) fn from_config(filters: &FilterConfig) -> Self {
        Self {
            noise: NOISE_DIRS.iter().map(|s| s.to_string()).collect(),
            dirs: filters.ignore_dirs.clone(),
            files: filters.ignore_files.clone(),
        }
    }

    pub(crate) fn is_ignored_name(&self, name: &str, is_dir: bool) -> bool {
        self.noise.iter().any(|pattern| glob_match(pattern, name))
            || if is_dir {
                self.dirs.iter().any(|pattern| glob_match(pattern, name))
            } else {
                self.files.iter().any(|pattern| glob_match(pattern, name))
            }
    }
}

/// Load configured ignore entries and merge them with the built-in list.
pub(crate) fn configured_ignore_patterns() -> IgnorePatterns {
    IgnorePatterns::from_config(&config::filters())
}

/// Match a filename against a glob pattern (supports `*` and `?`).
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), name.as_bytes())
}

fn glob_match_inner(pat: &[u8], name: &[u8]) -> bool {
    match (pat.first(), name.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            // '*' matches zero or more characters
            glob_match_inner(&pat[1..], name)
                || (!name.is_empty() && glob_match_inner(pat, &name[1..]))
        }
        (Some(b'?'), Some(_)) => glob_match_inner(&pat[1..], &name[1..]),
        (Some(&p), Some(&n)) if p == n => glob_match_inner(&pat[1..], &name[1..]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_patterns_apply_builtin_noise_and_type_specific_config() {
        let filters = FilterConfig {
            ignore_dirs: vec!["canary".to_string()],
            ignore_files: vec!["*.lock".to_string()],
        };

        let patterns = IgnorePatterns::from_config(&filters);

        assert!(patterns.is_ignored_name("canary", true));
        assert!(!patterns.is_ignored_name("canary", false));
        assert!(patterns.is_ignored_name("package.lock", false));
        assert!(!patterns.is_ignored_name("package.lock", true));
        assert!(patterns.is_ignored_name("node_modules", true));
        assert!(patterns.is_ignored_name(".DS_Store", false));
    }

    #[test]
    fn ignore_patterns_load_real_config_toml() {
        use std::io::Write;

        let mut file = tempfile::NamedTempFile::new().expect("temp config");
        writeln!(
            file,
            "[filters]\nignore_dirs = [\"canary\"]\nignore_files = [\"*.lock\"]"
        )
        .expect("write temp config");

        let filters = config::filters_at(file.path());
        let patterns = IgnorePatterns::from_config(&filters);

        assert!(patterns.is_ignored_name("canary", true));
        assert!(!patterns.is_ignored_name("canary", false));
        assert!(patterns.is_ignored_name("package.lock", false));
        assert!(!patterns.is_ignored_name("package.lock", true));
    }
}
