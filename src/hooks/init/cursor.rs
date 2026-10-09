//! Cursor agent: hook install/uninstall helpers.

use super::*;
use crate::hooks::constants::{
    CURSOR_DIR, CURSOR_HOOK_COMMAND, HOOKS_JSON, HOOKS_SUBDIR, REWRITE_HOOK_FILE,
};

// Cursor Agent support

pub(super) fn resolve_cursor_dir() -> Result<PathBuf> {
    resolve_home_subdir(CURSOR_DIR)
}

/// Install Cursor hooks: register binary command in hooks.json
pub(super) fn install_cursor_hooks(ctx: InitContext) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let cursor_dir = resolve_cursor_dir()?;

    // Migrate old hook script if present
    let old_hook = cursor_dir.join("hooks").join(REWRITE_HOOK_FILE);
    if old_hook.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove old Cursor hook script: {}",
                old_hook.display()
            );
        } else {
            // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
            let _ = fs::remove_file(&old_hook);
            if verbose > 0 {
                eprintln!(
                    "  [ok] Removed old Cursor hook script: {}",
                    old_hook.display()
                );
            }
        }
        // Clean stale hooks.json entry pointing to the deleted script
        let hooks_json_path = cursor_dir.join(HOOKS_JSON);
        if let Err(e) = remove_legacy_cursor_hooks_json_entries(&hooks_json_path, ctx)
            && verbose > 0
        {
            eprintln!("  [warn] Failed to clean legacy Cursor hooks.json entry: {e}");
        }
    }

    // Create or patch hooks.json with binary command
    let hooks_json_path = cursor_dir.join(HOOKS_JSON);
    let patched = patch_cursor_hooks_json(&hooks_json_path, ctx)?;

    // Report (skip in dry-run)
    if !dry_run {
        println!("\nCursor hook registered (global).\n");
        println!("  Command:    {}", CURSOR_HOOK_COMMAND);
        println!("  hooks.json: {}", hooks_json_path.display());

        if patched {
            println!("  hooks.json: RTK preToolUse entry added");
        } else {
            println!("  hooks.json: RTK preToolUse entry already present");
        }

        println!("  Cursor reloads hooks.json automatically. Test with: git status\n");
    }

    Ok(())
}

/// Patch ~/.cursor/hooks.json to add RTK preToolUse hook.
/// Returns true if the file was modified.
fn patch_cursor_hooks_json(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext { verbose, .. } = ctx;
    let mut root = read_json_file(path)?.unwrap_or_else(|| serde_json::json!({ "version": 1 }));

    // Check idempotency
    if cursor_hook_already_present(&root) {
        if verbose > 0 {
            eprintln!("Cursor hooks.json: RTK hook already present");
        }
        return Ok(false);
    }

    insert_cursor_hook_entry(&mut root)?;

    update_json_file(
        path,
        &root,
        ctx,
        "hooks.json",
        Report::new(format!(
            "[dry-run] would patch Cursor hooks.json: {}",
            path.display()
        ))
        .with_content(),
    )?;

    Ok(true)
}

/// Check if RTK preToolUse hook is already present in Cursor hooks.json
/// Matches on legacy rtk-rewrite.sh path OR new `rtk hook cursor` command
pub(super) fn cursor_hook_already_present(root: &serde_json::Value) -> bool {
    hook_present(
        root,
        "preToolUse",
        HookEntries::Flat,
        |entry| group_covers_tool(entry, "Shell"),
        is_cursor_hook_entry,
    )
}

fn is_cursor_hook_entry(hook: &serde_json::Value) -> bool {
    is_command_hook(hook, |cmd| {
        cmd.contains(REWRITE_HOOK_FILE) || cmd == CURSOR_HOOK_COMMAND
    })
}

/// Insert RTK preToolUse entry into Cursor hooks.json
fn insert_cursor_hook_entry(root: &mut serde_json::Value) -> Result<()> {
    if !root.is_object() {
        *root = serde_json::json!({});
    }
    root.as_object_mut()
        .expect("object")
        .entry("version")
        .or_insert(serde_json::json!(1));
    append_hook_entry(
        root,
        "preToolUse",
        serde_json::json!({
            "command": CURSOR_HOOK_COMMAND, "matcher": "Shell"
        }),
    )
}

/// Remove only legacy `rtk-rewrite.sh` entries from Cursor hooks.json.
/// Preserves any existing `rtk hook cursor` entries (new format).
fn remove_legacy_cursor_hooks_json_entries(path: &Path, ctx: InitContext) -> Result<()> {
    let Some(mut root) = read_json_file(path)? else {
        return Ok(());
    };

    if !remove_legacy_cursor_hook_entries_from_json(&mut root) {
        return Ok(());
    }

    update_json_file(
        path,
        &root,
        ctx,
        "hooks.json",
        Report::new(format!(
            "[dry-run] would remove legacy rtk-rewrite.sh entry from Cursor hooks.json: {}",
            path.display()
        ))
        .done_verbose(
            "  [ok] Removed legacy rtk-rewrite.sh entry from Cursor hooks.json".to_string(),
        ),
    )
}

