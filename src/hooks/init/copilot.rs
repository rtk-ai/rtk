//! Copilot agent: hook install/uninstall helpers.

use super::*;

// Copilot integration

// Single PascalCase `PreToolUse` entry, shared by VS Code Copilot Chat and
// Copilot CLI. Previously this file also declared a camelCase `preToolUse`
// entry for Copilot CLI's native schema, but Copilot CLI registers BOTH keys
// as independent hooks and runs them sequentially, chaining the camelCase
// hook's rewrite into the PascalCase hook's input — a redundant second
// process spawn per tool call for no behavioral benefit (confirmed live:
// Copilot CLI honors the PascalCase-only schema on its own, receiving the
// same `tool_name`/`tool_input.command` shape either way).
pub(crate) const COPILOT_HOOK_JSON: &str = r#"{
  "version": 1,
  "hooks": {
    "PreToolUse": [
      {
        "type": "command",
        "command": "rtk hook copilot",
        "cwd": ".",
        "timeout": 5
      }
    ]
  }
}
"#;

pub(crate) const COPILOT_INSTRUCTIONS: &str = r#"<!-- rtk-instructions v2 -->
# RTK — Token-Optimized CLI

**rtk** is a CLI proxy that filters and compresses command outputs, saving 60-90% tokens.

## Rule

Always prefix shell commands with `rtk`:

```bash
# Instead of:              Use:
git status                 rtk git status
git log -10                rtk git log -10
cargo test                 rtk cargo test
docker ps                  rtk docker ps
kubectl get pods           rtk kubectl get pods
```

## Meta commands (use directly)

```bash
rtk gain              # Token savings dashboard
rtk gain --history    # Per-command savings history
rtk discover          # Find missed rtk opportunities
rtk proxy <cmd>       # Run raw (no filtering) but track usage
```
<!-- /rtk-instructions -->
"#;

/// Entry point for `rtk init --copilot`.
///
/// Installs in the current working directory's `.github/` subdirectory.
pub fn run_copilot(ctx: InitContext) -> Result<()> {
    run_copilot_at(Path::new("."), ctx)
}

/// Same as [`run_copilot`] but operates relative to an explicit base path.
///
/// Used by tests to avoid mutating process-global `cwd` (which is racy under
/// `cargo test`'s default parallel execution).
pub(crate) fn run_copilot_at(base: &Path, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let github_dir = base.join(GITHUB_DIR);
    let hooks_dir = github_dir.join(HOOKS_SUBDIR);

    if !dry_run {
        fs::create_dir_all(&hooks_dir)
            .with_context(|| format!("Failed to create {} directory", hooks_dir.display()))?;
    }

    // 1. Upsert RTK marker block in copilot-instructions.md (preserves user content).
    //    Done BEFORE writing the hook config so a malformed file aborts the install
    //    without leaving a stale hook on disk.
    let instructions_path = github_dir.join(COPILOT_INSTRUCTIONS_FILE);
    write_rtk_block(
        &instructions_path,
        COPILOT_INSTRUCTIONS,
        "Copilot instructions",
        "rtk init --copilot",
        ctx,
    )?;

    // 2. Write hook config (only reached if the upsert above succeeded).
    let hook_path = hooks_dir.join(COPILOT_HOOK_FILE);
    write_if_changed(&hook_path, COPILOT_HOOK_JSON, "Copilot hook config", ctx)?;

    if dry_run {
        print_dry_run_footer();
    } else {
        println!("\nGitHub Copilot integration installed (project-scoped).\n");
        println!("  Hook config:    {}", hook_path.display());
        println!("  Instructions:   {}", instructions_path.display());
        println!("\n  Works with VS Code Copilot Chat (transparent rewrite)");
        println!("  and Copilot CLI (deny-with-suggestion).");
        println!("\n  Restart your IDE or Copilot CLI session to activate.\n");
    }

    Ok(())
}

