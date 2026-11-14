use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{
    AGENTS_MD, InitContext, RTK_MD, RTK_MD_REF, agents_md::patch_agents_md, atomic_write,
    print_dry_run_footer, resolve_home_subdir, write_if_changed,
};
use crate::hooks::constants::{PRE_TOOL_USE_KEY, QODER_DIR, SETTINGS_JSON};

/// Embedded Qoder RTK awareness instructions
const RTK_SLIM_QODER: &str = include_str!("../../../hooks/qoder/rtk-awareness.md");

fn resolve_qoder_dir() -> Result<PathBuf> {
    resolve_home_subdir(QODER_DIR)
}

/// Entry point for `rtk init --agent qoder`
pub fn run_qoder_mode(global: bool, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;

    if global {
        let qoder_dir = resolve_qoder_dir()?;
        if !dry_run {
            fs::create_dir_all(&qoder_dir).with_context(|| {
                format!("Failed to create Qoder config dir: {}", qoder_dir.display())
            })?;
        }

        // 1. Write RTK awareness instructions
        let rtk_md_path = qoder_dir.join(RTK_MD);
        write_if_changed(&rtk_md_path, RTK_SLIM_QODER, RTK_MD, ctx)?;

        // 2. Patch ~/.qoder/settings.json with PreToolUse hook
        let hook_status = patch_qoder_settings(&qoder_dir, ctx)?;

        if dry_run {
            print_dry_run_footer();
            return Ok(());
        }

        println!("\nRTK configured for Qoder CLI (global).\n");
        println!("  RTK.md:        {}", rtk_md_path.display());
        match hook_status {
            QoderHookStatus::AlreadyInstalled => {
                println!("  Hook:          Already installed (`rtk hook qoder` in settings.json)");
            }
            QoderHookStatus::Upgraded(ref old) => {
                println!(
                    "  Hook:          Upgraded to `rtk hook qoder` (replaced `{}`)",
                    old
                );
            }
            QoderHookStatus::Installed => {
                println!("  Hook:          Added `rtk hook qoder` to settings.json");
            }
        }
        println!("\n  Restart Qoder CLI. Test with: git status\n");
    } else {
        // Project-scoped: write RTK.md to CWD and patch AGENTS.md
        let rtk_md_path = PathBuf::from(RTK_MD);
        write_if_changed(&rtk_md_path, RTK_SLIM_QODER, RTK_MD, ctx)?;

        let agents_md_path = PathBuf::from(AGENTS_MD);
        let added_ref = patch_agents_md(&agents_md_path, RTK_MD_REF, ctx)?;

        // Write project-scoped hook config
        let project_qoder_dir = PathBuf::from(QODER_DIR);
        if !dry_run {
            fs::create_dir_all(&project_qoder_dir).with_context(|| {
                format!(
                    "Failed to create Qoder config dir: {}",
                    project_qoder_dir.display()
                )
            })?;
        }
        let hook_status = patch_qoder_settings(&project_qoder_dir, ctx)?;

        if dry_run {
            print_dry_run_footer();
            return Ok(());
        }

        println!("\nRTK configured for Qoder CLI (project-scoped).\n");
        println!("  RTK.md:    {}", rtk_md_path.display());
        if added_ref {
            println!("  AGENTS.md: @RTK.md reference added");
        } else {
            println!("  AGENTS.md: @RTK.md reference already present");
        }
        match hook_status {
            QoderHookStatus::AlreadyInstalled => {
                println!(
                    "  Hook:      Already installed (`rtk hook qoder` in .qoder/settings.json)"
                );
            }
            QoderHookStatus::Upgraded(ref old) => {
                println!(
                    "  Hook:      Upgraded to `rtk hook qoder` (replaced `{}`)",
                    old
                );
            }
            QoderHookStatus::Installed => {
                println!("  Hook:      Added `rtk hook qoder` to .qoder/settings.json");
            }
        }
        println!("\n  Restart Qoder CLI. Test with: git status\n");
    }

    Ok(())
}

