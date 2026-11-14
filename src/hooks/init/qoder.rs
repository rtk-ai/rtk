use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{
    AGENTS_MD, HookEntries, InitContext, RTK_MD, RTK_MD_REF,
    agents_md::{patch_agents_md, remove_rtk_reference_from_agents},
    append_hook_entry, atomic_write, group_covers_tool, hook_present, is_command_hook,
    print_dry_run_footer, read_json_file, remove_hook_entries, resolve_home_subdir,
};
use crate::core::user_dirs;
use crate::hooks::constants::{PRE_TOOL_USE_KEY, QODER_DIR, SETTINGS_JSON};

const QODER_AWARENESS: &str = include_str!("../../../hooks/qoder/rtk-awareness.md");
const QODER_HOOK_COMMAND: &str = "rtk hook qoder";
const QODER_LEGACY_HOOK: &str = "rtk-qoder-hook.sh";
const BASH_TOOL: &str = "Bash";

fn qoder_dir(global: bool) -> Result<PathBuf> {
    if global {
        resolve_home_subdir(QODER_DIR)
    } else {
        Ok(user_dirs::in_working_dir(QODER_DIR))
    }
}

fn awareness_path(qoder_dir: &Path, global: bool) -> PathBuf {
    if global {
        qoder_dir.join(RTK_MD)
    } else {
        user_dirs::in_working_dir(RTK_MD)
    }
}

fn read_qoder_settings(path: &Path) -> Result<Option<Value>> {
    let settings = read_json_file(path)?;
    if let Some(root) = &settings {
        ensure!(
            root.is_object(),
            "{} must contain an object",
            path.display()
        );
        ensure!(
            root.get("hooks").is_none_or(Value::is_object),
            "{}: hooks must be an object",
            path.display()
        );
        ensure!(
            root.pointer("/hooks/PreToolUse")
                .is_none_or(Value::is_array),
            "{}: hooks.PreToolUse must be an array",
            path.display()
        );
    }
    Ok(settings)
}

fn is_native_hook(hook: &Value) -> bool {
    is_command_hook(hook, |command| {
        super::super::is_rtk_hook_command(command, "qoder")
    })
}

fn is_legacy_hook(hook: &Value) -> bool {
    is_command_hook(hook, |command| {
        let parts = crate::discover::lexer::shell_split(command);
        matches!(parts.as_slice(), [script]
            if script.rsplit(['/', '\\']).next() == Some(QODER_LEGACY_HOOK))
    })
}

fn covers_bash(group: &Value) -> bool {
    group_covers_tool(group, BASH_TOOL)
}

fn settings_have_hook(settings: &Value, owns: impl Fn(&Value) -> bool) -> bool {
    hook_present(
        settings,
        PRE_TOOL_USE_KEY,
        HookEntries::Grouped,
        covers_bash,
        owns,
    )
}

fn active_hook_at(path: &Path) -> Result<bool> {
    Ok(read_qoder_settings(path)?
        .is_some_and(|settings| settings_have_hook(&settings, is_native_hook)))
}

fn save_settings(path: &Path, settings: &Value, action: &str, ctx: InitContext) -> Result<()> {
    let content = serde_json::to_string_pretty(settings)?;
    if ctx.dry_run {
        println!("[dry-run] would {action}: {}", path.display());
        if ctx.verbose > 0 {
            println!("[dry-run] content:\n{content}");
        }
    } else {
        atomic_write(path, &content)
            .with_context(|| format!("Failed to update {}", path.display()))?;
    }
    Ok(())
}

/// Install the hook and instructions in the selected scope.
pub fn run_qoder_mode(global: bool, ctx: InitContext) -> Result<()> {
    let dir = qoder_dir(global)?;
    let settings_path = dir.join(SETTINGS_JSON);
    let mut settings = read_qoder_settings(&settings_path)?.unwrap_or_else(|| json!({}));
    let hook_change = install_hook(&mut settings)?;

    if !ctx.dry_run {
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create Qoder config dir: {}", dir.display()))?;
    }
    let rtk_md = awareness_path(&dir, global);
    if !rtk_md.exists() {
        super::write_if_changed(&rtk_md, QODER_AWARENESS, RTK_MD, ctx)?;
    }
    if !global {
        patch_agents_md(&user_dirs::in_working_dir(AGENTS_MD), RTK_MD_REF, ctx)?;
    }
    if let Some(action) = hook_change {
        save_settings(&settings_path, &settings, action, ctx)?;
    }

    if ctx.dry_run {
        print_dry_run_footer();
    } else {
        let scope = if global { "global" } else { "project-scoped" };
        println!("RTK configured for Qoder CLI ({scope}).");
        println!("  RTK.md: {}", rtk_md.display());
        println!(
            "  Hook: {QODER_HOOK_COMMAND} in {}",
            settings_path.display()
        );
    }
    Ok(())
}

