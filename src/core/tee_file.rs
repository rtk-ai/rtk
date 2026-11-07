//! Legacy file-based recovery ("tee" mode) — may be deprecated. Prefer the
//! sqlite recall store (`[retriever] mode = "sqlite"`); see retriever.rs.

use crate::core::config::Config;
use crate::core::retriever::RetrieverConfig;
use crate::core::user_dirs;
use crate::core::user_env;
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

pub const LEGACY_TEE_MERGED_NOTICE: &str = "Legacy [tee] values are still applied alongside your [retriever] section.\nMove them to [retriever] (tee_max_files, tee_max_file_size, tee_directory)\nand remove the [tee] section.";

pub fn legacy_tee_fields_merged_in_use() -> bool {
    Config::load()
        .map(|c| c.legacy_tee_fields_merged)
        .unwrap_or(false)
}

pub(crate) fn resolved_tee_dir() -> Option<PathBuf> {
    let cfg = Config::load().ok()?.retriever;
    get_tee_dir(&cfg)
}

/// In a test build `RTK_TEE_DIR` is only what the test itself set, never an
/// exported one — that names the developer's own spool — and `tee_directory`
/// comes from the test's own configuration. `cleanup_old_files` deletes from
/// whatever this returns, so a test that sets either points it at a directory
/// of its own.
pub(crate) fn get_tee_dir(cfg: &RetrieverConfig) -> Option<PathBuf> {
    if let Some(dir) = user_env::var("RTK_TEE_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(ref dir) = cfg.tee_directory {
        return Some(dir.clone());
    }
    super::user_dirs::data().map(|d| d.join("tee"))
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
    let slug = sanitize_slug(slug);
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let filename = format!("{}_{}.log", epoch, slug);
    write_named_tee_file(raw, &filename, dir, max_file_size, max_files)
}

fn capped_content(raw: &str, max_file_size: usize) -> String {
    if raw.len() <= max_file_size {
        return raw.to_string();
    }

    let marker = |omitted| format!("\n\n--- {omitted} bytes omitted from middle ---\n\n");
    // The original length bounds the digit count even after UTF-8 rounding.
    let marker_size = marker(raw.len()).len();
    let Some(content_budget) = max_file_size.checked_sub(marker_size) else {
        // A tiny cap cannot hold an honest marker. Prefer the verdict over a
        // partial marker, and never exceed the configured on-disk byte limit.
        let mut start = raw.len() - max_file_size;
        while !raw.is_char_boundary(start) {
            start += 1;
        }
        return raw[start..].to_string();
    };

    // Keep the prologue, but give build/test summaries three quarters of the
    // remaining budget. Round inward so multibyte text cannot exceed the cap.
    let head_budget = content_budget / 4;
    let mut head_end = head_budget;
    while !raw.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = raw.len() - (content_budget - head_budget);
    while !raw.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!(
        "{}{}{}",
        &raw[..head_end],
        marker(tail_start - head_end),
        &raw[tail_start..]
    )
}