/// Remove only legacy `rtk-rewrite.sh` entries from parsed Cursor hooks.json.
/// Returns true if any entries were removed.
/// Does NOT remove `rtk hook cursor` entries — those are the new format.
pub(super) fn remove_legacy_cursor_hook_entries_from_json(root: &mut serde_json::Value) -> bool {
    remove_hook_entries(root, "preToolUse", HookEntries::Flat, |hook| {
        is_command_hook(hook, |cmd| cmd.contains(REWRITE_HOOK_FILE))
    })
}

/// Remove Cursor RTK artifacts: hook script + hooks.json entry
pub(super) fn remove_cursor_hooks(ctx: InitContext) -> Result<Vec<String>> {
    let cursor_dir = resolve_cursor_dir()?;
    remove_cursor_hooks_at(&cursor_dir, ctx)
}

/// Remove RTK preToolUse entry from Cursor hooks.json
/// Returns true if entry was found and removed
/// Matches both legacy script path and new binary command
fn remove_cursor_hook_from_json(root: &mut serde_json::Value) -> bool {
    remove_hook_entries(root, "preToolUse", HookEntries::Flat, is_cursor_hook_entry)
}

fn remove_cursor_hooks_at(cursor_dir: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext { dry_run, .. } = ctx;
    let mut removed = Vec::new();

    // 1. Remove hook script
    let hook_path = cursor_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
    if hook_path.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove Cursor hook: {}",
                hook_path.display()
            );
        } else {
            // nosemgrep: filesystem-deletion
            fs::remove_file(&hook_path).with_context(|| {
                format!("Failed to remove Cursor hook: {}", hook_path.display())
            })?;
        }
        removed.push(format!("Cursor hook: {}", hook_path.display()));
    }

    // 2. Remove RTK entry from hooks.json
    let hooks_json_path = cursor_dir.join(HOOKS_JSON);
    let root = match read_json_file(&hooks_json_path) {
        Ok(root) => root,
        Err(error) if error.downcast_ref::<serde_json::Error>().is_some() => {
            eprintln!(
                "rtk: warning: leaving malformed Cursor hooks.json unchanged during uninstall: {error:#}"
            );
            None
        }
        Err(error) => return Err(error),
    };
    if let Some(mut root) = root
        && remove_cursor_hook_from_json(&mut root)
    {
        update_json_file(
            &hooks_json_path,
            &root,
            ctx,
            "hooks.json",
            Report::new(format!(
                "[dry-run] would remove RTK entry from Cursor hooks.json: {}",
                hooks_json_path.display()
            ))
            .done_verbose("Removed RTK hook from Cursor hooks.json".to_string()),
        )?;
        removed.push("Cursor hooks.json: removed RTK entry".to_string());
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_registration_preserves_prompt_and_unrelated_entries() {
        let mut root = serde_json::json!({"hooks": {"preToolUse": [
            {"matcher": "Read", "command": CURSOR_HOOK_COMMAND},
            {"matcher": "Shell", "type": "prompt", "command": CURSOR_HOOK_COMMAND},
            {"matcher": "Shell", "command": "echo user"}
        ]}});
        assert!(!cursor_hook_already_present(&root));
        insert_cursor_hook_entry(&mut root).unwrap();
        assert!(cursor_hook_already_present(&root));
        assert!(remove_cursor_hook_from_json(&mut root));
        assert!(!remove_cursor_hook_from_json(&mut root));
        assert_eq!(
            root["hooks"]["preToolUse"],
            serde_json::json!([
                {"matcher": "Shell", "type": "prompt", "command": CURSOR_HOOK_COMMAND},
                {"matcher": "Shell", "command": "echo user"}
            ])
        );
    }

    // Cursor hooks.json tests

    #[test]
    fn test_cursor_hook_already_present_legacy_script() {
        let json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [{
                    "command": "./hooks/rtk-rewrite.sh",
                    "matcher": "Shell"
                }]
            }
        });
        assert!(cursor_hook_already_present(&json_content));
    }

    #[test]
    fn test_cursor_hook_already_present_new_command() {
        let json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [{
                    "command": CURSOR_HOOK_COMMAND,
                    "matcher": "Shell"
                }]
            }
        });
        assert!(cursor_hook_already_present(&json_content));
    }

    #[test]
    fn test_cursor_hook_already_present_false_empty() {
        let json_content = serde_json::json!({ "version": 1 });
        assert!(!cursor_hook_already_present(&json_content));
    }

    #[test]
    fn test_cursor_hook_already_present_false_other_hooks() {
        let json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [{
                    "command": "./hooks/some-other-hook.sh",
                    "matcher": "Shell"
                }]
            }
        });
        assert!(!cursor_hook_already_present(&json_content));
    }

    #[test]
    fn test_insert_cursor_hook_entry_empty() {
        let mut json_content = serde_json::json!({ "version": 1 });
        insert_cursor_hook_entry(&mut json_content).unwrap();

        let hooks = json_content["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], CURSOR_HOOK_COMMAND);
        assert_eq!(hooks[0]["matcher"], "Shell");
        assert_eq!(json_content["version"], 1);
    }

    #[test]
    fn test_insert_cursor_hook_preserves_existing() {
        let mut json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [{
                    "command": "./hooks/other.sh",
                    "matcher": "Shell"
                }],
                "afterFileEdit": [{
                    "command": "./hooks/format.sh"
                }]
            }
        });

        insert_cursor_hook_entry(&mut json_content).unwrap();

        let pre_tool_use = json_content["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 2);
        assert_eq!(pre_tool_use[0]["command"], "./hooks/other.sh");
        assert_eq!(pre_tool_use[1]["command"], CURSOR_HOOK_COMMAND);

        // afterFileEdit should be preserved
        assert!(json_content["hooks"]["afterFileEdit"].is_array());
    }

    #[test]
    fn test_remove_cursor_hook_from_json() {
        let mut json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [
                    { "command": "./hooks/other.sh", "matcher": "Shell" },
                    { "command": "./hooks/rtk-rewrite.sh", "matcher": "Shell" }
                ]
            }
        });

        let removed = remove_cursor_hook_from_json(&mut json_content);
        assert!(removed);

        let hooks = json_content["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], "./hooks/other.sh");
    }

    #[test]
    fn test_remove_cursor_hook_from_json_new_command() {
        let mut json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [
                    { "command": "./hooks/other.sh", "matcher": "Shell" },
                    { "command": CURSOR_HOOK_COMMAND, "matcher": "Shell" }
                ]
            }
        });

        let removed = remove_cursor_hook_from_json(&mut json_content);
        assert!(removed);

        let hooks = json_content["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], "./hooks/other.sh");
    }

    #[test]
    fn test_remove_cursor_hook_not_present() {
        let mut json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [
                    { "command": "./hooks/other.sh", "matcher": "Shell" }
                ]
            }
        });

        let removed = remove_cursor_hook_from_json(&mut json_content);
        assert!(!removed);
    }

    #[test]
    fn test_remove_legacy_cursor_entries_strips_old_script() {
        let mut root = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [{
                    "command": "./hooks/rtk-rewrite.sh",
                    "matcher": "Shell"
                }]
            }
        });

        assert!(remove_legacy_cursor_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["preToolUse"].as_array().unwrap();
        assert!(arr.is_empty());
    }

    #[test]
    fn test_remove_legacy_cursor_entries_preserves_new_command() {
        let mut root = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [
                    {
                        "command": "./hooks/rtk-rewrite.sh",
                        "matcher": "Shell"
                    },
                    {
                        "command": CURSOR_HOOK_COMMAND,
                        "matcher": "Shell"
                    }
                ]
            }
        });

        assert!(remove_legacy_cursor_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["command"].as_str().unwrap(), CURSOR_HOOK_COMMAND);
    }
    #[test]
    fn test_patch_cursor_hook_propagates_backup_failure_and_preserves_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join(HOOKS_JSON);
        let original = serde_json::to_string_pretty(&serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [{
                    "command": "./hooks/other.sh",
                    "matcher": "Shell"
                }]
            }
        }))
        .unwrap();
        fs::write(&path, &original).unwrap();
        fs::create_dir(path.with_extension("json.bak")).unwrap();

        let err = patch_cursor_hooks_json(&path, InitContext::default()).unwrap_err();

        assert!(format!("{err:#}").contains("backup"));
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn test_remove_cursor_hooks_keeps_malformed_json_best_effort() {
        let temp = TempDir::new().unwrap();
        let cursor_dir = temp.path().join(CURSOR_DIR);
        let hook_path = cursor_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
        let hooks_json = cursor_dir.join(HOOKS_JSON);
        fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
        fs::write(&hook_path, "legacy hook").unwrap();
        fs::write(&hooks_json, "{").unwrap();

        let removed = remove_cursor_hooks_at(&cursor_dir, InitContext::default()).unwrap();

        assert!(!hook_path.exists());
        assert_eq!(fs::read_to_string(hooks_json).unwrap(), "{");
        assert_eq!(
            removed,
            vec![format!("Cursor hook: {}", hook_path.display())]
        );
    }

    #[test]
    fn test_remove_cursor_hooks_still_propagates_read_errors() {
        let temp = TempDir::new().unwrap();
        let cursor_dir = temp.path().join(CURSOR_DIR);
        let hooks_json = cursor_dir.join(HOOKS_JSON);
        fs::create_dir_all(&hooks_json).unwrap();

        let err = remove_cursor_hooks_at(&cursor_dir, InitContext::default()).unwrap_err();

        assert!(format!("{err:#}").contains(&hooks_json.display().to_string()));
    }
}
