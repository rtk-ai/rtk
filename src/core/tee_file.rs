//! Legacy file-based recovery ("tee" mode) — may be deprecated. Prefer the
//! sqlite recall store (`[retriever] mode = "sqlite"`); see retriever.rs.

// Complexity ratchet — see clippy.toml. Ceilings may only fall.
#![deny(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::cognitive_complexity,
    clippy::excessive_nesting,
    clippy::fn_params_excessive_bools,
    clippy::struct_excessive_bools,
    clippy::type_complexity
)]

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
/// Every character that is not alphanumeric, underscore or hyphen becomes an
/// underscore. Deliberately a replacement rather than a removal: dropping the
/// separators in `go/build/cmd` would collapse it to one word, while replacing
/// them keeps the shape of the original readable in the filename.
fn filename_safe(slug: &str) -> String {
    slug.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn sanitize_slug(slug: &str) -> String {
    let sanitized = filename_safe(slug);
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

pub const LEGACY_TEE_ALWAYS_NOTICE: &str = "Legacy [tee] mode = \"always\" has no equivalent: recovery now captures\nfailures only, so successful commands no longer leave a .log file. Nothing\nelse changed. If you relied on capturing successful output, run\n`rtk config recall sqlite` — the sqlite store keeps elided output from\nsuccessful commands too, readable with `rtk recall <hash>`.";

/// True when legacy `[tee]` config is what keeps the user on file mode.
///
/// `LEGACY_TEE_CONFIG_NOTICE` claims "file mode kept", so the mode check is
/// part of the claim: a user whose `[tee]` section resolved to `Disabled`
/// (`enabled = false` / `mode = "never"`) is not on file mode and must not be
/// told otherwise.
pub fn legacy_tee_config_in_use() -> bool {
    Config::load()
        .map(|c| is_legacy_tee_in_use(&c))
        .unwrap_or(false)
}

fn is_legacy_tee_in_use(config: &Config) -> bool {
    config.migrated_from_legacy_tee
        && config.retriever.mode == crate::core::retriever::RecoveryMode::Tee
}

/// True when the loaded config carried `[tee] mode = "always"`, which the
/// migration silently downgrades to failures-only.
pub fn legacy_tee_always_downgraded() -> bool {
    Config::load()
        .map(|c| c.migrated_from_legacy_tee_always)
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

/// The bytes a tee file actually receives.
///
/// Note what this does on the truncating path: it appends a
/// `--- truncated at N bytes ---` marker *into* the payload. Tee mode is
/// therefore not byte-faithful, unlike the sqlite store, which truncates at a
/// line boundary and adds nothing. Keeping that in its own function
/// is the difference between a caller who knows the payload is annotated and
/// one who assumes it is the raw output.
///
/// The cut lands on a character boundary via `char_indices`, so a multi-byte
/// codepoint straddling the cap is dropped rather than split.
fn tee_body(raw: &str, max_file_size: usize) -> String {
    if raw.len() <= max_file_size {
        return raw.to_string();
    }
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
}

/// Where tee files go and the limits that govern them. Grouped because the
/// three are always derived from one `[retriever]` config together and are
/// never meaningful apart — a directory without its rotation limits describes
/// nothing the writer can act on.
struct TeeTarget<'a> {
    dir: &'a Path,
    max_file_size: usize,
    max_files: usize,
}

fn write_tee_file(raw: &str, slug: &str, target: &TeeTarget<'_>) -> Option<PathBuf> {
    let TeeTarget {
        dir,
        max_file_size,
        max_files,
    } = *target;
    create_tee_dir(dir)?;
    let slug = sanitize_slug(slug);
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let filepath = dir.join(format!("{}_{}.log", epoch, slug));
    write_private(&filepath, &tee_body(raw, max_file_size))?;
    cleanup_old_files(dir, max_files);
    Some(filepath)
}

/// Create-or-truncate `path` with owner-only permissions and write `content`.
///
/// `open_private` sets the mode at creation rather than chmod-ing afterwards,
/// so there is no window in which a tee file — which holds raw command output —
/// is readable by anyone else.
fn write_private(path: &Path, content: &str) -> Option<()> {
    use std::io::Write;
    let mut file = crate::core::utils::open_private(
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true),
        path,
    )
    .ok()?;
    file.write_all(content.as_bytes()).ok()
}

/// Only the POSIX hint path uses this: on Windows `display_shell_path` emits
/// the absolute path, since neither cmd nor PowerShell expands `~`.
#[cfg(not(windows))]
fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(relative) = path.strip_prefix(&home) {
            return format!("~/{}", relative.display());
        }
    }
    path.display().to_string()
}

/// Characters that make a path unsafe to paste into a shell unquoted. Quoting,
/// globbing, redirection, command substitution, separators and history
/// expansion — the hint we print is meant to be run, so anything the shell would
/// interpret has to be inside quotes.
const SHELL_METACHARS: &[char] = &[
    '\'', '"', '\\', '$', '`', '!', '#', '&', '(', ')', ';', '<', '>', '?', '[', ']', '{', '}',
    '|', '*',
];

fn needs_shell_quoting(path: &str) -> bool {
    path.chars()
        .any(|c| c.is_whitespace() || SHELL_METACHARS.contains(&c))
}

/// POSIX double-quote escaping: backslash, quote, dollar and backtick keep
/// their meaning inside `"…"` and must be escaped.
#[cfg(not(windows))]
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

