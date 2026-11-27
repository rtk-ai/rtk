//! WorkBuddy: hook install/uninstall helpers.
//!
//! WorkBuddy is a CodeBuddy-derived agent that speaks the Claude Code hook
//! protocol: `PreToolUse` events with a `Bash` matcher, and the hook answers
//! with `hookSpecificOutput.updatedInput`. RTK reuses `rtk hook claude`'s
//! processor for it (see `run_claude`), exposed as `rtk hook workbuddy` so the
// two agents stay independently diagnosable and can diverge later.
//!
//! Config lives in `~/.workbuddy/settings.json` (overridable through
//! `$WORKBUDDY_CONFIG_DIR`), with the RTK awareness block appended to the
//! global `CODEBUDDY.md` instruction file.

use super::*;
use crate::core::user_dirs;
use crate::hooks::constants::{
    CLAUDE_PRE_MATCHER, POST_TOOL_USE_KEY, PRE_TOOL_USE_KEY, WORKBUDDY_CONFIG_DIR_ENV,
    WORKBUDDY_DIR, WORKBUDDY_HOOK_COMMAND, WORKBUDDY_MD,
};

fn resolve_workbuddy_dir() -> Result<PathBuf> {
    resolve_config_dir(
        user_dirs::env_path(WORKBUDDY_CONFIG_DIR_ENV),
        user_dirs::home(),
        WORKBUDDY_DIR,
        "Cannot determine WorkBuddy config directory. Set $WORKBUDDY_CONFIG_DIR or $HOME.",
    )
}

/// True when a command is one of RTK's WorkBuddy (or the shared Claude) hooks.
fn owns_workbuddy_hook(command: &str) -> bool {
    command == WORKBUDDY_HOOK_COMMAND
        || crate::hooks::is_workbuddy_hook_command(command)
        || crate::hooks::is_claude_hook_command(command)
        || is_legacy_rewrite_hook(command)
}

/// Check if the RTK hook entry is already present under an event key.
fn workbuddy_hook_present(root: &serde_json::Value, event_key: &str) -> bool {
    root.get("hooks")
        .and_then(|h| h.get(event_key))
        .and_then(|p| p.as_array())
        .is_some_and(|arr| {
            arr.iter()
                .filter_map(|entry| entry.get("hooks")?.as_array())
                .flatten()
                .any(|hook| is_command_hook(hook, owns_workbuddy_hook))
        })
}

/// Insert the RTK hook for one event, creating the structure if missing.
fn insert_workbuddy_hook(
    root: &mut serde_json::Value,
    event_key: &str,
    matcher: &str,
) -> Result<()> {
    let obj = match root.as_object_mut() {
        Some(obj) => obj,
        None => {
            *root = serde_json::json!({});
            root.as_object_mut().expect("just-created object")
        }
    };
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("hooks value is not an object")?;
    let arr = hooks
        .entry(event_key)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .with_context(|| format!("{event_key} value is not an array"))?;
    arr.push(serde_json::json!({
        "matcher": matcher,
        "hooks": [{ "type": "command", "command": WORKBUDDY_HOOK_COMMAND }]
    }));
    Ok(())
}

/// Patch `~/.workbuddy/settings.json` with the PreToolUse/Bash hook.
/// Returns true when the file was changed.
fn patch_workbuddy_settings(
    settings_path: &Path,
    mode: PatchMode,
    ctx: InitContext,
) -> Result<PatchResult> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;

    let mut root = if settings_path.exists() {
        let content = fs::read_to_string(settings_path)
            .with_context(|| format!("Failed to read {}", settings_path.display()))?;
        let content = strip_leading_bom(&content);
        if content.trim().is_empty() {
            serde_json::json!({})
        } else {
            from_json_str(content)
                .with_context(|| format!("Failed to parse {} as JSON", settings_path.display()))?
        }
    } else {
        serde_json::json!({})
    };

    if workbuddy_hook_present(&root, PRE_TOOL_USE_KEY) {
        if verbose > 0 {
            eprintln!("settings.json: WorkBuddy hook already present");
        }
        return Ok(PatchResult::AlreadyPresent);
    }

    match mode {
        PatchMode::Skip => {
            print_manual_instructions(WORKBUDDY_HOOK_COMMAND, false);
            return Ok(PatchResult::Skipped);
        }
        PatchMode::Ask => {
            if dry_run {
                println!(
                    "[dry-run] would prompt before patching {}",
                    settings_path.display()
                );
            } else {
                eprintln!(
                    "\nRTK registers `{WORKBUDDY_HOOK_COMMAND}` to rewrite Bash commands (PreToolUse)."
                );
                if !prompt_user_consent(settings_path)? {
                    print_manual_instructions(WORKBUDDY_HOOK_COMMAND, false);
                    return Ok(PatchResult::Declined);
                }
            }
        }
        PatchMode::Auto => {}
    }

    insert_workbuddy_hook(&mut root, PRE_TOOL_USE_KEY, CLAUDE_PRE_MATCHER)?;
    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;

    if dry_run {
        println!(
            "[dry-run] would patch settings.json ({PRE_TOOL_USE_KEY}, matcher \"{}\"): {}",
            CLAUDE_PRE_MATCHER,
            settings_path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{serialized}");
        }
        return Ok(PatchResult::WouldPatch);
    }

    ensure_parent_dir(settings_path)?;
    if settings_path.exists() {
        let backup_path = settings_path.with_extension("json.bak");
        fs::copy(settings_path, &backup_path)
            .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;
        if verbose > 0 {
            eprintln!("Backup: {}", backup_path.display());
        }
    }
    atomic_write(settings_path, &serialized)?;

    println!("\n  {}: hook added", settings_path.display());
    println!("  Restart WorkBuddy. Test with: git status");
    Ok(PatchResult::Patched)
}

