//! Legacy file-based recovery ("tee" mode) — may be deprecated. Prefer the
//! sqlite recall store (`[retriever] mode = "sqlite"`); see retriever.rs.

use crate::core::config::Config;
use crate::core::constants::RTK_DATA_DIR;
use crate::core::retriever::RetrieverConfig;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Sanitize a command slug for use in filenames.
/// Replaces non-alphanumeric chars (except underscore/hyphen) with underscore.
/// Long slugs (usually an embedded file path that duplicates the command the LLM
/// already issued) collapse to a short readable prefix plus a short disambiguating
/// hash, keeping recovery filenames unique but compact — fewer tokens per tee hint.
pub(crate) fn sanitize_slug(slug: &str) -> String {
    let sanitized: String = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    const MAX_READABLE: usize = 24;
    if sanitized.len() <= MAX_READABLE {
        return sanitized;
    }
    let prefix: String = sanitized.chars().take(8).collect();
    format!("{}_{}", prefix, short_hash(&sanitized))
}

/// First 6 hex chars (24 bits) of the SHA-256 of `s` — a compact tag to keep
/// shortened slugs distinct. Not collision-resistant on its own: 24 bits hits a
/// birthday collision after only a few thousand distinct slugs. It's safe here
/// because a clash also requires the identical readable prefix *and* the same
/// epoch second, which together scope tee writes exactly as before.
fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(s.as_bytes()))[..6].to_string()
}

const LEGACY_ACTIVITY_WINDOW_SECS: u64 = 30 * 86_400;

fn has_recent_logs(dir: &Path, now: SystemTime) -> bool {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
        .filter_map(|e| e.metadata().ok())
        .filter_map(|m| m.modified().ok())
        .any(|mtime| {
            now.duration_since(mtime)
                .map(|age| age.as_secs() < LEGACY_ACTIVITY_WINDOW_SECS)
                .unwrap_or(false)
        })
}

pub fn legacy_tee_migration_pending() -> bool {
    let Ok(config) = Config::load() else {
        return false;
    };
    if config.retriever.mode != crate::core::retriever::RecoveryMode::Sqlite {
        return false;
    }
    let Some(dir) = get_tee_dir(&config.retriever) else {
        return false;
    };
    has_recent_logs(&dir, SystemTime::now())
}

pub const LEGACY_TEE_NOTICE: &str = "Recovery hints moved to a sqlite store: read them with `rtk recall <hash>`.\nRecent tee .log files were found; to keep the legacy file mode, run\n`rtk config recall tee`.";

pub const LEGACY_TEE_CONFIG_NOTICE: &str = "Legacy [tee] config detected — file mode kept. A sqlite recall store is now\navailable: run `rtk config recall sqlite` to adopt it (hints become\n`rtk recall <hash>`, see `rtk gain --recalls`).";

pub fn legacy_tee_config_in_use() -> bool {
    Config::load()
        .map(|c| c.migrated_from_legacy_tee)
        .unwrap_or(false)
}

pub(crate) fn resolved_tee_dir() -> Option<PathBuf> {
    let cfg = Config::load().ok()?.retriever;
    get_tee_dir(&cfg)
}

fn get_tee_dir(cfg: &RetrieverConfig) -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("RTK_TEE_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(ref dir) = cfg.tee_directory {
        return Some(dir.clone());
    }
    dirs::data_local_dir().map(|d| d.join(RTK_DATA_DIR).join("tee"))
}

fn cleanup_old_files(dir: &Path, max_files: usize) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
        .collect();
    if entries.len() <= max_files {
        return;
    }
    entries.sort_by_key(|e| e.file_name());
    let to_remove = entries.len() - max_files;
    for entry in entries.iter().take(to_remove) {
        // nosemgrep: filesystem-deletion
        let _ = std::fs::remove_file(entry.path());
    }
}

fn create_tee_dir(tee_dir: &Path) -> Option<()> {
    if let Some(parent) = tee_dir.parent() {
        let _ = crate::core::utils::create_private_dir(parent);
    }
    crate::core::utils::create_private_dir(tee_dir).ok()
}

