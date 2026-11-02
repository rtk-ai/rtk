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

/// All non-Claude agent probes for active RTK hook/plugin installation.
/// Every agent owns its own probe inside `src/hooks/init/<agent>.rs`.
const AGENT_PROBES: &[fn() -> bool] = &[
    super::init::antigravity::is_configured,
    super::init::cursor::is_configured,
    super::init::gemini::is_configured,
    super::init::opencode::is_configured,
    super::init::codex::is_configured,
    super::init::droid::is_configured,
    super::init::trae::is_configured,
    super::init::hermes::is_configured,
    super::init::pi::is_configured,
    super::init::vibe::is_configured,
];

/// Returns true if at least one supported non-Claude agent has an active hook/plugin configured.
pub fn is_any_agent_configured() -> bool {
    AGENT_PROBES.iter().any(|probe| probe())
}

/// Returns the hook status for Claude Code without printing anything.
/// Returns `HookStatus::Ok` if Claude Code is not installed on this system.
///
/// Note: This probe is intentionally specific to Claude Code because `rtk discover`
/// and legacy transcript analysis rely specifically on Claude's hook status.
/// General warning checks also consult [`is_any_agent_configured`].
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
    match status {
        HookStatus::Ok => None,
        HookStatus::Missing if suppress_missing => None,
        HookStatus::Missing => {
            Some("[rtk] /!\\ No hook installed — run `rtk init -g` for automatic token savings")
        }
        HookStatus::Outdated => Some("[rtk] /!\\ Hook outdated — run `rtk init -g` to update"),
    }
}

