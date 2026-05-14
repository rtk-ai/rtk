//! Content hint system - generic output saving with hint recovery.
//! Replaces and extends the former tee.rs functionality.
//!
//! This module provides a generic way to save content to disk and optionally
//! return a hint string that can be displayed to the user.
//! Used by: JSON truncation, command output recovery, etc.

use super::constants::RTK_DATA_DIR;
use crate::core::config::Config;
use std::path::PathBuf;

/// Minimum output size to save (smaller outputs don't need recovery)
pub const MIN_CONTENT_SIZE: usize = 500;

/// Default max files to keep
pub const DEFAULT_MAX_FILES: usize = 20;

/// Default max file size (1MB)
pub const DEFAULT_MAX_FILE_SIZE: usize = 1_048_576;

/// Sanitize a command slug for use in filenames.
/// Replaces non-alphanumeric chars (except underscore/hyphen) with underscore,
/// truncates at 40 chars.
pub fn sanitize_slug(slug: &str) -> String {
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
    if sanitized.len() > 40 {
        sanitized[..40].to_string()
    } else {
        sanitized
    }
}

/// Get the tee/hint directory, respecting config and env overrides.
pub fn get_hint_dir(config: &Config) -> Option<PathBuf> {
    // Env var override
    if let Ok(dir) = std::env::var("RTK_TEE_DIR") {
        return Some(PathBuf::from(dir));
    }

    // Config override
    if let Some(ref dir) = config.tee.directory {
        return Some(dir.clone());
    }

    // Default: ~/.local/share/rtk/tee/
    dirs::data_local_dir().map(|d| d.join(RTK_DATA_DIR).join("tee"))
}

/// Rotate old files: keep only the last `max_files`, delete oldest.
/// Filters by extension (e.g., ".log", ".json").
fn cleanup_old_files(dir: &std::path::Path, ext: &str, max_files: usize) {
    let ext_str = ext.trim_start_matches('.').to_string();
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext.to_string_lossy() == ext_str)
        })
        .collect();

    if entries.len() <= max_files {
        return;
    }

    // Sort by filename (which starts with epoch timestamp = chronological)
    entries.sort_by_key(|e| e.file_name());

    let to_remove = entries.len() - max_files;
    for entry in entries.iter().take(to_remove) {
        let _ = std::fs::remove_file(entry.path());
    }
}

/// Write content to a file in the given directory.
/// Returns file path on success.
/// - `content`: the content to save
/// - `slug`: identifier for the file (e.g., "json", "cargo_test")
/// - `ext`: extension including dot (e.g., ".json", ".log")
/// - `max_file_size`: maximum size before truncation
/// - `max_files`: maximum number of files to keep
fn write_content_file(
    content: &str,
    slug: &str,
    dir: &std::path::Path,
    ext: &str,
    max_file_size: usize,
    max_files: usize,
) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;

    let sanitized_slug = sanitize_slug(slug);
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let filename = format!("{}_{}{}", epoch, sanitized_slug, ext);
    let filepath = dir.join(filename);

    // Truncate at max_file_size (find a safe UTF-8 char boundary)
    let content_to_write = if content.len() > max_file_size {
        let boundary = content
            .char_indices()
            .take_while(|(i, _)| *i < max_file_size)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!(
            "{}\n\n--- truncated at {} bytes ---",
            &content[..boundary],
            max_file_size
        )
    } else {
        content.to_string()
    };

    std::fs::write(&filepath, content_to_write).ok()?;

    // Rotate old files (filter by extension)
    cleanup_old_files(dir, ext, max_files);

    Some(filepath)
}

pub fn display_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(relative) = path.strip_prefix(&home) {
            return format!("~/{}", relative.display());
        }
    }
    path.display().to_string()
}

pub fn format_hint(path: &std::path::Path) -> String {
    format!("[full output: {}]", display_path(path))
}

/// Check if hints are disabled via environment variable.
/// Supports both RTK_TEE=0 (backward compat) and RTK_HINTS=0.
fn hints_disabled() -> bool {
    // Check RTK_TEE=0 (backward compatibility)
    if std::env::var("RTK_TEE").ok().as_deref() == Some("0") {
        return true;
    }
    // Check RTK_HINTS=0 (new, more general)
    if std::env::var("RTK_HINTS").ok().as_deref() == Some("0") {
        return true;
    }
    false
}

/// Save content to file and return the file path.
/// The caller is responsible for checking conditions (size, etc.).
/// Returns None if save fails or hints are disabled.
pub fn save_output(content: &str, slug: &str, ext: &str) -> Option<PathBuf> {
    // Check env override (disable)
    if hints_disabled() {
        return None;
    }

    if content.is_empty() {
        return None;
    }

    let config = Config::load().ok()?;

    if !config.tee.enabled {
        return None;
    }

    let hint_dir = get_hint_dir(&config)?;
    let hint_dir = std::fs::create_dir_all(&hint_dir)
        .ok()
        .and(Some(hint_dir))?;

    write_content_file(
        content,
        slug,
        &hint_dir,
        ext,
        config.tee.max_file_size,
        config.tee.max_files,
    )
}

/// Save content to file and return the hint string.
/// Convenience wrapper combining save_output + format_hint.
/// Returns None if save fails or hints are disabled.
pub fn save_output_and_hint(content: &str, slug: &str, ext: &str) -> Option<String> {
    let path = save_output(content, slug, ext)?;
    Some(format_hint(&path))
}