/// Windows quoting: only the quote character needs escaping.
///
/// Backslash must NOT be, because it is the path separator — the POSIX rule
/// turned every `C:\Users\me` into `C:\\Users\\me`, mangling the path in a hint
/// whose whole purpose is to be pasted and run. `$` and backtick are likewise
/// literal in cmd, and `` ` `` is PowerShell's escape rather than a shell
/// metacharacter to neutralise here.
#[cfg(windows)]
fn escape_double_quoted_path(path: &str) -> String {
    path.replace('"', "\\\"")
}

fn display_shell_path(path: &Path) -> String {
    // `~` is expanded by POSIX shells, not by cmd or PowerShell, so on Windows
    // the hint carries the absolute path rather than a tilde that would be
    // taken literally.
    #[cfg(windows)]
    let display = path.display().to_string();
    #[cfg(not(windows))]
    let display = display_path(path);

    if !needs_shell_quoting(&display) {
        return display;
    }

    // `$HOME` is POSIX shell syntax. On Windows the hint is pasted into cmd or
    // PowerShell, where it expands to nothing and the path silently breaks, so
    // there the home directory stays spelled out.
    #[cfg(not(windows))]
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
        &TeeTarget {
            dir: &dir,
            max_file_size: cfg.tee_max_file_size,
            max_files: cfg.tee_max_files,
        },
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
// Test bodies are linear setup-act-assert scripts; splitting them to satisfy
// the ratchet makes them harder to read. See clippy.toml.
#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::cognitive_complexity,
    clippy::excessive_nesting
)]
mod tests {
    use super::*;
    use std::fs;

    const MAX_FILE_SIZE: usize = 1_048_576;

    /// `LEGACY_TEE_CONFIG_NOTICE` claims "file mode kept", so the gate must
    /// require Tee mode. Firing it for a migrated-to-Disabled user tells someone
    /// who explicitly turned recovery off that their files are still being kept.
    mod b10_legacy_config_gate {
        use super::super::is_legacy_tee_in_use;
        use crate::core::config::Config;
        use crate::core::retriever::RecoveryMode;

        fn config(migrated: bool, mode: RecoveryMode) -> Config {
            let mut c = Config::default();
            c.migrated_from_legacy_tee = migrated;
            c.retriever.mode = mode;
            c
        }

        #[test]
        fn test_fires_for_migrated_tee_mode() {
            assert!(is_legacy_tee_in_use(&config(true, RecoveryMode::Tee)));
        }

        #[test]
        fn test_silent_for_migrated_disabled_mode() {
            assert!(
                !is_legacy_tee_in_use(&config(true, RecoveryMode::Disabled)),
                "a user who set enabled=false is not on file mode"
            );
        }

        #[test]
        fn test_silent_for_migrated_sqlite_mode() {
            assert!(!is_legacy_tee_in_use(&config(true, RecoveryMode::Sqlite)));
        }

        #[test]
        fn test_silent_when_nothing_migrated() {
            for mode in [
                RecoveryMode::Tee,
                RecoveryMode::Sqlite,
                RecoveryMode::Disabled,
            ] {
                assert!(!is_legacy_tee_in_use(&config(false, mode)));
            }
        }
    }

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
        let result = write_tee_file(
            &content,
            "cargo_test",
            &TeeTarget {
                dir: tmpdir.path(),
                max_file_size: MAX_FILE_SIZE,
                max_files: 20,
            },
        );
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
        let path = write_tee_file(
            "secret output\n",
            "grep",
            &TeeTarget {
                dir: &tee_dir,
                max_file_size: MAX_FILE_SIZE,
                max_files: 20,
            },
        )
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
        let written = write_tee_file(
            "secret\n",
            "grep",
            &TeeTarget {
                dir: &tee_dir,
                max_file_size: MAX_FILE_SIZE,
                max_files: 20,
            },
        );
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
        let result = write_tee_file(
            &big_output,
            "test",
            &TeeTarget {
                dir: tmpdir.path(),
                max_file_size: 1000,
                max_files: 20,
            },
        );
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
        let result = write_tee_file(
            &japanese,
            "test_utf8",
            &TeeTarget {
                dir: tmpdir.path(),
                max_file_size: 998,
                max_files: 20,
            },
        );
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

    #[cfg(not(windows))]
    #[test]
    fn test_display_shell_path_quotes_backslashes() {
        let path = PathBuf::from(r"/tmp/rtk/tee/path\segment.log");
        assert_eq!(
            display_shell_path(&path),
            r#""/tmp/rtk/tee/path\\segment.log""#
        );
    }

    /// A Windows path must survive the hint intact.
    ///
    /// Runs on every platform by testing the escaper directly, because the bug
    /// it guards was found by reading rather than by CI — there is no Windows
    /// runner here, and `escape_double_quoted_path` is where the damage was
    /// done: the POSIX rule escapes `\`, which is Windows' path separator, so
    /// `C:\Users\me` became `C:\\Users\\me` in a string meant to be pasted and
    /// run.
    #[cfg(windows)]
    #[test]
    fn test_windows_paths_keep_their_separators() {
        assert_eq!(
            escape_double_quoted_path(r"C:\Users\me\file.log"),
            r"C:\Users\me\file.log"
        );
        assert_eq!(escape_double_quoted_path(r#"C:\a"b"#), r#"C:\a\"b"#);
    }

    /// The POSIX escaper's contract, pinned so the cfg split cannot drift.
    #[cfg(not(windows))]
    #[test]
    fn test_posix_escaper_escapes_shell_specials() {
        assert_eq!(escape_double_quoted_path(r"a\b"), r"a\\b");
        assert_eq!(escape_double_quoted_path("a$b`c\"d"), "a\\$b\\`c\\\"d");
    }

    #[cfg(not(windows))]
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
