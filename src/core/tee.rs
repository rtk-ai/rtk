//! Raw output recovery -- saves unfiltered output to disk on command failure.

use super::constants::RTK_DATA_DIR;
use crate::core::config::Config;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

pub const OUTPUT_META_PREFIX: &str = "RTK_OUTPUT_META_V1 ";

#[derive(Debug, Clone)]
pub struct TeeArtifact {
    pub path: PathBuf,
    pub complete: bool,
    pub source_bytes: usize,
    pub stored_bytes: usize,
    pub incomplete_reason: Option<&'static str>,
    pub sha256: String,
}

#[derive(serde::Serialize)]
struct OutputArtifactMeta {
    content: &'static str,
    path: String,
    complete: bool,
    source_bytes: usize,
    stored_bytes: usize,
    incomplete_reason: Option<&'static str>,
    sha256: String,
}

#[derive(serde::Serialize)]
struct OutputMeta<'a> {
    schema: &'static str,
    truncated: bool,
    reason: &'a str,
    raw_exit_code: Option<i32>,
    recovery_start_line: Option<usize>,
    shown_records: Option<usize>,
    omitted_records: Option<usize>,
    artifact: Option<OutputArtifactMeta>,
}

/// Minimum output size to tee (smaller outputs don't need recovery)
const MIN_TEE_SIZE: usize = 500;

/// Default max files to keep in tee directory
const DEFAULT_MAX_FILES: usize = 20;

/// Default max file size (1MB)
const DEFAULT_MAX_FILE_SIZE: usize = 1_048_576;

