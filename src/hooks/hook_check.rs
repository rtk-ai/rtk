//! Detects whether RTK hooks are installed and warns if they are outdated.

use crate::core::user_dirs;
use std::path::PathBuf;

pub const CURRENT_HOOK_VERSION: u8 = 4;
const WARN_INTERVAL_SECS: u64 = 24 * 3600;

/// Hook status for diagnostics and `rtk gain`.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HookStatus {
    /// Hook is installed and up to date.
    Ok,
    /// Hook exists but is outdated or unreadable.
    Outdated,
    /// No hook file found (but Claude Code is installed).
    Missing,
}

pub type AgentProbe = (&'static str, fn() -> bool);

/// All agent probes for active RTK hook/plugin installation.
/// Every agent owns its own probe inside `src/hooks/init/<agent>.rs`.
pub const AGENT_PROBES: &[AgentProbe] = &[
    ("claude", super::init::claude::is_configured),
    ("antigravity", super::init::antigravity::is_configured),
    ("cursor", super::init::cursor::is_configured),
    ("gemini", super::init::gemini::is_configured),
    ("opencode", super::init::opencode::is_configured),
    ("codex", super::init::codex::is_configured),
    ("droid", super::init::droid::is_configured),
    ("trae", super::init::trae::is_configured),
    ("hermes", super::init::hermes::is_configured),
    ("pi", super::init::pi::is_configured),
    ("vibe", super::init::vibe::is_configured),
];

/// Returns true if at least one supported agent has an active hook/plugin configured.
pub fn is_any_agent_configured() -> bool {
    AGENT_PROBES.iter().any(|(_, probe)| probe())
}

/// Returns true if any configured agent hook is outdated.
pub fn is_any_hook_outdated() -> bool {
    super::init::claude::is_outdated() || super::init::cursor::is_outdated()
}

/// Returns the hook status for Claude Code without printing anything.
/// Returns `HookStatus::Ok` if Claude Code is not installed on this system.
///
/// Note: This probe is intentionally specific to Claude Code because `rtk discover`
/// and legacy transcript analysis rely specifically on Claude's hook status.
/// General warning checks should use [`is_any_agent_configured`] and
/// [`is_any_hook_outdated`] instead.
pub fn status() -> HookStatus {
    super::init::claude::hook_status()
}

/// Check if the installed hook is missing or outdated, warn once per day.
pub fn maybe_warn() {
    // Don't block startup — fail silently on any error
    let _ = check_and_warn();
}

/// Message to print for `status`, if any.
/// `suppress_missing` only hides [`HookStatus::Missing`]; outdated stays visible.
fn warning_text(status: HookStatus, suppress_missing: bool) -> Option<&'static str> {
    if is_any_hook_outdated() || status == HookStatus::Outdated {
        Some("[rtk] /!\\ Hook outdated — run `rtk init -g` to update")
    } else {
        match status {
            HookStatus::Ok => None,
            HookStatus::Missing if suppress_missing => None,
            HookStatus::Missing => {
                Some("[rtk] /!\\ No hook installed — run `rtk init -g` for automatic token savings")
            }
            HookStatus::Outdated => Some("[rtk] /!\\ Hook outdated — run `rtk init -g` to update"),
        }
    }
}

/// Single source of truth: delegates to `status()` then rate-limits the warning.
fn check_and_warn() -> Option<()> {
    // Probe first so the common HookStatus::Ok path never reads config.toml.
    // Suppression is consulted only when a missing-hook warning would print.
    let status = status();
    if status == HookStatus::Ok && !is_any_hook_outdated() {
        return Some(());
    }
    let suppress_missing = status == HookStatus::Missing
        && (crate::core::config::hook_warning_suppressed() || is_any_agent_configured());
    let warning = warning_text(status, suppress_missing)?;

    // Rate limit: warn once per day
    let marker = warn_marker_path()?;
    if let Ok(meta) = std::fs::metadata(&marker)
        && let Ok(modified) = meta.modified()
        && modified.elapsed().map(|e| e.as_secs()).unwrap_or(u64::MAX) < WARN_INTERVAL_SECS
    {
        return Some(());
    }

    eprintln!("{}", warning);

    // Touch marker after warning is printed
    let _ = crate::core::utils::create_private_dir(marker.parent()?);
    let _ = std::fs::write(&marker, b"");

    Some(())
}

