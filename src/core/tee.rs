//! Raw output recovery -- saves unfiltered output to disk on command failure.
//!
//! ## Sensitive-output handling (see `docs/superpowers/specs/2026-05-20-…`)
//!
//! Three layers of defence, increasing strictness:
//!
//! 1. **Per-slug blocklist** — commands that *always* deal in credentials
//!    (e.g. `aws_secretsmanager_get-secret-value`, `kubectl_get_secret`,
//!    `op_*`, `vault_*`) short-circuit before any disk write and emit no
//!    hint at all. See [`is_blocklisted_slug`].
//! 2. **Content-level redactor** — all surviving writes pass through
//!    [`crate::core::redact::redact_content`] which masks Bearer tokens,
//!    `AKIA…` keys, `ghp_…` PATs, URL userinfo, and inline credential env
//!    assignments. If any matches were found, a `--- rtk: N credential-like
//!    patterns redacted ---` audit header is prepended to the on-disk file.
//! 3. **Per-call-site Sensitive opt-in** — callers that know they handle
//!    credentials (`aws_cmd.rs`, `curl_cmd.rs`, `psql_cmd.rs`) use the
//!    `*_sensitive` variants below. These force the redactor pass even when
//!    the slug is not on the blocklist, so a yet-to-be-classified sensitive
//!    command still gets scrubbed.

use super::constants::RTK_DATA_DIR;
use crate::core::config::Config;
use crate::core::redact;
use std::path::PathBuf;

/// Minimum output size to tee (smaller outputs don't need recovery)
const MIN_TEE_SIZE: usize = 500;

/// Per-slug blocklist for commands that should *never* persist raw output
/// to disk, regardless of content. Matched with `slug.starts_with(prefix)`
/// so e.g. `aws_secretsmanager` covers `aws_secretsmanager_get-secret-value`,
/// `aws_secretsmanager_list-secrets`, etc.
///
/// Sourced from the security & privacy design doc, Finding 4.
const SENSITIVE_SLUG_PREFIXES: &[&str] = &[
    // AWS secret/identity surfaces
    "aws_secretsmanager",
    "aws_kms",
    "aws_sts_get-session-token",
    "aws_sts_assume-role",
    // Kubernetes
    "kubectl_get_secret",
    "kubectl_describe_secret",
    // GitHub / GitLab CLIs
    "gh_secret",
    "gh_auth",
    "glab_secret",
    "glab_auth",
    // Password / secret managers
    "op_",
    "vault_",
    "doppler_",
    "bw_",
    "pass_",
    // Helm values often contain secrets
    "helm_get_values",
    // Git config can leak credential.helper / PATs
    "git_config",
];

/// Audit header prepended to tee files when the content redactor fired.
fn redaction_header(count: usize) -> String {
    format!("--- rtk: {} credential-like patterns redacted ---\n", count)
}

/// True when the slug matches any sensitive prefix from `SENSITIVE_SLUG_PREFIXES`.
fn is_blocklisted_slug(slug: &str) -> bool {
    SENSITIVE_SLUG_PREFIXES
        .iter()
        .any(|prefix| slug.starts_with(prefix))
}

/// Default max files to keep in tee directory
const DEFAULT_MAX_FILES: usize = 20;

/// Default max file size (1MB)
const DEFAULT_MAX_FILE_SIZE: usize = 1_048_576;

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
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
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

/// Whether the content redactor should be applied unconditionally
/// (`Forced`, used by `*_sensitive` variants for known-credential-bearing
/// commands) or only when the default safety-net policy says to (`Auto`).
///
/// At present `Auto` *also* runs the redactor on every write — the safety net
/// is on by default. `Forced` exists so the call-site intent stays explicit
/// and so a future config knob that lets users opt out of `Auto` redaction
/// (e.g. `RTK_TEE_REDACT=0`) will still leave `Sensitive` callers protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RedactPolicy {
    Auto,
    Forced,
}

