//! Utility functions for text processing and command execution.
//!
//! Provides common helpers used across rtk commands:
//! - ANSI color code stripping
//! - Text truncation
//! - Command execution with error context

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;
use std::sync::OnceLock;

/// Compute `days` ago from now, clamped instead of panicking on overflow.
///
/// `days` is typically user-supplied (`--since <days>`) and unbounded. Naively
/// building `chrono::Duration::days(days as i64)` and subtracting it from
/// `Utc::now()` panics for a large enough value ("DateTime - TimeDelta
/// overflowed") — this happened identically in `rtk discover --since` and `rtk
/// hook audit --since` before both were routed through this helper. `days` is
/// capped to `i64::MAX` before the `as i64` cast (a `u64` past `i64::MAX` would
/// otherwise reinterpret as negative, turning "look back" into "look forward"),
/// and any remaining overflow building or applying the `Duration` falls back to
/// `DateTime::<Utc>::MIN_UTC` — an arbitrarily distant cutoff means "no lower
/// bound", which is exactly what an absurdly large `--since` should behave like.
pub fn days_ago_cutoff(days: u64) -> DateTime<Utc> {
    let days = days.min(i64::MAX as u64) as i64;
    chrono::Duration::try_days(days)
        .and_then(|d| Utc::now().checked_sub_signed(d))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

/// Truncates a string to `max_len` characters, appending `...` if needed.
///
/// # Arguments
/// * `s` - The string to truncate
/// * `max_len` - Maximum length before truncation (minimum 3 to include "...")
///
/// # Examples
/// ```
/// use rtk::utils::truncate;
/// assert_eq!(truncate("hello world", 8), "hello...");
/// assert_eq!(truncate("hi", 10), "hi");
/// ```
pub fn truncate(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else if max_len < 3 {
        // If max_len is too small, just return "..."
        "...".to_string()
    } else {
        format!("{}...", s.chars().take(max_len - 3).collect::<String>())
    }
}

/// Strip ANSI escape codes (colors, styles) from a string.
///
/// # Arguments
/// * `text` - Text potentially containing ANSI escape codes
///
/// # Examples
/// ```
/// use rtk::utils::strip_ansi;
/// let colored = "\x1b[31mError\x1b[0m";
/// assert_eq!(strip_ansi(colored), "Error");
/// ```
pub fn strip_ansi(text: &str) -> String {
    static ANSI_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap());

    ANSI_RE.replace_all(text, "").to_string()
}

/// Strip one or more leading UTF-8 BOMs (`EF BB BF`, sometimes doubled),
/// which serde_json refuses to parse. Windows editors (Notepad, PowerShell
/// 5.1 `Out-File -Encoding utf8`) prepend one to hand-edited config files,
/// and Cursor on Windows ships hook payloads with them. Strip defensively
/// wherever RTK parses JSON a human or another tool may have written.
pub fn strip_leading_bom(input: &str) -> &str {
    let mut s = input;
    while let Some(rest) = s.strip_prefix('\u{feff}') {
        s = rest;
    }
    s
}

/// BOM-tolerant `serde_json::from_str`. Prefer this over `serde_json::from_str`
/// anywhere the JSON may have been written by a human, an editor, or another tool.
pub fn from_json_str<'a, T: serde::Deserialize<'a>>(s: &'a str) -> serde_json::Result<T> {
    serde_json::from_str(strip_leading_bom(s))
}

/// Executes a command and returns cleaned stdout/stderr.
///
/// # Arguments
/// * `cmd` - Command to execute (e.g., "eslint")
/// * `args` - Command arguments
///
/// # Returns
/// `(stdout: String, stderr: String, exit_code: i32)`
/// Formats a token count with K/M suffixes for readability.
///
/// # Arguments
/// * `n` - Number of tokens
///
/// # Returns
/// Formatted string (e.g., "1.2M", "59.2K", "694")
///
/// # Examples
/// ```
/// use rtk::utils::format_tokens;
/// assert_eq!(format_tokens(1_234_567), "1.2M");
/// assert_eq!(format_tokens(59_234), "59.2K");
/// assert_eq!(format_tokens(694), "694");
/// ```
pub fn format_tokens(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

/// Formats a USD amount with adaptive precision.
///
/// # Arguments
/// * `amount` - Amount in dollars
///
/// # Returns
/// Formatted string with $ prefix
///
/// # Examples
/// ```
/// use rtk::utils::format_usd;
/// assert_eq!(format_usd(1234.567), "$1234.57");
/// assert_eq!(format_usd(12.345), "$12.35");
/// assert_eq!(format_usd(0.123), "$0.12");
/// assert_eq!(format_usd(0.0096), "$0.0096");
/// ```
pub fn format_usd(amount: f64) -> String {
    if !amount.is_finite() {
        return "$0.00".to_string();
    }
    if amount >= 0.01 {
        format!("${:.2}", amount)
    } else {
        format!("${:.4}", amount)
    }
}

/// Format cost-per-token as $/MTok (e.g., "$3.86/MTok")
///
/// # Arguments
/// * `cpt` - Cost per token (not per million tokens)
///
/// # Returns
/// Formatted string like "$3.86/MTok"
///
/// # Examples
/// ```
/// use rtk::utils::format_cpt;
/// assert_eq!(format_cpt(0.000003), "$3.00/MTok");
/// assert_eq!(format_cpt(0.0000038), "$3.80/MTok");
/// assert_eq!(format_cpt(0.00000386), "$3.86/MTok");
/// ```
pub fn format_cpt(cpt: f64) -> String {
    if !cpt.is_finite() || cpt <= 0.0 {
        return "$0.00/MTok".to_string();
    }
    let cpt_per_million = cpt * 1_000_000.0;
    format!("${:.2}/MTok", cpt_per_million)
}

/// Join items into a newline-separated string, appending an overflow hint when total > max.
///
/// # Examples
/// ```
/// use rtk::utils::join_with_overflow;
/// let items = vec!["a".to_string(), "b".to_string()];
/// assert_eq!(join_with_overflow(&items, 5, 3, "items"), "a\nb\n... +2 more items");
/// assert_eq!(join_with_overflow(&items, 2, 3, "items"), "a\nb");
/// ```
pub fn join_with_overflow(items: &[String], total: usize, max: usize, label: &str) -> String {
    let mut out = items.join("\n");
    if total > max {
        out.push_str(&format!("\n… +{} more {}", total - max, label));
    }
    out
}

/// Truncate an ISO 8601 datetime string to just the date portion (first 10 chars).
///
/// # Examples
/// ```
/// use rtk::utils::truncate_iso_date;
/// assert_eq!(truncate_iso_date("2024-01-15T10:30:00Z"), "2024-01-15");
/// assert_eq!(truncate_iso_date("2024-01-15"), "2024-01-15");
/// assert_eq!(truncate_iso_date("short"), "short");
/// ```
pub fn truncate_iso_date(date: &str) -> &str {
    if date.len() >= 10 {
        &date[..10]
    } else {
        date
    }
}

/// Format a confirmation message: "ok \<action\> \<detail\>"
/// Used for write operations (merge, create, comment, edit, etc.)
///
/// # Examples
/// ```
/// use rtk::utils::ok_confirmation;
/// assert_eq!(ok_confirmation("merged", "#42"), "ok merged #42");
/// assert_eq!(ok_confirmation("created", "PR #5 https://..."), "ok created PR #5 https://...");
/// ```
pub fn ok_confirmation(action: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("ok {}", action)
    } else {
        format!("ok {} {}", action, detail)
    }
}

