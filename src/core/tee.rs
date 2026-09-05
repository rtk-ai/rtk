//! Raw output recovery -- saves unfiltered output to disk on command failure.

use super::constants::RTK_DATA_DIR;
use crate::core::config::Config;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex, MutexGuard,
};

/// Minimum output size to tee (smaller outputs don't need recovery)
pub(crate) const MIN_TEE_SIZE: usize = 500;

/// Default max files to keep in tee directory
const DEFAULT_MAX_FILES: usize = 20;

/// Default max file size (1MB)
const DEFAULT_MAX_FILE_SIZE: usize = 1_048_576;

static LOSSLESS_TEE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static LOSSLESS_TEE_COMMIT_LOCK: Mutex<()> = Mutex::new(());

/// Sanitize a command slug for use in filenames.
/// Replaces non-alphanumeric chars (except underscore/hyphen) with underscore.
/// Long slugs (usually an embedded file path that duplicates the command the LLM
/// already issued) collapse to a short readable prefix plus a short disambiguating
/// hash, keeping recovery filenames unique but compact — fewer tokens per tee hint.
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

/// Get the tee directory, respecting config and env overrides.
pub(crate) fn get_tee_dir(config: &Config) -> Option<PathBuf> {
    get_tee_dir_with_env(config, std::env::var_os("RTK_TEE_DIR"))
}

fn get_tee_dir_with_env(
    config: &Config,
    env_override: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    // Env var override
    if let Some(dir) = env_override {
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
    cleanup_files_except(dir, max_files, None, |path| {
        path.extension().is_some_and(|extension| extension == "log") && !is_lossless_log(path)
    });
}

/// Rotate tee files while preserving a newly committed recovery artifact even
/// when a clock-skewed filename would otherwise sort after it.
fn cleanup_lossless_files_except(
    dir: &std::path::Path,
    max_files: usize,
    preserve: Option<&std::path::Path>,
) {
    cleanup_files_except(dir, max_files, preserve, is_lossless_log);
}

fn cleanup_files_except(
    dir: &std::path::Path,
    max_files: usize,
    preserve: Option<&std::path::Path>,
    include: impl Fn(&std::path::Path) -> bool,
) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|entry| include(&entry.path()))
        .filter(|entry| preserve != Some(entry.path().as_path()))
        .collect();

    let allowed_entries = max_files.saturating_sub(usize::from(preserve.is_some()));

    if entries.len() <= allowed_entries {
        return;
    }

    // Sort by filename (which starts with epoch timestamp = chronological)
    entries.sort_by_key(|e| e.file_name());

    let to_remove = entries.len() - allowed_entries;
    for entry in entries.iter().take(to_remove) {
        let _ = std::fs::remove_file(entry.path());
    }
}

fn is_lossless_log(path: &std::path::Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with(".lossless.log"))
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

/// Acquire an advisory lock shared by every process using this tee directory.
/// The lock file is intentionally retained: unlike a create-new sentinel, an
/// operating-system file lock is released automatically if its owner exits.
fn acquire_lossless_tee_commit_lock(tee_dir: &std::path::Path) -> Option<std::fs::File> {
    let lock_path = tee_dir.join(".lossless-tee.lock");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    let file = crate::core::utils::open_private(&mut options, &lock_path).ok()?;
    file.lock().ok()?;
    Some(file)
}

#[cfg(debug_assertions)]
fn wait_for_test_lossless_tee_commit_lock() {
    let Ok(directory) = std::env::var("RTK_TEST_TEE_COMMIT_HOLD_DIR") else {
        return;
    };
    let directory = std::path::Path::new(&directory);
    let entered = directory.join(format!("entered-{}", std::process::id()));
    if std::fs::write(entered, "locked").is_err() {
        return;
    }
    let release = directory.join("release");
    while !release.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(debug_assertions)]
fn observe_test_lossless_tee_lock_attempt() {
    let Ok(directory) = std::env::var("RTK_TEST_TEE_COMMIT_HOLD_DIR") else {
        return;
    };
    let marker =
        std::path::Path::new(&directory).join(format!("attempting-{}", std::process::id()));
    let _ = std::fs::write(marker, "attempting");
}