/// Write raw output to a tee file in the given directory.
/// Returns file path on success.
///
/// Refuses to write (returns `None`) when `command_slug` matches a prefix in
/// [`SENSITIVE_SLUG_PREFIXES`]. For all surviving writes, the content is
/// passed through [`redact::redact_content`] — if any credential-shaped
/// substring was masked, a one-line audit header is prepended to the file.
fn write_tee_file(
    raw: &str,
    command_slug: &str,
    tee_dir: &std::path::Path,
    max_file_size: usize,
    max_files: usize,
    policy: RedactPolicy,
) -> Option<PathBuf> {
    // Layer 1: hard blocklist. Refuse to write at all for slugs known to deal
    // in secrets — the cost of a missed recovery is "the LLM can't read the
    // failed secret-fetch", which is the correct trade-off.
    if is_blocklisted_slug(command_slug) {
        return None;
    }

    std::fs::create_dir_all(tee_dir).ok()?;

    let slug = sanitize_slug(command_slug);
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let filename = format!("{}_{}.log", epoch, slug);
    let filepath = tee_dir.join(filename);

    // Layer 2: content-level redactor. Single-pass over `lazy_static!`-cached
    // regex, a no-op (byte-identical output, no header) when the input has no
    // credential-shaped substrings — so the cost on clean cargo-test /
    // npm-install output is one regex scan per pattern, no allocations.
    //
    // `Auto` and `Forced` currently behave identically; the policy is plumbed
    // through so the `*_sensitive` callers remain explicit even if a future
    // config flag turns `Auto` off.
    let _ = policy; // reserved for future per-policy behaviour
    let (scrubbed, match_count) = redact::redact_content(raw);
    let body: std::borrow::Cow<'_, str> = if match_count > 0 {
        std::borrow::Cow::Owned(format!("{}{}", redaction_header(match_count), scrubbed))
    } else if scrubbed == raw {
        // Fast path: byte-identical output, avoid the second allocation.
        std::borrow::Cow::Borrowed(raw)
    } else {
        std::borrow::Cow::Owned(scrubbed)
    };

    // Truncate at max_file_size (find a safe UTF-8 char boundary)
    let content = if body.len() > max_file_size {
        let boundary = body
            .char_indices()
            .take_while(|(i, _)| *i < max_file_size)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!(
            "{}\n\n--- truncated at {} bytes ---",
            &body[..boundary],
            max_file_size
        )
    } else {
        body.into_owned()
    };

    std::fs::write(&filepath, content).ok()?;

    // Rotate old files
    cleanup_old_files(tee_dir, max_files);

    Some(filepath)
}

/// Write raw output to tee file if conditions are met.
/// Returns file path on success, None if skipped/failed.
pub fn tee_raw(raw: &str, command_slug: &str, exit_code: i32) -> Option<PathBuf> {
    tee_raw_with_policy(raw, command_slug, exit_code, RedactPolicy::Auto)
}

