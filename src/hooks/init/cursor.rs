//! Cursor agent: hook install/uninstall helpers.

use super::*;

// ─── Cursor Agent support ─────────────────────────────────────────────

pub(crate) fn resolve_cursor_dir() -> Result<PathBuf> {
    resolve_home_subdir(CURSOR_DIR)
}

/// Install Cursor hooks: register binary command in hooks.json
pub(crate) fn install_cursor_hooks(ctx: InitContext) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
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
        if let Err(e) = remove_legacy_cursor_hooks_json_entries(&hooks_json_path, ctx) {
            if verbose > 0 {
                eprintln!("  [warn] Failed to clean legacy Cursor hooks.json entry: {e}");
            }
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
pub(crate) fn patch_cursor_hooks_json(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext { verbose, dry_run } = ctx;
    let mut root = if path.exists() {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let content = strip_leading_bom(&content);
        if content.trim().is_empty() {
            serde_json::json!({ "version": 1 })
        } else {
            from_json_str(content)
                .with_context(|| format!("Failed to parse {} as JSON", path.display()))?
        }
    } else {
        serde_json::json!({ "version": 1 })
    };

    // Check idempotency
    if cursor_hook_already_present(&root) {
        if verbose > 0 {
            eprintln!("Cursor hooks.json: RTK hook already present");
        }
        return Ok(false);
    }

    insert_cursor_hook_entry(&mut root)?;

    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize hooks.json")?;

    if dry_run {
        println!(
            "[dry-run] would patch Cursor hooks.json: {}",
            path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{}", serialized);
        }
        return Ok(true);
    }

    // Backup if exists
    if path.exists() {
        let backup_path = path.with_extension("json.bak");
        fs::copy(path, &backup_path)
            .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;
        if verbose > 0 {
            eprintln!("Backup: {}", backup_path.display());
        }
    }

    // Atomic write
    atomic_write(path, &serialized)?;

    Ok(true)
}

/// Check if RTK preToolUse hook is already present in Cursor hooks.json
/// Matches on legacy rtk-rewrite.sh path OR new `rtk hook cursor` command
pub(crate) fn cursor_hook_already_present(root: &serde_json::Value) -> bool {
    let hooks = match root
        .get("hooks")
        .and_then(|h| h.get("preToolUse"))
        .and_then(|p| p.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };

    hooks.iter().any(|entry| {
        entry
            .get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|cmd| cmd.contains(REWRITE_HOOK_FILE) || cmd == CURSOR_HOOK_COMMAND)
    })
}

/// Insert RTK preToolUse entry into Cursor hooks.json
pub(crate) fn insert_cursor_hook_entry(root: &mut serde_json::Value) -> Result<()> {
    let root_obj = match root.as_object_mut() {
        Some(obj) => obj,
        None => {
            *root = serde_json::json!({ "version": 1 });
            root.as_object_mut().expect("just-created json object")
        }
    };

    root_obj.entry("version").or_insert(serde_json::json!(1));

    let hooks = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("hooks value is not an object")?;

    let pre_tool_use = hooks
        .entry("preToolUse")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .context("preToolUse value is not an array")?;

    pre_tool_use.push(serde_json::json!({
        "command": CURSOR_HOOK_COMMAND,
        "matcher": "Shell"
    }));
    Ok(())
}

/// Remove only legacy `rtk-rewrite.sh` entries from Cursor hooks.json.
/// Preserves any existing `rtk hook cursor` entries (new format).
pub(crate) fn remove_legacy_cursor_hooks_json_entries(path: &Path, ctx: InitContext) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    if !path.exists() {
        return Ok(());
    }

    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let content = strip_leading_bom(&content);
    if content.trim().is_empty() {
        return Ok(());
    }

    let mut root: serde_json::Value =
        from_json_str(content).with_context(|| format!("Failed to parse {}", path.display()))?;

    if !remove_legacy_cursor_hook_entries_from_json(&mut root) {
        return Ok(());
    }

    if dry_run {
        println!(
            "[dry-run] would remove legacy rtk-rewrite.sh entry from Cursor hooks.json: {}",
            path.display()
        );
        return Ok(());
    }

    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize hooks.json")?;
    atomic_write(path, &serialized)?;

    if verbose > 0 {
        eprintln!("  [ok] Removed legacy rtk-rewrite.sh entry from Cursor hooks.json");
    }
    Ok(())
}

/// Remove only legacy `rtk-rewrite.sh` entries from parsed Cursor hooks.json.
/// Returns true if any entries were removed.
/// Does NOT remove `rtk hook cursor` entries — those are the new format.
pub(crate) fn remove_legacy_cursor_hook_entries_from_json(root: &mut serde_json::Value) -> bool {
    let pre_tool_use = match root
        .get_mut("hooks")
        .and_then(|h| h.get_mut("preToolUse"))
        .and_then(|p| p.as_array_mut())
    {
        Some(arr) => arr,
        None => return false,
    };

    let original_len = pre_tool_use.len();
    pre_tool_use.retain(|entry| {
        !entry
            .get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|cmd| cmd.contains(REWRITE_HOOK_FILE))
    });

    pre_tool_use.len() < original_len
}

/// Remove Cursor RTK artifacts: hook script + hooks.json entry
pub(crate) fn remove_cursor_hooks(ctx: InitContext) -> Result<Vec<String>> {
    let InitContext { verbose, dry_run } = ctx;
    let cursor_dir = resolve_cursor_dir()?;
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
    if hooks_json_path.exists() {
        let content = fs::read_to_string(&hooks_json_path)
            .with_context(|| format!("Failed to read {}", hooks_json_path.display()))?;
        let content = strip_leading_bom(&content);

        if !content.trim().is_empty() {
            if let Ok(mut root) = from_json_str::<serde_json::Value>(content) {
                if remove_cursor_hook_from_json(&mut root) {
                    if dry_run {
                        println!(
                            "[dry-run] would remove RTK entry from Cursor hooks.json: {}",
                            hooks_json_path.display()
                        );
                    } else {
                        let backup_path = hooks_json_path.with_extension("json.bak");
                        fs::copy(&hooks_json_path, &backup_path).ok();

                        let serialized = serde_json::to_string_pretty(&root)
                            .context("Failed to serialize hooks.json")?;
                        atomic_write(&hooks_json_path, &serialized)?;

                        if verbose > 0 {
                            eprintln!("Removed RTK hook from Cursor hooks.json");
                        }
                    }
                    removed.push("Cursor hooks.json: removed RTK entry".to_string());
                }
            }
        }
    }

    Ok(removed)
}

/// Remove RTK preToolUse entry from Cursor hooks.json
/// Returns true if entry was found and removed
/// Matches both legacy script path and new binary command
pub(crate) fn remove_cursor_hook_from_json(root: &mut serde_json::Value) -> bool {
    let pre_tool_use = match root
        .get_mut("hooks")
        .and_then(|h| h.get_mut("preToolUse"))
        .and_then(|p| p.as_array_mut())
    {
        Some(arr) => arr,
        None => return false,
    };

    let original_len = pre_tool_use.len();
    pre_tool_use.retain(|entry| {
        !entry
            .get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|cmd| cmd.contains(REWRITE_HOOK_FILE) || cmd == CURSOR_HOOK_COMMAND)
    });

    pre_tool_use.len() < original_len
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Cursor hooks.json tests ───

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
}