#[cfg(debug_assertions)]
fn observe_test_lossless_tee_commit(path: &std::path::Path, max_files: usize) {
    let Ok(directory) = std::env::var("RTK_TEST_TEE_COMMIT_OBSERVATION_DIR") else {
        return;
    };
    if path.is_file() {
        let observation =
            std::path::Path::new(&directory).join(format!("committed-{}.txt", std::process::id()));
        let _ = std::fs::write(observation, format!("{max_files}\n{}", path.display()));
    }
}

#[cfg(debug_assertions)]
fn lossless_tee_max_files_for_test(configured: usize) -> usize {
    std::env::var("RTK_TEST_TEE_MAX_FILES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(configured)
}

#[cfg(not(debug_assertions))]
fn lossless_tee_max_files_for_test(configured: usize) -> usize {
    configured
}

/// Write raw output to a tee file in the given directory.
/// Returns file path on success.
fn write_tee_file(
    raw: &str,
    command_slug: &str,
    tee_dir: &std::path::Path,
    max_file_size: usize,
    max_files: usize,
) -> Option<PathBuf> {
    create_tee_dir(tee_dir)?;

    let slug = sanitize_slug(command_slug);
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let filename = format!("{}_{}.log", epoch, slug);
    let filepath = tee_dir.join(filename);

    // Truncate at max_file_size (find a safe UTF-8 char boundary)
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

    // Rotate old files
    cleanup_old_files(tee_dir, max_files);

    Some(filepath)
}

/// A complete raw-output artifact that is deleted unless its caller accepts
/// the associated compact display.
pub struct LosslessTeeReservation {
    pending_path: PathBuf,
    committed_path: PathBuf,
    max_files: usize,
    committed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LosslessTeeReservationError {
    Oversized,
    Unavailable,
}

/// A selected lossy CMD display whose recovery artifact remains protected
/// until the caller has written its hint to stdout.
pub struct LosslessTeeCommit {
    output: String,
    #[cfg(debug_assertions)]
    path: PathBuf,
    _process_lock: MutexGuard<'static, ()>,
    _interprocess_lock: std::fs::File,
}

impl LosslessTeeCommit {
    pub fn as_bytes(&self) -> &[u8] {
        self.output.as_bytes()
    }

    #[cfg(debug_assertions)]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl LosslessTeeReservation {
    #[cfg(test)]
    fn path(&self) -> &std::path::Path {
        &self.committed_path
    }

    /// Format the exact recovery hint without making the reservation durable.
    pub fn hint(&self) -> String {
        format!("[full output: {}]", self.recovery_command())
    }

    pub fn recovery_command(&self) -> String {
        format!("rtk read -l none --recovery {}", self.recovery_identifier())
    }

    pub fn recovery_identifier(&self) -> &str {
        self.committed_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("lossless recovery filenames are generated as ASCII")
    }

    fn cmd_hint(&self) -> String {
        self.hint()
    }

    /// Keep the complete artifact once the caller has selected compact output.
    fn commit_path_locked(&mut self) -> Option<PathBuf> {
        std::fs::rename(&self.pending_path, &self.committed_path).ok()?;
        cleanup_lossless_files_except(
            self.committed_path.parent().expect("tee path has parent"),
            self.max_files,
            Some(&self.committed_path),
        );
        self.committed = true;
        Some(self.committed_path.clone())
    }

    fn commit_with_lock(mut self, output: String) -> Option<LosslessTeeCommit> {
        let process_lock = LOSSLESS_TEE_COMMIT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        #[cfg(debug_assertions)]
        observe_test_lossless_tee_lock_attempt();
        let interprocess_lock = acquire_lossless_tee_commit_lock(
            self.committed_path.parent().expect("tee path has parent"),
        )?;
        #[cfg(debug_assertions)]
        wait_for_test_lossless_tee_commit_lock();
        let committed_path = self.commit_path_locked()?;
        #[cfg(debug_assertions)]
        observe_test_lossless_tee_commit(&committed_path, self.max_files);
        #[cfg(not(debug_assertions))]
        let _ = committed_path;
        Some(LosslessTeeCommit {
            output,
            #[cfg(debug_assertions)]
            path: committed_path,
            _process_lock: process_lock,
            _interprocess_lock: interprocess_lock,
        })
    }

    #[cfg(test)]
    fn commit_path(self) -> Option<PathBuf> {
        let output = self.hint();
        self.commit_with_lock(output).map(|commit| commit.path)
    }

    pub fn commit_hint(self) -> Option<String> {
        let hint = self.hint();
        self.commit_with_lock(hint).map(|commit| commit.output)
    }

    pub fn commit_output_if_better(self, raw: &str, output: String) -> Option<LosslessTeeCommit> {
        if !crate::core::ai_output::strictly_smaller(raw, &output) {
            return None;
        }
        self.commit_with_lock(output)
    }
}

impl Drop for LosslessTeeReservation {
    fn drop(&mut self) {
        if !self.committed {
            // nosemgrep: filesystem-deletion -- removes an uncommitted pending lossless tee artifact.
            let _ = std::fs::remove_file(&self.pending_path);
        }
    }
}

/// Commit a complete recovery artifact with a hint that can be pasted into
/// CMD directly. Other adapters retain the shell-neutral recovery hint.
pub fn commit_lossless_if_better_for_cmd(
    raw: &str,
    filtered: &str,
    reservation: LosslessTeeReservation,
) -> Option<LosslessTeeCommit> {
    let hint = reservation.cmd_hint();
    let shown = format!("{filtered}\r\n{hint}\r\n");
    if !crate::core::ai_output::strictly_smaller(raw, &shown) {
        return None;
    }
    reservation.commit_with_lock(shown)
}

/// Commit a complete recovery artifact for PowerShell's shell-neutral display.
/// The caller supplies a compact success-stream representation; the original
/// bytes remain available at the emitted recovery path.
pub fn commit_lossless_if_better_for_powershell(
    raw: &str,
    filtered: &str,
    reservation: LosslessTeeReservation,
) -> Option<LosslessTeeCommit> {
    let hint = reservation.hint();
    let shown = format!("{filtered}\r\n{hint}\r\n");
    if !crate::core::ai_output::strictly_smaller(raw, &shown) {
        return None;
    }
    reservation.commit_with_lock(shown)
}

/// Reserve a complete recovery artifact. Unlike the normal tee path, this
/// refuses oversized output rather than truncating a file advertised as full.
pub(crate) fn reserve_lossless_tee_file(
    raw: &str,
    command_slug: &str,
    tee_dir: &std::path::Path,
    max_file_size: usize,
    max_files: usize,
) -> Option<LosslessTeeReservation> {
    reserve_lossless_tee_file_with_limit(raw, command_slug, tee_dir, max_file_size, max_files)
}

fn reserve_lossless_tee_file_with_limit(
    raw: &str,
    command_slug: &str,
    tee_dir: &std::path::Path,
    max_file_size: usize,
    max_files: usize,
) -> Option<LosslessTeeReservation> {
    if raw.is_empty() || max_files == 0 || raw.len() > max_file_size {
        return None;
    }
    create_tee_dir(tee_dir)?;
    let slug = sanitize_slug(command_slug);
    use std::io::Write;
    for _ in 0..32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        let sequence = LOSSLESS_TEE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let committed_path = tee_dir.join(format!("{nanos}_{sequence}_{slug}.lossless.log"));
        let pending_path = tee_dir.join(format!("{nanos}_{sequence}_{slug}.lossless.pending"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        match crate::core::utils::open_private(&mut options, &pending_path) {
            Ok(mut file) => {
                if file.write_all(raw.as_bytes()).is_ok() {
                    return Some(LosslessTeeReservation {
                        pending_path,
                        committed_path,
                        max_files,
                        committed: false,
                    });
                }
                // nosemgrep: filesystem-deletion -- removes an incomplete pending tee artifact after a failed write.
                let _ = std::fs::remove_file(pending_path);
                return None;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Reserve a complete recovery artifact using the configured directory and
/// size/retention limits. Oversized output is reported separately so semantic
/// emitters can stay compact without silently retaining an unbounded payload.
pub(crate) fn reserve_lossless_tee_for_emission(
    raw: &str,
    command_slug: &str,
) -> Result<LosslessTeeReservation, LosslessTeeReservationError> {
    if std::env::var("RTK_TEE").ok().as_deref() == Some("0") || raw.is_empty() {
        return Err(LosslessTeeReservationError::Unavailable);
    }
    let config = Config::load().map_err(|_| LosslessTeeReservationError::Unavailable)?;
    if !config.tee.enabled {
        return Err(LosslessTeeReservationError::Unavailable);
    }
    let tee_dir = get_tee_dir(&config).ok_or(LosslessTeeReservationError::Unavailable)?;
    let max_files = lossless_tee_max_files_for_test(config.tee.max_files);
    reserve_lossless_tee_file(
        raw,
        command_slug,
        &tee_dir,
        config.tee.max_file_size,
        max_files,
    )
    .ok_or(if raw.len() > config.tee.max_file_size {
        LosslessTeeReservationError::Oversized
    } else {
        LosslessTeeReservationError::Unavailable
    })
}

/// Reserve a complete recovery artifact using the configured directory and
/// retention policy. Dropping the reservation removes an unselected artifact.
pub fn reserve_lossless_tee(raw: &str, command_slug: &str) -> Option<LosslessTeeReservation> {
    reserve_lossless_tee_for_emission(raw, command_slug).ok()
}

fn resolve_lossless_recovery_file(identifier: &str, tee_dir: &std::path::Path) -> Option<PathBuf> {
    if !identifier.ends_with(".lossless.log")
        || identifier.is_empty()
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return None;
    }
    let root = normalize_canonical_path(tee_dir.canonicalize().ok()?);
    let path = normalize_canonical_path(tee_dir.join(identifier).canonicalize().ok()?);
    if !path.starts_with(&root) || !path.is_file() {
        return None;
    }
    Some(path)
}

fn normalize_canonical_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let display = path.to_string_lossy();
        if let Some(rest) = display.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{}", rest));
        }
        if let Some(rest) = display.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

pub(crate) fn resolve_lossless_recovery(identifier: &str) -> Option<PathBuf> {
    let config = Config::load().ok()?;
    let tee_dir = get_tee_dir(&config)?;
    resolve_lossless_recovery_file(identifier, &tee_dir)
}

/// Write raw output to tee file if conditions are met.
/// Returns file path on success, None if skipped/failed.
pub fn tee_raw(raw: &str, command_slug: &str, exit_code: i32) -> Option<PathBuf> {
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

/// Convenience: tee + format hint in one call.
/// Returns hint string if file was written, None if skipped.
pub fn tee_and_hint(raw: &str, command_slug: &str, exit_code: i32) -> Option<String> {
    let path = tee_raw(raw, command_slug, exit_code)?;
    Some(format_hint(&path))
}

fn force_tee_path(content: &str, command_slug: &str) -> Option<PathBuf> {
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

    write_tee_file(
        content,
        command_slug,
        &tee_dir,
        config.tee.max_file_size,
        config.tee.max_files,
    )
}

/// Returns `[full output: ~/path]`, or None if tee is disabled/skipped.
pub fn force_tee_hint(raw: &str, command_slug: &str) -> Option<String> {
    let path = force_tee_path(raw, command_slug)?;
    Some(format_hint(&path))
}

/// Return a recovery hint only when the artifact contains the complete raw
/// output and has a collision-resistant, non-overwriting filename.
#[allow(dead_code)] // Retained for callers that need an immediate lossless hint.
pub fn force_tee_lossless_hint(raw: &str, command_slug: &str) -> Option<String> {
    reserve_lossless_tee(raw, command_slug)?.commit_hint()
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
        display_shell_path(&path)
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

    #[test]
    fn test_sanitize_slug() {
        assert_eq!(sanitize_slug("cargo_test"), "cargo_test");
        assert_eq!(sanitize_slug("cargo test"), "cargo_test");
        assert_eq!(sanitize_slug("cargo-test"), "cargo-test");
        assert_eq!(sanitize_slug("go/test/./pkg"), "go_test___pkg");
        // Long slugs (embedded paths) collapse to a readable prefix + hash, staying short.
        let long = format!("grep_0_{}", "a".repeat(50));
        let short = sanitize_slug(&long);
        assert!(
            short.len() < 24,
            "long slug should shorten, got '{}'",
            short
        );
        assert!(
            short.starts_with("grep_0_a"),
            "keeps a readable prefix, got '{}'",
            short
        );
        // Deterministic, and different slugs never collide onto the same filename.
        assert_eq!(sanitize_slug(&long), short);
        let other = sanitize_slug(&format!("grep_1_{}", "a".repeat(50)));
        assert_ne!(other, short, "distinct slugs must not collide");
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
    fn lossless_tee_artifacts_are_unique_and_complete() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let first = reserve_lossless_tee_file("first", "cmd-dir", tmpdir.path(), 1024, 20)
            .expect("first tee")
            .commit_path()
            .expect("first commit");
        let second = reserve_lossless_tee_file("second", "cmd-dir", tmpdir.path(), 1024, 20)
            .expect("second tee")
            .commit_path()
            .expect("second commit");
        assert_ne!(first, second);
        assert_eq!(fs::read_to_string(first).unwrap(), "first");
        assert_eq!(fs::read_to_string(second).unwrap(), "second");
    }

    #[test]
    fn lossless_tee_rejects_oversize_output_without_a_misleading_hint() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        assert!(
            reserve_lossless_tee_file(&"x".repeat(1025), "cmd-dir", tmpdir.path(), 1024, 20)
                .is_none()
        );
        assert!(fs::read_dir(tmpdir.path()).unwrap().next().is_none());
    }

    #[test]
    fn lossless_tee_artifacts_do_not_collide_under_concurrency() {
        let tmpdir = std::sync::Arc::new(tempfile::tempdir().expect("tempdir"));
        let paths = (0..8)
            .map(|index| {
                let tmpdir = std::sync::Arc::clone(&tmpdir);
                std::thread::spawn(move || {
                    reserve_lossless_tee_file(
                        &format!("{index}"),
                        "cmd-dir",
                        tmpdir.path(),
                        1024,
                        20,
                    )
                    .expect("lossless tee")
                    .commit_path()
                    .expect("lossless commit")
                })
            })
            .map(|thread| thread.join().expect("thread"))
            .collect::<Vec<_>>();
        assert_eq!(
            paths.iter().collect::<std::collections::HashSet<_>>().len(),
            8
        );
        for index in 0..8 {
            assert!(
                paths
                    .iter()
                    .any(|path| fs::read_to_string(path).unwrap() == index.to_string()),
                "concurrent tee {index} must remain complete"
            );
        }
    }

    #[test]
    fn uncommitted_lossless_reservation_removes_the_recovery_artifact() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let reservation = reserve_lossless_tee_file("raw", "cmd-dir", tmpdir.path(), 1024, 2)
            .expect("reservation");
        assert_eq!(fs::read_dir(tmpdir.path()).unwrap().count(), 1);
        drop(reservation);
        assert!(fs::read_dir(tmpdir.path()).unwrap().next().is_none());
    }

    #[test]
    fn recovery_command_uses_rtk_read() {
        let temp = tempfile::tempdir().unwrap();
        let reservation =
            reserve_lossless_tee_file("complete raw output", "cargo test", temp.path(), 1_024, 20)
                .unwrap();
        let command = reservation.recovery_command();
        assert!(command.starts_with("rtk read -l none --recovery "));
        assert!(!command.contains(&temp.path().display().to_string()));
        assert!(!command.contains("$HOME"));
        assert!(!command.contains('%'));
    }

    #[test]
    fn recovery_identifier_resolves_inside_a_metacharacter_directory() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("home & 100% ! $ ` space");
        let reservation =
            reserve_lossless_tee_file("complete raw output", "cargo test", &directory, 1_024, 20)
                .unwrap();
        let identifier = reservation.recovery_identifier().to_string();
        let expected = reservation.committed_path.clone();
        reservation.commit_hint().unwrap();

        assert_eq!(
            resolve_lossless_recovery_file(&identifier, &directory),
            Some(expected)
        );
    }

    #[test]
    fn recovery_identifier_rejects_paths_and_pending_artifacts() {
        let temp = tempfile::tempdir().unwrap();

        assert_eq!(
            resolve_lossless_recovery_file("../escape.lossless.log", temp.path()),
            None
        );
        assert_eq!(
            resolve_lossless_recovery_file("artifact.lossless.pending", temp.path()),
            None
        );
    }

    #[test]
    fn rejected_candidate_removes_pending_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let raw = "small";
        let reservation = reserve_lossless_tee_file(raw, "test", temp.path(), 1_024, 20).unwrap();
        assert!(reservation
            .commit_output_if_better(raw, "a much larger rendered candidate".to_string())
            .is_none());
        assert!(std::fs::read_dir(temp.path()).unwrap().next().is_none());
    }

    #[test]
    fn equal_token_candidate_is_rejected_and_removes_pending_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let raw = "abcd";
        let reservation = reserve_lossless_tee_file(raw, "test", temp.path(), 1_024, 20).unwrap();
        assert!(reservation
            .commit_output_if_better(raw, "wxyz".to_string())
            .is_none());
        assert!(std::fs::read_dir(temp.path()).unwrap().next().is_none());
    }

    #[test]
    fn lossless_reservation_is_not_a_retained_log_until_commit() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let reservation = reserve_lossless_tee_file("raw", "cmd-dir", tmpdir.path(), 1024, 1)
            .expect("reservation");
        assert!(
            !reservation.path().exists(),
            "a pending reservation must not advertise a committed log"
        );
        let pending = fs::read_dir(tmpdir.path())
            .unwrap()
            .next()
            .expect("pending artifact")
            .unwrap()
            .path();
        assert_eq!(
            pending.extension().and_then(|ext| ext.to_str()),
            Some("pending")
        );
    }

    #[test]
    fn final_never_worse_guard_aborts_lossless_recovery_without_an_artifact() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let reservation = reserve_lossless_tee_file("raw", "cmd-dir", tmpdir.path(), 1024, 2)
            .expect("reservation");
        let candidate = format!("summary\r\n{}\r\n", reservation.recovery_command());
        assert!(
            reservation
                .commit_output_if_better("raw", candidate)
                .is_none(),
            "the recovery hint makes this compact output worse than native raw output"
        );
        assert!(fs::read_dir(tmpdir.path()).unwrap().next().is_none());
    }

    #[test]
    fn cmd_lossless_recovery_hint_is_shell_neutral_and_path_free() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let raw = "native output\r\n".repeat(80);
        let reservation = reserve_lossless_tee_file(&raw, "cmd-dir", tmpdir.path(), 4096, 2)
            .expect("reservation");
        let commit = commit_lossless_if_better_for_cmd(&raw, "[dir] summary", reservation)
            .expect("compact display should win");
        let shown = std::str::from_utf8(commit.as_bytes()).unwrap();

        assert!(
            shown.contains("[full output: rtk read -l none --recovery "),
            "{shown}"
        );
        assert!(
            !shown.contains(&tmpdir.path().display().to_string()),
            "{shown}"
        );
        assert!(!shown.contains("$HOME"), "{shown}");
        assert!(!shown.contains("cat "), "{shown}");
    }

    #[test]
    fn committed_lossless_reservation_keeps_configured_number_of_complete_artifacts() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let first = reserve_lossless_tee_file("first", "cmd-dir", tmpdir.path(), 1024, 2)
            .expect("first")
            .commit_hint()
            .expect("first commit");
        let second = reserve_lossless_tee_file("second", "cmd-dir", tmpdir.path(), 1024, 2)
            .expect("second")
            .commit_hint()
            .expect("second commit");
        let third = reserve_lossless_tee_file("third", "cmd-dir", tmpdir.path(), 1024, 2)
            .expect("third")
            .commit_hint()
            .expect("third commit");

        assert!(first.contains(".log"));
        assert!(second.contains(".log"));
        assert!(third.contains(".log"));
        let artifacts = fs::read_dir(tmpdir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| is_lossless_log(&entry.path()))
            .collect::<Vec<_>>();
        assert_eq!(artifacts.len(), 2);
        assert!(artifacts
            .iter()
            .any(|entry| fs::read_to_string(entry.path()).unwrap() == "second"));
        assert!(artifacts
            .iter()
            .any(|entry| fs::read_to_string(entry.path()).unwrap() == "third"));
    }

    #[test]
    fn committed_lossless_reservation_never_deletes_its_own_hint_target() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let reservation = reserve_lossless_tee_file("selected", "cmd-dir", tmpdir.path(), 1024, 1)
            .expect("reservation");
        let selected = reservation.path().to_path_buf();
        fs::write(
            tmpdir.path().join("999999999999999999999_future.log"),
            "other",
        )
        .unwrap();

        reservation.commit_hint().expect("selected commit");

        assert!(selected.exists(), "returned recovery hint must resolve");
        assert_eq!(
            fs::read_dir(tmpdir.path())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| is_lossless_log(&entry.path()))
                .count(),
            1
        );
    }

    #[test]
    fn ordinary_tee_rotation_never_removes_lossless_recovery_artifacts() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let reservation = reserve_lossless_tee_file("recovery", "cmd-set", tmpdir.path(), 1024, 1)
            .expect("reservation");
        let recovery = reservation.path().to_path_buf();
        reservation.commit_hint().expect("lossless commit");

        write_tee_file("ordinary-one", "ordinary", tmpdir.path(), 1024, 1)
            .expect("first ordinary tee");
        write_tee_file("ordinary-two", "ordinary-next", tmpdir.path(), 1024, 1)
            .expect("second ordinary tee");

        assert!(
            recovery.is_file(),
            "ordinary rotation must not delete recovery"
        );
        assert_eq!(fs::read_to_string(recovery).unwrap(), "recovery");
    }

    #[test]
    fn concurrent_max_one_commit_never_returns_a_deleted_reservation_target() {
        use std::sync::{mpsc, Arc, Barrier};

        let tmpdir = Arc::new(tempfile::tempdir().expect("tempdir"));
        let first = reserve_lossless_tee_file("first", "cmd-dir", tmpdir.path(), 1024, 1)
            .expect("first reservation");
        let first_target = first.path().to_path_buf();
        let second = reserve_lossless_tee_file("second", "cmd-dir", tmpdir.path(), 1024, 1)
            .expect("second reservation");
        let second_target = second.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(2));
        let (second_committed, first_can_commit) = mpsc::channel();

        let first_barrier = Arc::clone(&barrier);
        let first_thread = std::thread::spawn(move || {
            first_barrier.wait();
            first_can_commit.recv().expect("second commit signal");
            let _hint = first.commit_hint().expect("first commit");
            assert!(
                first_target.exists(),
                "first current hint target survives its return"
            );
            assert_eq!(fs::read_to_string(&first_target).unwrap(), "first");
        });

        let second_barrier = Arc::clone(&barrier);
        let second_thread = std::thread::spawn(move || {
            second_barrier.wait();
            let _hint = second.commit_hint().expect("second commit");
            assert!(
                second_target.exists(),
                "second current hint target survives its return"
            );
            assert_eq!(fs::read_to_string(&second_target).unwrap(), "second");
            second_committed.send(()).expect("first commit signal");
        });

        first_thread.join().expect("first commit thread");
        second_thread.join().expect("second commit thread");
        assert_eq!(
            fs::read_dir(tmpdir.path())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "log"))
                .count(),
            1
        );
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
    fn configured_tee_directory_has_precedence_over_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let configured = temp.path().join("custom-tee");
        let mut config = Config::default();
        config.tee.directory = Some(configured.clone());
        assert_eq!(get_tee_dir_with_env(&config, None), Some(configured));
    }

    #[test]
    fn test_force_tee_hint_respects_env_disable() {
        // When RTK_TEE=0, force_tee_hint should return None
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
}