/// Single source of truth: delegates to `status()` then rate-limits the warning.
fn check_and_warn() -> Option<()> {
    // Probe first so the common HookStatus::Ok path never reads config.toml.
    // Suppression is consulted only when a missing-hook warning would print.
    let status = status();
    if status == HookStatus::Ok {
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
    fn test_probes_ignore_foreign_hooks_in_agent_files() {
        let tmp = test_isolation::tempdir();
        let home = tmp.path().join("home");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(home.join(".gemini")).expect("create .gemini");
        std::fs::create_dir_all(home.join(".vibe")).expect("create .vibe");
        std::fs::create_dir_all(&ws).expect("create ws");

        test_isolation::with_root(&home, || {
            let _entered = test_isolation::enter(&ws);
            std::fs::write(
                home.join(".gemini/settings.json"),
                r#"{"hooks":{"BeforeTool":[{"matcher":"run_shell_command","hooks":[{"type":"command","command":"/usr/local/bin/audit-shell"}]}]}}"#,
            )
            .expect("write gemini settings");
            std::fs::write(
                home.join(".vibe/hooks.toml"),
                "[[hooks]]\nname = \"audit\"\ntype = \"pre_tool\"\nmatch = \"bash\"\ncommand = \"rtk-audit\"\n",
            )
            .expect("write vibe hooks");

            assert!(
                !super::super::init::gemini::is_configured(),
                "a non-rtk BeforeTool hook is not an rtk install"
            );
            assert!(
                !super::super::init::vibe::is_configured(),
                "a Vibe hook not named rtk-rewrite is not an rtk install"
            );
            assert!(!is_any_agent_configured(), "no agent is configured");
        });
    }

    #[test]
    fn test_check_and_warn_suppressed_when_agent_configured() {
        let tmp = test_isolation::tempdir();
        let home = tmp.path().join("home");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(home.join(".claude")).expect("create .claude");
        std::fs::create_dir_all(&ws).expect("create ws");

        test_isolation::with_root(&home, || {
            let _entered = test_isolation::enter(&ws);
            let marker = warn_marker_path().expect("marker path");

            // With OpenCode plugin present, check_and_warn must not warn or touch marker
            let opencode_plugin = home.join(".config/opencode/plugins/rtk.ts");
            std::fs::create_dir_all(opencode_plugin.parent().unwrap()).unwrap();
            std::fs::write(&opencode_plugin, b"export default {}").unwrap();

            check_and_warn();
            assert!(
                !marker.exists(),
                "check_and_warn must not warn when an agent is configured"
            );

            // Once removed, check_and_warn warns and touches the daily marker
            std::fs::remove_file(&opencode_plugin).unwrap();
            check_and_warn();
            assert!(
                marker.exists(),
                "check_and_warn must warn and write marker when no agent is configured"
            );
        });
    }

    #[test]
    fn test_all_agent_probes_and_arms_in_process() {
        use crate::hooks::init::{InitContext, PatchMode};

        // Helper that runs `f(home, ws)` inside an isolated home + workspace
        let run_isolated = |f: &dyn Fn(&std::path::Path, &std::path::Path)| {
            let tmp = test_isolation::tempdir();
            let home = tmp.path().join("home");
            let ws = tmp.path().join("ws");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&ws).unwrap();
            test_isolation::with_root(&home, || {
                let _entered = test_isolation::enter(&ws);
                assert!(
                    !is_any_agent_configured(),
                    "precondition: fresh isolated env has no configured agent"
                );
                f(&home, &ws);
            });
        };

        // 1. Antigravity (local & global)
        run_isolated(&|_home, _ws| {
            super::super::init::antigravity::run_antigravity_mode(false, InitContext::default())
                .unwrap();
            assert!(is_any_agent_configured(), "antigravity local");
            super::super::init::antigravity::uninstall_antigravity_mode(
                false,
                InitContext::default(),
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "antigravity local uninstalled");

            super::super::init::antigravity::run_antigravity_mode(true, InitContext::default())
                .unwrap();
            assert!(is_any_agent_configured(), "antigravity global");
            super::super::init::antigravity::uninstall_antigravity_mode(
                true,
                InitContext::default(),
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "antigravity global uninstalled");
        });

        // 2. Cursor (configured vs unregistered script)
        run_isolated(&|home, _ws| {
            let cursor_hooks = home.join(".cursor/hooks");
            std::fs::create_dir_all(&cursor_hooks).unwrap();
            std::fs::write(cursor_hooks.join("rtk-rewrite.sh"), "#!/bin/sh\n").unwrap();
            assert!(
                !is_any_agent_configured(),
                "unregistered cursor script must not count as configured"
            );
            std::fs::remove_file(cursor_hooks.join("rtk-rewrite.sh")).unwrap();

            std::fs::write(
                home.join(".cursor/hooks.json"),
                r#"{"version":1,"hooks":{"preToolUse":[{"matcher":"Shell","command":"rtk hook cursor"}]}}"#,
            )
            .unwrap();
            assert!(is_any_agent_configured(), "cursor hooks.json");
            std::fs::remove_file(home.join(".cursor/hooks.json")).unwrap();
            assert!(!is_any_agent_configured(), "cursor removed");
        });

        // 3. Gemini (run_shell_command, wildcard *, non-shell matcher, unregistered script)
        run_isolated(&|home, _ws| {
            super::super::init::gemini::run_gemini(
                true,
                false,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "gemini init");

            // Wildcard matcher
            std::fs::write(
                home.join(".gemini/settings.json"),
                r#"{"hooks":{"BeforeTool":[{"matcher":"*","hooks":[{"command":"rtk hook gemini"}]}]}}"#,
            )
            .unwrap();
            assert!(is_any_agent_configured(), "gemini wildcard matcher");

            // Non-shell matcher must be rejected
            std::fs::write(
                home.join(".gemini/settings.json"),
                r#"{"hooks":{"BeforeTool":[{"matcher":"ReadFile","hooks":[{"command":"rtk hook gemini"}]}]}}"#,
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "gemini non-shell matcher");

            // Unregistered script file alone must be rejected
            std::fs::remove_file(home.join(".gemini/settings.json")).unwrap();
            let gemini_hooks = home.join(".gemini/hooks");
            std::fs::create_dir_all(&gemini_hooks).unwrap();
            std::fs::write(gemini_hooks.join("rtk-hook-gemini.sh"), "#!/bin/sh\n").unwrap();
            assert!(!is_any_agent_configured(), "gemini unregistered script");
        });

        // 4. Codex
        run_isolated(&|home, _ws| {
            std::fs::create_dir_all(home.join(".codex")).unwrap();
            std::fs::write(
                home.join(".codex/hooks.json"),
                r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook codex"}]}]}}"#,
            )
            .unwrap();
            assert!(is_any_agent_configured(), "codex configured");
            std::fs::remove_file(home.join(".codex/hooks.json")).unwrap();
            assert!(!is_any_agent_configured(), "codex removed");
        });

        // 5. Droid
        run_isolated(&|_home, _ws| {
            super::super::init::droid::run_droid_mode(true, InitContext::default()).unwrap();
            assert!(is_any_agent_configured(), "droid global");
            super::super::init::droid::uninstall_droid(true, InitContext::default()).unwrap();
            assert!(!is_any_agent_configured(), "droid uninstalled");
        });

        // 6. Trae (local & global)
        run_isolated(&|_home, _ws| {
            super::super::init::trae::run_trae_mode(false, InitContext::default()).unwrap();
            assert!(is_any_agent_configured(), "trae local");
            super::super::init::trae::uninstall_trae_mode(false, InitContext::default()).unwrap();
            assert!(!is_any_agent_configured(), "trae local uninstalled");

            super::super::init::trae::run_trae_mode(true, InitContext::default()).unwrap();
            assert!(is_any_agent_configured(), "trae global");
            super::super::init::trae::uninstall_trae_mode(true, InitContext::default()).unwrap();
            assert!(!is_any_agent_configured(), "trae global uninstalled");
        });

        // 7. Hermes (full init/uninstall + individual arms)
        run_isolated(&|home, _ws| {
            super::super::init::hermes::run_hermes_mode(InitContext::default()).unwrap();
            assert!(is_any_agent_configured(), "hermes init");
            super::super::init::hermes::uninstall_hermes(InitContext::default()).unwrap();
            assert!(!is_any_agent_configured(), "hermes uninstalled");

            // Plugin file arm alone
            let plugin_init = home.join(".hermes/plugins/rtk-rewrite/__init__.py");
            std::fs::create_dir_all(plugin_init.parent().unwrap()).unwrap();
            std::fs::write(&plugin_init, "# plugin\n").unwrap();
            assert!(is_any_agent_configured(), "hermes plugin file arm");
            std::fs::remove_file(&plugin_init).unwrap();
            assert!(!is_any_agent_configured(), "hermes plugin file removed");

            // config.yaml arm alone
            std::fs::write(
                home.join(".hermes/config.yaml"),
                "plugins:\n  enabled:\n    - rtk-rewrite\n",
            )
            .unwrap();
            assert!(is_any_agent_configured(), "hermes config.yaml arm");
            std::fs::write(
                home.join(".hermes/config.yaml"),
                "plugins:\n  enabled:\n    - other\n",
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "hermes unrelated config.yaml");
        });

        // 8. Pi & OMP (local & global)
        run_isolated(&|_home, _ws| {
            super::super::init::pi::run_pi_mode_with_patch_mode(
                false,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "pi local");
            std::fs::remove_file(user_dirs::in_working_dir(".pi/extensions/rtk.ts")).unwrap();
            assert!(!is_any_agent_configured(), "pi local removed");

            super::super::init::pi::run_pi_mode_with_patch_mode(
                true,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "pi global");
            std::fs::remove_file(
                user_dirs::home()
                    .unwrap()
                    .join(".pi/agent/extensions/rtk.ts"),
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "pi global removed");

            super::super::init::pi::run_omp_mode_with_patch_mode(
                false,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "omp local");
            std::fs::remove_file(user_dirs::in_working_dir(".omp/extensions/rtk.ts")).unwrap();
            assert!(!is_any_agent_configured(), "omp local removed");

            super::super::init::pi::run_omp_mode_with_patch_mode(
                true,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "omp global");
            std::fs::remove_file(
                user_dirs::home()
                    .unwrap()
                    .join(".omp/agent/extensions/rtk.ts"),
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "omp global removed");
        });

        // 9. Vibe
        run_isolated(&|_home, _ws| {
            super::super::init::vibe::run_vibe_mode(
                true,
                false,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "vibe global");
            super::super::init::vibe::uninstall_vibe(InitContext::default()).unwrap();
            assert!(!is_any_agent_configured(), "vibe uninstalled");
        });
    }
}
