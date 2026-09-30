//! Claude Code: the default `rtk init` flow -- settings.json hook registration, RTK.md and
//! CLAUDE.md patching, legacy-hook migration -- plus the `--claude-md` injection mode.

use super::opencode::{ensure_opencode_plugin_installed, prepare_opencode_plugin_path};
use super::*;
use crate::hooks::constants::{
    CLAUDE_DIR, CLAUDE_HOOK_COMMAND, CURSOR_DIR, HOOKS_SUBDIR, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE,
    SETTINGS_JSON,
};

/// Legacy mode (--claude-md): inject the full RTK_INSTRUCTIONS block into CLAUDE.md.
pub(super) fn run_claude_md_mode(
    global: bool,
    install_opencode: bool,
    ctx: InitContext,
) -> Result<()> {
    run_claude_md_mode_with(global, install_opencode, RTK_INSTRUCTIONS, ctx)
}

fn run_claude_md_mode_with(
    global: bool,
    install_opencode: bool,
    block: &str,
    ctx: InitContext,
) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let path = if global {
        resolve_claude_dir()?.join(CLAUDE_MD)
    } else {
        PathBuf::from(CLAUDE_MD)
    };

    if verbose > 0 {
        eprintln!("Writing rtk instructions to: {}", path.display());
    }

    let recovery_cmd = if global {
        "rtk init -g --claude-md"
    } else {
        "rtk init --claude-md"
    };

    let action = write_rtk_block(&path, block, "rtk instructions", recovery_cmd, ctx)?;

    if matches!(action, RtkBlockUpsert::Unchanged) {
        return Ok(());
    }

    if global {
        if install_opencode {
            let opencode_plugin_path = prepare_opencode_plugin_path()?;
            ensure_opencode_plugin_installed(&opencode_plugin_path, ctx)?;
            if !dry_run {
                println!(
                    "[ok] OpenCode plugin installed: {}",
                    opencode_plugin_path.display()
                );
            }
        }
        if !dry_run {
            println!("   Claude Code will now use rtk in all sessions");
        }
    } else if !dry_run {
        println!("   Claude Code will use rtk in this project");
    }

    Ok(())
}

/// Patch CLAUDE.md: add @RTK.md, migrate if old block exists
fn patch_claude_md(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext { verbose, .. } = ctx;
    let mut content = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let mut migrated = false;

    // Check for old block and migrate
    if content.contains(RTK_BLOCK_START) {
        let (new_content, did_migrate) = remove_rtk_block(&content);
        if did_migrate {
            content = new_content;
            migrated = true;
            if verbose > 0 {
                eprintln!("Migrated: removed old RTK block from CLAUDE.md");
            }
        }
    }

    // Check if @RTK.md already present
    if content.contains(RTK_MD_REF) {
        if verbose > 0 {
            eprintln!("@RTK.md reference already present in CLAUDE.md");
        }
        if migrated {
            let report = Report::new(format!(
                "[dry-run] would migrate old RTK block in CLAUDE.md: {}",
                path.display()
            ));
            write_reported(path, WriteKind::Instructions, &content, ctx, report)?;
        }
        return Ok(migrated);
    }

    // Add @RTK.md
    let new_content = if content.is_empty() {
        "@RTK.md\n".to_string()
    } else {
        format!("{}\n\n@RTK.md\n", content.trim())
    };

    let report = Report::new(format!(
        "[dry-run] would add @RTK.md reference to CLAUDE.md: {}",
        path.display()
    ))
    .with_content()
    .done_verbose("Added @RTK.md reference to CLAUDE.md");
    write_reported(path, WriteKind::Instructions, &new_content, ctx, report)?;

    Ok(migrated)
}

pub(super) fn remove_hook_from_json(root: &mut serde_json::Value) -> bool {
    remove_hook_entries(root, PRE_TOOL_USE_KEY, HookEntries::Grouped, |hook| {
        is_command_hook(hook, |cmd| {
            is_claude_hook_command(cmd) || cmd.contains(REWRITE_HOOK_FILE)
        })
    })
}

/// Remove RTK hook from settings.json file
/// Backs up before modification, returns true if hook was found and removed
pub(super) fn remove_hook_from_settings(ctx: InitContext) -> Result<bool> {
    let InitContext { verbose, .. } = ctx;
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join(SETTINGS_JSON);

    let Some(mut root) = read_json_file(&settings_path)? else {
        if verbose > 0 {
            eprintln!("settings.json not found, nothing to remove");
        }
        return Ok(false);
    };

    let removed = remove_hook_from_json(&mut root);
    if removed {
        update_json_file(
            &settings_path,
            &root,
            ctx,
            "settings.json",
            Report::new(format!(
                "[dry-run] would remove RTK hook entry from {}",
                settings_path.display()
            ))
            .with_content()
            .done_verbose("Removed RTK hook from settings.json".to_string()),
        )?;
    }

    Ok(removed)
}