pub(crate) fn write_named_tee_file(
    raw: &str,
    filename: &str,
    dir: &Path,
    max_file_size: usize,
    max_files: usize,
) -> Option<PathBuf> {
    create_tee_dir(dir)?;
    let filepath = dir.join(filename);
    let content = capped_content(raw, max_file_size);
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
    if let Some(home) = user_dirs::home()
        && let Ok(relative) = path.strip_prefix(&home)
    {
        return format!("~/{}", relative.display());
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

pub(crate) fn format_hint(path: &Path) -> String {
    format!("[full output: {}]", display_shell_path(path))
}

pub fn tee_and_hint(cfg: &RetrieverConfig, raw: &str, slug: &str) -> Option<String> {
    let path = write(cfg, raw, slug)?;
    Some(format_hint(&path))
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
    // Middle elision changes line numbers; the original offset can skip the
    // retained tail entirely. Read the whole capped file in that case.
    let line_offset = if content.len() > cfg.tee_max_file_size {
        1
    } else {
        line_offset
    };
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
        let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        for cap in [MAX_FILE_SIZE, 100, 4] {
            let path = write_tee_file(&"secret output\n".repeat(100), "grep", &tee_dir, cap, 20)
                .expect("tee file written");
            assert_eq!(mode(&path), 0o600, "tee file must be owner-only");
            assert_eq!(mode(&tee_dir), 0o700, "tee dir must be owner-only");
        }
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
        let big_output = format!(
            "build started\n{}\nerror: final test failed\n",
            "x".repeat(2000)
        );
        let result = write_tee_file(&big_output, "test", tmpdir.path(), 1000, 20);
        let path = result.expect("tee file written");
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("build started\n"));
        assert!(content.ends_with("\nerror: final test failed\n"));
        assert!(content.len() <= 1000, "marker must fit within the cap");
        assert_omitted_bytes(&big_output, &content);
        let (head, _) = content.split_once("\n\n--- ").unwrap();
        let (_, tail) = content.rsplit_once(" ---\n\n").unwrap();
        assert!(tail.len() >= 3 * head.len());
    }

    fn assert_omitted_bytes(raw: &str, content: &str) {
        let (head, rest) = content.split_once("\n\n--- ").expect("omission marker");
        let (omitted, tail) = rest
            .split_once(" bytes omitted from middle ---\n\n")
            .expect("complete omission marker");
        assert!(raw.starts_with(head));
        assert!(raw.ends_with(tail));
        assert!(tail.len() >= head.len(), "retain more of the failure tail");
        assert_eq!(
            omitted.parse::<usize>().unwrap(),
            raw.len() - head.len() - tail.len()
        );
    }

    #[test]
    fn test_write_tee_file_truncation_utf8_boundary() {
        let tmpdir = tempfile::tempdir().unwrap();
        for raw in ["\u{6F22}".repeat(400), "\u{1F600}".repeat(400)] {
            // Vary both cuts across every byte of three- and four-byte characters.
            for cap in 980..1000 {
                let path = write_tee_file(&raw, "utf8", tmpdir.path(), cap, 20).unwrap();
                let content = fs::read_to_string(&path).expect("valid UTF-8 at both cuts");
                assert!(content.len() <= cap);
                assert_omitted_bytes(&raw, &content);
                assert!(content.starts_with(raw.chars().next().unwrap()));
                assert!(content.ends_with(raw.chars().last().unwrap()));
            }
        }
    }

    #[test]
    fn test_write_tee_file_at_or_below_cap_is_unchanged() {
        let tmpdir = tempfile::tempdir().unwrap();
        for raw in ["", "short output\n", "\u{6F22}\u{1F600}"] {
            for cap in [raw.len(), raw.len() + 1, usize::MAX] {
                let path = write_tee_file(raw, "unchanged", tmpdir.path(), cap, 20).unwrap();
                assert_eq!(fs::read_to_string(path).unwrap(), raw);
            }
        }
    }

    #[test]
    fn test_write_tee_file_tiny_caps_keep_utf8_tail() {
        let tmpdir = tempfile::tempdir().unwrap();
        let raw = format!("{}\u{6F22}\u{1F600}", "x".repeat(100));
        for (cap, expected) in [
            (0, ""),
            (1, ""),
            (3, ""),
            (4, "\u{1F600}"),
            (6, "\u{1F600}"),
            (7, "\u{6F22}\u{1F600}"),
        ] {
            let path = write_tee_file(&raw, "tiny", tmpdir.path(), cap, 20).unwrap();
            let content = fs::read_to_string(path).unwrap();
            assert_eq!(content, expected, "cap {cap}");
            assert!(content.len() <= cap);
        }
    }

    #[test]
    fn test_write_tee_file_marker_budget_and_byte_counts() {
        let tmpdir = tempfile::tempdir().unwrap();
        for size in [99, 100, 999, 1000, 9999, 10000] {
            let raw = "x".repeat(size);
            let marker = format!("\n\n--- {size} bytes omitted from middle ---\n\n");
            for cap in [
                marker.len() - 1,
                marker.len(),
                marker.len() + 1,
                marker.len() + 17,
            ] {
                let path = write_tee_file(&raw, "budget", tmpdir.path(), cap, 20).unwrap();
                let content = fs::read_to_string(path).unwrap();
                assert!(content.len() <= cap, "size {size}, cap {cap}");
                if cap < marker.len() {
                    assert_eq!(content, raw[raw.len() - cap..]);
                } else {
                    assert_omitted_bytes(&raw, &content);
                }
            }
        }
    }

    #[test]
    fn test_truncated_tee_files_still_rotate() {
        let tmpdir = tempfile::tempdir().unwrap();
        let raw = format!("{}\nfinal failure\n", "x".repeat(2000));
        for i in 0..3 {
            let filename = format!("100000000{i}_test.log");
            let path = write_named_tee_file(&raw, &filename, tmpdir.path(), 100, 2).unwrap();
            let content = fs::read_to_string(path).unwrap();
            assert!(content.len() <= 100);
            assert!(content.ends_with("\nfinal failure\n"));
        }
        assert!(!tmpdir.path().join("1000000000_test.log").exists());
        assert!(tmpdir.path().join("1000000001_test.log").exists());
        assert!(tmpdir.path().join("1000000002_test.log").exists());
        assert_eq!(fs::read_dir(tmpdir.path()).unwrap().count(), 2);
    }

    #[test]
    fn test_truncated_tail_hint_reads_the_capped_file_from_the_start() {
        let tmpdir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            tee_directory: Some(tmpdir.path().to_path_buf()),
            tee_max_file_size: 250,
            ..Default::default()
        };
        let mut raw: String = (1..=40).map(|n| format!("item {n:02}\n")).collect();
        raw.push_str(&format!("final item: {}\n", "x".repeat(1000)));
        let hint = force_tee_tail_hint(&cfg, &raw, "list", 21).unwrap();
        let path = fs::read_dir(tmpdir.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let written = fs::read_to_string(&path).unwrap();
        assert!(
            written.lines().count() < 21,
            "original offset no longer exists"
        );
        assert_eq!(
            hint,
            format!("[see remaining: tail -n +1 {}]", display_shell_path(&path))
        );
        assert!(written.ends_with(&format!("{}\n", "x".repeat(100))));
    }

    #[test]
    fn test_unchanged_tail_hint_keeps_the_requested_offset() {
        let tmpdir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            tee_directory: Some(tmpdir.path().to_path_buf()),
            tee_max_file_size: 250,
            ..Default::default()
        };
        let raw = "first\nsecond\nthird\n";
        let hint = force_tee_tail_hint(&cfg, raw, "list", 3).unwrap();
        assert!(hint.starts_with("[see remaining: tail -n +3 "));
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
        let Some(home) = user_dirs::home() else {
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