/// Extract exit code from a process output. Returns the actual exit code, or
/// `128 + signal` per Unix convention when terminated by a signal (no exit code
/// available). Falls back to 1 on non-Unix platforms.
pub fn exit_code_from_output(output: &std::process::Output, label: &str) -> i32 {
    match output.status.code() {
        Some(code) => code,
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = output.status.signal() {
                    eprintln!("[rtk] {}: process terminated by signal {}", label, sig);
                    return 128 + sig;
                }
            }
            eprintln!("[rtk] {}: process terminated by signal", label);
            1
        }
    }
}

/// Extract exit code from an ExitStatus (for `.status()` calls, not `.output()`).
/// Returns the actual exit code, or `128 + signal` per Unix convention when
/// terminated by a signal. Falls back to 1 on non-Unix platforms.
pub fn exit_code_from_status(status: &std::process::ExitStatus, label: &str) -> i32 {
    match status.code() {
        Some(code) => code,
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = status.signal() {
                    eprintln!("[rtk] {}: process terminated by signal {}", label, sig);
                    return 128 + sig;
                }
            }
            eprintln!("[rtk] {}: process terminated by signal", label);
            1
        }
    }
}

/// Return the last `n` lines of output with a label, for use as a fallback
/// when filter parsing fails. Logs a diagnostic to stderr.
pub fn fallback_tail(output: &str, label: &str, n: usize) -> String {
    eprintln!(
        "[rtk] {}: output format not recognized, showing last {} lines",
        label, n
    );
    let lines: Vec<&str> = output.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Create a directory owner-only (0700 on Unix), tightening one that already exists.
pub fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    set_owner_only(path, 0o700);
    Ok(())
}

/// Restrict an existing file to owner-only access (0600 on Unix).
pub fn restrict_file(path: &std::path::Path) {
    set_owner_only(path, 0o600);
}

/// Open a file owner-only (0600 on Unix), applied at creation so content is
/// never briefly readable under a permissive umask. `mode` is ignored for a
/// file that already exists, so an older one is still tightened afterwards.
pub fn open_private(
    opts: &mut fs::OpenOptions,
    path: &std::path::Path,
) -> std::io::Result<fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    set_owner_only(path, 0o600);
    Ok(file)
}

/// Pure core of `set_owner_only`'s "is a chmod even needed" check, taking the raw
/// `st_mode` reading directly so it's deterministically testable without touching
/// the filesystem (an unprivileged `chmod` setting setuid/setgid isn't reliably
/// honored across every environment — e.g. some sandboxed/containerized
/// filesystems silently drop it — so a test that round-trips through a real file
/// would be flaky rather than actually exercising this logic).
///
/// Masks with `0o7777`, NOT `0o777`: setuid/setgid/sticky live in the `0o7000`
/// range, above the rwx bits but below the file-type bits `st_mode` also carries.
/// A `0o777` mask would strip those dangerous bits out of the comparison too, so
/// a file with e.g. setuid set (`0o4600`) would read as "already correct" against
/// a `target` of `0o600` and permanently skip the chmod that would clear it —
/// defeating the self-heal `set_owner_only` exists to provide, for exactly the
/// files (external tampering, restored backups) it's meant to catch.
#[cfg(unix)]
fn mode_already_correct(actual: u32, target: u32) -> bool {
    actual & 0o7777 == target
}

#[cfg(unix)]
fn set_owner_only(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    // Skip the chmod syscall once permissions already match. This runs on every
    // `Tracker::new()` call, which is now on the PreToolUse hook's hot path for
    // *every* Bash tool call (not just RTK-covered ones — see rtk-ai/rtk#3206's
    // hook_decisions ground-truth logging), under the project's <10ms latency
    // budget, so a redundant chmod on the already-correct common case adds up.
    // Falls through to chmod on any metadata-read failure, erring toward
    // enforcing the permission rather than silently skipping it.
    //
    if let Ok(meta) = fs::metadata(path) {
        if mode_already_correct(meta.permissions().mode(), mode) {
            return;
        }
    }
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_owner_only(_path: &std::path::Path, _mode: u32) {}

/// Build a Command for Ruby tools, auto-detecting bundle exec.
/// Uses `bundle exec <tool>` when a Gemfile exists (transitive deps like rake
/// won't appear in the Gemfile but still need bundler for version isolation).
pub fn ruby_exec(tool: &str) -> Command {
    if std::path::Path::new("Gemfile").exists() {
        let mut c = Command::new("bundle");
        c.arg("exec").arg(tool);
        return c;
    }
    Command::new(tool)
}

/// Count whitespace-delimited tokens in text. Used by filter tests to verify
/// token savings claims.
#[cfg(test)]
pub fn count_tokens(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Detect the package manager used in the current directory.
/// Returns "pnpm", "yarn", "bun", or "npm" based on lockfile presence.
///
/// # Examples
/// ```no_run
/// use rtk::utils::detect_package_manager;
/// let pm = detect_package_manager();
/// // "pnpm" for pnpm-lock.yaml, "yarn" for yarn.lock, "bun" for bun.lock(b), else "npm"
/// ```
#[allow(dead_code)]
pub fn detect_package_manager() -> &'static str {
    detect_package_manager_in(std::path::Path::new("."))
}

/// Lockfile detection against an explicit directory, so callers (and tests) do
/// not have to move the process's current directory to ask the question.
pub fn detect_package_manager_in(dir: &std::path::Path) -> &'static str {
    if dir.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if dir.join("yarn.lock").exists() {
        "yarn"
    } else if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
        "bun"
    } else {
        "npm"
    }
}

/// Build a Command using the detected package manager's exec mechanism.
/// Returns a Command ready to have tool-specific args appended.
pub fn package_manager_exec(tool: &str) -> Command {
    tool_exec(None, tool, MissingTool::Fail)
}

/// What the npm arm does when `tool` is not installed. `npx` fetches on demand
/// by default, which is right for a tool the user named themselves and
/// surprising for one rtk picked on their behalf.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MissingTool {
    Fetch,
    Fail,
}