/// Entry point for `rtk init -g --agent workbuddy`.
///
/// Registers the PreToolUse/Bash hook in `~/.workbuddy/settings.json` and
/// appends the RTK awareness block to `~/.workbuddy/CODEBUDDY.md`.
pub fn run_workbuddy_mode(
    global: bool,
    hook_only: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    if !global {
        anyhow::bail!("WorkBuddy support is global-only. Use: rtk init -g --agent workbuddy");
    }
    let wb_dir = resolve_workbuddy_dir()?;
    let settings_path = wb_dir.join(SETTINGS_JSON);

    let patch_result = patch_workbuddy_settings(&settings_path, patch_mode, ctx)?;

    if !hook_only {
        // CODEBUDDY.md is a user-owned instruction file, so upsert a marked
        // block instead of overwriting it; uninstall can then strip it again.
        let md_path = wb_dir.join(WORKBUDDY_MD);
        write_rtk_block(
            &md_path,
            &rtk_block(&instructions_body(ctx.awareness)),
            "RTK awareness block",
            "rtk init -g --agent workbuddy",
            ctx,
        )?;
    }

    if !ctx.dry_run
        && matches!(
            patch_result,
            PatchResult::Patched | PatchResult::AlreadyPresent
        )
    {
        println!("\nRTK hook registered for WorkBuddy.\n");
        println!("  Command: {WORKBUDDY_HOOK_COMMAND}");
    }
    Ok(())
}

/// Remove the RTK hook from WorkBuddy's settings.json and the awareness
/// block from CODEBUDDY.md.
pub fn uninstall_workbuddy(global: bool, ctx: InitContext) -> Result<()> {
    if !global {
        anyhow::bail!(
            "WorkBuddy support is global-only. Use: rtk init -g --agent workbuddy --uninstall"
        );
    }
    let wb_dir = resolve_workbuddy_dir()?;
    let settings_path = wb_dir.join(SETTINGS_JSON);

    if settings_path.exists() && !ctx.dry_run {
        let content = fs::read_to_string(&settings_path)?;
        let mut root: serde_json::Value = from_json_str(strip_leading_bom(&content))?;
        let mut removed = false;
        if let Some(hooks) = root
            .get_mut("hooks")
            .and_then(serde_json::Value::as_object_mut)
        {
            for event_key in [PRE_TOOL_USE_KEY, POST_TOOL_USE_KEY] {
                if let Some(arr) = hooks
                    .get_mut(event_key)
                    .and_then(serde_json::Value::as_array_mut)
                {
                    let before = arr.len();
                    arr.retain(|entry| {
                        !entry
                            .get("hooks")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|hs| {
                                hs.iter().any(|h| {
                                    h.get("command")
                                        .and_then(serde_json::Value::as_str)
                                        .is_some_and(owns_workbuddy_hook)
                                })
                            })
                    });
                    removed |= arr.len() != before;
                }
            }
        }
        if removed {
            let serialized = serde_json::to_string_pretty(&root)?;
            atomic_write(&settings_path, &serialized)?;
            println!("  {}: RTK hook removed", settings_path.display());
        }
    }

    let md_path = wb_dir.join(WORKBUDDY_MD);
    if md_path.exists() && !ctx.dry_run {
        let content = fs::read_to_string(&md_path)?;
        let (cleaned, changed) = remove_rtk_block(&content);
        if changed {
            atomic_write(&md_path, &cleaned)?;
            println!("  {}: RTK awareness block removed", md_path.display());
        }
    }
    Ok(())
}

/// Returns true if WorkBuddy has an active RTK hook configured.
pub fn is_configured() -> bool {
    let Ok(wb_dir) = resolve_workbuddy_dir() else {
        return false;
    };
    let Ok(content) = fs::read_to_string(wb_dir.join(SETTINGS_JSON)) else {
        return false;
    };
    let Ok(root) = from_json_str::<serde_json::Value>(strip_leading_bom(&content)) else {
        return false;
    };
    workbuddy_hook_present(&root, PRE_TOOL_USE_KEY)
}