fn install_hook(settings: &mut Value) -> Result<Option<&'static str>> {
    if settings_have_hook(settings, is_native_hook) {
        return Ok(None);
    }

    if let Some(groups) = settings
        .pointer_mut("/hooks/PreToolUse")
        .and_then(Value::as_array_mut)
    {
        for group in groups.iter_mut().filter(|group| covers_bash(group)) {
            if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut)
                && let Some(hook) = hooks.iter_mut().find(|hook| is_legacy_hook(hook))
            {
                hook["command"] = json!(QODER_HOOK_COMMAND);
                return Ok(Some("upgrade the Qoder hook in"));
            }
        }
    }

    append_hook_entry(
        settings,
        PRE_TOOL_USE_KEY,
        json!({
            "matcher": BASH_TOOL,
            "hooks": [{"type": "command", "command": QODER_HOOK_COMMAND}]
        }),
    )?;
    Ok(Some("install the Qoder hook in"))
}

pub fn show_qoder_config(global: bool) -> Result<()> {
    let dir = qoder_dir(global)?;
    let path = dir.join(SETTINGS_JSON);
    let settings = read_qoder_settings(&path)?;
    let status = match settings {
        Some(ref root) if settings_have_hook(root, is_native_hook) => QODER_HOOK_COMMAND,
        Some(ref root) if settings_have_hook(root, is_legacy_hook) => "legacy RTK shell hook",
        _ => "not installed",
    };
    println!("Qoder CLI configuration");
    println!("  Hook: {status}");
    println!("  Settings: {}", path.display());
    let rtk_md = awareness_path(&dir, global);
    if rtk_md.is_file() {
        println!("  RTK.md: {}", rtk_md.display());
    }
    Ok(())
}

/// Remove RTK's Qoder entries and generated instructions from the selected scope.
pub fn uninstall_qoder_mode(global: bool, ctx: InitContext) -> Result<()> {
    let dir = qoder_dir(global)?;
    let path = dir.join(SETTINGS_JSON);
    let removed = if let Some(mut settings) = read_qoder_settings(&path)? {
        let removed = remove_hook_entries(
            &mut settings,
            PRE_TOOL_USE_KEY,
            HookEntries::Grouped,
            |hook| is_native_hook(hook) || is_legacy_hook(hook),
        );
        if removed {
            save_settings(&path, &settings, "remove the Qoder hook from", ctx)?;
        }
        removed
    } else {
        false
    };

    let rtk_md = awareness_path(&dir, global);
    let owned_awareness = match fs::read_to_string(&rtk_md) {
        Ok(content) => content.replace("\r\n", "\n") == QODER_AWARENESS.replace("\r\n", "\n"),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => {
            return Err(err).with_context(|| format!("Failed to read {}", rtk_md.display()));
        }
    };
    if owned_awareness {
        if !global {
            remove_rtk_reference_from_agents(
                &user_dirs::in_working_dir(AGENTS_MD),
                &[RTK_MD_REF],
                ctx,
            )?;
        }
        if ctx.dry_run {
            println!("[dry-run] would remove {}", rtk_md.display());
        } else {
            fs::remove_file(&rtk_md)
                .with_context(|| format!("Failed to remove {}", rtk_md.display()))?;
        }
    }

    if ctx.dry_run {
        print_dry_run_footer();
    } else if removed || owned_awareness {
        println!("RTK uninstalled for Qoder CLI ({})", path.display());
    } else {
        println!("No RTK hook or generated instructions found for Qoder CLI.");
    }
    Ok(())
}

pub fn is_configured() -> bool {
    let project = user_dirs::project_root(QODER_DIR).map(|root| root.join(QODER_DIR));
    let global = qoder_dir(true).ok();
    project.is_some_and(|dir| {
        [SETTINGS_JSON, "settings.local.json"]
            .iter()
            .any(|file| active_hook_at(&dir.join(file)).unwrap_or(false))
    }) || global.is_some_and(|dir| active_hook_at(&dir.join(SETTINGS_JSON)).unwrap_or(false))
}