/// The runner used to resolve a tool that is not on PATH. A runner the user
/// named wins outright. Otherwise lockfile detection picks one, except that a
/// detected bun resolves through npx: bunx always fetches a missing tool into
/// its cache and cannot be told not to, so `MissingTool` could not be honoured
/// and the presence of a lockfile alone would decide what gets downloaded.
pub fn exec_runner(runner: Option<&str>, missing: MissingTool) -> &str {
    match runner {
        Some(named) => named,
        // Nothing was named and rtk may fetch: npx is the only runner that can,
        // and lockfile detection would hand `pnpm exec` a tool the project
        // does not have.
        None if missing == MissingTool::Fetch => "npx",
        None => match detect_package_manager() {
            // bunx always fetches; npm's runner is npx.
            "bun" | "npm" => "npx",
            other => other,
        },
    }
}

/// Build a Command that runs `tool`, preferring the package runner the user
/// actually named over lockfile detection.
///
/// RTK forwards what was typed rather than substituting an engine for it: a
/// user who types `bunx tsc` must not get `pnpm` because a lockfile is present.
/// Detection applies only when nothing was named, as with a bare `rtk tsc`.
///
/// A tool already on PATH is run directly only when no runner was named.
pub fn tool_exec(runner: Option<&str>, tool: &str, missing: MissingTool) -> Command {
    // Only when nothing was named: `bunx tsc` must resolve the project's tsc,
    // not a global one that happens to be on PATH. Both bunx and npx prefer
    // node_modules/.bin before fetching, so naming one is a real choice.
    if runner.is_none() && tool_exists(tool) {
        resolved_command(tool)
    } else {
        match exec_runner(runner, missing) {
            "pnpm" => {
                let mut c = resolved_command("pnpm");
                c.arg("exec").arg("--").arg(tool);
                c
            }
            "yarn" => {
                let mut c = resolved_command("yarn");
                c.arg("exec").arg("--").arg(tool);
                c
            }
            "bun" | "bunx" => {
                let mut c = resolved_command("bunx");
                c.arg(tool);
                c
            }
            _ => {
                let mut c = resolved_command("npx");
                if missing == MissingTool::Fail {
                    c.arg("--no-install");
                }
                c.arg("--").arg(tool);
                c
            }
        }
    }
}

/// Resolve a binary name to its full path, honoring PATHEXT on Windows.
///
/// On Windows, Node.js tools are installed as `.CMD`/`.BAT`/`.PS1` shims.
/// Rust's `std::process::Command::new()` does NOT honor PATHEXT, so
/// `Command::new("vitest")` fails even when `vitest.CMD` is on PATH.
///
/// This function uses the `which` crate to perform proper PATH+PATHEXT resolution.
///
/// # Arguments
/// * `name` - Binary name (e.g., "vitest", "eslint", "tsc")
///
/// # Returns
/// Full path to the resolved binary, or error if not found.
pub fn resolve_binary(name: &str) -> Result<PathBuf> {
    which::which(name).context(format!("Binary '{}' not found on PATH", name))
}

/// Create a `Command` with PATHEXT-aware binary resolution.
///
/// Drop-in replacement for `Command::new(name)` that works on Windows
/// with `.CMD`/`.BAT`/`.PS1` wrappers.
///
/// Falls back to `Command::new(name)` if resolution fails, so native
/// commands (git, cargo) still work even if `which` can't find them.
///
/// # Arguments
/// * `name` - Binary name (e.g., "vitest", "eslint")
///
/// # Returns
/// A `Command` configured with the resolved binary path.
pub fn resolved_command(name: &str) -> Command {
    match resolve_binary(name) {
        Ok(path) => Command::new(path),
        Err(e) => {
            // On Windows, resolution failure likely means a .CMD/.BAT wrapper
            // wasn't found — always warn so users have a signal.
            // On Unix, this is less common; only log in debug builds.
            if cfg!(any(target_os = "windows", debug_assertions)) {
                eprintln!(
                    "rtk: Failed to resolve '{}' via PATH, falling back to direct exec: {}",
                    name, e
                );
            }

            Command::new(name)
        }
    }
}

/// Return Composer bin directories in precedence order.
///
/// Composer allows overriding the default `vendor/bin` via `COMPOSER_BIN_DIR`
/// or `composer.json` `config.bin-dir`. Keep the default as a fallback so we
/// continue recognizing the common layout even when the repo is not configured.
pub fn composer_bin_dirs() -> Vec<PathBuf> {
    // Resolution depends only on the process's env + cwd composer.json, both
    // constant for a single rtk invocation. The rewrite hot path queries this
    // several times per command segment, so read the file once and cache.
    static CACHE: OnceLock<Vec<PathBuf>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let env_bin_dir = std::env::var("COMPOSER_BIN_DIR").ok();
            let composer_json = fs::read_to_string("composer.json").ok();
            composer_bin_dirs_from(env_bin_dir.as_deref(), composer_json.as_deref())
        })
        .clone()
}

pub fn composer_tool_paths(tool: &str) -> Vec<PathBuf> {
    composer_bin_dirs()
        .into_iter()
        .map(|dir| dir.join(tool))
        .collect()
}

fn composer_bin_dirs_from(env_bin_dir: Option<&str>, composer_json: Option<&str>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(dir) = env_bin_dir
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| composer_json.and_then(read_composer_bin_dir))
    {
        dirs.push(dir);
    }

    let default_dir = PathBuf::from("vendor/bin");
    if !dirs.iter().any(|dir| dir == &default_dir) {
        dirs.push(default_dir);
    }

    dirs
}

fn read_composer_bin_dir(composer_json: &str) -> Option<PathBuf> {
    let parsed: Value = from_json_str(composer_json).ok()?;
    let bin_dir = parsed.get("config")?.get("bin-dir")?.as_str()?.trim();
    if bin_dir.is_empty() {
        None
    } else {
        Some(PathBuf::from(bin_dir))
    }
}

/// Join lines with newlines, or return "ok" when there is nothing left to show.
///
/// Shared by output filters that collapse fully to a success marker.
pub fn join_or_ok(lines: &[&str]) -> String {
    if lines.is_empty() {
        "ok".to_string()
    } else {
        lines.join("\n")
    }
}

/// Check if a tool exists on PATH (PATHEXT-aware on Windows).
///
/// Replaces manual `Command::new("which").arg(tool)` checks that fail on Windows.
pub fn tool_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

/// Check if a compile-time environment variable was set to a non-empty value.
///
/// `option_env!` yields `Some("")` when the build environment exports the
/// variable with an empty value, which a CI `env:` block fed by an unset
/// repository variable does. `.is_some()` alone then reports a feature as
/// configured when it is not, so pair every `option_env!` gate with this.
///
/// # Examples
/// ```
/// use rtk::utils::env_is_some;
/// assert!(env_is_some(Some("https://example.com")));
/// assert!(!env_is_some(Some("")));
/// assert!(!env_is_some(None));
/// ```
pub fn env_is_some(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty())
}

/// Extract short name from AWS ARN.
/// Example: `arn:aws:ecs:region:acct:service/cluster/name` -> `name`
/// For simple ARNs like `arn:aws:iam::123:user/alice`, returns `alice`.
pub fn shorten_arn(arn: &str) -> &str {
    // ARNs use "/" or ":" as separators. Try "/" first (service/cluster/name pattern),
    // then fall back to ":" for Lambda/IAM ARNs.
    let slash_result = arn.rsplit('/').next().unwrap_or(arn);
    // If rsplit('/') returned the whole string (no '/' found), try ':'
    if slash_result == arn {
        arn.rsplit(':').next().unwrap_or(arn)
    } else {
        slash_result
    }
}

