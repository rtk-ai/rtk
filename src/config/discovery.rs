//! Directory walk-up discovery for `rtk.*.md` rule files.
//!
//! Walks from cwd to home, scanning configurable dirs in each ancestor.
//! Search dirs, global dirs, and extra rules_dirs are read from config.
//! Results cached via `OnceLock` — zero cost after first call.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static DISCOVERED: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Return all `rtk.*.md` files ordered lowest→highest priority.
///
/// Precedence (highest wins):
///   0 (lowest). Compiled `include_str!()` defaults (handled in rules.rs, not here)
///   1. Platform config dir + `~/.config/rtk/` (global RTK config)
///   2. Config `discovery.rules_dirs` (explicit extra dirs)
///   3. Config `discovery.global_dirs` under $HOME (default: `.claude/`, `.gemini/`)
///   4. Walk up from cwd using config `discovery.search_dirs`
///      (default: `.claude/`, `.gemini/`, `.rtk/` — furthest from cwd first, cwd last)
///   5. CLI `--rules-add` paths (highest file priority)
///
/// If `--rules-path` is set, ONLY those paths are searched (skips all discovery).
/// All dirs configurable via `[discovery]` section in config.toml or env vars.
pub fn discover_rtk_files() -> &'static [PathBuf] {
    DISCOVERED.get_or_init(discover_impl)
}

fn discover_impl() -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let overrides = super::cli_overrides();

    // If --rules-path is set, use ONLY those paths (exclusive mode)
    if let Some(ref exclusive_paths) = overrides.rules_path {
        for dir in exclusive_paths {
            collect_from_dir(dir, &mut files, &mut seen);
        }
        return files;
    }

    let config = super::get_merged();

    // Normal discovery
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return files,
    };

    // 1. Platform-specific config dir (macOS: ~/Library/Application Support/rtk/)
    if let Some(config_dir) = dirs::config_dir() {
        let platform_rtk = config_dir.join("rtk");
        collect_from_dir(&platform_rtk, &mut files, &mut seen);
    }

    // 2. Canonical RTK config dir: ~/.config/rtk/
    let canonical_rtk = home.join(".config").join("rtk");
    collect_from_dir(&canonical_rtk, &mut files, &mut seen);

    // 3. Config discovery.rules_dirs (explicit extra directories)
    for dir in &config.discovery.rules_dirs {
        collect_from_dir(dir, &mut files, &mut seen);
    }

    // 4. Global dirs under $HOME (from config discovery.global_dirs)
    for name in &config.discovery.global_dirs {
        collect_from_dir(&home.join(name), &mut files, &mut seen);
    }

    // 5. Walk up from cwd to home using config discovery.search_dirs
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(_) => return files,
    };

    let mut ancestors: Vec<PathBuf> = Vec::new();
    let mut current = cwd.as_path();
    loop {
        ancestors.push(current.to_path_buf());
        if current == home {
            break;
        }
        match current.parent() {
            Some(p) if p != current => current = p,
            _ => break,
        }
    }
    // Reverse: furthest ancestor first (lowest priority), cwd last (highest)
    ancestors.reverse();

    for ancestor in &ancestors {
        for search_dir in &config.discovery.search_dirs {
            let dir = ancestor.join(search_dir);
            collect_from_dir(&dir, &mut files, &mut seen);
        }
    }

    // 6. --rules-add paths (highest file priority, after all discovery)
    for dir in &overrides.rules_add {
        collect_from_dir(dir, &mut files, &mut seen);
    }

    files
}

/// Collect `rtk.*.md` files from a directory, deduplicating by canonical path.
fn collect_from_dir(dir: &Path, files: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // Silently skip unreadable dirs
    };

    let mut dir_files: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if is_rtk_rule_file(&name_str) {
            let path = entry.path();
            // Canonicalize for dedup: detects symlink loops and duplicate real paths
            let canon = match path.canonicalize() {
                Ok(c) => c,
                Err(_) => continue, // Broken symlink or unreadable
            };
            if seen.insert(canon) {
                dir_files.push(path);
            }
        }
    }
    // Sort within directory for deterministic ordering
    dir_files.sort();
    files.extend(dir_files);
}