fn write_tee_file(
    raw: &str,
    slug: &str,
    dir: &Path,
    max_file_size: usize,
    max_files: usize,
) -> Option<PathBuf> {
    create_tee_dir(dir)?;
    let slug = sanitize_slug(slug);
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let filepath = dir.join(format!("{}_{}.log", epoch, slug));
    let content = if raw.len() > max_file_size {
        let boundary = raw
            .char_indices()
            .take_while(|(i, _)| *i < max_file_size)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!(
            "{}\n\n--- truncated at {} bytes ---",
            &raw[..boundary],
            max_file_size
        )
    } else {
        raw.to_string()
    };
    let mut file = crate::core::utils::open_private(
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true),
        &filepath,
    )
    .ok()?;
    use std::io::Write;
    file.write_all(content.as_bytes()).ok()?;
    cleanup_old_files(dir, max_files);
    Some(filepath)
}

fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(relative) = path.strip_prefix(&home) {
            return format!("~/{}", relative.display());
        }
    }
    path.display().to_string()
}

fn needs_shell_quoting(path: &str) -> bool {
    path.chars().any(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '\'' | '"'
                    | '\\'
                    | '$'
                    | '`'
                    | '!'
                    | '#'
                    | '&'
                    | '('
                    | ')'
                    | ';'
                    | '<'
                    | '>'
                    | '?'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '|'
                    | '*'
            )
    })
}