/// Entry point for `rtk init --uninstall --copilot` (project-scoped, like install).
pub fn uninstall_copilot(ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let removed = uninstall_copilot_at(Path::new("."), ctx)?;

    if removed.is_empty() {
        println!("RTK Copilot support was not installed (nothing to remove)");
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK (GitHub Copilot):"
        } else {
            "RTK uninstalled (GitHub Copilot):"
        };
        println!("{}", header);
        for item in &removed {
            println!("  - {}", item);
        }
        if !dry_run {
            println!("\nRestart your IDE or Copilot CLI session to apply changes.");
        }
    }

    if dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

/// Same as [`uninstall_copilot`] but operates relative to an explicit base path.
pub(crate) fn uninstall_copilot_at(base: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext { dry_run, .. } = ctx;
    let github_dir = base.join(GITHUB_DIR);
    let mut removed = Vec::new();

    let hook_path = github_dir.join(HOOKS_SUBDIR).join(COPILOT_HOOK_FILE);
    if hook_path.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove hook config: {}",
                hook_path.display()
            );
        } else {
            // nosemgrep: filesystem-deletion -- Copilot uninstall removes only the RTK-managed hook config.
            fs::remove_file(&hook_path)
                .with_context(|| format!("Failed to remove hook: {}", hook_path.display()))?;
        }
        removed.push(format!("Hook config: {}", hook_path.display()));
    }

    let instructions_path = github_dir.join(COPILOT_INSTRUCTIONS_FILE);
    if instructions_path.exists() {
        let content = fs::read_to_string(&instructions_path)
            .with_context(|| format!("Failed to read {}", instructions_path.display()))?;
        if content.contains(RTK_BLOCK_START) {
            let (cleaned, did_remove) = remove_rtk_block(&content);
            if did_remove {
                if dry_run {
                    println!(
                        "[dry-run] would remove rtk-instructions block from {}",
                        instructions_path.display()
                    );
                } else {
                    atomic_write(&instructions_path, &cleaned).with_context(|| {
                        format!("Failed to write {}", instructions_path.display())
                    })?;
                }
                removed.push(format!(
                    "{}: removed rtk-instructions block",
                    COPILOT_INSTRUCTIONS_FILE
                ));
            }
        }
    }

    Ok(removed)
}

pub(crate) fn copilot_user_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var(COPILOT_HOME_ENV) {
        return Ok(PathBuf::from(custom));
    }
    let home = dirs::home_dir().context("could not determine home directory")?;
    Ok(home.join(COPILOT_USER_DIR))
}

pub fn run_copilot_global(ctx: InitContext) -> Result<()> {
    let copilot_dir = copilot_user_dir()?;
    run_copilot_global_at(&copilot_dir, ctx)
}

pub(crate) fn run_copilot_global_at(copilot_dir: &Path, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let hooks_dir = copilot_dir.join(HOOKS_SUBDIR);

    if !dry_run {
        fs::create_dir_all(&hooks_dir)
            .with_context(|| format!("Failed to create {} directory", hooks_dir.display()))?;
    }

    let instructions_path = copilot_dir.join(COPILOT_INSTRUCTIONS_FILE);
    write_rtk_block(
        &instructions_path,
        COPILOT_INSTRUCTIONS,
        "Copilot user-level instructions",
        "rtk init --global --copilot",
        ctx,
    )?;

    let hook_path = hooks_dir.join(COPILOT_HOOK_FILE);
    write_if_changed(
        &hook_path,
        COPILOT_HOOK_JSON,
        "Copilot global hook config",
        ctx,
    )?;

    if dry_run {
        print_dry_run_footer();
    } else {
        println!("\nGitHub Copilot global integration installed (user-scoped).\n");
        println!("  Hook config:    {}", hook_path.display());
        println!("  Instructions:   {}", instructions_path.display());
        println!("\n  Applies to all Copilot CLI sessions on this machine.");
        println!("  Restart your Copilot CLI session to activate.\n");
    }

    Ok(())
}

pub fn uninstall_copilot_global(ctx: InitContext) -> Result<()> {
    let copilot_dir = copilot_user_dir()?;
    let InitContext { dry_run, .. } = ctx;
    let removed = uninstall_copilot_global_at(&copilot_dir, ctx)?;

    if removed.is_empty() {
        println!("RTK global Copilot support was not installed (nothing to remove)");
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK (global GitHub Copilot):"
        } else {
            "RTK uninstalled (global GitHub Copilot):"
        };
        println!("{}", header);
        for item in &removed {
            println!("  - {}", item);
        }
        if !dry_run {
            println!("\nRestart your Copilot CLI session to apply changes.");
        }
    }

    if dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