/// Convert bytes to human-readable format (KB, MB, GB, TB).
/// Used for S3 object sizes.
pub fn human_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Decode child process output bytes, respecting the Windows console code page.
///
/// Valid UTF-8 is returned untouched — the overwhelmingly common case, and the
/// only one on Unix. Otherwise the buffer is decoded a line at a time: lines
/// that are valid UTF-8 keep their bytes, and only the lines that are not get
/// re-decoded through the console code page (GBK, Big5, CP850, …). Decoding
/// the whole buffer as a legacy code page at the first bad byte would mangle
/// output that was almost entirely valid UTF-8.
///
/// Falls back to lossy UTF-8 (`U+FFFD`) when no code page applies, matching the
/// previous behavior.
pub fn decode_process_output(bytes: &[u8]) -> String {
    // Fast path: fully valid UTF-8 needs no scanning and no allocation beyond
    // the copy. This is every Unix run and every UTF-8 console on Windows.
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => decode_mixed(bytes, output_codepage()),
    }
}

/// Decode `bytes` line by line, keeping valid UTF-8 lines verbatim and passing
/// the rest through code page `cp` (lossy UTF-8 when `cp` is `None`).
///
/// The line is the decoding unit because a byte run is not one: GB18030's
/// four-byte sequences embed bytes in the ASCII digit range, so any rule that
/// stops a run at the first byte under `0x80` splits them. `\n` is unambiguous
/// in every encoding handled here — none of UTF-8, GBK, gb18030, Big5,
/// Shift_JIS, EUC-KR or the single-byte pages can produce it as a trail byte —
/// and a process does not switch encoding mid-line, so the whole line can be
/// handed to one decoder.
///
/// Takes the code page as a parameter so the walk and the mapping are
/// unit-testable on every platform, not only Windows.
fn decode_mixed(bytes: &[u8], cp: Option<u16>) -> String {
    let mut out = String::with_capacity(bytes.len());
    // `split_inclusive` keeps the terminator, so line endings survive intact
    // and an empty input yields no chunks at all.
    for line in bytes.split_inclusive(|&b| b == b'\n') {
        match std::str::from_utf8(line) {
            Ok(valid) => out.push_str(valid),
            Err(_) => out.push_str(&decode_line(line, cp)),
        }
    }
    out
}

/// Decode one line that failed UTF-8 validation using code page `cp`.
///
/// A code page result is only accepted when it decodes cleanly. A line that is
/// really UTF-8 with a corrupt byte usually fails the code page decoder too,
/// and lossy UTF-8 preserves its valid characters where the code page would
/// turn all of them into mojibake.
fn decode_line(line: &[u8], cp: Option<u16>) -> String {
    if let Some(cp) = cp {
        // encoding_rs covers the ANSI and DBCS pages (1252, GBK, gb18030,
        // Shift_JIS, Big5, …); `codepage` maps the Windows page number onto it.
        if let Some(encoding) = codepage::to_encoding(cp) {
            let (decoded, _, had_errors) = encoding.decode(line);
            if !had_errors {
                return decoded.into_owned();
            }
        // encoding_rs implements only WHATWG encodings, which exclude the
        // legacy OEM/DOS pages (437, 850, 852, …) that plain cmd.exe still
        // defaults to in many locales. `oem_cp` supplies those tables.
        } else if let Some(table) = oem_cp::code_table::DECODING_TABLE_CP_MAP.get(&cp) {
            if let Some(decoded) = table.decode_string_checked(line) {
                return decoded;
            }
        } else {
            warn_unmapped_codepage(cp);
        }
    }
    String::from_utf8_lossy(line).into_owned()
}

/// Warn once that output is being decoded lossily because the console code
/// page has no known table — previously this fell back silently.
fn warn_unmapped_codepage(cp: u16) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "[rtk] warning: no decoder for console code page {}; \
             non-UTF-8 output will be shown with replacement characters",
            cp
        );
    });
}

/// The code page child output should be decoded with, or `None` when the
/// platform has no such concept (every Unix) and lossy UTF-8 should be used.
#[cfg(not(windows))]
fn output_codepage() -> Option<u16> {
    None
}