/// Orchestrator: patch settings.json with RTK hook (binary command variant)
/// Handles reading, checking, prompting, merging, backing up, and atomic writing
fn patch_settings_json_command(
    hook_command: &str,
    mode: PatchMode,
    include_opencode: bool,
    ctx: InitContext,
) -> Result<PatchResult> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join(SETTINGS_JSON);

    let mut root = read_json_file(&settings_path)?.unwrap_or_else(|| serde_json::json!({}));

    // Check idempotency
    if hook_already_present(&root, hook_command) {
        if verbose > 0 {
            eprintln!("settings.json: hook already present");
        }
        return Ok(PatchResult::AlreadyPresent);
    }

    // A file the patch would refuse is neither asked about nor patched, whatever the mode:
    // say why, and leave it to the user as a declined prompt does.
    if mode != PatchMode::Skip
        && let Err(error) = ensure_patchable(&settings_path)
    {
        eprintln!("[warn] {error:#}");
        print_manual_instructions(hook_command, include_opencode);
        return Ok(PatchResult::Skipped);
    }

    // Handle mode
    match mode {
        PatchMode::Skip => {
            print_manual_instructions(hook_command, include_opencode);
            return Ok(PatchResult::Skipped);
        }
        PatchMode::Ask => {
            // Skip the interactive prompt in dry-run: we must not mutate state or block on stdin.
            if dry_run {
                println!(
                    "[dry-run] would prompt before patching {}",
                    settings_path.display()
                );
            } else if !prompt_user_consent(&settings_path)? {
                print_manual_instructions(hook_command, include_opencode);
                return Ok(PatchResult::Declined);
            }
        }
        PatchMode::Auto => {
            // Proceed without prompting
        }
    }

    insert_hook_entry(&mut root, hook_command)?;

    update_json_file(
        &settings_path,
        &root,
        ctx,
        "settings.json",
        Report::new(format!(
            "[dry-run] would patch settings.json: {}",
            settings_path.display()
        ))
        .with_content(),
    )?;
    if dry_run {
        return Ok(PatchResult::WouldPatch);
    }

    println!("\n  settings.json: hook added");
    if settings_path.with_extension("json.bak").exists() {
        println!(
            "  Backup: {}",
            settings_path.with_extension("json.bak").display()
        );
    }
    if include_opencode {
        println!("  Restart Claude Code and OpenCode. Test with: git status");
    } else {
        println!("  Restart Claude Code. Test with: git status");
    }

    Ok(PatchResult::Patched)
}

/// Claude treats simple matchers as exact names/lists, otherwise as regexes.
fn claude_group_covers_bash(group: &serde_json::Value) -> bool {
    if let Some(pattern) = group.get("matcher").and_then(serde_json::Value::as_str)
        && !pattern.is_empty()
        && pattern
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_- ,|".contains(c))
    {
        return pattern.split(['|', ',']).any(|name| name.trim() == "Bash");
    }
    group_covers_tool(group, "Bash")
}

/// Check if RTK hook is already present in settings.json
/// Matches on legacy rtk-rewrite.sh path OR new `rtk hook claude` command
pub(super) fn hook_already_present(root: &serde_json::Value, hook_command: &str) -> bool {
    hook_present(
        root,
        PRE_TOOL_USE_KEY,
        HookEntries::Grouped,
        claude_group_covers_bash,
        |hook| {
            is_command_hook(hook, |cmd| {
                cmd == hook_command
                    || is_claude_hook_command(cmd)
                    || cmd.contains(REWRITE_HOOK_FILE)
            })
        },
    )
}