fn tee_raw_with_policy(
    raw: &str,
    command_slug: &str,
    exit_code: i32,
    policy: RedactPolicy,
) -> Option<PathBuf> {
    // Check RTK_TEE=0 env override (disable)
    if std::env::var("RTK_TEE").ok().as_deref() == Some("0") {
        return None;
    }

    let config = Config::load().ok()?;
    let tee_dir = get_tee_dir(&config)?;

    let tee_dir = should_tee(&config.tee, raw.len(), exit_code, Some(tee_dir))?;

    write_tee_file(
        raw,
        command_slug,
        &tee_dir,
        config.tee.max_file_size,
        config.tee.max_files,
        policy,
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

fn format_hint(path: &std::path::Path) -> String {
    format!("[full output: {}]", display_path(path))
}

/// Convenience: tee + format hint in one call.
/// Returns hint string if file was written, None if skipped.
pub fn tee_and_hint(raw: &str, command_slug: &str, exit_code: i32) -> Option<String> {
    let path = tee_raw(raw, command_slug, exit_code)?;
    Some(format_hint(&path))
}

fn force_tee_path(content: &str, command_slug: &str) -> Option<PathBuf> {
    force_tee_path_with_policy(content, command_slug, RedactPolicy::Auto)
}

fn force_tee_path_with_policy(
    content: &str,
    command_slug: &str,
    policy: RedactPolicy,
) -> Option<PathBuf> {
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
    let tee_dir = std::fs::create_dir_all(&tee_dir).ok().and(Some(tee_dir))?;

    write_tee_file(
        content,
        command_slug,
        &tee_dir,
        config.tee.max_file_size,
        config.tee.max_files,
        policy,
    )
}

/// Returns `[full output: ~/path]`, or None if tee is disabled/skipped.
pub fn force_tee_hint(raw: &str, command_slug: &str) -> Option<String> {
    let path = force_tee_path(raw, command_slug)?;
    Some(format_hint(&path))
}

/// Returns `[see remaining: tail -n +{line_offset} ~/path]`, or None if tee is disabled/skipped.
pub fn force_tee_tail_hint(
    content: &str,
    command_slug: &str,
    line_offset: usize,
) -> Option<String> {
    let path = force_tee_path(content, command_slug)?;
    Some(format!(
        "[see remaining: tail -n +{} {}]",
        line_offset,
        display_path(&path)
    ))
}

// --- Sensitive variants ---
//
// Call sites that know they're dealing with credential-shaped content
// (`aws_cmd.rs`, `curl_cmd.rs`, `psql_cmd.rs`) use these variants. They
// share the existing tee bookkeeping (rotation, mode, MIN_TEE_SIZE) but
// force the content redactor pass and stay scrubbed even if a future
// config knob lets users disable the default Auto redaction.

/// Like [`tee_and_hint`], but force the credential redactor unconditionally.
///
/// Use for command families where output is known to often contain
/// secrets (AWS API responses, curl bodies, psql results that may carry
/// connection strings in NOTICE lines, …).
pub fn tee_and_hint_sensitive(raw: &str, command_slug: &str, exit_code: i32) -> Option<String> {
    let path = tee_raw_with_policy(raw, command_slug, exit_code, RedactPolicy::Forced)?;
    Some(format_hint(&path))
}

/// Like [`force_tee_hint`], but force the credential redactor unconditionally.
pub fn force_tee_hint_sensitive(raw: &str, command_slug: &str) -> Option<String> {
    let path = force_tee_path_with_policy(raw, command_slug, RedactPolicy::Forced)?;
    Some(format_hint(&path))
}

/// Like [`force_tee_tail_hint`], but force the credential redactor unconditionally.
///
/// Reserved for future call sites that emit a `tail -n +offset` recovery
/// hint for known-credential-bearing command families (none ship today but
/// the variant is documented as part of the sensitive surface).
#[allow(dead_code)]
pub fn force_tee_tail_hint_sensitive(
    content: &str,
    command_slug: &str,
    line_offset: usize,
) -> Option<String> {
    let path = force_tee_path_with_policy(content, command_slug, RedactPolicy::Forced)?;
    Some(format!(
        "[see remaining: tail -n +{} {}]",
        line_offset,
        display_path(&path)
    ))
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

    /// Serialises tests that touch `RTK_TEE*` / `RTK_CONFIG_PATH` env vars so
    /// parallel `cargo test` runs don't see each other's `set_var`. Tests
    /// that don't mutate env can run concurrently without taking this lock.
    static TEE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
            RedactPolicy::Auto,
        );
        assert!(result.is_some());

        let path = result.unwrap();
        assert!(path.exists());
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("error: test failed"));
    }

    #[test]
    fn test_write_tee_file_truncation() {
        let tmpdir = tempfile::tempdir().unwrap();
        let big_output = "x".repeat(2000);
        // Set max_file_size to 1000 bytes
        let result = write_tee_file(
            &big_output,
            "test",
            tmpdir.path(),
            1000,
            20,
            RedactPolicy::Auto,
        );
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
        let result = write_tee_file(
            &japanese,
            "test_utf8",
            tmpdir.path(),
            998,
            20,
            RedactPolicy::Auto,
        );
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
        let result = write_tee_file(
            &emojis,
            "test_emoji",
            tmpdir.path(),
            201,
            20,
            RedactPolicy::Auto,
        );
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

        // Create 25 .log files
        for i in 0..25 {
            let filename = format!("{:010}_{}.log", 1000000 + i, "test");
            fs::write(dir.join(&filename), "content").unwrap();
        }

        cleanup_old_files(dir, 20);

        let remaining: Vec<_> = fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(remaining.len(), 20);

        // Oldest 5 should be removed
        for i in 0..5 {
            let filename = format!("{:010}_{}.log", 1000000 + i, "test");
            assert!(!dir.join(&filename).exists());
        }
        // Newest 20 should remain
        for i in 5..25 {
            let filename = format!("{:010}_{}.log", 1000000 + i, "test");
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
    fn test_force_tee_hint_respects_env_disable() {
        // When RTK_TEE=0, force_tee_hint should return None.
        // Serialise via TEE_ENV_LOCK so we don't race other env-touching tests.
        let _guard = TEE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("RTK_TEE", "0");
        let large_output = "x".repeat(1000);
        let hint = force_tee_hint(&large_output, "test_cmd");
        std::env::remove_var("RTK_TEE");
        assert!(hint.is_none(), "Should respect RTK_TEE=0");
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

    // --- Sensitive output handling (Finding 4) ---
    //
    // The slug-blocklist tests stay below `write_tee_file` so they don't
    // touch the user's real `~/.config/rtk/` or `~/.local/share/rtk/`.
    // The high-level `*_sensitive` tests set `RTK_TEE_DIR` to a tempdir
    // and `RTK_CONFIG_PATH` to a non-existent path so `Config::load()`
    // returns the default config without reading the user's file. They
    // serialise via the module-level `TEE_ENV_LOCK` above to keep
    // concurrent `cargo test` runs deterministic.

    #[test]
    fn test_blocklisted_slug_skips_write() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let content = "AWS secret payload that must never hit disk\n".repeat(20);
        let result = write_tee_file(
            &content,
            "aws_secretsmanager_get-secret-value",
            tmpdir.path(),
            DEFAULT_MAX_FILE_SIZE,
            20,
            RedactPolicy::Auto,
        );
        assert!(result.is_none(), "blocklisted slug must refuse to write");

        // And no file should have been created in the tempdir.
        let count = fs::read_dir(tmpdir.path())
            .expect("read tempdir")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
            .count();
        assert_eq!(count, 0, "no .log file may exist for blocklisted slug");
    }

    #[test]
    fn test_redactor_masks_bearer_in_content() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let content = format!(
            "{}\nFailed request — Authorization: Bearer abc123XYZdef456GHIjkl\n{}",
            "noise line ".repeat(30),
            "more noise ".repeat(30),
        );
        let result = write_tee_file(
            &content,
            "cargo_test",
            tmpdir.path(),
            DEFAULT_MAX_FILE_SIZE,
            20,
            RedactPolicy::Auto,
        );
        let path = result.expect("write_tee_file should succeed");
        let written = fs::read_to_string(&path).expect("read written file");

        assert!(
            written.contains("Bearer ****"),
            "Bearer token must be masked: {}",
            written
        );
        assert!(
            !written.contains("abc123XYZdef456GHIjkl"),
            "raw bearer token must not survive: {}",
            written
        );
        assert!(
            written.starts_with("--- rtk: 1 credential-like patterns redacted ---\n"),
            "audit header must be prepended: {}",
            &written[..written.len().min(120)],
        );
    }

    #[test]
    fn test_redactor_passes_through_clean_content() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let content = "running 12 tests\n".repeat(60); // > MIN_TEE_SIZE
        let result = write_tee_file(
            &content,
            "cargo_test",
            tmpdir.path(),
            DEFAULT_MAX_FILE_SIZE,
            20,
            RedactPolicy::Auto,
        );
        let path = result.expect("write_tee_file should succeed");
        let written = fs::read_to_string(&path).expect("read written file");

        assert_eq!(
            written, content,
            "clean content must be byte-identical (no header, no mutation)"
        );
        assert!(
            !written.contains("--- rtk:"),
            "no audit header for clean content"
        );
    }

    #[test]
    fn test_redactor_masks_aws_keys() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        // Synthetic, scanner-safe AWS-style key (Z-suffixed) — matches
        // RTK's CREDENTIAL_PREFIX_RE without being a real AWS access key.
        let content = format!(
            "Failed deploy:\nAccessKeyId: AKIAZZZZZZZZZZZZZZZZ\n{}",
            "trailing noise ".repeat(40),
        );
        let result = write_tee_file(
            &content,
            "docker_inspect",
            tmpdir.path(),
            DEFAULT_MAX_FILE_SIZE,
            20,
            RedactPolicy::Auto,
        );
        let path = result.expect("write_tee_file should succeed");
        let written = fs::read_to_string(&path).expect("read written file");

        assert!(
            !written.contains("AKIAZZZZZZZZZZZZZZZZ"),
            "AKIA-prefixed key must be masked: {}",
            written
        );
        assert!(
            written.contains("****"),
            "redactor must leave its **** marker: {}",
            written
        );
        assert!(
            written.starts_with("--- rtk: 1 credential-like patterns redacted ---\n"),
            "audit header must be prepended: {}",
            &written[..written.len().min(120)],
        );
    }

    #[test]
    fn test_tee_and_hint_sensitive_always_redacts() {
        let _guard = TEE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        let tmpdir = tempfile::tempdir().expect("tempdir");
        // Point Config::load() at a non-existent path so it returns
        // Config::default() (tee enabled, mode = Failures).
        let fake_config = tmpdir.path().join("no-such-config.toml");
        std::env::set_var("RTK_CONFIG_PATH", &fake_config);
        std::env::set_var("RTK_TEE_DIR", tmpdir.path());
        std::env::remove_var("RTK_TEE");

        // Non-blocklisted slug, exit_code != 0 (so default Failures mode tees).
        let content = format!(
            "request failed:\nAuthorization: Bearer abc123XYZdef456GHIjkl\n{}",
            "padding ".repeat(80),
        );
        let hint = tee_and_hint_sensitive(&content, "cargo_test", 1);

        // Restore env before any assertion can fail the test.
        std::env::remove_var("RTK_CONFIG_PATH");
        std::env::remove_var("RTK_TEE_DIR");

        let hint = hint.expect("sensitive tee should write for non-blocklisted slug");
        assert!(hint.starts_with("[full output: "), "hint shape: {hint}");

        // Find the file inside the tempdir and assert it's scrubbed.
        let entry = fs::read_dir(tmpdir.path())
            .expect("read tempdir")
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().is_some_and(|x| x == "log"))
            .expect("tee file must exist");
        let written = fs::read_to_string(entry.path()).expect("read tee file");

        assert!(
            written.contains("Bearer ****"),
            "sensitive variant must redact Bearer: {}",
            written
        );
        assert!(
            !written.contains("abc123XYZdef456GHIjkl"),
            "raw bearer must not survive: {}",
            written
        );
        assert!(
            written.starts_with("--- rtk: "),
            "audit header must be prepended: {}",
            &written[..written.len().min(120)],
        );
    }

    #[test]
    fn test_aws_sts_get_session_token_does_not_persist_to_disk() {
        let _guard = TEE_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        let tmpdir = tempfile::tempdir().expect("tempdir");
        let fake_config = tmpdir.path().join("no-such-config.toml");
        std::env::set_var("RTK_CONFIG_PATH", &fake_config);
        std::env::set_var("RTK_TEE_DIR", tmpdir.path());
        std::env::remove_var("RTK_TEE");

        // Slug matches `aws_sts_get-session-token` prefix → blocklisted.
        let content = format!(
            "{{\"Credentials\":{{\"AccessKeyId\":\"AKIAZZZZZZZZZZZZZZZZ\",\"SecretAccessKey\":\"xyz\"}}}}\n{}",
            "padding ".repeat(80),
        );
        let hint = tee_and_hint_sensitive(&content, "aws_sts_get-session-token", 1);

        std::env::remove_var("RTK_CONFIG_PATH");
        std::env::remove_var("RTK_TEE_DIR");

        assert!(
            hint.is_none(),
            "blocklisted slug must not emit a tee hint: {:?}",
            hint
        );
        let log_count = fs::read_dir(tmpdir.path())
            .expect("read tempdir")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
            .count();
        assert_eq!(
            log_count, 0,
            "blocklisted slug must not persist any .log file"
        );
    }

    #[test]
    fn test_is_blocklisted_slug_examples() {
        // Positive: all the slugs from the design doc Finding 4.
        for slug in [
            "aws_secretsmanager_get-secret-value",
            "aws_kms_decrypt",
            "aws_sts_get-session-token",
            "aws_sts_assume-role",
            "kubectl_get_secret_my-secret",
            "kubectl_describe_secret_my-secret",
            "gh_secret_set",
            "gh_auth_status",
            "glab_secret_list",
            "glab_auth_login",
            "op_item_get",
            "vault_kv_get",
            "doppler_secrets",
            "bw_get_password",
            "pass_show",
            "helm_get_values_release",
            "git_config_user.email",
        ] {
            assert!(is_blocklisted_slug(slug), "slug {slug} must be blocklisted");
        }
        // Negative: ordinary slugs must not be blocked.
        for slug in [
            "cargo_test",
            "cargo_build",
            "npm_install",
            "docker_ps",
            "kubectl_get_pods",
            "git_log",
            "git_status",
            "gh_pr_view",
        ] {
            assert!(
                !is_blocklisted_slug(slug),
                "slug {slug} must NOT be blocklisted"
            );
        }
    }
}