pub(crate) fn uninstall_copilot_global_at(
    copilot_dir: &Path,
    ctx: InitContext,
) -> Result<Vec<String>> {
    let InitContext { dry_run, .. } = ctx;
    let hook_path = copilot_dir.join(HOOKS_SUBDIR).join(COPILOT_HOOK_FILE);
    let mut removed = Vec::new();

    if hook_path.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove hook config: {}",
                hook_path.display()
            );
        } else {
            // nosemgrep: filesystem-deletion -- Copilot global uninstall removes only the RTK-managed hook config.
            fs::remove_file(&hook_path)
                .with_context(|| format!("Failed to remove hook: {}", hook_path.display()))?;
        }
        removed.push(format!("Hook config: {}", hook_path.display()));
    }

    let instructions_path = copilot_dir.join(COPILOT_INSTRUCTIONS_FILE);
    if instructions_path.exists() {
        let content = fs::read_to_string(&instructions_path)
            .with_context(|| format!("Failed to read {}", instructions_path.display()))?;
        if content.contains(RTK_BLOCK_START) {
            let (cleaned, did_remove) = remove_rtk_block(&content);
            if did_remove {
                if dry_run {
                    println!(
                        "[dry-run] would remove rtk-instructions block from {}",
                        instructions_path.display()
                    );
                } else {
                    atomic_write(&instructions_path, &cleaned).with_context(|| {
                        format!("Failed to write {}", instructions_path.display())
                    })?;
                }
                removed.push(format!(
                    "{}: removed rtk-instructions block",
                    COPILOT_INSTRUCTIONS_FILE
                ));
            }
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Copilot tests

    #[test]
    fn test_copilot_init_preserves_existing_instructions() {
        let temp = TempDir::new().unwrap();
        let github_dir = temp.path().join(".github");
        fs::create_dir_all(&github_dir).unwrap();

        let instructions_path = github_dir.join("copilot-instructions.md");
        let user_content = "# My Copilot Instructions\n\n\
            Always respond in Spanish.\n\
            Never suggest npm; prefer pnpm.\n";
        fs::write(&instructions_path, user_content).unwrap();

        run_copilot_at(temp.path(), InitContext::default()).unwrap();

        let final_content = fs::read_to_string(&instructions_path).unwrap();

        assert!(
            final_content.contains("Always respond in Spanish."),
            "User custom rule was destroyed. Got: {final_content}"
        );
        assert!(
            final_content.contains("Never suggest npm; prefer pnpm."),
            "User custom rule was destroyed. Got: {final_content}"
        );
        assert!(
            final_content.contains(RTK_BLOCK_START),
            "RTK block start marker missing"
        );
        assert!(
            final_content.contains(RTK_BLOCK_END),
            "RTK block end marker missing"
        );
    }

    #[test]
    fn test_copilot_init_idempotent_repeats() {
        let temp = TempDir::new().unwrap();
        let github_dir = temp.path().join(".github");
        fs::create_dir_all(&github_dir).unwrap();

        run_copilot_at(temp.path(), InitContext::default()).unwrap();
        let after_first = fs::read_to_string(github_dir.join("copilot-instructions.md")).unwrap();

        run_copilot_at(temp.path(), InitContext::default()).unwrap();
        let after_second = fs::read_to_string(github_dir.join("copilot-instructions.md")).unwrap();

        assert_eq!(
            after_first, after_second,
            "Second init must be a no-op (idempotent)"
        );

        let count_start = after_first.matches(RTK_BLOCK_START).count();
        let count_end = after_first.matches(RTK_BLOCK_END).count();
        assert_eq!(
            count_start, 1,
            "RTK_BLOCK_START must appear once, got {count_start}"
        );
        assert_eq!(
            count_end, 1,
            "RTK_BLOCK_END must appear once, got {count_end}"
        );
    }

    #[test]
    fn test_copilot_init_updates_stale_block() {
        let temp = TempDir::new().unwrap();
        let github_dir = temp.path().join(".github");
        fs::create_dir_all(&github_dir).unwrap();

        let instructions_path = github_dir.join("copilot-instructions.md");
        let stale = format!(
            "# Project rules\n\nUse rg.\n\n{}\n# OLD RTK CONTENT\nrtk foo\n{}\n",
            RTK_BLOCK_START, RTK_BLOCK_END
        );
        fs::write(&instructions_path, &stale).unwrap();

        run_copilot_at(temp.path(), InitContext::default()).unwrap();

        let updated = fs::read_to_string(&instructions_path).unwrap();

        assert!(
            updated.contains("Use rg."),
            "User content outside the block must be preserved"
        );
        assert!(
            !updated.contains("# OLD RTK CONTENT"),
            "Stale RTK block content must be removed"
        );
        assert!(
            updated.contains("rtk cargo test"),
            "Fresh COPILOT_INSTRUCTIONS content must be present"
        );
    }

    #[test]
    fn test_copilot_init_dry_run_no_write() {
        let temp = TempDir::new().unwrap();
        let instructions_path = temp.path().join(".github").join("copilot-instructions.md");
        assert!(!instructions_path.exists());

        let ctx = InitContext {
            dry_run: true,
            ..InitContext::default()
        };
        run_copilot_at(temp.path(), ctx).unwrap();

        assert!(
            !instructions_path.exists(),
            "Dry-run must not create copilot-instructions.md"
        );
    }

    #[test]
    fn test_copilot_init_fresh_install_creates_file() {
        let temp = TempDir::new().unwrap();
        let instructions_path = temp.path().join(".github").join("copilot-instructions.md");
        assert!(!instructions_path.exists());

        run_copilot_at(temp.path(), InitContext::default()).unwrap();

        assert!(
            instructions_path.exists(),
            "Fresh install must create copilot-instructions.md"
        );
        let content = fs::read_to_string(&instructions_path).unwrap();
        assert!(content.contains(RTK_BLOCK_START));
        assert!(content.contains(RTK_BLOCK_END));
        assert!(content.contains("rtk cargo test"));
    }

    #[test]
    fn test_copilot_hook_json_serves_single_pascalcase_schema() {
        let v: serde_json::Value = serde_json::from_str(COPILOT_HOOK_JSON).unwrap();

        let vscode = &v["hooks"]["PreToolUse"][0];
        assert_eq!(vscode["command"], "rtk hook copilot");
        assert!(vscode["timeout"].is_number(), "VS Code uses `timeout`");
        assert_eq!(v["version"], 1);

        assert!(
            v["hooks"].get("preToolUse").is_none(),
            "must not register a second, redundant camelCase hook — Copilot CLI treats \
             PreToolUse and preToolUse as independent hooks and runs both sequentially"
        );
    }

    #[test]
    fn test_copilot_init_writes_single_schema_to_disk() {
        let temp = TempDir::new().unwrap();
        run_copilot_at(temp.path(), InitContext::default()).unwrap();

        let hook_path = temp
            .path()
            .join(".github")
            .join("hooks")
            .join("rtk-rewrite.json");
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&hook_path).unwrap()).unwrap();

        assert_eq!(v["hooks"]["PreToolUse"][0]["command"], "rtk hook copilot");
        assert_eq!(v["version"], 1);
        assert!(v["hooks"].get("preToolUse").is_none());
    }

    #[test]
    fn test_copilot_init_upgrades_old_dual_schema_install() {
        // Simulates a pre-existing install from before this fix, which wrote
        // both a PascalCase PreToolUse and a camelCase preToolUse entry.
        // Re-running `rtk init --copilot` must overwrite it with the current
        // single-schema config, not leave the stale camelCase entry in place.
        let old_dual_schema_json = r#"{
  "version": 1,
  "hooks": {
    "PreToolUse": [
      { "type": "command", "command": "rtk hook copilot", "cwd": ".", "timeout": 5 }
    ],
    "preToolUse": [
      { "type": "command", "bash": "rtk hook copilot", "powershell": "rtk hook copilot", "cwd": ".", "timeoutSec": 5 }
    ]
  }
}
"#;

        let temp = TempDir::new().unwrap();
        let hooks_dir = temp.path().join(".github").join("hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join("rtk-rewrite.json");
        fs::write(&hook_path, old_dual_schema_json).unwrap();

        run_copilot_at(temp.path(), InitContext::default()).unwrap();

        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&hook_path).unwrap()).unwrap();
        assert_eq!(v["hooks"]["PreToolUse"][0]["command"], "rtk hook copilot");
        assert!(
            v["hooks"].get("preToolUse").is_none(),
            "re-running init must upgrade an old dual-schema install, dropping the \
             redundant camelCase preToolUse entry"
        );
    }

    #[test]
    fn test_copilot_uninstall_removes_hook_and_block() {
        let temp = TempDir::new().unwrap();
        run_copilot_at(temp.path(), InitContext::default()).unwrap();

        let hook_path = temp
            .path()
            .join(".github")
            .join("hooks")
            .join("rtk-rewrite.json");
        let instructions_path = temp.path().join(".github").join("copilot-instructions.md");
        assert!(hook_path.exists());

        let removed = uninstall_copilot_at(temp.path(), InitContext::default()).unwrap();

        assert!(!removed.is_empty());
        assert!(!hook_path.exists(), "hook config must be removed");
        let instructions = fs::read_to_string(&instructions_path).unwrap();
        assert!(
            !instructions.contains(RTK_BLOCK_START),
            "RTK block must be removed"
        );
    }

    #[test]
    fn test_copilot_uninstall_preserves_user_instructions() {
        let temp = TempDir::new().unwrap();
        let github_dir = temp.path().join(".github");
        fs::create_dir_all(&github_dir).unwrap();
        let instructions_path = github_dir.join("copilot-instructions.md");
        fs::write(&instructions_path, "# My rules\n\nAlways use pnpm.\n").unwrap();

        run_copilot_at(temp.path(), InitContext::default()).unwrap();
        uninstall_copilot_at(temp.path(), InitContext::default()).unwrap();

        let after = fs::read_to_string(&instructions_path).unwrap();
        assert!(after.contains("Always use pnpm."), "user content preserved");
        assert!(!after.contains(RTK_BLOCK_START), "RTK block removed");
    }

    #[test]
    fn test_copilot_uninstall_dry_run_keeps_files() {
        let temp = TempDir::new().unwrap();
        run_copilot_at(temp.path(), InitContext::default()).unwrap();
        let hook_path = temp
            .path()
            .join(".github")
            .join("hooks")
            .join("rtk-rewrite.json");

        let ctx = InitContext {
            verbose: 0,
            dry_run: true,
        };
        uninstall_copilot_at(temp.path(), ctx).unwrap();

        assert!(hook_path.exists(), "dry-run must not remove hook config");
    }

    #[test]
    fn test_copilot_uninstall_nothing_when_absent() {
        let temp = TempDir::new().unwrap();
        let removed = uninstall_copilot_at(temp.path(), InitContext::default()).unwrap();
        assert!(removed.is_empty(), "nothing to remove in a clean project");
    }

    #[test]
    fn test_copilot_install_does_not_touch_other_hooks() {
        let temp = TempDir::new().unwrap();
        let hooks_dir = temp.path().join(".github").join("hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        let other_hook = hooks_dir.join("user-policy.json");
        let other_content =
            r#"{"hooks":{"sessionStart":[{"type":"command","command":"echo hi"}]}}"#;
        fs::write(&other_hook, other_content).unwrap();

        run_copilot_at(temp.path(), InitContext::default()).unwrap();

        assert!(other_hook.exists(), "third-party hook file must remain");
        assert_eq!(
            fs::read_to_string(&other_hook).unwrap(),
            other_content,
            "third-party hook content must be unchanged by rtk install"
        );
    }

    #[test]
    fn test_copilot_uninstall_does_not_touch_other_hooks() {
        let temp = TempDir::new().unwrap();
        run_copilot_at(temp.path(), InitContext::default()).unwrap();

        let hooks_dir = temp.path().join(".github").join("hooks");
        let other_hook = hooks_dir.join("user-policy.json");
        let other_content =
            r#"{"hooks":{"sessionStart":[{"type":"command","command":"echo hi"}]}}"#;
        fs::write(&other_hook, other_content).unwrap();

        uninstall_copilot_at(temp.path(), InitContext::default()).unwrap();

        assert!(
            other_hook.exists(),
            "third-party hook file must survive rtk uninstall"
        );
        assert_eq!(
            fs::read_to_string(&other_hook).unwrap(),
            other_content,
            "third-party hook content must be unchanged by rtk uninstall"
        );
        assert!(
            !hooks_dir.join("rtk-rewrite.json").exists(),
            "rtk's own hook must still be removed"
        );
    }

    #[test]
    fn test_copilot_global_install_writes_hook() {
        let temp = TempDir::new().unwrap();
        run_copilot_global_at(temp.path(), InitContext::default()).unwrap();

        let hook_path = temp.path().join("hooks").join("rtk-rewrite.json");
        assert!(hook_path.exists());
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&hook_path).unwrap()).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["hooks"]["PreToolUse"][0]["command"], "rtk hook copilot");
        assert!(v["hooks"].get("preToolUse").is_none());
    }

    #[test]
    fn test_copilot_global_install_upgrades_old_dual_schema_install() {
        let old_dual_schema_json = r#"{
  "version": 1,
  "hooks": {
    "PreToolUse": [
      { "type": "command", "command": "rtk hook copilot", "cwd": ".", "timeout": 5 }
    ],
    "preToolUse": [
      { "type": "command", "bash": "rtk hook copilot", "powershell": "rtk hook copilot", "cwd": ".", "timeoutSec": 5 }
    ]
  }
}
"#;

        let temp = TempDir::new().unwrap();
        let hooks_dir = temp.path().join("hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join("rtk-rewrite.json");
        fs::write(&hook_path, old_dual_schema_json).unwrap();

        run_copilot_global_at(temp.path(), InitContext::default()).unwrap();

        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&hook_path).unwrap()).unwrap();
        assert_eq!(v["hooks"]["PreToolUse"][0]["command"], "rtk hook copilot");
        assert!(
            v["hooks"].get("preToolUse").is_none(),
            "re-running global init must upgrade an old dual-schema install"
        );
    }

    #[test]
    fn test_copilot_global_install_writes_instructions() {
        let temp = TempDir::new().unwrap();
        run_copilot_global_at(temp.path(), InitContext::default()).unwrap();
        let instructions = temp.path().join(COPILOT_INSTRUCTIONS_FILE);
        assert!(
            instructions.exists(),
            "user-level instructions must be written"
        );
        let content = fs::read_to_string(&instructions).unwrap();
        assert!(content.contains(RTK_BLOCK_START));
        assert!(content.contains("rtk cargo test"));
    }

    #[test]
    fn test_copilot_global_install_preserves_existing_user_instructions() {
        let temp = TempDir::new().unwrap();
        let instructions = temp.path().join(COPILOT_INSTRUCTIONS_FILE);
        fs::write(&instructions, "# My rules\n\nAlways use pnpm.\n").unwrap();

        run_copilot_global_at(temp.path(), InitContext::default()).unwrap();

        let content = fs::read_to_string(&instructions).unwrap();
        assert!(
            content.contains("Always use pnpm."),
            "user content must be preserved"
        );
        assert!(content.contains(RTK_BLOCK_START));
    }

    #[test]
    fn test_copilot_global_uninstall_preserves_user_instructions() {
        let temp = TempDir::new().unwrap();
        let instructions = temp.path().join(COPILOT_INSTRUCTIONS_FILE);
        fs::write(&instructions, "# My rules\n\nAlways use pnpm.\n").unwrap();

        run_copilot_global_at(temp.path(), InitContext::default()).unwrap();
        uninstall_copilot_global_at(temp.path(), InitContext::default()).unwrap();

        let content = fs::read_to_string(&instructions).unwrap();
        assert!(content.contains("Always use pnpm."));
        assert!(!content.contains(RTK_BLOCK_START), "RTK block removed");
    }

    #[test]
    fn test_copilot_global_uninstall_removes_hook() {
        let temp = TempDir::new().unwrap();
        run_copilot_global_at(temp.path(), InitContext::default()).unwrap();
        let hook_path = temp.path().join("hooks").join("rtk-rewrite.json");
        assert!(hook_path.exists());

        let removed = uninstall_copilot_global_at(temp.path(), InitContext::default()).unwrap();
        assert!(!removed.is_empty());
        assert!(!hook_path.exists());
    }

    #[test]
    fn test_copilot_global_uninstall_nothing_when_absent() {
        let temp = TempDir::new().unwrap();
        let removed = uninstall_copilot_global_at(temp.path(), InitContext::default()).unwrap();
        assert!(removed.is_empty());
    }

    #[test]
    fn test_copilot_global_install_does_not_touch_other_hooks() {
        let temp = TempDir::new().unwrap();
        let hooks_dir = temp.path().join("hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        let other = hooks_dir.join("notification-hooks.json");
        let payload = r#"{"version":1,"hooks":{"agentStop":[{"type":"command","bash":"true"}]}}"#;
        fs::write(&other, payload).unwrap();

        run_copilot_global_at(temp.path(), InitContext::default()).unwrap();

        assert_eq!(fs::read_to_string(&other).unwrap(), payload);
    }

    #[test]
    fn test_copilot_global_uninstall_does_not_touch_other_hooks() {
        let temp = TempDir::new().unwrap();
        run_copilot_global_at(temp.path(), InitContext::default()).unwrap();
        let hooks_dir = temp.path().join("hooks");
        let other = hooks_dir.join("notification-hooks.json");
        let payload = r#"{"version":1,"hooks":{"agentStop":[{"type":"command","bash":"true"}]}}"#;
        fs::write(&other, payload).unwrap();

        uninstall_copilot_global_at(temp.path(), InitContext::default()).unwrap();

        assert!(other.exists());
        assert_eq!(fs::read_to_string(&other).unwrap(), payload);
        assert!(!hooks_dir.join("rtk-rewrite.json").exists());
    }

    #[test]
    fn test_copilot_global_install_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        let ctx = InitContext {
            verbose: 0,
            dry_run: true,
        };
        run_copilot_global_at(temp.path(), ctx).unwrap();
        assert!(!temp.path().join("hooks").join("rtk-rewrite.json").exists());
    }

    #[test]
    fn test_copilot_init_refuses_malformed_block() {
        let temp = TempDir::new().unwrap();
        let github_dir = temp.path().join(".github");
        fs::create_dir_all(&github_dir).unwrap();

        let instructions_path = github_dir.join("copilot-instructions.md");
        let malformed = format!("# My rules\n\n{}\nincomplete RTK block\n", RTK_BLOCK_START);
        fs::write(&instructions_path, &malformed).unwrap();

        let result = run_copilot_at(temp.path(), InitContext::default());

        assert!(
            result.is_err(),
            "Malformed file must cause an error, not silent rewrite"
        );

        let after = fs::read_to_string(&instructions_path).unwrap();
        assert_eq!(after, malformed, "File must not be modified when malformed");
    }

    #[test]
    fn test_copilot_init_malformed_leaves_no_hook_on_disk() {
        // Regression: a malformed copilot-instructions.md aborted the install
        // mid-way, but the hook config had already been written. The upsert
        // now runs first, so the hook config must not appear when the upsert
        // bails.
        let temp = TempDir::new().unwrap();
        let github_dir = temp.path().join(".github");
        fs::create_dir_all(&github_dir).unwrap();

        let instructions_path = github_dir.join("copilot-instructions.md");
        let malformed = format!("# My rules\n\n{}\nincomplete RTK block\n", RTK_BLOCK_START);
        fs::write(&instructions_path, &malformed).unwrap();

        let hook_path = github_dir.join("hooks").join("rtk-rewrite.json");

        let result = run_copilot_at(temp.path(), InitContext::default());

        assert!(result.is_err(), "Malformed file must cause a hard error");
        assert!(
            !hook_path.exists(),
            "Hook config must not be written when the upsert aborts: {}",
            hook_path.display()
        );
    }
}