/// Save content to file only if exit_code != 0.
/// Convenience for "save on failure" pattern.
/// Returns None if save fails, skipped, or exit_code == 0.
pub fn save_output_on_failure(
    content: &str,
    slug: &str,
    ext: &str,
    exit_code: i32,
) -> Option<String> {
    // Check env override (disable)
    if hints_disabled() {
        return None;
    }

    // Only save on failure
    if exit_code == 0 {
        return None;
    }

    let config = Config::load().ok()?;

    // Check config
    if !config.tee.enabled {
        return None;
    }

    match config.tee.mode {
        super::tee::TeeMode::Never => return None,
        super::tee::TeeMode::Always => {}
        super::tee::TeeMode::Failures => {
            // Already checked exit_code != 0 above
        }
    }

    // Skip if output too small
    if content.len() < MIN_CONTENT_SIZE {
        return None;
    }

    let hint_dir = get_hint_dir(&config)?;
    let hint_dir = std::fs::create_dir_all(&hint_dir)
        .ok()
        .and(Some(hint_dir))?;

    let path = write_content_file(
        content,
        slug,
        &hint_dir,
        ext,
        config.tee.max_file_size,
        config.tee.max_files,
    )?;

    Some(format_hint(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_sanitize_slug() {
        assert_eq!(sanitize_slug("cargo_test"), "cargo_test");
        assert_eq!(sanitize_slug("cargo test"), "cargo_test");
        assert_eq!(sanitize_slug("cargo-test"), "cargo-test");
        assert_eq!(sanitize_slug("go/test/./pkg"), "go_test___pkg");
        let long = "a".repeat(50);
        assert_eq!(sanitize_slug(&long).len(), 40);
    }

    #[test]
    fn test_format_hint() {
        let path = PathBuf::from("/tmp/rtk/tee/1234567890_json.json");
        let hint = format_hint(&path);
        assert!(hint.starts_with("[full output: "));
        assert!(hint.ends_with(']'));
        assert!(hint.contains("1234567890_json.json"));
    }

    #[test]
    fn test_cleanup_old_files_by_extension() {
        let tmpdir = tempfile::tempdir().unwrap();
        let dir = tmpdir.path();

        // Create 25 .json files
        for i in 0..25 {
            let filename = format!("{:010}_{}.json", 1000000 + i, "test");
            fs::write(dir.join(&filename), "content").unwrap();
        }

        cleanup_old_files(dir, ".json", 20);

        let remaining: Vec<_> = fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(remaining.len(), 20);

        // Oldest 5 should be removed
        for i in 0..5 {
            let filename = format!("{:010}_{}.json", 1000000 + i, "test");
            assert!(!dir.join(&filename).exists());
        }
    }

    #[test]
    fn test_write_content_file_creates_file() {
        let tmpdir = tempfile::tempdir().unwrap();
        let content = "error: test failed\n".repeat(50);

        let result = write_content_file(
            &content,
            "cargo_test",
            tmpdir.path(),
            ".log",
            DEFAULT_MAX_FILE_SIZE,
            20,
        );
        assert!(result.is_some());

        let path = result.unwrap();
        assert!(path.exists());
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("error: test failed"));
    }

    #[test]
    fn test_write_content_file_truncation() {
        let tmpdir = tempfile::tempdir().unwrap();
        let big_output = "x".repeat(2000);
        // Set max_file_size to 1000 bytes
        let result = write_content_file(&big_output, "test", tmpdir.path(), ".log", 1000, 20);
        assert!(result.is_some());

        let path = result.unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("--- truncated at 1000 bytes ---"));
        assert!(content.len() < 2000);
    }

    #[test]
    fn test_write_content_file_truncation_utf8_boundary() {
        let tmpdir = tempfile::tempdir().unwrap();
        // Create a string where the truncation point falls inside a multi-byte char.
        // Japanese chars are 3 bytes each in UTF-8.
        // 332 chars * 3 bytes = 996 bytes, then one more = 999 bytes.
        // With max_file_size=998, the cut falls mid-character.
        let japanese = "\u{6F22}".repeat(333); // 999 bytes of 3-byte chars
        assert_eq!(japanese.len(), 999);

        // Truncate at 998 — falls in the middle of the 333rd character
        let result = write_content_file(&japanese, "test_utf8", tmpdir.path(), ".log", 998, 20);
        assert!(result.is_some());

        let path = result.unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("--- truncated at 998 bytes ---"));
        // Should contain 332 full characters (996 bytes), not panic
        assert!(content.starts_with(&"\u{6F22}".repeat(332)));
    }

    #[test]
    fn test_write_content_file_truncation_emoji() {
        let tmpdir = tempfile::tempdir().unwrap();
        // Emoji are 4 bytes each in UTF-8
        let emojis = "\u{1F600}".repeat(100); // 400 bytes
        assert_eq!(emojis.len(), 400);

        // Truncate at 201 — falls mid-emoji (4-byte boundary is at 200, 204)
        let result = write_content_file(&emojis, "test_emoji", tmpdir.path(), ".log", 201, 20);
        assert!(result.is_some());

        let path = result.unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("--- truncated at 201 bytes ---"));
        // The emoji portion should be exactly 200 bytes (50 emojis),
        // rounded down from 201 to the nearest char boundary
        let target = "\u{1F600}".repeat(50);
        assert!(content.starts_with(&target));
    }
}