pub fn parse_hook_version(content: &str) -> u8 {
    // Version tag must be in the first 5 lines (shebang + header convention)
    for line in content.lines().take(5) {
        if let Some(rest) = line.strip_prefix("# rtk-hook-version:")
            && let Ok(v) = rest.trim().parse::<u8>()
        {
            return v;
        }
    }
    0 // No version tag = version 0 (outdated)
}

fn warn_marker_path() -> Option<PathBuf> {
    let data_dir = user_dirs::data()?;
    Some(data_dir.join(".hook_warn_last"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_isolation;
    use crate::core::user_env;

    #[test]
    fn test_parse_hook_version_present() {
        let content = "#!/usr/bin/env bash\n# rtk-hook-version: 2\n# some comment\n";
        assert_eq!(parse_hook_version(content), 2);
    }

    #[test]
    fn test_parse_hook_version_missing() {
        let content = "#!/usr/bin/env bash\n# old hook without version\n";
        assert_eq!(parse_hook_version(content), 0);
    }

    /// The shipped Claude hook script must carry the current version. `rtk init`
    /// no longer installs it, so the version grades copies already deployed:
    /// raising it reports older copies as outdated, which sends their owners to
    /// `rtk init -g` and from there to the in-process hook. The constant and the
    /// script move together.
    #[test]
    fn test_shipped_claude_hook_carries_the_current_version() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let content = std::fs::read_to_string(root.join("hooks/claude/rtk-rewrite.sh"))
            .expect("read hooks/claude/rtk-rewrite.sh");
        assert_eq!(
            parse_hook_version(&content),
            CURRENT_HOOK_VERSION,
            "hooks/claude/rtk-rewrite.sh and CURRENT_HOOK_VERSION disagree"
        );
    }

    #[test]
    fn test_parse_hook_version_future() {
        let content = "#!/usr/bin/env bash\n# rtk-hook-version: 5\n";
        assert_eq!(parse_hook_version(content), 5);
    }

    #[test]
    fn test_parse_hook_version_no_tag() {
        assert_eq!(parse_hook_version("no version here"), 0);
        assert_eq!(parse_hook_version(""), 0);
    }

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
    fn test_warning_text_scopes_suppression_to_missing() {
        assert_eq!(warning_text(HookStatus::Ok, false), None);
        assert_eq!(warning_text(HookStatus::Ok, true), None);
        assert!(
            warning_text(HookStatus::Missing, false).is_some(),
            "missing hook must warn when the flag is off"
        );
        assert_eq!(
            warning_text(HookStatus::Missing, true),
            None,
            "suppress_hook_warning must hide HookStatus::Missing"
        );
        assert!(
            warning_text(HookStatus::Outdated, false).is_some(),
            "outdated hook must warn when the flag is off"
        );
        assert!(
            warning_text(HookStatus::Outdated, true).is_some(),
            "suppress_hook_warning must not hide the outdated-hook upgrade prompt"
        );
    }

    #[test]
    fn test_status_returns_valid_variant() {
        // `status()` resolves through `CLAUDE_CONFIG_DIR`; pinned so both
        // states can be asserted.
        let tmp = test_isolation::tempdir();
        let claude_dir = tmp.path().join(".claude");
        user_env::with_path("CLAUDE_CONFIG_DIR", Some(&claude_dir), || {
            assert_eq!(
                status(),
                HookStatus::Ok,
                "no Claude dir: nothing to warn about"
            );
            std::fs::create_dir_all(&claude_dir).expect("create Claude dir");
            assert_eq!(
                status(),
                HookStatus::Missing,
                "a Claude dir with no rtk hook"
            );
        });
    }

    #[test]
    fn test_agent_probes_covers_all_supported_agents() {
        let mut agent_names: Vec<&str> = AGENT_PROBES.iter().map(|(name, _)| *name).collect();
        agent_names.sort_unstable();
        assert_eq!(
            agent_names,
            vec![
                "antigravity",
                "claude",
                "codex",
                "cursor",
                "droid",
                "gemini",
                "hermes",
                "opencode",
                "pi",
                "trae",
                "vibe"
            ]
        );
    }
}