fn escape_double_quoted_path(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn display_shell_path(path: &Path) -> String {
    let display = display_path(path);
    if !needs_shell_quoting(&display) {
        return display;
    }

    if let Some(relative) = display.strip_prefix("~/") {
        let relative = relative.replace(std::path::MAIN_SEPARATOR, "/");
        return format!("\"$HOME/{}\"", escape_double_quoted_path(&relative));
    }

    format!("\"{}\"", escape_double_quoted_path(&display))
}

fn write(cfg: &RetrieverConfig, content: &str, slug: &str) -> Option<PathBuf> {
    let dir = get_tee_dir(cfg)?;
    write_tee_file(
        content,
        slug,
        &dir,
        cfg.tee_max_file_size,
        cfg.tee_max_files,
    )
}

pub fn tee_and_hint(cfg: &RetrieverConfig, raw: &str, slug: &str) -> Option<String> {
    let path = write(cfg, raw, slug)?;
    Some(format!("[full output: {}]", display_shell_path(&path)))
}

pub fn force_tee_hint(cfg: &RetrieverConfig, content: &str, slug: &str) -> Option<String> {
    let path = write(cfg, content, slug)?;
    Some(format!("[full output: {}]", display_shell_path(&path)))
}

pub fn force_tee_tail_hint(
    cfg: &RetrieverConfig,
    content: &str,
    slug: &str,
    line_offset: usize,
) -> Option<String> {
    let path = write(cfg, content, slug)?;
    Some(format!(
        "[see remaining: tail -n +{} {}]",
        line_offset,
        display_shell_path(&path)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const MAX_FILE_SIZE: usize = 1_048_576;

    #[test]
    fn test_sanitize_slug() {
        assert_eq!(sanitize_slug("cargo_test"), "cargo_test");
        assert_eq!(sanitize_slug("cargo test"), "cargo_test");
        assert_eq!(sanitize_slug("cargo-test"), "cargo-test");
        assert_eq!(sanitize_slug("go/test/./pkg"), "go_test___pkg");
        let long = format!("grep_0_{}", "a".repeat(50));
        let short = sanitize_slug(&long);
        assert!(short.len() < 24, "long slug should shorten, got '{short}'");
        assert!(
            short.starts_with("grep_0_a"),
            "keeps a readable prefix, got '{short}'"
        );
        assert_eq!(sanitize_slug(&long), short);
        let other = sanitize_slug(&format!("grep_1_{}", "a".repeat(50)));
        assert_ne!(other, short, "distinct slugs must not collide");
    }

    #[test]
    fn test_write_tee_file_creates_file() {
        let tmpdir = tempfile::tempdir().unwrap();
        let content = "error: test failed\n".repeat(50);
        let result = write_tee_file(&content, "cargo_test", tmpdir.path(), MAX_FILE_SIZE, 20);
        let path = result.expect("tee file written");
        assert!(path.exists());
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("error: test failed"));
    }

    #[test]
    #[cfg(unix)]
    fn test_write_tee_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let tmpdir = tempfile::tempdir().unwrap();
        let tee_dir = tmpdir.path().join("tee");
        let path = write_tee_file("secret output\n", "grep", &tee_dir, MAX_FILE_SIZE, 20)
            .expect("tee file written");

        let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600, "tee file must be owner-only");
        assert_eq!(mode(&tee_dir), 0o700, "tee dir must be owner-only");
    }

    #[test]
    #[cfg(unix)]
    #[allow(unsafe_code)]
    fn test_write_tee_file_owner_only_under_permissive_umask() {
        use std::os::unix::fs::PermissionsExt;

        // nosemgrep: unsafe-block
        let previous = unsafe { libc::umask(0o000) };
        let tmpdir = tempfile::tempdir().unwrap();
        let tee_dir = tmpdir.path().join("tee");
        let written = write_tee_file("secret\n", "grep", &tee_dir, MAX_FILE_SIZE, 20);
        // nosemgrep: unsafe-block
        unsafe { libc::umask(previous) };

        let path = written.expect("tee file written");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "umask 000 must not widen the tee file");
    }

    #[test]
    fn test_write_tee_file_truncation() {
        let tmpdir = tempfile::tempdir().unwrap();
        let big_output = "x".repeat(2000);
        let result = write_tee_file(&big_output, "test", tmpdir.path(), 1000, 20);
        let path = result.expect("tee file written");
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("--- truncated at 1000 bytes ---"));
        assert!(content.len() < 2000);
    }

    #[test]
    fn test_write_tee_file_truncation_utf8_boundary() {
        let tmpdir = tempfile::tempdir().unwrap();
        let japanese = "\u{6F22}".repeat(333);
        assert_eq!(japanese.len(), 999);
        let result = write_tee_file(&japanese, "test_utf8", tmpdir.path(), 998, 20);
        let path = result.expect("tee file written");
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("--- truncated at 998 bytes ---"));
        assert!(content.starts_with(&"\u{6F22}".repeat(332)));
    }

    #[test]
    fn test_cleanup_old_files() {
        let tmpdir = tempfile::tempdir().unwrap();
        let dir = tmpdir.path();
        for i in 0..25 {
            let filename = format!("{:010}_{}.log", 1000000 + i, "test");
            fs::write(dir.join(&filename), "content").unwrap();
        }
        cleanup_old_files(dir, 20);
        let remaining: Vec<_> = fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(remaining.len(), 20);
        for i in 0..5 {
            let filename = format!("{:010}_{}.log", 1000000 + i, "test");
            assert!(!dir.join(&filename).exists());
        }
    }

    #[test]
    fn test_has_recent_logs_detects_fresh_log() {
        let tmpdir = tempfile::tempdir().unwrap();
        fs::write(tmpdir.path().join("1755590000_cargo_test.log"), "x").unwrap();
        assert!(has_recent_logs(tmpdir.path(), SystemTime::now()));
    }

    #[test]
    fn test_has_recent_logs_ignores_old_logs() {
        let tmpdir = tempfile::tempdir().unwrap();
        fs::write(tmpdir.path().join("1_old.log"), "x").unwrap();
        let future = SystemTime::now() + std::time::Duration::from_secs(31 * 86_400);
        assert!(!has_recent_logs(tmpdir.path(), future));
    }

    #[test]
    fn test_has_recent_logs_ignores_non_log_files_and_missing_dir() {
        let tmpdir = tempfile::tempdir().unwrap();
        fs::write(tmpdir.path().join("notes.txt"), "x").unwrap();
        assert!(!has_recent_logs(tmpdir.path(), SystemTime::now()));
        assert!(!has_recent_logs(
            &tmpdir.path().join("missing"),
            SystemTime::now()
        ));
    }

    #[test]
    fn test_display_shell_path_preserves_simple_paths() {
        let path = PathBuf::from("/tmp/rtk/tee/123_cargo_test.log");
        assert_eq!(display_shell_path(&path), "/tmp/rtk/tee/123_cargo_test.log");
    }

    #[test]
    fn test_display_shell_path_quotes_paths_with_spaces() {
        let path = PathBuf::from("/tmp/rtk/Application Support/123_go_test.log");
        assert_eq!(
            display_shell_path(&path),
            "\"/tmp/rtk/Application Support/123_go_test.log\""
        );
    }

    #[test]
    fn test_display_shell_path_quotes_backslashes() {
        let path = PathBuf::from(r"/tmp/rtk/tee/path\segment.log");
        assert_eq!(
            display_shell_path(&path),
            r#""/tmp/rtk/tee/path\\segment.log""#
        );
    }

    #[test]
    fn test_display_shell_path_uses_home_var_for_home_paths_with_spaces() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let path = home
            .join("Library")
            .join("Application Support")
            .join("rtk")
            .join("tee")
            .join("123_go_test.log");

        assert_eq!(
            display_shell_path(&path),
            "\"$HOME/Library/Application Support/rtk/tee/123_go_test.log\""
        );
    }
}