static TEE_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Sanitize a command slug for use in filenames.
/// Replaces non-alphanumeric chars (except underscore/hyphen) with underscore,
/// truncates at 40 chars.
fn sanitize_slug(slug: &str) -> String {
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

/// Get the tee directory, respecting config and env overrides.
fn get_tee_dir(config: &Config) -> Option<PathBuf> {
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

/// Rotate old tee files: keep only the last `max_files`, delete oldest.
fn cleanup_old_files(dir: &std::path::Path, max_files: usize) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|entry| {
            let Ok(file_type) = entry.file_type() else {
                return false;
            };
            if !file_type.is_file() {
                return false;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            let Some(stem) = name.strip_suffix(".log") else {
                return false;
            };
            let mut parts = stem.splitn(4, '_');
            parts
                .by_ref()
                .take(3)
                .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
                && parts.next().is_some_and(|slug| !slug.is_empty())
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

/// Check if tee should be skipped based on config, mode, exit code, and size.
/// Returns None if should skip, Some(tee_dir) if should proceed.
fn should_tee(
    config: &TeeConfig,
    raw_len: usize,
    exit_code: i32,
    tee_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    if !config.enabled {
        return None;
    }

    match config.mode {
        TeeMode::Never => return None,
        TeeMode::Failures => {
            if exit_code == 0 {
                return None;
            }
        }
        TeeMode::Always => {}
    }

    if raw_len < MIN_TEE_SIZE {
        return None;
    }

    tee_dir
}

/// Creates the parent as its own step, otherwise `create_dir_all` leaves the
/// data root at the umask as an intermediate.
fn create_tee_dir(tee_dir: &std::path::Path) -> Option<()> {
    if let Some(parent) = tee_dir.parent() {
        let _ = crate::core::utils::create_private_dir(parent);
    }
    crate::core::utils::create_private_dir(tee_dir).ok()
}

/// Write raw output to a tee file in the given directory.
/// Returns file path on success.
#[cfg(test)]
fn write_tee_artifact(
    raw: &str,
    command_slug: &str,
    tee_dir: &std::path::Path,
    max_file_size: usize,
    max_files: usize,
) -> Option<TeeArtifact> {
    write_tee_artifact_with_source_completeness(
        raw,
        command_slug,
        tee_dir,
        max_file_size,
        max_files,
        true,
    )
}

fn write_tee_artifact_with_source_completeness(
    raw: &str,
    command_slug: &str,
    tee_dir: &std::path::Path,
    max_file_size: usize,
    max_files: usize,
    source_complete: bool,
) -> Option<TeeArtifact> {
    create_tee_dir(tee_dir)?;

    let slug = sanitize_slug(command_slug);
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let nonce = TEE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = format!("{}_{}_{}_{}.log", epoch, std::process::id(), nonce, slug);
    let filepath = tee_dir.join(filename);

    // Truncate at max_file_size (find a safe UTF-8 char boundary)
    let storage_complete = raw.len() <= max_file_size;
    let complete = source_complete && storage_complete;
    let content = if !storage_complete {
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
    let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));

    // Rotate old files
    cleanup_old_files(tee_dir, max_files);
    if !filepath.exists() {
        return None;
    }

    Some(TeeArtifact {
        path: filepath,
        complete,
        source_bytes: raw.len(),
        stored_bytes: content.len(),
        incomplete_reason: if !source_complete {
            Some("capture_limit")
        } else if !storage_complete {
            Some("storage_limit")
        } else {
            None
        },
        sha256,
    })
}

#[cfg(test)]
fn write_tee_file(
    raw: &str,
    command_slug: &str,
    tee_dir: &std::path::Path,
    max_file_size: usize,
    max_files: usize,
) -> Option<PathBuf> {
    write_tee_artifact(raw, command_slug, tee_dir, max_file_size, max_files)
        .map(|artifact| artifact.path)
}

fn tee_raw_artifact_with_source_completeness(
    raw: &str,
    command_slug: &str,
    exit_code: i32,
    source_complete: bool,
) -> Option<TeeArtifact> {
    // Check RTK_TEE=0 env override (disable)
    if std::env::var("RTK_TEE").ok().as_deref() == Some("0") {
        return None;
    }

    let config = Config::load().ok()?;
    let tee_dir = get_tee_dir(&config)?;

    let tee_dir = should_tee(&config.tee, raw.len(), exit_code, Some(tee_dir))?;

    write_tee_artifact_with_source_completeness(
        raw,
        command_slug,
        &tee_dir,
        config.tee.max_file_size,
        config.tee.max_files,
        source_complete,
    )
}

fn display_path(path: &std::path::Path) -> String {
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

fn display_shell_path(path: &std::path::Path) -> String {
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

fn format_hint(path: &std::path::Path) -> String {
    format!("[full output: {}]", display_shell_path(path))
}

fn format_partial_hint(path: &std::path::Path) -> String {
    format!("[partial recovery: {}]", display_shell_path(path))
}

pub fn output_meta_marker(
    artifact: &TeeArtifact,
    reason: &str,
    raw_exit_code: Option<i32>,
    recovery_start_line: Option<usize>,
    shown_records: Option<usize>,
    omitted_records: Option<usize>,
) -> String {
    let machine_path = if artifact.path.is_absolute() {
        artifact.path.clone()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&artifact.path))
            .unwrap_or_else(|_| artifact.path.clone())
    };
    let meta = OutputMeta {
        schema: "rtk.output.v1",
        truncated: true,
        reason,
        raw_exit_code,
        recovery_start_line,
        shown_records,
        omitted_records,
        artifact: Some(OutputArtifactMeta {
            content: if artifact.complete {
                "complete_user_visible_output"
            } else {
                "partial_user_visible_output"
            },
            path: machine_path.display().to_string(),
            complete: artifact.complete,
            source_bytes: artifact.source_bytes,
            stored_bytes: artifact.stored_bytes,
            incomplete_reason: artifact.incomplete_reason,
            sha256: artifact.sha256.clone(),
        }),
    };
    let json = serde_json::to_string(&meta).expect("RTK output metadata must serialize");
    format!("{OUTPUT_META_PREFIX}{json}")
}

pub fn incomplete_output_meta_marker(reason: &str, shown_records: Option<usize>) -> String {
    incomplete_output_meta_marker_with_exit(reason, shown_records, None)
}

fn incomplete_output_meta_marker_with_exit(
    reason: &str,
    shown_records: Option<usize>,
    raw_exit_code: Option<i32>,
) -> String {
    let meta = OutputMeta {
        schema: "rtk.output.v1",
        truncated: true,
        reason,
        raw_exit_code,
        recovery_start_line: None,
        shown_records,
        omitted_records: None,
        artifact: None,
    };
    let json = serde_json::to_string(&meta).expect("RTK output metadata must serialize");
    format!("{OUTPUT_META_PREFIX}{json}")
}

fn unavailable_recovery(reason: &str) -> String {
    format!(
        "[recovery unavailable]\n{}",
        incomplete_output_meta_marker(reason, None)
    )
}

fn unavailable_recovery_with_exit(reason: &str, raw_exit_code: Option<i32>) -> String {
    format!(
        "[recovery unavailable]\n{}",
        incomplete_output_meta_marker_with_exit(reason, None, raw_exit_code)
    )
}

fn format_recovery(
    human_hint: String,
    artifact: &TeeArtifact,
    reason: &str,
    raw_exit_code: Option<i32>,
    recovery_start_line: Option<usize>,
    shown_records: Option<usize>,
    omitted_records: Option<usize>,
) -> String {
    format!(
        "{}\n{}",
        human_hint,
        output_meta_marker(
            artifact,
            reason,
            raw_exit_code,
            recovery_start_line,
            shown_records,
            omitted_records,
        )
    )
}

pub fn full_output_recovery(
    artifact: &TeeArtifact,
    reason: &str,
    raw_exit_code: Option<i32>,
    shown_records: Option<usize>,
    omitted_records: Option<usize>,
) -> String {
    format_recovery(
        format_hint(&artifact.path),
        artifact,
        reason,
        raw_exit_code,
        None,
        shown_records,
        omitted_records,
    )
}

/// Convenience: tee + recovery metadata in one call.
/// Non-empty lossy output always returns a machine marker, even when recovery
/// is disabled, fails, or is only partial.
pub fn tee_and_hint(raw: &str, command_slug: &str, exit_code: i32) -> Option<String> {
    tee_and_hint_with_source_completeness(raw, command_slug, exit_code, true)
}

pub fn tee_and_hint_with_source_completeness(
    raw: &str,
    command_slug: &str,
    exit_code: i32,
    source_complete: bool,
) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    Some(
        match tee_raw_artifact_with_source_completeness(
            raw,
            command_slug,
            exit_code,
            source_complete,
        ) {
            Some(artifact) => {
                let hint = if artifact.complete {
                    format_hint(&artifact.path)
                } else {
                    format_partial_hint(&artifact.path)
                };
                format_recovery(
                    hint,
                    &artifact,
                    "filtered_failure_output",
                    Some(exit_code),
                    None,
                    None,
                    None,
                )
            }
            None => unavailable_recovery("filtered_failure_output"),
        },
    )
}

pub fn force_tee_artifact(content: &str, command_slug: &str) -> Option<TeeArtifact> {
    force_tee_artifact_with_source_completeness(content, command_slug, true)
}

fn force_tee_artifact_with_source_completeness(
    content: &str,
    command_slug: &str,
    source_complete: bool,
) -> Option<TeeArtifact> {
    if std::env::var("RTK_TEE").ok().as_deref() == Some("0") {
        return None;
    }

    if content.is_empty() {
        return None;
    }

    let config = Config::load().ok()?;

    if !config.tee.enabled {
        return None;
    }

    let tee_dir = get_tee_dir(&config)?;
    let tee_dir = create_tee_dir(&tee_dir).and(Some(tee_dir))?;

    write_tee_artifact_with_source_completeness(
        content,
        command_slug,
        &tee_dir,
        config.tee.max_file_size,
        config.tee.max_files,
        source_complete,
    )
}

/// Returns human recovery text plus machine metadata. Non-empty lossy output
/// still gets an incomplete marker when tee is disabled or unavailable.
pub fn force_tee_hint(raw: &str, command_slug: &str) -> Option<String> {
    force_tee_hint_internal(raw, command_slug, None, true)
}

pub fn force_tee_hint_with_source_completeness(
    raw: &str,
    command_slug: &str,
    exit_code: i32,
    source_complete: bool,
) -> Option<String> {
    force_tee_hint_internal(raw, command_slug, Some(exit_code), source_complete)
}

fn force_tee_hint_internal(
    raw: &str,
    command_slug: &str,
    raw_exit_code: Option<i32>,
    source_complete: bool,
) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    Some(
        match force_tee_artifact_with_source_completeness(raw, command_slug, source_complete) {
            Some(artifact) => {
                let hint = if artifact.complete {
                    format_hint(&artifact.path)
                } else {
                    format_partial_hint(&artifact.path)
                };
                format_recovery(
                    hint,
                    &artifact,
                    "filtered_output",
                    raw_exit_code,
                    None,
                    None,
                    None,
                )
            }
            None => unavailable_recovery_with_exit("filtered_output", raw_exit_code),
        },
    )
}

/// Returns a tail recovery hint plus machine metadata when complete. Partial or
/// unavailable recovery is labelled explicitly and never called full output.
pub fn force_tee_tail_hint(
    content: &str,
    command_slug: &str,
    line_offset: usize,
) -> Option<String> {
    if content.is_empty() {
        return None;
    }
    Some(match force_tee_artifact(content, command_slug) {
        Some(artifact) => {
            let hint = if artifact.complete {
                format!(
                    "[see remaining: tail -n +{} {}]",
                    line_offset,
                    display_shell_path(&artifact.path)
                )
            } else {
                format_partial_hint(&artifact.path)
            };
            format_recovery(
                hint,
                &artifact,
                "filtered_output_tail",
                None,
                Some(line_offset),
                None,
                None,
            )
        }
        None => unavailable_recovery("filtered_output_tail"),
    })
}

/// TeeMode controls when tee writes files.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TeeMode {
    #[default]
    Failures,
    Always,
    Never,
}