/// Outcome of patching Qoder settings.json
#[derive(Debug)]
enum QoderHookStatus {
    /// `rtk hook qoder` is already the active hook — nothing to do
    AlreadyInstalled,
    /// Upgraded from a workaround hook (e.g. shell script) to the native binary
    Upgraded(String),
    /// Fresh install — no previous RTK hook existed
    Installed,
}

/// Patch Qoder settings.json with the PreToolUse hook for rtk
fn patch_qoder_settings(qoder_dir: &Path, ctx: InitContext) -> Result<QoderHookStatus> {
    let InitContext { verbose, .. } = ctx;
    let settings_path = qoder_dir.join(SETTINGS_JSON);
    let target_command = "rtk hook qoder";

    // Read or create settings.json
    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read {}", settings_path.display()))?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Check existing hooks
    let pre_tool_use_pointer = format!("/hooks/{}", PRE_TOOL_USE_KEY);
    if let Some(hooks) = settings.pointer(&pre_tool_use_pointer) {
        if let Some(arr) = hooks.as_array() {
            for entry in arr {
                if let Some(cmd) = entry.pointer("/hooks/0/command").and_then(|c| c.as_str()) {
                    if cmd == target_command {
                        if verbose > 0 {
                            eprintln!("Qoder settings.json already has `rtk hook qoder`");
                        }
                        return Ok(QoderHookStatus::AlreadyInstalled);
                    }
                    if cmd.contains("rtk") {
                        // Found a workaround hook — upgrade it to the native binary
                        let old_cmd = cmd.to_string();
                        return upgrade_existing_hook(
                            &settings_path,
                            &mut settings,
                            &pre_tool_use_pointer,
                            old_cmd,
                            ctx,
                        );
                    }
                }
            }
        }
    }

    // No existing RTK hook — add new entry
    install_fresh_hook(&settings_path, &mut settings, ctx)?;
    Ok(QoderHookStatus::Installed)
}

/// Replace an existing workaround hook command with the native binary
fn upgrade_existing_hook(
    settings_path: &Path,
    settings: &mut serde_json::Value,
    pre_tool_use_pointer: &str,
    old_command: String,
    ctx: InitContext,
) -> Result<QoderHookStatus> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    // Navigate to the hooks array and replace the command
    if let Some(arr) = settings
        .pointer_mut(pre_tool_use_pointer)
        .and_then(|v| v.as_array_mut())
    {
        for entry in arr.iter_mut() {
            let needs_upgrade = entry
                .pointer("/hooks/0/command")
                .and_then(|c| c.as_str())
                .is_some_and(|c| c == old_command);
            if needs_upgrade {
                if let Some(hook) = entry.pointer_mut("/hooks/0") {
                    if let Some(obj) = hook.as_object_mut() {
                        obj.insert(
                            "command".to_string(),
                            serde_json::Value::String("rtk hook qoder".to_string()),
                        );
                    }
                }
            }
        }
    }

    let serialized = serde_json::to_string_pretty(&settings)?;
    if dry_run {
        println!(
            "[dry-run] would upgrade Qoder hook in: {}",
            settings_path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{}", serialized);
        }
    } else {
        atomic_write(settings_path, &serialized)
            .with_context(|| format!("Failed to write {}", settings_path.display()))?;
    }

    if verbose > 0 && !dry_run {
        eprintln!(
            "Upgraded Qoder hook from `{}` to `rtk hook qoder`",
            old_command
        );
    }

    Ok(QoderHookStatus::Upgraded(old_command))
}