/// Default mode: hook + slim RTK.md + @RTK.md reference
pub(super) fn run_default_mode(
    global: bool,
    patch_mode: PatchMode,
    install_opencode: bool,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !global {
        // Local init: inject the awareness block into CLAUDE.md + generate
        // project-local filters template. The legacy full-instruction block
        // stays behind the explicit --claude-md opt-in.
        run_claude_md_mode_with(
            false,
            install_opencode,
            &rtk_block(awareness_content(ctx.awareness)),
            ctx,
        )?;
        generate_project_filters_template(ctx)?;
        return Ok(());
    }

    let claude_dir = resolve_claude_dir()?;
    let rtk_md_path = claude_dir.join(RTK_MD);
    let claude_md_path = claude_dir.join(CLAUDE_MD);

    // 1. Migrate old hook script if present
    migrate_old_hook_script(ctx);

    // 2. Write RTK.md
    write_if_changed(&rtk_md_path, awareness_content(ctx.awareness), RTK_MD, ctx)?;
    if dry_run {
        println!("[dry-run] awareness level: {}", ctx.awareness);
    }

    let opencode_plugin_path = if install_opencode {
        let path = prepare_opencode_plugin_path()?;
        ensure_opencode_plugin_installed(&path, ctx)?;
        Some(path)
    } else {
        None
    };

    // 3. Patch CLAUDE.md (add @RTK.md, migrate if needed)
    let migrated = patch_claude_md(&claude_md_path, ctx)?;

    // 4. Print success message (skip in dry-run)
    if !dry_run {
        println!("\nRTK hook registered (global).\n");
        println!("  Command:   {}", CLAUDE_HOOK_COMMAND);
        println!(
            "  RTK.md:    {} (awareness: {})",
            rtk_md_path.display(),
            ctx.awareness
        );
        if let Some(path) = &opencode_plugin_path {
            println!("  OpenCode:  {}", path.display());
        }
        println!("  CLAUDE.md: @RTK.md reference added");

        if migrated {
            println!("\n  [ok] Migrated: removed 137-line RTK block from CLAUDE.md");
            println!(
                "              replaced with @RTK.md (awareness: {})",
                ctx.awareness
            );
        }
    }

    // 5. Patch settings.json with binary command
    let patch_result =
        patch_settings_json_command(CLAUDE_HOOK_COMMAND, patch_mode, install_opencode, ctx)?;

    // Report result
    if !dry_run {
        match patch_result {
            PatchResult::Patched => {
                // Already printed by patch_settings_json_command
            }
            PatchResult::AlreadyPresent => {
                println!("\n  settings.json: hook already present");
                if install_opencode {
                    println!("  Restart Claude Code and OpenCode. Test with: git status");
                } else {
                    println!("  Restart Claude Code. Test with: git status");
                }
            }
            PatchResult::Declined | PatchResult::Skipped => {
                // Manual instructions already printed
            }
            PatchResult::WouldPatch => {
                // Cannot happen outside dry_run
            }
        }
    }

    // 6. Generate user-global filters template (~/.config/rtk/filters.toml)
    generate_global_filters_template(ctx)?;

    if !dry_run {
        println!(); // Final newline
    }

    Ok(())
}

/// Migrate old hook script to new binary command.
/// Deletes `~/.claude/hooks/rtk-rewrite.sh` and `.rtk-hook.sha256` if present,
/// and removes the stale settings.json entry so the new `rtk hook claude` entry
/// can be registered.
fn migrate_old_hook_script(ctx: InitContext) {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    if let Some(home) = dirs::home_dir() {
        let old_hook = home
            .join(CLAUDE_DIR)
            .join(HOOKS_SUBDIR)
            .join(REWRITE_HOOK_FILE);
        if old_hook.exists() {
            if dry_run {
                println!(
                    "[dry-run] would migrate legacy hook script: {}",
                    old_hook.display()
                );
            // nosemgrep: filesystem-deletion
            } else if let Err(e) = std::fs::remove_file(&old_hook) {
                if verbose > 0 {
                    eprintln!("  [warn] Failed to remove old hook script: {e}");
                }
            } else {
                if verbose > 0 {
                    eprintln!("  [ok] Removed old hook script: {}", old_hook.display());
                }
                // Clean up the stale settings.json entry that pointed to the deleted script
                if let Err(e) = remove_legacy_settings_entries(ctx)
                    && verbose > 0
                {
                    eprintln!("  [warn] Failed to clean legacy settings.json entry: {e}");
                }
            }
        }
        // Remove legacy hash file
        let hash_file = home
            .join(CLAUDE_DIR)
            .join(HOOKS_SUBDIR)
            .join(".rtk-hook.sha256");
        if hash_file.exists() {
            if dry_run {
                println!(
                    "[dry-run] would remove legacy hash file: {}",
                    hash_file.display()
                );
            } else {
                // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
                let _ = std::fs::remove_file(&hash_file);
            }
        }
        // Remove Cursor legacy hook
        let cursor_hook = home.join(CURSOR_DIR).join("hooks").join(REWRITE_HOOK_FILE);
        if cursor_hook.exists() {
            if dry_run {
                println!(
                    "[dry-run] would remove legacy Cursor hook: {}",
                    cursor_hook.display()
                );
            } else {
                // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
                let _ = std::fs::remove_file(&cursor_hook);
            }
        }
    }
}

