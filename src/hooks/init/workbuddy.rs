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