/// Install a fresh RTK hook entry into settings.json
fn install_fresh_hook(
    settings_path: &Path,
    settings: &mut serde_json::Value,
    ctx: InitContext,
) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let hook_entry = serde_json::json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": "rtk hook qoder"
        }]
    });

    // Insert into settings
    let hooks = settings
        .as_object_mut()
        .context("settings.json is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let entry_for_fallback = hook_entry.clone();
    hooks[PRE_TOOL_USE_KEY] = serde_json::Value::Array(
        hooks
            .get(PRE_TOOL_USE_KEY)
            .and_then(|v| v.as_array())
            .map(|existing| {
                let mut arr = existing.clone();
                // Check for duplicate
                if !arr.iter().any(|entry| {
                    entry.get("matcher").and_then(|m| m.as_str()) == Some("Bash")
                        && entry
                            .pointer("/hooks/0/command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c == "rtk hook qoder")
                }) {
                    arr.push(hook_entry);
                }
                arr
            })
            .unwrap_or_else(|| vec![entry_for_fallback]),
    );

    let serialized = serde_json::to_string_pretty(&settings)?;
    if dry_run {
        println!(
            "[dry-run] would patch Qoder settings.json: {}",
            settings_path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{}", serialized);
        }
        return Ok(());
    }

    atomic_write(settings_path, &serialized)
        .with_context(|| format!("Failed to write {}", settings_path.display()))?;

    if verbose > 0 {
        eprintln!("Patched {} with RTK hook", settings_path.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn settings_has_qoder_hook(path: &Path) -> bool {
        let content = fs::read_to_string(path).unwrap();
        content.contains("rtk hook qoder")
    }

    #[test]
    fn test_qoder_patch_creates_new() {
        let dir = TempDir::new().unwrap();
        let status = patch_qoder_settings(dir.path(), InitContext::default()).unwrap();
        assert!(
            matches!(status, QoderHookStatus::Installed),
            "Fresh init should return Installed"
        );
        assert!(
            settings_has_qoder_hook(&dir.path().join("settings.json")),
            "settings.json should contain rtk hook qoder"
        );
    }

    #[test]
    fn test_qoder_patch_upgrades_shell_script() {
        let dir = TempDir::new().unwrap();
        let settings = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "~/.qoder/hooks/rtk-qoder-hook.sh"
                    }]
                }]
            }
        });
        fs::write(
            dir.path().join("settings.json"),
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .unwrap();

        let status = patch_qoder_settings(dir.path(), InitContext::default()).unwrap();
        match &status {
            QoderHookStatus::Upgraded(old) => {
                assert!(
                    old.contains("rtk-qoder-hook.sh"),
                    "Should detect shell script"
                );
            }
            other => panic!("Expected Upgraded, got {:?}", other),
        }
        assert!(
            settings_has_qoder_hook(&dir.path().join("settings.json")),
            "Shell script should be upgraded to rtk hook qoder"
        );
    }

    #[test]
    fn test_qoder_patch_idempotent() {
        let dir = TempDir::new().unwrap();
        let s1 = patch_qoder_settings(dir.path(), InitContext::default()).unwrap();
        assert!(matches!(s1, QoderHookStatus::Installed));
        let s2 = patch_qoder_settings(dir.path(), InitContext::default()).unwrap();
        assert!(
            matches!(s2, QoderHookStatus::AlreadyInstalled),
            "Second call should return AlreadyInstalled"
        );
    }

    #[test]
    fn test_qoder_patch_preserves_existing() {
        let dir = TempDir::new().unwrap();
        let settings = serde_json::json!({
            "model": {"name": "custom-model"},
            "permissions": {"trustDirectories": ["/tmp/test"]}
        });
        fs::write(
            dir.path().join("settings.json"),
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .unwrap();

        patch_qoder_settings(dir.path(), InitContext::default()).unwrap();
        let content = fs::read_to_string(dir.path().join("settings.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            v.pointer("/model/name").and_then(|v| v.as_str()),
            Some("custom-model"),
            "Existing model config must be preserved"
        );
        assert!(
            v.pointer("/hooks/PreToolUse").is_some(),
            "Hook must be added alongside existing config"
        );
    }

    #[test]
    fn test_qoder_patch_dry_run_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");

        let status = patch_qoder_settings(
            dir.path(),
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        )
        .unwrap();

        assert!(matches!(status, QoderHookStatus::Installed));
        assert!(!settings_path.exists());
    }
}