/// Remove only legacy `rtk-rewrite.sh` entries from settings.json.
/// Preserves any existing `rtk hook claude` entries (new format).
fn remove_legacy_settings_entries(ctx: InitContext) -> Result<()> {
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join(SETTINGS_JSON);

    if !settings_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&settings_path)
        .with_context(|| format!("Failed to read {}", settings_path.display()))?;
    let content = strip_leading_bom(&content);
    if content.trim().is_empty() {
        return Ok(());
    }

    let mut root: serde_json::Value = from_json_str(content)
        .with_context(|| format!("Failed to parse {}", settings_path.display()))?;

    if !remove_legacy_hook_entries_from_json(&mut root) {
        return Ok(());
    }

    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;
    let report = Report::new(format!(
        "[dry-run] would remove legacy rtk-rewrite.sh entry from {}",
        settings_path.display()
    ))
    .done_verbose("  [ok] Removed legacy rtk-rewrite.sh entry from settings.json");
    write_reported(&settings_path, WriteKind::Config, &serialized, ctx, report)?;
    Ok(())
}

/// Remove only legacy `rtk-rewrite.sh` hook entries from a parsed settings.json.
/// Returns true if any entries were removed.
/// Does NOT remove `rtk hook claude` entries — those are the new format.
pub(super) fn remove_legacy_hook_entries_from_json(root: &mut serde_json::Value) -> bool {
    remove_hook_entries(root, PRE_TOOL_USE_KEY, HookEntries::Grouped, |hook| {
        is_command_hook(hook, |cmd| cmd.contains(REWRITE_HOOK_FILE))
    })
}

