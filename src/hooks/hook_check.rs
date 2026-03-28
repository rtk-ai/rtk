//! Detects whether RTK hooks are installed and warns if they are outdated.

use std::path::PathBuf;
use serde_json::Value as JsonValue;

const WARN_INTERVAL_SECS: u64 = 24 * 3600;

/// Hook status for diagnostics and `rtk gain`.
#[derive(Debug, PartialEq, Clone)]
pub enum HookStatus {
    /// Hook is installed and up to date.
    Ok,
    /// Hook exists but is outdated or unreadable.
    Outdated,
    /// No hook file found (but Claude Code is installed).
    Missing,
}

/// Return the current hook status without printing anything.
/// Returns `Ok` if no Claude Code is detected (not applicable).
/// Returns actual status based on settings.json inspection.
pub fn status() -> HookStatus {
    use std::fs;

    // Don't warn users who don't have Claude Code installed
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return HookStatus::Ok,
    };
    let claude_dir = home.join(".claude");
    if !claude_dir.exists() {
        return HookStatus::Ok;
    }

    let settings_path = claude_dir.join("settings.json");
    if !settings_path.exists() {
        // Claude Code dir exists but no settings.json
        return HookStatus::Missing;
    }

    // Read and parse settings.json
    let content = match fs::read_to_string(&settings_path) {
        Ok(c) => c,
        Err(_) => return HookStatus::Missing, // Can't read = treat as missing
    };

    if content.trim().is_empty() {
        return HookStatus::Missing;
    }

    let root: JsonValue = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return HookStatus::Outdated, // Invalid JSON = outdated
    };

    // Check for native hook "rtk hook claude"
    if hook_present(&root, "rtk hook claude") {
        return HookStatus::Ok;
    }

    // Check for legacy hook (rtk-rewrite.sh script)
    if legacy_hook_present(&root) {
        return HookStatus::Outdated;
    }

    // Hook not configured at all
    HookStatus::Missing
}

/// Check if native RTK hook is present in settings.json
fn hook_present(root: &JsonValue, hook_command: &str) -> bool {
    let pre_tool_use = match root
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(|p| p.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };

    pre_tool_use
        .iter()
        .filter_map(|entry| entry.get("hooks")?.as_array())
        .flatten()
        .filter_map(|h| h.get("command")?.as_str())
        .any(|cmd| cmd == hook_command || cmd.contains("rtk hook"))
}

/// Check if legacy rtk-rewrite.sh hook is present
fn legacy_hook_present(root: &JsonValue) -> bool {
    let pre_tool_use = match root
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(|p| p.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };

    pre_tool_use.iter().any(|entry| {
        entry.get("matcher")
            .and_then(|m| m.as_str())
            .map(|m| m == "Bash")
            .unwrap_or(false)
            && entry
                .pointer("/hooks/0/type")
                .and_then(|t| t.as_str())
                .map(|t| t == "command")
                .unwrap_or(false)
            && entry
                .pointer("/hooks/0/command")
                .and_then(|c| c.as_str())
                .map(|c| c.contains("rtk-rewrite.sh"))
                .unwrap_or(false)
    })
}

/// Check if the installed hook is missing or outdated, warn once per day.
pub fn maybe_warn() {
    // Don't block startup — fail silently on any error
    let _ = check_and_warn();
}

/// Single source of truth: delegates to `status()` then rate-limits the warning.
fn check_and_warn() -> Option<()> {
    let warning = match status() {
        HookStatus::Ok => return Some(()),
        HookStatus::Missing => {
            "[rtk] /!\\ No hook installed — run `rtk init -g` for automatic token savings"
        }
        HookStatus::Outdated => "[rtk] /!\\ Hook outdated — run `rtk init -g` to update",
    };

    // Rate limit: warn once per day
    let marker = warn_marker_path()?;
    if let Ok(meta) = std::fs::metadata(&marker) {
        if let Ok(modified) = meta.modified() {
            if modified.elapsed().map(|e| e.as_secs()).unwrap_or(u64::MAX) < WARN_INTERVAL_SECS {
                return Some(());
            }
        }
    }

    eprintln!("{}", warning);

    // Touch marker after warning is printed
    let _ = std::fs::create_dir_all(marker.parent()?);
    let _ = std::fs::write(&marker, b"");

    Some(())
}


fn warn_marker_path() -> Option<PathBuf> {
    let data_dir = dirs::data_local_dir()?.join("rtk");
    Some(data_dir.join(".hook_warn_last"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_status_enum() {
        assert_ne!(HookStatus::Ok, HookStatus::Missing);
        assert_ne!(HookStatus::Outdated, HookStatus::Missing);
        assert_eq!(HookStatus::Ok, HookStatus::Ok);
        // Clone works
        let s = HookStatus::Missing;
        assert_eq!(s.clone(), HookStatus::Missing);
    }

    #[test]
    fn test_status_returns_valid_variant() {
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => {
                // No home dir - should return Ok (not applicable)
                assert_eq!(status(), HookStatus::Ok);
                return;
            }
        };

        if !home.join(".claude").exists() {
            // Claude Code not installed - not applicable
            assert_eq!(status(), HookStatus::Ok);
            return;
        }

        // If we reach here, Claude Code is installed
        // The actual status depends on whether hook is installed
        let s = status();
        // All variants are valid depending on hook state
        assert!(matches!(
            s,
            HookStatus::Ok | HookStatus::Missing | HookStatus::Outdated
        ));
    }

    #[test]
    fn test_hook_present_detection() {
        let json = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "rtk hook claude"
                    }]
                }]
            }
        });
        assert!(hook_present(&json, "rtk hook claude"));
    }

    #[test]
    fn test_hook_present_fuzzy_match() {
        // Should also match commands containing "rtk hook"
        let json = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "rtk hook gemini"
                    }]
                }]
            }
        });
        assert!(hook_present(&json, "rtk hook claude"));
    }

    #[test]
    fn test_hook_present_false_when_missing() {
        let json = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "other-command"
                    }]
                }]
            }
        });
        assert!(!hook_present(&json, "rtk hook claude"));
    }

    #[test]
    fn test_hook_present_no_hooks_array() {
        let json = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash"
                }]
            }
        });
        assert!(!hook_present(&json, "rtk hook claude"));
    }

    #[test]
    fn test_hook_present_no_pretooluse() {
        let json = serde_json::json!({
            "hooks": {}
        });
        assert!(!hook_present(&json, "rtk hook claude"));
    }

    #[test]
    fn test_legacy_hook_detection() {
        let json = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/path/to/rtk-rewrite.sh"
                    }]
                }]
            }
        });
        assert!(legacy_hook_present(&json));
    }

    #[test]
    fn test_legacy_hook_false_with_native_hook() {
        let json = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "rtk hook claude"
                    }]
                }]
            }
        });
        assert!(!legacy_hook_present(&json));
    }

    #[test]
    fn test_legacy_hook_false_no_matcher() {
        let json = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "hooks": [{
                        "type": "command",
                        "command": "/path/to/rtk-rewrite.sh"
                    }]
                }]
            }
        });
        assert!(!legacy_hook_present(&json));
    }

    #[test]
    fn test_legacy_hook_false_wrong_type() {
        let json = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "builtin",
                        "command": "/path/to/rtk-rewrite.sh"
                    }]
                }]
            }
        });
        assert!(!legacy_hook_present(&json));
    }
}
