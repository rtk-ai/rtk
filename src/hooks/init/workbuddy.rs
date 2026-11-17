//! WorkBuddy agent: global hook installation and lifecycle.

use super::*;
use crate::hooks::constants::{WORKBUDDY_DIR, WORKBUDDY_HOOK_COMMAND, WORKBUDDY_MATCHER};

fn is_workbuddy_hook_command(command: &str) -> bool {
    crate::hooks::is_rtk_hook_command(command, "workbuddy")
}

fn workbuddy_settings_path(global: bool) -> Result<PathBuf> {
    if !global {
        anyhow::bail!(
            "WorkBuddy installation is global-only. Use: rtk init -g --agent workbuddy. Project settings are shared with CodeBuddy and are not managed by this installer."
        );
    }
    let dir = user_dirs::env_path("WORKBUDDY_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| resolve_home_subdir(WORKBUDDY_DIR))?;
    Ok(dir.join(SETTINGS_JSON))
}

fn is_workbuddy_hook_entry(hook: &serde_json::Value) -> bool {
    is_command_hook(hook, is_workbuddy_hook_command)
}

fn workbuddy_hook_already_present(root: &serde_json::Value) -> bool {
    ["Bash", "execute_command"].iter().all(|tool| {
        hook_present(
            root,
            PRE_TOOL_USE_KEY,
            HookEntries::Grouped,
            |group| group_covers_tool(group, tool),
            is_workbuddy_hook_entry,
        )
    })
}

fn patch_workbuddy_settings(path: &Path, mode: PatchMode, ctx: InitContext) -> Result<PatchResult> {
    let mut root = read_json_file(path)?.unwrap_or_else(|| serde_json::json!({}));
    if workbuddy_hook_already_present(&root) {
        return Ok(PatchResult::AlreadyPresent);
    }
    match mode {
        PatchMode::Skip => return Ok(PatchResult::Skipped),
        PatchMode::Ask if !ctx.dry_run && !prompt_user_consent(path)? => {
            return Ok(PatchResult::Declined);
        }
        _ => {}
    }
    append_hook_entry(
        &mut root,
        PRE_TOOL_USE_KEY,
        serde_json::json!({
            "matcher": WORKBUDDY_MATCHER,
            "hooks": [{"type": "command", "command": WORKBUDDY_HOOK_COMMAND}]
        }),
    )?;
    if !ctx.dry_run
        && let Some(parent) = path.parent()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    update_json_file(
        path,
        &root,
        ctx,
        "WorkBuddy settings",
        &format!(
            "[dry-run] would patch WorkBuddy settings: {}",
            path.display()
        ),
        true,
        Written::Backup,
    )?;
    Ok(if ctx.dry_run {
        PatchResult::WouldPatch
    } else {
        PatchResult::Patched
    })
}

pub fn run_workbuddy_mode(global: bool, mode: PatchMode, ctx: InitContext) -> Result<()> {
    let path = workbuddy_settings_path(global)?;
    match patch_workbuddy_settings(&path, mode, ctx)? {
        PatchResult::Patched => {
            println!("WorkBuddy hook installed: {}", path.display());
            println!("Restart WorkBuddy, then ask it to run git status.");
        }
        PatchResult::AlreadyPresent => {
            println!("WorkBuddy hook already installed: {}", path.display())
        }
        PatchResult::WouldPatch => {}
        PatchResult::Skipped | PatchResult::Declined => {
            println!("WorkBuddy hook was not installed.");
            println!("To install, run: rtk init -g --agent workbuddy --auto-patch");
        }
    }
    Ok(())
}

fn remove_workbuddy_settings(path: &Path, ctx: InitContext) -> Result<bool> {
    let Some(mut root) = read_json_file(path)? else {
        return Ok(false);
    };
    if !remove_hook_entries(
        &mut root,
        PRE_TOOL_USE_KEY,
        HookEntries::Grouped,
        is_workbuddy_hook_entry,
    ) {
        return Ok(false);
    }
    update_json_file(
        path,
        &root,
        ctx,
        "WorkBuddy settings",
        &format!("[dry-run] would remove WorkBuddy hook: {}", path.display()),
        true,
        Written::Backup,
    )?;
    Ok(true)
}

pub fn uninstall_workbuddy_mode(global: bool, ctx: InitContext) -> Result<()> {
    let path = workbuddy_settings_path(global)?;
    let removed = remove_workbuddy_settings(&path, ctx)?;
    if !ctx.dry_run {
        if removed {
            println!(
                "WorkBuddy hook removed: {}. Restart WorkBuddy.",
                path.display()
            );
        } else {
            println!("WorkBuddy hook was not installed (nothing to remove).");
        }
    }
    Ok(())
}

pub fn show_workbuddy_config(global: bool) -> Result<()> {
    let path = workbuddy_settings_path(global)?;
    let present = read_json_file(&path)?.is_some_and(|root| workbuddy_hook_already_present(&root));
    println!(
        "WorkBuddy hook: {} ({})",
        if present {
            "installed"
        } else {
            "not installed"
        },
        path.display()
    );
    Ok(())
}

/// Detect only the global settings this installer manages, including its override.
pub fn is_configured() -> bool {
    workbuddy_settings_path(true)
        .ok()
        .and_then(|path| read_json_file(&path).ok().flatten())
        .is_some_and(|root| workbuddy_hook_already_present(&root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{test_isolation, user_env};

    #[test]
    fn workbuddy_probe_matches_installer_scopes_and_valid_hooks() {
        let tmp = test_isolation::tempdir();
        test_isolation::with_root(tmp.path(), || {
            user_env::with_vars(&[("WORKBUDDY_CONFIG_DIR", None)], || {
                let path = workbuddy_settings_path(true).unwrap();
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                assert!(!is_configured());
                for content in [
                    "not json",
                    "{}",
                    r#"{"hooks":{"PreToolUse":[{"matcher":"Bash|execute_command","hooks":[{"type":"command","command":"rtk hook trae"}]}]}}"#,
                    r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook workbuddy"}]}]}}"#,
                ] {
                    fs::write(&path, content).unwrap();
                    assert!(!is_configured());
                }
                let valid = serde_json::json!({"hooks":{"PreToolUse":[{"matcher":WORKBUDDY_MATCHER,"hooks":[{"type":"command","command":WORKBUDDY_HOOK_COMMAND}]}]}});
                fs::write(&path, format!("\u{feff}{valid}")).unwrap();
                assert!(is_configured());
                assert!(crate::hooks::hook_check::is_any_agent_configured());
                user_env::with_vars(&[("WORKBUDDY_CONFIG_DIR", Some(""))], || {
                    assert!(is_configured())
                });
                let override_dir = tmp.path().join("override");
                user_env::with_path("WORKBUDDY_CONFIG_DIR", Some(&override_dir), || {
                    assert!(!is_configured(), "override must not fall back to home");
                    fs::create_dir_all(&override_dir).unwrap();
                    fs::write(override_dir.join(SETTINGS_JSON), valid.to_string()).unwrap();
                    assert!(is_configured());
                });
                fs::remove_file(path).unwrap();
                let project = tmp.path().join(".codebuddy");
                fs::create_dir_all(&project).unwrap();
                fs::write(project.join(SETTINGS_JSON), valid.to_string()).unwrap();
                assert!(
                    !is_configured(),
                    "shared CodeBuddy project scope is not managed"
                );
            });
        });
    }
}