/// Hook-only mode: just the hook, no RTK.md
pub(super) fn run_hook_only_mode(
    global: bool,
    patch_mode: PatchMode,
    install_opencode: bool,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !global {
        eprintln!("[warn] Warning: --hook-only only makes sense with --global");
        eprintln!("    For local projects, use default mode or --claude-md");
        return Ok(());
    }

    // Migrate old hook script if present
    migrate_old_hook_script(ctx);

    let opencode_plugin_path = if install_opencode {
        let path = prepare_opencode_plugin_path()?;
        ensure_opencode_plugin_installed(&path, ctx)?;
        Some(path)
    } else {
        None
    };

    if !dry_run {
        println!("\nRTK hook registered (hook-only mode).\n");
        println!("  Command: {}", CLAUDE_HOOK_COMMAND);
        if let Some(path) = &opencode_plugin_path {
            println!("  OpenCode: {}", path.display());
        }
        println!(
            "  Note: No RTK.md created. Claude won't know about meta commands (gain, discover, proxy)."
        );
    }

    // Patch settings.json with binary command
    let patch_result =
        patch_settings_json_command(CLAUDE_HOOK_COMMAND, patch_mode, install_opencode, ctx)?;

    // Report result
    if !dry_run {
        match patch_result {
            PatchResult::Patched => {
                // Already printed by patch_settings_json_command
            }
            PatchResult::AlreadyPresent => {
                println!("\n  settings.json: hook already present");
                if install_opencode {
                    println!("  Restart Claude Code and OpenCode. Test with: git status");
                } else {
                    println!("  Restart Claude Code. Test with: git status");
                }
            }
            PatchResult::Declined | PatchResult::Skipped => {
                // Manual instructions already printed
            }
            PatchResult::WouldPatch => {
                // Cannot happen outside dry_run
            }
        }
    }

    if !dry_run {
        println!(); // Final newline
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_hook_already_present_exact_match() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/Users/test/.claude/hooks/rtk-rewrite.sh"
                    }]
                }]
            }
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(hook_already_present(&json_content, hook_command));
    }

    #[test]
    fn test_hook_already_present_different_path() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                    }]
                }]
            }
        });

        let hook_command = "~/.claude/hooks/rtk-rewrite.sh";
        // Should match on rtk-rewrite.sh substring
        assert!(hook_already_present(&json_content, hook_command));
    }

    #[test]
    fn test_hook_not_present_empty() {
        let json_content = serde_json::json!({});
        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(!hook_already_present(&json_content, hook_command));
    }

    #[test]
    fn test_hook_already_present_new_command() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": CLAUDE_HOOK_COMMAND
                    }]
                }]
            }
        });

        assert!(hook_already_present(&json_content, CLAUDE_HOOK_COMMAND));
    }

    #[test]
    fn test_hook_already_present_absolute_new_command() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/opt/homebrew/bin/rtk hook claude",
                        "timeout": 5
                    }]
                }]
            }
        });

        assert!(hook_already_present(&json_content, CLAUDE_HOOK_COMMAND));
    }

    #[test]
    fn test_hook_not_present_other_hooks() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/some/other/hook.sh"
                    }]
                }]
            }
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(!hook_already_present(&json_content, hook_command));
    }

    #[test]
    fn test_remove_hook_from_json() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/some/other/hook.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/Users/test/.claude/hooks/rtk-rewrite.sh"
                        }]
                    }
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        // Should have only one hook left
        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);

        // Check it's the other hook
        let command = pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(command, "/some/other/hook.sh");
    }

    #[test]
    fn test_remove_hook_from_json_new_command() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/some/other/hook.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": CLAUDE_HOOK_COMMAND
                        }]
                    }
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);
        assert_eq!(
            pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap(),
            "/some/other/hook.sh"
        );
    }

    #[test]
    fn test_remove_hook_from_json_absolute_new_command() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/some/other/hook.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/opt/homebrew/bin/rtk hook claude"
                        }]
                    }
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);
        assert_eq!(
            pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap(),
            "/some/other/hook.sh"
        );
    }

    #[test]
    fn test_remove_hook_when_not_present() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/some/other/hook.sh"
                    }]
                }]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(!removed);
    }

    #[test]
    fn test_remove_legacy_hook_entries_strips_old_script() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                    }]
                }]
            }
        });

        assert!(remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert!(arr.is_empty());
    }

    #[test]
    fn test_remove_legacy_hook_entries_preserves_new_command() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": CLAUDE_HOOK_COMMAND
                        }]
                    }
                ]
            }
        });

        assert!(remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let cmd = arr[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, CLAUDE_HOOK_COMMAND);
    }

    #[test]
    fn test_remove_legacy_hook_entries_noop_when_no_legacy() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": CLAUDE_HOOK_COMMAND
                    }]
                }]
            }
        });

        assert!(!remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn test_remove_legacy_hook_entries_preserves_third_party_hooks() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "some-other-tool --hook"
                        }]
                    }
                ]
            }
        });

        assert!(remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let cmd = arr[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, "some-other-tool --hook");
    }

    #[test]
    fn test_global_default_mode_creates_artifacts() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(claude_dir.join(RTK_MD).exists(), "RTK.md must be created");
            assert!(
                claude_dir.join(CLAUDE_MD).exists(),
                "CLAUDE.md must be created"
            );

            let settings = claude_dir.join(SETTINGS_JSON);
            assert!(settings.exists(), "settings.json must be created");
            let content = fs::read_to_string(&settings).unwrap();
            assert!(
                content.contains(CLAUDE_HOOK_COMMAND),
                "settings.json must contain hook command"
            );
        });
    }

    #[test]
    fn test_patch_settings_json_dry_run_idempotency_and_backup() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |dir| {
            let path = dir.join(SETTINGS_JSON);
            let original = "\u{feff}{\"permissions\":{\"allow\":[\"Bash(ls)\"]},\"hooks\":{\"PreToolUse\":[{\"matcher\":\"Bash\",\"hooks\":[{\"type\":\"command\",\"command\":\"echo user\"}]}]}}";
            fs::write(&path, original).unwrap();
            let dry = InitContext {
                dry_run: true,
                ..Default::default()
            };
            assert!(matches!(
                patch_settings_json_command(CLAUDE_HOOK_COMMAND, PatchMode::Auto, false, dry)
                    .unwrap(),
                PatchResult::WouldPatch
            ));
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
            assert!(!path.with_extension("json.bak").exists());
            assert!(matches!(
                patch_settings_json_command(
                    CLAUDE_HOOK_COMMAND,
                    PatchMode::Auto,
                    false,
                    InitContext::default()
                )
                .unwrap(),
                PatchResult::Patched
            ));
            let installed = fs::read_to_string(&path).unwrap();
            assert!(matches!(
                patch_settings_json_command(
                    CLAUDE_HOOK_COMMAND,
                    PatchMode::Auto,
                    false,
                    InitContext::default()
                )
                .unwrap(),
                PatchResult::AlreadyPresent
            ));
            assert_eq!(fs::read_to_string(&path).unwrap(), installed);
            assert_eq!(
                fs::read_to_string(path.with_extension("json.bak")).unwrap(),
                original
            );
            let root = read_json_file(&path).unwrap().unwrap();
            assert_eq!(root["permissions"]["allow"][0], "Bash(ls)");
            assert_eq!(
                root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
                "echo user"
            );
        });
    }

    #[test]
    fn test_patch_settings_json_backup_failure_preserves_original() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |dir| {
            let path = dir.join(SETTINGS_JSON);
            fs::write(&path, "{}").unwrap();
            fs::create_dir(path.with_extension("json.bak")).unwrap();
            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
            );
            // Left to the manual instructions, and untouched.
            assert!(matches!(result, Ok(PatchResult::Skipped)), "{result:?}");
            assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
        });
    }

    #[test]
    fn test_global_default_mode_creates_missing_claude_dir() {
        let tmp = TempDir::new().unwrap();
        with_missing_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(
                claude_dir.exists(),
                "missing Claude config dir must be created"
            );
            assert!(claude_dir.join(RTK_MD).exists(), "RTK.md must be created");
            assert!(
                claude_dir.join(CLAUDE_MD).exists(),
                "CLAUDE.md must be created"
            );
            assert!(
                claude_dir.join(SETTINGS_JSON).exists(),
                "settings.json must be created"
            );
        });
    }

    /// `--opencode` is installed after RTK.md. A missing Claude dir must not
    /// abort before that install. Unix only: OpenCode resolves through
    /// `dirs::home_dir()`, which follows `$HOME` on Unix and ignores it on Windows.
    #[cfg(unix)]
    #[test]
    fn test_global_opencode_installs_when_claude_dir_missing() {
        use crate::hooks::constants::{
            CONFIG_DIR, OPENCODE_PLUGIN_FILE, OPENCODE_SUBDIR, PLUGIN_SUBDIR,
        };

        let tmp = TempDir::new().unwrap();
        with_missing_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, true, InitContext::default()).unwrap();

            assert!(claude_dir.join(RTK_MD).exists(), "RTK.md must be created");
            let plugin = tmp
                .path()
                .join("home")
                .join(CONFIG_DIR)
                .join(OPENCODE_SUBDIR)
                .join(PLUGIN_SUBDIR)
                .join(OPENCODE_PLUGIN_FILE);
            assert!(
                plugin.exists(),
                "OpenCode plugin must be installed when ~/.claude was missing"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn test_patch_settings_json_skips_a_read_only_file_without_asking() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            let settings = claude_dir.join(SETTINGS_JSON);
            fs::write(&settings, "{}").unwrap();
            fs::set_permissions(&settings, fs::Permissions::from_mode(0o444)).unwrap();
            if !read_only_is_enforced(&settings) {
                return;
            }

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Ask,
                false,
                InitContext::default(),
            );
            fs::set_permissions(&settings, fs::Permissions::from_mode(0o644)).unwrap();

            // Not asked about: skipped with the manual instructions, as a declined prompt is.
            assert!(matches!(result, Ok(PatchResult::Skipped)), "{result:?}");
            assert_eq!(fs::read_to_string(&settings).unwrap(), "{}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn test_patch_settings_json_auto_patch_skips_a_read_only_file_instead_of_failing() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            let settings = claude_dir.join(SETTINGS_JSON);
            fs::write(&settings, "{}").unwrap();
            fs::set_permissions(&settings, fs::Permissions::from_mode(0o444)).unwrap();
            if !read_only_is_enforced(&settings) {
                return;
            }

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
            );
            fs::set_permissions(&settings, fs::Permissions::from_mode(0o644)).unwrap();

            assert!(matches!(result, Ok(PatchResult::Skipped)), "{result:?}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn test_patch_settings_json_skips_a_link_into_a_missing_directory_without_asking() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            let settings = claude_dir.join(SETTINGS_JSON);
            std::os::unix::fs::symlink("../unmounted/settings.json", &settings).unwrap();

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Ask,
                false,
                InitContext::default(),
            );

            assert!(matches!(result, Ok(PatchResult::Skipped)), "{result:?}");
            assert!(fs::symlink_metadata(&settings).unwrap().is_symlink());
        });
    }

    #[test]
    fn test_patch_settings_json_tolerates_utf8_bom() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            // Notepad and PowerShell 5.1 `Out-File -Encoding utf8` prepend a BOM.
            let settings = claude_dir.join(SETTINGS_JSON);
            fs::write(&settings, "\u{feff}{\"foo\": 1}").unwrap();

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
            );
            assert!(
                result.is_ok(),
                "BOM-prefixed settings.json must not abort init: {:?}",
                result.err()
            );

            let content = fs::read_to_string(&settings).unwrap();
            let v: serde_json::Value = serde_json::from_str(&content).unwrap();
            assert_eq!(v["foo"], 1, "existing keys must survive the patch");
            assert!(
                content.contains(CLAUDE_HOOK_COMMAND),
                "hook must be installed"
            );
        });
    }

    #[test]
    fn test_patch_settings_json_bom_plus_invalid_json_still_errors() {
        // Stripping the BOM must not mask genuinely broken JSON: the
        // parse-error context has to survive so the user gets blamed for
        // the right thing.
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            let settings = claude_dir.join(SETTINGS_JSON);
            fs::write(&settings, "\u{feff}{not valid json").unwrap();

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
            );
            let err = result.expect_err("invalid JSON must still fail");
            assert!(
                err.to_string().contains("Failed to parse"),
                "error must carry the parse context, got: {err:#}"
            );
        });
    }

    #[test]
    fn test_patch_settings_json_bom_only_file() {
        // U+FEFF is not whitespace, so the `content.trim().is_empty()`
        // empty-file guard does not catch a BOM-only file.
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            let settings = claude_dir.join(SETTINGS_JSON);
            fs::write(&settings, "\u{feff}").unwrap();

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
            );
            assert!(
                result.is_ok(),
                "BOM-only settings.json must be treated as empty: {:?}",
                result.err()
            );
        });
    }

    #[test]
    fn test_global_uninstall_removes_artifacts() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();
            uninstall(
                true,
                false,
                false,
                false,
                false,
                false,
                InitContext::default(),
            )
            .unwrap();

            assert!(!claude_dir.join(RTK_MD).exists(), "RTK.md must be removed");
            let settings_content =
                fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap_or_default();
            assert!(
                !settings_content.contains(CLAUDE_HOOK_COMMAND),
                "hook entry must be removed from settings.json"
            );
        });
    }

    #[test]
    fn test_global_default_mode_idempotent() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            let settings = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            let count = settings.matches(CLAUDE_HOOK_COMMAND).count();
            assert_eq!(count, 1, "hook command must appear exactly once");
        });
    }

    #[test]
    fn test_local_init_no_hook() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let result = run_default_mode(false, PatchMode::Auto, false, InitContext::default());
        std::env::set_current_dir(&cwd).unwrap();

        result.unwrap();
        assert!(
            tmp.path().join(CLAUDE_MD).exists(),
            "local CLAUDE.md must be created"
        );
        assert!(
            !tmp.path().join(SETTINGS_JSON).exists(),
            "settings.json must not be created for local init"
        );
    }

    #[test]
    fn test_global_hook_only_mode_creates_settings() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_hook_only_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(
                !claude_dir.join(RTK_MD).exists(),
                "RTK.md must NOT be created in hook-only mode"
            );
            let settings = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            assert!(
                settings.contains(CLAUDE_HOOK_COMMAND),
                "settings.json must contain hook command"
            );
        });
    }

    #[test]
    fn test_global_hook_only_mode_creates_missing_claude_dir() {
        let tmp = TempDir::new().unwrap();
        with_missing_claude_dir_override(&tmp, |claude_dir| {
            run_hook_only_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(
                claude_dir.exists(),
                "missing Claude config dir must be created"
            );
            assert!(
                !claude_dir.join(RTK_MD).exists(),
                "RTK.md must NOT be created in hook-only mode"
            );
            let settings = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            assert!(
                settings.contains(CLAUDE_HOOK_COMMAND),
                "settings.json must contain hook command"
            );
        });
    }

    #[test]
    fn test_run_default_mode_dry_run_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            let dry = InitContext {
                dry_run: true,
                ..Default::default()
            };
            run_default_mode(true, PatchMode::Auto, false, dry).unwrap();

            assert!(
                !claude_dir.join(RTK_MD).exists(),
                "dry-run must not create RTK.md"
            );
            assert!(
                !claude_dir.join(CLAUDE_MD).exists(),
                "dry-run must not create CLAUDE.md"
            );
            assert!(
                !claude_dir.join(SETTINGS_JSON).exists(),
                "dry-run must not create settings.json"
            );
        });
    }

    #[test]
    fn test_run_default_mode_dry_run_does_not_create_missing_claude_dir() {
        let tmp = TempDir::new().unwrap();
        with_missing_claude_dir_override(&tmp, |claude_dir| {
            let dry = InitContext {
                dry_run: true,
                ..Default::default()
            };
            run_default_mode(true, PatchMode::Auto, false, dry).unwrap();

            assert!(
                !claude_dir.exists(),
                "dry-run must not create missing Claude config dir"
            );
        });
    }

    #[test]
    fn test_uninstall_dry_run_preserves_artifacts() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            // Stage a real install first
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();
            assert!(claude_dir.join(RTK_MD).exists());
            assert!(claude_dir.join(SETTINGS_JSON).exists());

            let settings_before = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            let rtk_md_before = fs::read_to_string(claude_dir.join(RTK_MD)).unwrap();

            // Dry-run uninstall
            let dry = InitContext {
                dry_run: true,
                ..Default::default()
            };
            uninstall(true, false, false, false, false, false, dry).unwrap();

            // Files must still exist with identical content
            assert!(
                claude_dir.join(RTK_MD).exists(),
                "dry-run uninstall must not remove RTK.md"
            );
            assert!(
                claude_dir.join(SETTINGS_JSON).exists(),
                "dry-run uninstall must not remove settings.json"
            );
            assert_eq!(
                fs::read_to_string(claude_dir.join(RTK_MD)).unwrap(),
                rtk_md_before,
                "dry-run uninstall must not modify RTK.md"
            );
            assert_eq!(
                fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap(),
                settings_before,
                "dry-run uninstall must not modify settings.json"
            );
        });
    }

    #[test]
    fn test_write_if_changed_switches_awareness_level() {
        let temp = TempDir::new().unwrap();
        let rtk_md_path = temp.path().join("RTK.md");

        let default_ctx = InitContext::default();
        assert!(
            write_if_changed(
                &rtk_md_path,
                awareness_content(default_ctx.awareness),
                RTK_MD,
                default_ctx
            )
            .unwrap()
        );
        assert_eq!(
            fs::read_to_string(&rtk_md_path).unwrap(),
            RTK_AWARENESS_DEFAULT
        );

        let high_ctx = InitContext {
            awareness: AwarenessLevel::High,
            ..Default::default()
        };
        assert!(
            write_if_changed(
                &rtk_md_path,
                awareness_content(high_ctx.awareness),
                RTK_MD,
                high_ctx
            )
            .unwrap()
        );
        assert_eq!(
            fs::read_to_string(&rtk_md_path).unwrap(),
            RTK_AWARENESS_HIGH
        );

        assert!(
            !write_if_changed(
                &rtk_md_path,
                awareness_content(high_ctx.awareness),
                RTK_MD,
                high_ctx
            )
            .unwrap()
        );
    }

    #[test]
    fn test_claude_md_mode_creates_full_injection() {
        // Just verify RTK_INSTRUCTIONS constant has the right content
        assert!(RTK_INSTRUCTIONS.contains(RTK_BLOCK_START));
        assert!(RTK_INSTRUCTIONS.contains("rtk cargo test"));
        assert!(RTK_INSTRUCTIONS.contains(RTK_BLOCK_END));
        assert!(RTK_INSTRUCTIONS.len() > 4000);
    }
    #[test]
    fn test_resolve_claude_dir_prefers_rtk_override() {
        let result = resolve_claude_dir_from(
            Some(PathBuf::from("/custom/rtk-claude")),
            Some(PathBuf::from("/home/user")),
        )
        .unwrap();
        assert_eq!(result, PathBuf::from("/custom/rtk-claude"));
    }

    #[test]
    fn test_resolve_claude_dir_uses_claude_config_dir() {
        let result = resolve_claude_dir_from(
            Some(PathBuf::from("/custom/claude-config")),
            Some(PathBuf::from("/home/user")),
        )
        .unwrap();
        assert_eq!(result, PathBuf::from("/custom/claude-config"));
    }

    #[test]
    fn test_resolve_claude_dir_falls_back_to_home() {
        let result = resolve_claude_dir_from(None, Some(PathBuf::from("/home/user"))).unwrap();
        assert_eq!(result, PathBuf::from("/home/user/.claude"));
    }

    #[test]
    fn test_resolve_claude_dir_ignores_empty_overrides() {
        let empty =
            resolve_claude_dir_from(Some(PathBuf::new()), Some(PathBuf::from("/home/user")))
                .unwrap();
        assert_eq!(empty, PathBuf::from("/home/user/.claude"));
    }

    #[test]
    fn test_resolve_claude_dir_errors_without_home() {
        let err = resolve_claude_dir_from(None, None).unwrap_err();
        assert!(err.to_string().contains("Cannot determine Claude config"));
    }

    #[test]
    fn test_upgrade_from_claude_md_to_hook_mode() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_claude_md_mode(true, false, InitContext::default()).unwrap();
            let claude_md_content = fs::read_to_string(claude_dir.join(CLAUDE_MD)).unwrap();
            assert!(
                claude_md_content.contains(RTK_BLOCK_START),
                "pre-condition: old block must exist"
            );

            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(claude_dir.join(RTK_MD).exists(), "RTK.md must be created");
            let settings = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            assert!(
                settings.contains(CLAUDE_HOOK_COMMAND),
                "hook must be in settings.json after upgrade"
            );
        });
    }

    #[test]
    fn test_uninstall_integration_claude_md_only() {
        let (cleaned, did_remove) = remove_rtk_block(RTK_INSTRUCTIONS);
        assert!(did_remove, "remove_rtk_block must succeed for valid block");
        assert!(
            cleaned.trim().is_empty(),
            "CLAUDE.md with only RTK content should be empty after removal"
        );
    }

    #[test]
    fn test_claude_md_mode_refuses_malformed_block() {
        // Mirrors `copilot::tests::test_copilot_init_refuses_malformed_block`: a malformed
        // CLAUDE.md previously emitted a warning and exited 0, silently
        // skipping the OpenCode plugin step. The shared `write_rtk_block`
        // dispatcher now bails for both paths.
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            let claude_md = claude_dir.join(CLAUDE_MD);
            let malformed = format!(
                "# Existing notes\n\n{}\nincomplete RTK block\n",
                RTK_BLOCK_START
            );
            fs::write(&claude_md, &malformed).unwrap();

            let result = run_claude_md_mode(true, false, InitContext::default());

            assert!(
                result.is_err(),
                "Malformed CLAUDE.md must cause a hard error, not silent skip"
            );

            let after = fs::read_to_string(&claude_md).unwrap();
            assert_eq!(after, malformed, "File must not be modified when malformed");
        });
    }
}