/// Configuration for the tee feature.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TeeConfig {
    pub enabled: bool,
    pub mode: TeeMode,
    pub max_files: usize,
    pub max_file_size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<PathBuf>,
}

impl Default for TeeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: TeeMode::default(),
            max_files: DEFAULT_MAX_FILES,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            directory: None,
        }
    }
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
        // Truncate at 40
        let long = "a".repeat(50);
        assert_eq!(sanitize_slug(&long).len(), 40);
    }

    #[test]
    fn test_should_tee_disabled() {
        let config = TeeConfig {
            enabled: false,
            ..TeeConfig::default()
        };
        let dir = PathBuf::from("/tmp/tee");
        assert!(should_tee(&config, 1000, 1, Some(dir)).is_none());
    }

    #[test]
    fn test_should_tee_never_mode() {
        let config = TeeConfig {
            mode: TeeMode::Never,
            ..TeeConfig::default()
        };
        let dir = PathBuf::from("/tmp/tee");
        assert!(should_tee(&config, 1000, 1, Some(dir)).is_none());
    }

    #[test]
    fn test_should_tee_skip_small_output() {
        let config = TeeConfig::default();
        let dir = PathBuf::from("/tmp/tee");
        // Below MIN_TEE_SIZE (500)
        assert!(should_tee(&config, 100, 1, Some(dir)).is_none());
    }

    #[test]
    fn test_should_tee_skip_success_in_failures_mode() {
        let config = TeeConfig::default(); // mode = Failures
        let dir = PathBuf::from("/tmp/tee");
        assert!(should_tee(&config, 1000, 0, Some(dir)).is_none());
    }

    #[test]
    fn test_should_tee_proceed_on_failure() {
        let config = TeeConfig::default(); // mode = Failures
        let dir = PathBuf::from("/tmp/tee");
        assert!(should_tee(&config, 1000, 1, Some(dir)).is_some());
    }

    #[test]
    fn test_should_tee_always_mode_success() {
        let config = TeeConfig {
            mode: TeeMode::Always,
            ..TeeConfig::default()
        };
        let dir = PathBuf::from("/tmp/tee");
        assert!(should_tee(&config, 1000, 0, Some(dir)).is_some());
    }

    #[test]
    fn test_write_tee_file_creates_file() {
        let tmpdir = tempfile::tempdir().unwrap();
        let content = "error: test failed\n".repeat(50);
        let result = write_tee_file(
            &content,
            "cargo_test",
            tmpdir.path(),
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
    fn test_write_tee_artifact_uses_unique_paths_for_same_slug() {
        let tmpdir = tempfile::tempdir().unwrap();
        let first =
            write_tee_artifact("first", "grep", tmpdir.path(), 1000, 20).expect("first artifact");
        let second =
            write_tee_artifact("second", "grep", tmpdir.path(), 1000, 20).expect("second artifact");

        assert_ne!(first.path, second.path);
        assert_eq!(fs::read_to_string(first.path).unwrap(), "first");
        assert_eq!(fs::read_to_string(second.path).unwrap(), "second");
    }

    #[test]
    fn test_write_tee_artifact_returns_none_when_rotation_removes_it() {
        let tmpdir = tempfile::tempdir().unwrap();
        let artifact = write_tee_artifact("content", "grep", tmpdir.path(), 1000, 0);

        assert!(artifact.is_none());
    }

    #[test]
    #[cfg(unix)]
    fn test_write_tee_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let tmpdir = tempfile::tempdir().unwrap();
        let tee_dir = tmpdir.path().join("tee");
        let path = write_tee_file(
            "secret output\n",
            "grep",
            &tee_dir,
            DEFAULT_MAX_FILE_SIZE,
            20,
        )
        .expect("tee file written");

        let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600, "tee file must be owner-only");
        assert_eq!(mode(&tee_dir), 0o700, "tee dir must be owner-only");
    }

    // umask is process-global, so this must not run alongside another test that
    // depends on it. Restored before the assertion can unwind.
    #[test]
    #[cfg(unix)]
    #[allow(unsafe_code)]
    fn test_write_tee_file_owner_only_under_permissive_umask() {
        use std::os::unix::fs::PermissionsExt;

        // nosemgrep: unsafe-block
        let previous = unsafe { libc::umask(0o000) };
        let tmpdir = tempfile::tempdir().unwrap();
        let tee_dir = tmpdir.path().join("tee");
        let written = write_tee_file("secret\n", "grep", &tee_dir, DEFAULT_MAX_FILE_SIZE, 20);
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
        // Set max_file_size to 1000 bytes
        let result = write_tee_file(&big_output, "test", tmpdir.path(), 1000, 20);
        assert!(result.is_some());

        let path = result.unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("--- truncated at 1000 bytes ---"));
        assert!(content.len() < 2000);
    }

    #[test]
    fn test_write_tee_file_truncation_utf8_boundary() {
        let tmpdir = tempfile::tempdir().unwrap();
        // Create a string where the truncation point falls inside a multi-byte char.
        // Japanese chars are 3 bytes each in UTF-8.
        // 332 chars * 3 bytes = 996 bytes, then one more = 999 bytes.
        // With max_file_size=998, the cut falls mid-character.
        let japanese = "\u{6F22}".repeat(333); // 999 bytes of 3-byte chars
        assert_eq!(japanese.len(), 999);

        // Truncate at 998 — falls in the middle of the 333rd character
        let result = write_tee_file(&japanese, "test_utf8", tmpdir.path(), 998, 20);
        assert!(result.is_some());

        let path = result.unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("--- truncated at 998 bytes ---"));
        // Should contain 332 full characters (996 bytes), not panic
        assert!(content.starts_with(&"\u{6F22}".repeat(332)));
    }

    #[test]
    fn test_write_tee_file_truncation_emoji() {
        let tmpdir = tempfile::tempdir().unwrap();
        // Emoji are 4 bytes each in UTF-8
        let emojis = "\u{1F600}".repeat(100); // 400 bytes
        assert_eq!(emojis.len(), 400);

        // Truncate at 201 — falls mid-emoji (4-byte boundary is at 200, 204)
        let result = write_tee_file(&emojis, "test_emoji", tmpdir.path(), 201, 20);
        assert!(result.is_some());

        let path = result.unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("--- truncated at 201 bytes ---"));
        // The emoji portion should be exactly 200 bytes (50 emojis),
        // rounded down from 201 to the nearest char boundary
        let target = "\u{1F600}".repeat(50);
        assert!(content.starts_with(&target));
    }

    #[test]
    fn test_cleanup_old_files() {
        let tmpdir = tempfile::tempdir().unwrap();
        let dir = tmpdir.path();

        // Create 25 RTK-owned .log files plus one unrelated log.
        for i in 0..25 {
            let filename = format!("{:010}_1_{}_test.log", 1000000 + i, i);
            fs::write(dir.join(&filename), "content").unwrap();
        }
        fs::write(dir.join("application.log"), "must remain").unwrap();

        cleanup_old_files(dir, 20);

        let remaining: Vec<_> = fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(remaining.len(), 21);
        assert!(dir.join("application.log").exists());

        // Oldest 5 should be removed
        for i in 0..5 {
            let filename = format!("{:010}_1_{}_test.log", 1000000 + i, i);
            assert!(!dir.join(&filename).exists());
        }
        // Newest 20 should remain
        for i in 5..25 {
            let filename = format!("{:010}_1_{}_test.log", 1000000 + i, i);
            assert!(dir.join(&filename).exists());
        }
    }

    #[test]
    fn test_format_hint() {
        let path = PathBuf::from("/tmp/rtk/tee/123_cargo_test.log");
        let hint = format_hint(&path);
        assert!(hint.starts_with("[full output: "));
        assert!(hint.ends_with(']'));
        assert!(hint.contains("123_cargo_test.log"));
    }

    #[test]
    fn test_output_meta_marker_is_machine_parseable() {
        let artifact = TeeArtifact {
            path: PathBuf::from("/tmp/rtk/tee/full grep.log"),
            complete: true,
            source_bytes: 4096,
            stored_bytes: 4096,
            incomplete_reason: None,
            sha256: format!("{:x}", Sha256::digest(b"test")),
        };
        let marker = output_meta_marker(
            &artifact,
            "grep_result_limit",
            Some(0),
            None,
            Some(25),
            Some(7),
        );
        let payload = marker
            .strip_prefix(OUTPUT_META_PREFIX)
            .expect("stable RTK metadata prefix");
        let parsed: serde_json::Value = serde_json::from_str(payload).expect("valid JSON metadata");

        assert_eq!(parsed["schema"], "rtk.output.v1");
        assert_eq!(parsed["truncated"], true);
        assert_eq!(parsed["reason"], "grep_result_limit");
        assert_eq!(parsed["raw_exit_code"], 0);
        assert_eq!(parsed["shown_records"], 25);
        assert_eq!(parsed["omitted_records"], 7);
        assert_eq!(
            parsed["artifact"]["content"],
            "complete_user_visible_output"
        );
        assert_eq!(parsed["artifact"]["path"], "/tmp/rtk/tee/full grep.log");
        assert_eq!(parsed["artifact"]["complete"], true);
    }

    #[test]
    fn test_write_tee_artifact_reports_incomplete_storage() {
        let tmpdir = tempfile::tempdir().unwrap();
        let source = "x".repeat(2000);
        let artifact = write_tee_artifact(&source, "grep", tmpdir.path(), 1000, 20)
            .expect("tee artifact written");

        assert!(!artifact.complete);
        assert_eq!(artifact.source_bytes, 2000);
        assert!(artifact.stored_bytes < artifact.source_bytes);
        assert_eq!(artifact.incomplete_reason, Some("storage_limit"));
    }

    #[test]
    fn test_capture_truncation_can_never_be_reported_as_complete_storage() {
        let tmpdir = tempfile::tempdir().unwrap();
        let source = "captured prefix only";
        let artifact = write_tee_artifact_with_source_completeness(
            source,
            "pytest",
            tmpdir.path(),
            1000,
            20,
            false,
        )
        .expect("tee artifact written");

        assert!(!artifact.complete);
        assert_eq!(artifact.source_bytes, artifact.stored_bytes);
        assert_eq!(artifact.incomplete_reason, Some("capture_limit"));
        let marker = output_meta_marker(&artifact, "filtered_output", Some(0), None, None, None);
        let payload = marker.strip_prefix(OUTPUT_META_PREFIX).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(parsed["artifact"]["content"], "partial_user_visible_output");
        assert_eq!(parsed["artifact"]["complete"], false);
        assert_eq!(parsed["artifact"]["incomplete_reason"], "capture_limit");
    }

    #[test]
    fn test_incomplete_artifact_never_claims_full_output() {
        let artifact = TeeArtifact {
            path: PathBuf::from("/tmp/rtk/tee/partial.log"),
            complete: false,
            source_bytes: 2_000_000,
            stored_bytes: 1_048_610,
            incomplete_reason: Some("storage_limit"),
            sha256: format!("{:x}", Sha256::digest(b"partial")),
        };
        let recovery = format_recovery(
            format_partial_hint(&artifact.path),
            &artifact,
            "filtered_output",
            None,
            None,
            None,
            None,
        );

        assert!(recovery.starts_with("[partial recovery: "));
        assert!(!recovery.contains("[full output:"));
        let marker = recovery.lines().nth(1).expect("metadata marker");
        let parsed: serde_json::Value = serde_json::from_str(
            marker
                .strip_prefix(OUTPUT_META_PREFIX)
                .expect("metadata prefix"),
        )
        .expect("valid metadata");
        assert_eq!(parsed["artifact"]["complete"], false);
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

    #[test]
    fn test_format_hint_avoids_backslash_escaped_whitespace() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let path = home
            .join("Library")
            .join("Application Support")
            .join("rtk")
            .join("tee")
            .join("123_go_test.log");
        let hint = format_hint(&path);

        assert_eq!(
            hint,
            "[full output: \"$HOME/Library/Application Support/rtk/tee/123_go_test.log\"]"
        );
        assert!(
            !hint.contains("\\ "),
            "hint should not encourage backslash-escaped whitespace"
        );
    }

    #[test]
    fn test_tee_config_default() {
        let config = TeeConfig::default();
        assert!(config.enabled);
        assert_eq!(config.mode, TeeMode::Failures);
        assert_eq!(config.max_files, 20);
        assert_eq!(config.max_file_size, 1_048_576);
        assert!(config.directory.is_none());
    }

    #[test]
    fn test_tee_config_deserialize() {
        let toml_str = r#"
enabled = true
mode = "always"
max_files = 10
max_file_size = 524288
directory = "/tmp/rtk-tee"
"#;
        let config: TeeConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.mode, TeeMode::Always);
        assert_eq!(config.max_files, 10);
        assert_eq!(config.max_file_size, 524288);
        assert_eq!(config.directory, Some(PathBuf::from("/tmp/rtk-tee")));

        // Round-trip
        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: TeeConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.mode, TeeMode::Always);
        assert_eq!(deserialized.max_files, 10);
    }

    #[test]
    fn test_tee_mode_serde() {
        // Test all modes via JSON
        let mode: TeeMode = serde_json::from_str(r#""always""#).unwrap();
        assert_eq!(mode, TeeMode::Always);

        let mode: TeeMode = serde_json::from_str(r#""failures""#).unwrap();
        assert_eq!(mode, TeeMode::Failures);

        let mode: TeeMode = serde_json::from_str(r#""never""#).unwrap();
        assert_eq!(mode, TeeMode::Never);
    }

    #[test]
    fn test_force_tee_hint_skip_empty() {
        let hint = force_tee_hint("", "test_cmd");
        assert!(hint.is_none(), "Should skip empty content");
    }

    #[test]
    fn test_force_tee_hint_marks_unrecoverable_loss_when_env_disables_tee() {
        std::env::set_var("RTK_TEE", "0");
        let large_output = "x".repeat(1000);
        let hint = force_tee_hint(&large_output, "test_cmd");
        std::env::remove_var("RTK_TEE");
        let hint = hint.expect("loss must remain machine-visible when tee is disabled");
        assert!(hint.starts_with("[recovery unavailable]\n"));
        let marker = hint.lines().nth(1).expect("standalone metadata line");
        let parsed: serde_json::Value = serde_json::from_str(
            marker
                .strip_prefix(OUTPUT_META_PREFIX)
                .expect("metadata prefix"),
        )
        .expect("valid metadata");
        assert_eq!(parsed["truncated"], true);
        assert!(parsed["artifact"].is_null());
    }

    #[test]
    fn test_unavailable_marker_stays_at_line_start_when_caller_prefixes_hint() {
        let wrapped = format!("issues found: {}", unavailable_recovery("filtered_output"));
        let marker = wrapped.lines().nth(1).expect("standalone metadata line");

        assert!(marker.starts_with(OUTPUT_META_PREFIX));
    }

    #[test]
    fn test_force_tee_tail_hint_skip_empty() {
        let hint = force_tee_tail_hint("", "test_cmd", 22);
        assert!(hint.is_none(), "Should skip empty content");
    }

    #[test]
    fn test_force_tee_tail_hint_format() {
        let path = std::path::PathBuf::from("/tmp/rtk/tee/123_docker_images.log");
        let display = display_path(&path);
        let hint = format!("[see remaining: tail -n +{} {}]", 22, display);
        assert!(hint.starts_with("[see remaining: tail -n +22 "));
        assert!(hint.ends_with(']'));
        assert!(hint.contains("123_docker_images.log"));
    }
}