/// Windows: the console output code page, cached after the first lookup.
///
/// `GetConsoleOutputCP` describes the console rtk is attached to, which the
/// child inherits. It returns 0 when there is no console — rtk running under a
/// hook with its output piped, the common case flagged in review — and a
/// console program writing to a pipe uses the ANSI code page instead, so
/// `GetACP` is the fallback rather than giving up and decoding lossily.
///
/// This remains a best guess: a child is free to emit any encoding regardless
/// of either code page. It is only ever consulted for bytes that already
/// failed UTF-8 validation, so a wrong guess degrades to the same replacement
/// characters that the previous lossy conversion produced.
#[cfg(windows)]
fn output_codepage() -> Option<u16> {
    static CODEPAGE: OnceLock<Option<u16>> = OnceLock::new();
    *CODEPAGE.get_or_init(|| {
        #[allow(unsafe_code)]
        // nosemgrep: unsafe-block — read-only Win32 APIs, no memory or thread safety risk
        let cp = unsafe {
            let console = windows_sys::Win32::System::Console::GetConsoleOutputCP();
            if console != 0 {
                console
            } else {
                windows_sys::Win32::Globalization::GetACP()
            }
        };
        u16::try_from(cp).ok().filter(|&cp| cp != 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_leading_bom_helper() {
        // Direct unit test on the helper so future refactors can't
        // regress the loop semantics without a clear failure signal.
        assert_eq!(strip_leading_bom(""), "");
        assert_eq!(strip_leading_bom("hello"), "hello");
        assert_eq!(strip_leading_bom("\u{feff}hello"), "hello");
        assert_eq!(strip_leading_bom("\u{feff}\u{feff}hello"), "hello");
        assert_eq!(strip_leading_bom("\u{feff}\u{feff}\u{feff}hello"), "hello");
        // BOM in the middle is preserved (not "leading").
        assert_eq!(strip_leading_bom("a\u{feff}b"), "a\u{feff}b");
    }

    #[test]
    fn test_from_json_str_strips_bom() {
        let v: Value = from_json_str("\u{feff}{\"foo\": 1}").unwrap();
        assert_eq!(v["foo"], 1);
    }

    #[test]
    fn test_from_json_str_no_bom() {
        let v: Value = from_json_str("{\"foo\": 1}").unwrap();
        assert_eq!(v["foo"], 1);
    }

    #[test]
    fn test_from_json_str_borrowed_type() {
        // Deserialize<'a> (not DeserializeOwned) must keep zero-copy
        // borrowing working through the wrapper.
        #[derive(serde::Deserialize)]
        struct Borrowed<'a> {
            name: &'a str,
        }
        let b: Borrowed = from_json_str("\u{feff}{\"name\": \"rtk\"}").unwrap();
        assert_eq!(b.name, "rtk");
    }

    #[test]
    fn env_is_some_rejects_unset_and_empty() {
        assert!(env_is_some(Some("https://telemetry.example")));
        // An unset CI repository variable still exports the env var, so the
        // empty string reaches `option_env!` as `Some("")`.
        assert!(!env_is_some(Some("")));
        assert!(!env_is_some(None));
    }

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_days_ago_cutoff_normal_value_is_in_the_past() {
        let cutoff = days_ago_cutoff(30);
        assert!(cutoff < Utc::now());
    }

    #[test]
    fn test_days_ago_cutoff_does_not_panic_on_huge_since_days() {
        // rtk-ai/rtk#3206 review: `rtk discover --since 100000000` and `rtk hook
        // audit --since 100000000` both panicked with "DateTime - TimeDelta
        // overflowed". Must clamp instead of crashing.
        assert_eq!(days_ago_cutoff(100_000_000), DateTime::<Utc>::MIN_UTC);
    }

    #[test]
    fn test_days_ago_cutoff_does_not_panic_on_u64_max() {
        assert_eq!(days_ago_cutoff(u64::MAX), DateTime::<Utc>::MIN_UTC);
    }

    #[test]
    fn test_truncate_long_string() {
        let result = truncate("hello world", 8);
        assert_eq!(result, "hello...");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_edge_case() {
        // max_len < 3 returns just "..."
        assert_eq!(truncate("hello", 2), "...");
        // When string length equals max_len, return as is
        assert_eq!(truncate("abc", 3), "abc");
        // When string is longer and max_len is exactly 3, return "..."
        assert_eq!(truncate("hello world", 3), "...");
    }

    #[test]
    fn test_composer_bin_dirs_use_default_when_unconfigured() {
        assert_eq!(
            composer_bin_dirs_from(None, None),
            vec![PathBuf::from("vendor/bin")]
        );
    }

    #[test]
    fn test_composer_bin_dirs_prefer_env_override() {
        let composer_json = r#"{"config":{"bin-dir":"tools/bin"}}"#;
        assert_eq!(
            composer_bin_dirs_from(Some("custom/bin"), Some(composer_json)),
            vec![PathBuf::from("custom/bin"), PathBuf::from("vendor/bin")]
        );
    }

    #[test]
    fn test_composer_bin_dirs_read_composer_config() {
        let composer_json = r#"{"config":{"bin-dir":"tools/bin"}}"#;
        assert_eq!(
            composer_bin_dirs_from(None, Some(composer_json)),
            vec![PathBuf::from("tools/bin"), PathBuf::from("vendor/bin")]
        );
    }

    #[test]
    fn test_strip_ansi_simple() {
        let input = "\x1b[31mError\x1b[0m";
        assert_eq!(strip_ansi(input), "Error");
    }

    #[test]
    fn test_strip_ansi_multiple() {
        let input = "\x1b[1m\x1b[32mSuccess\x1b[0m\x1b[0m";
        assert_eq!(strip_ansi(input), "Success");
    }

    #[test]
    fn test_strip_ansi_no_codes() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn test_strip_ansi_complex() {
        let input = "\x1b[32mGreen\x1b[0m normal \x1b[31mRed\x1b[0m";
        assert_eq!(strip_ansi(input), "Green normal Red");
    }

    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(format_tokens(1_234_567), "1.2M");
        assert_eq!(format_tokens(12_345_678), "12.3M");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(format_tokens(59_234), "59.2K");
        assert_eq!(format_tokens(1_000), "1.0K");
    }

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(694), "694");
        assert_eq!(format_tokens(0), "0");
    }

    #[test]
    fn test_format_usd_large() {
        assert_eq!(format_usd(1234.567), "$1234.57");
        assert_eq!(format_usd(1000.0), "$1000.00");
    }

    #[test]
    fn test_format_usd_medium() {
        assert_eq!(format_usd(12.345), "$12.35");
        assert_eq!(format_usd(0.99), "$0.99");
    }

    #[test]
    fn test_format_usd_small() {
        assert_eq!(format_usd(0.0096), "$0.0096");
        assert_eq!(format_usd(0.0001), "$0.0001");
    }

    #[test]
    fn test_format_usd_edge() {
        assert_eq!(format_usd(0.01), "$0.01");
        assert_eq!(format_usd(0.009), "$0.0090");
    }

    #[test]
    fn test_ok_confirmation_with_detail() {
        assert_eq!(ok_confirmation("merged", "#42"), "ok merged #42");
        assert_eq!(
            ok_confirmation("created", "PR #5 https://github.com/foo/bar/pull/5"),
            "ok created PR #5 https://github.com/foo/bar/pull/5"
        );
    }

    #[test]
    fn test_ok_confirmation_no_detail() {
        assert_eq!(ok_confirmation("commented", ""), "ok commented");
    }

    #[test]
    fn test_format_cpt_normal() {
        assert_eq!(format_cpt(0.000003), "$3.00/MTok");
        assert_eq!(format_cpt(0.0000038), "$3.80/MTok");
        assert_eq!(format_cpt(0.00000386), "$3.86/MTok");
    }

    #[test]
    fn test_format_cpt_edge_cases() {
        assert_eq!(format_cpt(0.0), "$0.00/MTok"); // zero
        assert_eq!(format_cpt(-0.000001), "$0.00/MTok"); // negative
        assert_eq!(format_cpt(f64::INFINITY), "$0.00/MTok"); // infinite
        assert_eq!(format_cpt(f64::NAN), "$0.00/MTok"); // NaN
    }

    #[test]
    fn test_detect_package_manager_default() {
        // In the test environment (rtk repo), there's no JS lockfile
        // so it should default to "npm"
        let pm = detect_package_manager();
        assert!(["pnpm", "yarn", "bun", "npm"].contains(&pm));
    }

    #[test]
    fn test_truncate_multibyte_thai() {
        // Thai characters are 3 bytes each
        let thai = "สวัสดีครับ";
        let result = truncate(thai, 5);
        // Should not panic, should produce valid UTF-8
        assert!(result.len() <= thai.len());
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_multibyte_emoji() {
        let emoji = "🎉🎊🎈🎁🎂🎄🎃🎆🎇✨";
        let result = truncate(emoji, 5);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_multibyte_cjk() {
        let cjk = "你好世界测试字符串";
        let result = truncate(cjk, 6);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_multibyte_cyrillic_issue_2787() {
        // Regression: `rtk gain --history` panicked slicing this at byte 22,
        // which falls inside 'н' (Cyrillic chars are 2 bytes each)
        let cmd = "rtk ls -la Ародинамический расчёт Новый";
        let result = truncate(cmd, 25);
        assert_eq!(result.chars().count(), 25);
        assert!(result.ends_with("..."));
        assert_eq!(result, "rtk ls -la Ародинамиче...");
    }

    // ===== resolve_binary tests (issue #212) =====

    #[test]
    fn test_resolve_binary_finds_known_command() {
        // "cargo" must be on PATH in any Rust dev environment
        let result = resolve_binary("cargo");
        assert!(
            result.is_ok(),
            "resolve_binary('cargo') should succeed, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_resolve_binary_returns_absolute_path() {
        let path = resolve_binary("cargo").expect("cargo should be resolvable");
        assert!(
            path.is_absolute(),
            "resolve_binary should return absolute path, got: {:?}",
            path
        );
    }

    #[test]
    fn test_resolve_binary_fails_for_unknown() {
        let result = resolve_binary("nonexistent_binary_xyz_99999");
        assert!(
            result.is_err(),
            "resolve_binary should fail for nonexistent binary"
        );
    }

    #[test]
    fn test_resolve_binary_path_contains_binary_name() {
        let path = resolve_binary("cargo").expect("cargo should be resolvable");
        let filename = path
            .file_name()
            .expect("should have filename")
            .to_string_lossy();
        // On Windows this could be "cargo.exe", on Unix just "cargo"
        assert!(
            filename.starts_with("cargo"),
            "resolved path filename should start with 'cargo', got: {}",
            filename
        );
    }

    // ===== resolved_command tests (issue #212) =====

    #[test]
    fn test_resolved_command_executes_known_command() {
        let output = resolved_command("cargo")
            .arg("--version")
            .output()
            .expect("resolved_command('cargo') should execute");
        assert!(
            output.status.success(),
            "cargo --version should succeed via resolved_command"
        );
    }

    // ===== tool_exists tests (issue #212) =====

    #[test]
    fn test_tool_exists_finds_cargo() {
        assert!(
            tool_exists("cargo"),
            "tool_exists('cargo') should return true"
        );
    }

    #[test]
    fn test_tool_exists_rejects_unknown() {
        assert!(
            !tool_exists("nonexistent_binary_xyz_99999"),
            "tool_exists should return false for nonexistent binary"
        );
    }

    #[test]
    fn test_tool_exists_finds_git() {
        assert!(tool_exists("git"), "tool_exists('git') should return true");
    }

    // ===== Windows-specific PATHEXT resolution tests (issue #212) =====

    #[cfg(target_os = "windows")]
    mod windows_tests {
        use super::super::*;
        use std::fs;

        /// Create a temporary .cmd wrapper to simulate Node.js tool installation
        fn create_temp_cmd_wrapper(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
            let cmd_path = dir.join(format!("{}.cmd", name));
            fs::write(&cmd_path, "@echo off\r\necho fake-tool-output\r\n")
                .expect("failed to create .cmd wrapper");
            cmd_path
        }

        /// Build a PATH string that includes the temp dir
        fn path_with_dir(dir: &std::path::Path) -> std::ffi::OsString {
            let original = std::env::var_os("PATH").unwrap_or_default();
            let mut new_path = std::ffi::OsString::from(dir.as_os_str());
            new_path.push(";");
            new_path.push(&original);
            new_path
        }

        #[test]
        fn test_resolve_binary_finds_cmd_wrapper() {
            let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
            create_temp_cmd_wrapper(temp_dir.path(), "fake-tool-test");

            // Use which::which_in to avoid mutating global PATH (thread-safe)
            let search_path = path_with_dir(temp_dir.path());
            let result = which::which_in(
                "fake-tool-test",
                Some(search_path),
                std::env::current_dir().unwrap(),
            );

            assert!(
                result.is_ok(),
                "which_in should find .cmd wrapper on Windows, got: {:?}",
                result.err()
            );

            let path = result.unwrap();
            let ext = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            assert!(
                ext == "cmd" || ext == "bat",
                "resolved path should have .cmd/.bat extension, got: {:?}",
                path
            );
        }

        #[test]
        fn test_resolve_binary_finds_bat_wrapper() {
            let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
            let bat_path = temp_dir.path().join("fake-bat-tool.bat");
            fs::write(&bat_path, "@echo off\r\necho bat-output\r\n")
                .expect("failed to create .bat wrapper");

            let search_path = path_with_dir(temp_dir.path());
            let result = which::which_in(
                "fake-bat-tool",
                Some(search_path),
                std::env::current_dir().unwrap(),
            );

            assert!(
                result.is_ok(),
                "which_in should find .bat wrapper on Windows, got: {:?}",
                result.err()
            );
        }

        #[test]
        fn test_resolved_command_executes_cmd_wrapper() {
            let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
            create_temp_cmd_wrapper(temp_dir.path(), "fake-exec-test");

            // Resolve the full path, then execute it directly (no PATH mutation)
            let search_path = path_with_dir(temp_dir.path());
            let resolved = which::which_in(
                "fake-exec-test",
                Some(search_path),
                std::env::current_dir().unwrap(),
            )
            .expect("should resolve fake-exec-test");

            let output = Command::new(&resolved).output();

            assert!(
                output.is_ok(),
                "Command with resolved path should execute .cmd wrapper on Windows"
            );
            let output = output.unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                stdout.contains("fake-tool-output"),
                "should get output from .cmd wrapper, got: {}",
                stdout
            );
        }

        #[test]
        fn test_resolved_command_fallback_on_unknown_binary() {
            // When resolve_binary fails, resolved_command should fall back to
            // Command::new(name) instead of panicking.  On Windows this also
            // prints a warning to stderr.
            let mut cmd = resolved_command("nonexistent_binary_xyz_99999");
            // The Command should be created (not panic).  Attempting to run it
            // will fail, but that's expected — we just verify the fallback path
            // produces a usable Command.
            let result = cmd.output();
            assert!(
                result.is_err() || !result.unwrap().status.success(),
                "nonexistent binary should fail to execute, but resolved_command must not panic"
            );
        }

        #[test]
        fn test_tool_exists_finds_cmd_wrapper() {
            let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
            create_temp_cmd_wrapper(temp_dir.path(), "fake-exists-test");

            let search_path = path_with_dir(temp_dir.path());
            let result = which::which_in(
                "fake-exists-test",
                Some(search_path),
                std::env::current_dir().unwrap(),
            );

            assert!(
                result.is_ok(),
                "which_in should find .cmd wrapper on Windows"
            );
        }
    }

    // ===== AWS helper function tests =====

    #[test]
    fn test_shorten_arn_ecs_service() {
        assert_eq!(
            shorten_arn("arn:aws:ecs:us-east-1:123:service/cluster/api-service"),
            "api-service"
        );
    }

    #[test]
    fn test_shorten_arn_iam_user() {
        assert_eq!(shorten_arn("arn:aws:iam::123456789012:user/alice"), "alice");
    }

    #[test]
    fn test_shorten_arn_lambda() {
        assert_eq!(
            shorten_arn("arn:aws:lambda:us-west-2:123:function:my-function"),
            "my-function"
        );
    }

    #[test]
    fn test_shorten_arn_fallback() {
        // Non-ARN string - return as-is
        assert_eq!(shorten_arn("simple-name"), "simple-name");
    }

    #[test]
    fn test_human_bytes_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1023), "1023 B");
    }

    #[test]
    fn test_human_bytes_kb() {
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(1536), "1.5 KB");
    }

    #[test]
    fn test_human_bytes_mb() {
        assert_eq!(human_bytes(1_048_576), "1.0 MB");
        assert_eq!(human_bytes(5_242_880), "5.0 MB");
    }

    #[test]
    fn test_human_bytes_gb() {
        assert_eq!(human_bytes(1_073_741_824), "1.0 GB");
        assert_eq!(human_bytes(2_147_483_648), "2.0 GB");
    }

    #[test]
    fn test_human_bytes_tb() {
        assert_eq!(human_bytes(1_099_511_627_776), "1.0 TB");
    }

    #[test]
    fn test_count_tokens_basic() {
        assert_eq!(count_tokens("hello world"), 2);
        assert_eq!(count_tokens("one two three four"), 4);
    }

    #[test]
    fn test_count_tokens_empty() {
        assert_eq!(count_tokens(""), 0);
        assert_eq!(count_tokens("   "), 0);
    }

    #[test]
    fn test_count_tokens_multiple_spaces() {
        assert_eq!(count_tokens("hello    world"), 2);
        assert_eq!(count_tokens("  hello   world  "), 2);
    }

    #[cfg(unix)]
    fn mode_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    #[cfg(unix)]
    fn test_create_private_dir_is_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("rtk").join("tee");
        create_private_dir(&nested).unwrap();
        assert_eq!(mode_of(&nested), 0o700);
    }

    #[test]
    #[cfg(unix)]
    fn test_create_private_dir_tightens_existing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("legacy");
        fs::create_dir_all(&dir).unwrap();
        set_owner_only(&dir, 0o755);

        create_private_dir(&dir).unwrap();
        assert_eq!(mode_of(&dir), 0o700);
    }

    #[test]
    fn test_create_private_dir_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a").join("b");
        create_private_dir(&dir).unwrap();
        create_private_dir(&dir).expect("second call must succeed");
        assert!(dir.is_dir());
    }

    #[test]
    #[cfg(unix)]
    fn test_restrict_file_is_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("history.db");
        fs::write(&file, b"x").unwrap();
        set_owner_only(&file, 0o644);

        restrict_file(&file);
        assert_eq!(mode_of(&file), 0o600);
    }

    #[test]
    fn test_restrict_file_ignores_missing_path() {
        let tmp = tempfile::tempdir().unwrap();
        restrict_file(&tmp.path().join("absent.db-wal"));
    }

    #[test]
    fn test_decode_process_output_valid_utf8() {
        assert_eq!(decode_process_output(b"hello world"), "hello world");
    }

    #[test]
    fn test_decode_process_output_chinese_utf8() {
        let input = "测试中文".as_bytes();
        assert_eq!(decode_process_output(input), "测试中文");
    }

    #[test]
    fn test_decode_process_output_empty() {
        assert_eq!(decode_process_output(b""), "");
    }

    #[test]
    fn test_decode_process_output_invalid_utf8_no_panic() {
        let bytes: &[u8] = &[0xFF, 0xFE, 0x41, 0x42];
        let result = decode_process_output(bytes);
        assert!(!result.is_empty());
    }

    // `decode_mixed` takes the code page as a parameter so these run on every
    // platform, not only Windows — the CI runners that would exercise the
    // Windows-gated versions do not exist.

    /// The bug this fix targets: GBK output from a Chinese-locale console.
    #[test]
    fn test_decode_mixed_gbk_run() {
        assert_eq!(decode_mixed(&[0xB2, 0xE2, 0xCA, 0xD4], Some(936)), "测试");
    }

    /// GB18030 (54936) must not be treated as plain GBK: it has to keep its
    /// own table so 4-byte sequences decode.
    #[test]
    fn test_decode_mixed_gb18030_is_not_gbk() {
        // 0x81 0x35 0xF4 0x37 is a 4-byte GB18030 sequence.
        let decoded = decode_mixed(&[0x81, 0x35, 0xF4, 0x37], Some(54936));
        assert_eq!(
            decoded.chars().count(),
            1,
            "expected one char: {:?}",
            decoded
        );
        assert!(
            !decoded.contains('\u{FFFD}'),
            "got replacement: {:?}",
            decoded
        );
        assert_eq!(
            codepage::to_encoding(54936).map(|e| e.name()),
            Some("gb18030")
        );
    }

    /// Legacy OEM/DOS pages are cmd.exe's default in many locales and are not
    /// WHATWG encodings, so they come from `oem_cp` rather than encoding_rs.
    #[test]
    fn test_decode_mixed_oem_codepages() {
        assert_eq!(decode_mixed(&[0xB0, 0xDB], Some(437)), "░█");
        // 850 and 852 are likewise absent from encoding_rs.
        assert!(codepage::to_encoding(437).is_none());
        assert!(!decode_mixed(&[0xE1], Some(850)).contains('\u{FFFD}'));
    }

    /// The regression the review flagged: a buffer that is almost entirely
    /// valid UTF-8 must not be reinterpreted wholesale because of one bad
    /// line. Only the GBK line is re-decoded; the UTF-8 lines keep their bytes.
    #[test]
    fn test_decode_mixed_only_redecodes_invalid_lines() {
        let mut bytes = "héllo wörld\n".as_bytes().to_vec();
        bytes.extend_from_slice(&[0xB2, 0xE2, 0xCA, 0xD4, b'\n']); // GBK 测试
        bytes.extend_from_slice("grüße\n".as_bytes());

        assert_eq!(
            decode_mixed(&bytes, Some(936)),
            "héllo wörld\n测试\ngrüße\n"
        );
    }

    /// A corrupt byte inside an otherwise-UTF-8 line falls back to lossy UTF-8
    /// rather than mojibake, because the code page decode does not come out
    /// clean. The surrounding lines are untouched either way.
    #[test]
    fn test_decode_mixed_corrupt_byte_prefers_lossy_utf8() {
        let mut bytes = "first\n".as_bytes().to_vec();
        bytes.extend_from_slice("naïve".as_bytes());
        bytes.push(0xC3); // dangling lead byte: invalid UTF-8 and invalid GBK
        bytes.extend_from_slice(b"\nlast\n");

        let decoded = decode_mixed(&bytes, Some(936));
        assert!(decoded.starts_with("first\n"), "{:?}", decoded);
        assert!(decoded.contains("naïve"), "{:?}", decoded);
        assert!(decoded.ends_with("\nlast\n"), "{:?}", decoded);
    }

    /// Line endings and the final line without a terminator must survive.
    #[test]
    fn test_decode_mixed_preserves_line_structure() {
        let mut bytes = b"a\r\n".to_vec();
        bytes.extend_from_slice(&[0xB2, 0xE2, b'\n']); // GBK 测
        bytes.extend_from_slice(b"tail"); // no trailing newline

        assert_eq!(decode_mixed(&bytes, Some(936)), "a\r\n测\ntail");
    }

    /// No code page (every Unix run) keeps the old lossy behavior.
    #[test]
    fn test_decode_mixed_without_codepage_is_lossy() {
        let mut bytes = b"ok ".to_vec();
        bytes.push(0xFF);
        assert_eq!(decode_mixed(&bytes, None), "ok \u{FFFD}");
    }

    /// An unmapped code page must degrade to lossy rather than panic.
    #[test]
    fn test_decode_mixed_unknown_codepage_is_lossy() {
        assert_eq!(decode_mixed(&[0xFF], Some(60000)), "\u{FFFD}");
    }

    /// Truncated trailing UTF-8 must terminate rather than loop forever.
    #[test]
    fn test_decode_mixed_truncated_utf8_terminates() {
        // First two bytes of a three-byte character, cut short.
        let decoded = decode_mixed(&[b'a', 0xE6, 0xB5], None);
        assert!(decoded.starts_with('a'), "{:?}", decoded);
    }

    /// Every byte value must survive both paths without panicking.
    #[test]
    fn test_decode_mixed_all_byte_values_no_panic() {
        let all: Vec<u8> = (0u8..=255).collect();
        for cp in [None, Some(936), Some(437), Some(65001), Some(1252)] {
            let _ = decode_mixed(&all, cp);
        }
    }

    /// Without a code page — every Unix run — the result must stay identical to
    /// the `String::from_utf8_lossy` this replaced. Splitting on `\n` cannot
    /// change it: a newline is valid ASCII, so no ill-formed sequence spans one.
    #[test]
    fn test_decode_process_output_unchanged_without_codepage() {
        let cases: [&[u8]; 6] = [
            b"",
            b"plain ascii\n",
            "utf8 ünïcödé\nsecond ライン\n".as_bytes(),
            &[0xFF, 0xFE, b'\n', b'o', b'k'],
            &[b'a', b'\n', 0xE6, 0xB5, b'\n', b'b'],
            &[0xB2, 0xE2, 0xCA, 0xD4],
        ];
        for bytes in cases {
            assert_eq!(
                decode_mixed(bytes, None),
                String::from_utf8_lossy(bytes),
                "lossy mismatch for {:?}",
                bytes
            );
        }
    }

    // rtk-ai/rtk#3206 review: set_owner_only now skips the chmod syscall once
    // permissions already match, since Tracker::new() (which calls this via
    // create_private_dir/open_private/restrict_db_files) is on the PreToolUse
    // hook's hot path for every Bash tool call. These don't observe the skipped
    // syscall directly, but confirm the already-correct case stays correct
    // (idempotent) across repeated calls, on both a directory and a file.

    #[test]
    #[cfg(unix)]
    fn test_set_owner_only_idempotent_when_already_correct_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("already-private");
        create_private_dir(&dir).unwrap();
        assert_eq!(mode_of(&dir), 0o700);

        // Second call hits the already-correct fast path — must stay 0o700.
        create_private_dir(&dir).unwrap();
        assert_eq!(mode_of(&dir), 0o700);
    }

    #[test]
    #[cfg(unix)]
    fn test_restrict_file_idempotent_when_already_correct() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("history.db");
        fs::write(&file, b"x").unwrap();
        restrict_file(&file);
        assert_eq!(mode_of(&file), 0o600);

        // Second call hits the already-correct fast path — must stay 0o600.
        restrict_file(&file);
        assert_eq!(mode_of(&file), 0o600);
    }

    // Regression: the fast-path comparison must mask with 0o7777 (including
    // setuid/setgid/sticky), not 0o777 — a 0o777 mask would make a file with e.g.
    // setuid set (0o4600) read as "already 0o600" and permanently skip the chmod
    // that clears the dangerous bit. Tested against the pure mask logic directly
    // (not round-tripped through a real chmod) since an unprivileged chmod setting
    // setuid isn't reliably honored across every environment — some sandboxed/
    // containerized filesystems silently drop it, which would make a
    // filesystem-based version of this test flaky rather than exercise the logic.
    #[test]
    #[cfg(unix)]
    fn test_mode_already_correct_rejects_setuid_bit() {
        assert!(
            !mode_already_correct(0o4600, 0o600),
            "setuid set (0o4600) must not read as already-correct against target 0o600"
        );
        assert!(
            !mode_already_correct(0o2600, 0o600),
            "setgid set (0o2600) must not read as already-correct against target 0o600"
        );
        assert!(
            !mode_already_correct(0o1600, 0o600),
            "sticky bit set (0o1600) must not read as already-correct against target 0o600"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_mode_already_correct_accepts_exact_match() {
        assert!(mode_already_correct(0o600, 0o600));
        assert!(mode_already_correct(0o700, 0o700));
        assert!(!mode_already_correct(0o644, 0o600));
    }

    #[test]
    #[cfg(unix)]
    fn test_mode_already_correct_ignores_file_type_bits() {
        // `st_mode` carries file-type bits (e.g. S_IFREG = 0o100000) above the
        // 0o7777 permission range this function masks to — those must not affect
        // the comparison either way.
        assert!(mode_already_correct(0o100600, 0o600));
    }

    #[test]
    fn test_detect_package_manager_recognizes_bun() {
        // Asks about an explicit directory: chdir is process-global and would
        // race the other tests in this binary that assert on the real cwd.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("bun.lockb"), "").expect("write lockfile");
        assert_eq!(detect_package_manager_in(dir.path()), "bun");
    }

    #[test]
    fn test_missing_tool_policy_controls_npx_no_install() {
        // Semantics are per caller: a tool the user named may be fetched, one
        // rtk chose may not. Changing either would change what runs, not what
        // is printed, which is not rtk's job.
        let args: Vec<String> =
            tool_exec(Some("npm"), "definitely-not-installed", MissingTool::Fetch)
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
        assert!(!args.contains(&"--no-install".to_string()), "{args:?}");

        let args: Vec<String> =
            tool_exec(Some("npm"), "definitely-not-installed", MissingTool::Fail)
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
        assert!(args.contains(&"--no-install".to_string()), "{args:?}");
    }

    #[test]
    fn test_exec_runner_prefers_the_named_runner_and_never_detects_bunx() {
        // A named runner is used as given, whatever the fetch policy.
        for missing in [MissingTool::Fetch, MissingTool::Fail] {
            assert_eq!(exec_runner(Some("bunx"), missing), "bunx");
            assert_eq!(exec_runner(Some("pnpm"), missing), "pnpm");
        }

        // Detection never resolves through bunx: it always fetches a missing
        // tool, so MissingTool::Fail could not be honoured.
        assert_ne!(exec_runner(None, MissingTool::Fail), "bun");
        assert_ne!(exec_runner(None, MissingTool::Fail), "bunx");

        // Nothing named and rtk may fetch: npx is the only runner that can.
        assert_eq!(exec_runner(None, MissingTool::Fetch), "npx");
    }
}