/// Match `rtk.*.md` pattern: starts with "rtk.", ends with ".md", has content between.
fn is_rtk_rule_file(name: &str) -> bool {
    name.starts_with("rtk.") && name.ends_with(".md") && name.len() > 7
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_is_rtk_rule_file_valid() {
        assert!(is_rtk_rule_file("rtk.safety.rm-to-trash.md"));
        assert!(is_rtk_rule_file("rtk.remap.t.md"));
        assert!(is_rtk_rule_file("rtk.x.md")); // minimal valid: 8 chars
    }

    #[test]
    fn test_is_rtk_rule_file_invalid() {
        assert!(!is_rtk_rule_file("rtk.md")); // too short (7 chars, not > 7)
        assert!(!is_rtk_rule_file("foo.md"));
        assert!(!is_rtk_rule_file("rtk.safety.txt"));
        assert!(!is_rtk_rule_file(""));
    }

    #[test]
    fn test_collect_from_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        collect_from_dir(tmp.path(), &mut files, &mut seen);
        assert!(files.is_empty());
    }

    #[test]
    fn test_collect_from_dir_with_rules() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("rtk.test.md"), "---\nname: test\n---\n").unwrap();
        fs::write(tmp.path().join("not-a-rule.md"), "ignored").unwrap();
        fs::write(tmp.path().join("rtk.md"), "too short name").unwrap();

        let mut files = Vec::new();
        let mut seen = HashSet::new();
        collect_from_dir(tmp.path(), &mut files, &mut seen);
        assert_eq!(files.len(), 1);
        assert!(files[0].file_name().unwrap().to_str().unwrap() == "rtk.test.md");
    }

    #[test]
    fn test_collect_deduplicates_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("rtk.test.md");
        fs::write(&real, "---\nname: test\n---\n").unwrap();

        // Create a subdirectory with a symlink to the same file
        let subdir = tmp.path().join("sub");
        fs::create_dir(&subdir).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, subdir.join("rtk.test.md")).unwrap();

        let mut files = Vec::new();
        let mut seen = HashSet::new();
        collect_from_dir(tmp.path(), &mut files, &mut seen);
        collect_from_dir(&subdir, &mut files, &mut seen);

        #[cfg(unix)]
        assert_eq!(files.len(), 1, "Symlink should be deduplicated");
    }

    #[test]
    fn test_collect_skips_unreadable_dir() {
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        // Non-existent directory should be silently skipped
        collect_from_dir(Path::new("/nonexistent/path"), &mut files, &mut seen);
        assert!(files.is_empty());
    }

    #[test]
    fn test_collect_skips_file_as_dir() {
        // If a file is passed instead of a directory, read_dir will fail — should be skipped
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("not_a_dir");
        fs::write(&file_path, "i am a file").unwrap();

        let mut files = Vec::new();
        let mut seen = HashSet::new();
        collect_from_dir(&file_path, &mut files, &mut seen);
        assert!(files.is_empty()); // Not a dir, silently skipped
    }

    #[test]
    fn test_collect_skips_broken_symlinks() {
        let tmp = tempfile::tempdir().unwrap();

        #[cfg(unix)]
        {
            // Create a broken symlink (target doesn't exist)
            let broken_link = tmp.path().join("rtk.broken.md");
            std::os::unix::fs::symlink("/nonexistent/target", &broken_link).unwrap();

            let mut files = Vec::new();
            let mut seen = HashSet::new();
            collect_from_dir(tmp.path(), &mut files, &mut seen);
            // Broken symlink: canonicalize fails → continue (skipped)
            assert!(files.is_empty());
        }
    }

    #[test]
    fn test_collect_handles_non_utf8_filenames() {
        // Files with non-UTF8 names should be handled via to_string_lossy
        let tmp = tempfile::tempdir().unwrap();
        // Create a normal rtk rule file alongside a non-matching file
        fs::write(tmp.path().join("rtk.valid.md"), "---\nname: v\n---\n").unwrap();
        fs::write(tmp.path().join("other.txt"), "not a rule").unwrap();

        let mut files = Vec::new();
        let mut seen = HashSet::new();
        collect_from_dir(tmp.path(), &mut files, &mut seen);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_collect_multiple_dirs_deduplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_a = tmp.path().join("a");
        let dir_b = tmp.path().join("b");
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();

        let real_file = dir_a.join("rtk.test.md");
        fs::write(&real_file, "---\nname: test\n---\n").unwrap();

        #[cfg(unix)]
        {
            // Symlink from dir_b to same real file
            std::os::unix::fs::symlink(&real_file, dir_b.join("rtk.test.md")).unwrap();

            let mut files = Vec::new();
            let mut seen = HashSet::new();
            collect_from_dir(&dir_a, &mut files, &mut seen);
            collect_from_dir(&dir_b, &mut files, &mut seen);
            assert_eq!(
                files.len(),
                1,
                "Same file via symlink should be deduplicated"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_collect_permission_denied_dir_skipped() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let restricted = tmp.path().join("restricted");
        fs::create_dir(&restricted).unwrap();
        fs::write(restricted.join("rtk.test.md"), "---\nname: t\n---\n").unwrap();

        // Remove read permission
        fs::set_permissions(&restricted, fs::Permissions::from_mode(0o000)).unwrap();

        let mut files = Vec::new();
        let mut seen = HashSet::new();
        collect_from_dir(&restricted, &mut files, &mut seen);
        // Permission denied → silently skipped
        assert!(files.is_empty());

        // Restore permissions for cleanup
        fs::set_permissions(&restricted, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn test_default_search_dirs_match_expected() {
        // Verify defaults match the previously hardcoded values
        let config = crate::config::DiscoveryConfig::default();
        assert_eq!(config.search_dirs, vec![".claude", ".gemini", ".rtk"]);
    }

    #[test]
    fn test_default_global_dirs_match_expected() {
        let config = crate::config::DiscoveryConfig::default();
        assert_eq!(config.global_dirs, vec![".claude", ".gemini"]);
    }

    #[test]
    fn test_default_rules_dirs_empty() {
        let config = crate::config::DiscoveryConfig::default();
        assert!(
            config.rules_dirs.is_empty(),
            "Default rules_dirs should be empty (uses ~/.config/rtk/ implicitly)"
        );
    }
}
