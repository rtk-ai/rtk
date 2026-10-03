//! Google Antigravity support: transparent command rewriting via PreToolUse lifecycle hook plugin.

use super::*;
use crate::core::user_dirs;

pub const ANTIGRAVITY_PLUGIN_JSON: &str = r#"{
  "name": "rtk",
  "version": "1.0.0",
  "description": "Antigravity plugin for transparent command rewriting and token optimization using rtk"
}
"#;

pub const ANTIGRAVITY_HOOKS_JSON: &str = r#"{
  "rtk-rewrite": {
    "enabled": true,
    "PreToolUse": [
      {
        "matcher": "run_command",
        "hooks": [
          {
            "type": "command",
            "command": "rtk hook antigravity",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
"#;

/// The awareness rules inside the plugin bundle. The plugin docs bundled with `agy`
/// recommend `rules/AGENTS.md`: plain markdown, no frontmatter, always on while the
/// plugin is enabled.
const ANTIGRAVITY_RULES_FILE: &str = "AGENTS.md";

pub const LEGACY_RULES_REL_PATH: &str = ".agents/rules/antigravity-rtk-rules.md";

const LEGACY_ANTIGRAVITY_RULES_V1: &str = r#"# RTK - Rust Token Killer (Google Antigravity)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk` to minimize token consumption.

Examples:

```bash
rtk git status
rtk cargo test
rtk ls src/
rtk grep "pattern" src/
rtk find "*.rs" .
rtk docker ps
rtk gh pr list
```

## Meta Commands

```bash
rtk gain              # Show token savings
rtk gain --history    # Command history with savings
rtk discover          # Find missed RTK opportunities
rtk proxy <cmd>       # Run raw (no filtering, for debugging)
```

## Why

RTK filters and compresses command output before it reaches the LLM context, saving 60-90% tokens on common operations. Always use `rtk <cmd>` instead of raw commands.
"#;

#[derive(Debug, PartialEq, Eq)]
pub enum LegacyRulesAction {
    RemovedFile(PathBuf),
    StrippedSuffix(PathBuf),
}

/// Detect and migrate legacy `.agents/rules/antigravity-rtk-rules.md` files written
/// by earlier RTK releases.
///
/// - If the file content is solely an awareness text an earlier release wrote, remove it.
/// - If the file has that awareness text appended after user content following a blank line,
///   remove only the RTK suffix, keeping the user content byte-identical.
/// - Leave any other file at that path untouched and print nothing.
/// - In dry-run mode, report what would be removed/stripped and write nothing.
pub fn migrate_legacy_rules_at(
    workspace_root: &Path,
    ctx: InitContext,
) -> Result<Option<LegacyRulesAction>> {
    let legacy_file = workspace_root.join(LEGACY_RULES_REL_PATH);
    if !legacy_file.is_file() {
        return Ok(None);
    }

    let raw = match fs::read_to_string(&legacy_file) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    let normalized = raw.replace("\r\n", "\n");
    let trimmed = normalized.trim();

    let known_texts = [
        RTK_AWARENESS_FULL,
        RTK_AWARENESS_HIGH,
        RTK_AWARENESS_DEFAULT,
        LEGACY_ANTIGRAVITY_RULES_V1,
    ];

    // 1. Exact match: file content is solely an awareness text an earlier release wrote
    for known in &known_texts {
        let known_norm = known.replace("\r\n", "\n");
        if trimmed == known_norm.trim() {
            if ctx.dry_run {
                println!(
                    "[dry-run] would remove legacy rules file: {}",
                    legacy_file.display()
                );
            } else {
                // nosemgrep: filesystem-deletion -- legacy migration removes only RTK's legacy awareness rules file.
                fs::remove_file(&legacy_file).with_context(|| {
                    format!(
                        "Failed to remove legacy Antigravity rules file: {}",
                        legacy_file.display()
                    )
                })?;
                if ctx.verbose > 0 {
                    eprintln!("Removed legacy rules file: {}", legacy_file.display());
                }
            }
            return Ok(Some(LegacyRulesAction::RemovedFile(legacy_file)));
        }
    }

    // 2. Suffix removal: awareness text follows user content after a blank line
    for known in &known_texts {
        let known_norm = known.replace("\r\n", "\n");
        let target = format!("\n\n{}", known_norm.trim());
        if trimmed.ends_with(&target) {
            let prefix_len = trimmed.len() - target.len();
            let remaining_trimmed = &trimmed[..prefix_len];

            if remaining_trimmed.trim().is_empty() {
                if ctx.dry_run {
                    println!(
                        "[dry-run] would remove legacy rules file: {}",
                        legacy_file.display()
                    );
                } else {
                    // nosemgrep: filesystem-deletion -- legacy migration removes only RTK's legacy awareness rules file.
                    fs::remove_file(&legacy_file).with_context(|| {
                        format!(
                            "Failed to remove legacy Antigravity rules file: {}",
                            legacy_file.display()
                        )
                    })?;
                    if ctx.verbose > 0 {
                        eprintln!("Removed legacy rules file: {}", legacy_file.display());
                    }
                }
                return Ok(Some(LegacyRulesAction::RemovedFile(legacy_file)));
            }

            let target_trimmed = known_norm.trim();
            let crlf_suffix = format!("\r\n\r\n{}", target_trimmed.replace('\n', "\r\n"));
            let lf_suffix = format!("\n\n{}", target_trimmed);

            let remaining_bytes = if raw.trim_end().ends_with(&crlf_suffix) {
                let cut_idx = raw.trim_end().len() - crlf_suffix.len();
                &raw[..cut_idx]
            } else if raw.trim_end().ends_with(&lf_suffix) {
                let cut_idx = raw.trim_end().len() - lf_suffix.len();
                &raw[..cut_idx]
            } else {
                remaining_trimmed
            };

            if ctx.dry_run {
                println!(
                    "[dry-run] would remove legacy awareness text from {}",
                    legacy_file.display()
                );
            } else {
                atomic_write(&legacy_file, remaining_bytes).with_context(|| {
                    format!(
                        "Failed to update legacy Antigravity rules file: {}",
                        legacy_file.display()
                    )
                })?;
                if ctx.verbose > 0 {
                    eprintln!(
                        "Removed legacy awareness text from: {}",
                        legacy_file.display()
                    );
                }
            }
            return Ok(Some(LegacyRulesAction::StrippedSuffix(legacy_file)));
        }
    }

    // 3. Foreign / unrelated file: leave alone and say nothing
    Ok(None)
}

pub fn run_antigravity_mode(global: bool, ctx: InitContext) -> Result<()> {
    if global {
        let home = user_dirs::home().context("Could not determine user home directory")?;
        let base_dir = home.join(".gemini/config");
        run_antigravity_mode_at(&base_dir, true, ctx)?;
        if let Ok(cwd) = std::env::current_dir() {
            let _ = migrate_legacy_rules_at(&cwd, ctx)?;
        }
        Ok(())
    } else {
        let cwd = user_dirs::current_dir().context("Failed to read current directory")?;
        run_antigravity_mode_at(&cwd, false, ctx)
    }
}

pub fn run_antigravity_mode_at(base_dir: &Path, global: bool, ctx: InitContext) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let plugin_dir = if global {
        base_dir.join("plugins/rtk")
    } else {
        base_dir.join(".agents/plugins/rtk")
    };

    let legacy_action = if !global {
        migrate_legacy_rules_at(base_dir, ctx)?
    } else {
        None
    };

    let plugin_json_path = plugin_dir.join("plugin.json");
    let hooks_json_path = plugin_dir.join("hooks.json");
    // Rules under `rules/` apply whenever the plugin is active, as plain markdown
    // with no frontmatter (Antigravity's plugin docs); `awareness.level` picks
    // the text, as it does for every agent with a command hook.
    let rules_dir = plugin_dir.join("rules");
    let rules_path = rules_dir.join(ANTIGRAVITY_RULES_FILE);
    let rules_content = awareness_content(ctx.awareness);

    if dry_run {
        println!(
            "[dry-run] would create plugin directory: {}",
            plugin_dir.display()
        );
        println!("[dry-run] would write {}", rules_path.display());
        println!("[dry-run] would write {}", hooks_json_path.display());
        println!("[dry-run] would write {}", plugin_json_path.display());
        if verbose > 0 {
            println!(
                "[dry-run] plugin.json content:\n{}",
                ANTIGRAVITY_PLUGIN_JSON
            );
            println!("[dry-run] hooks.json content:\n{}", ANTIGRAVITY_HOOKS_JSON);
            println!(
                "[dry-run] rules/{ANTIGRAVITY_RULES_FILE} content:\n{}",
                rules_content
            );
        }
        print_dry_run_footer();
    } else {
        fs::create_dir_all(&rules_dir).with_context(|| {
            format!(
                "Failed to create Antigravity plugin rules directory: {}",
                rules_dir.display()
            )
        })?;
        // plugin.json is what makes Antigravity discover the directory, so it goes last:
        // a first install that fails halfway leaves no plugin rather than one missing its
        // rules. A re-run over an existing plugin has no such guarantee.
        atomic_write(&rules_path, rules_content)
            .context("Failed to write Antigravity plugin rules")?;
        atomic_write(&hooks_json_path, ANTIGRAVITY_HOOKS_JSON)
            .context("Failed to write Antigravity hooks.json")?;
        atomic_write(&plugin_json_path, ANTIGRAVITY_PLUGIN_JSON)
            .context("Failed to write Antigravity plugin.json")?;

        if verbose > 0 {
            eprintln!("Wrote {}", rules_path.display());
            eprintln!("Wrote {}", hooks_json_path.display());
            eprintln!("Wrote {}", plugin_json_path.display());
        }

        println!("\nRTK plugin configured for Google Antigravity.\n");
        println!("  Plugin: {} (installed)", plugin_dir.display());
        println!("  Hooks:  PreToolUse -> rtk hook antigravity");
        println!(
            "  Rules:  rules/{ANTIGRAVITY_RULES_FILE} (awareness level: {})",
            ctx.awareness
        );
        match legacy_action {
            Some(LegacyRulesAction::RemovedFile(ref p)) => {
                println!("  Legacy: removed {}", p.display());
            }
            Some(LegacyRulesAction::StrippedSuffix(ref p)) => {
                println!("  Legacy: removed awareness text from {}", p.display());
            }
            None => {}
        }
        println!("  Restart Antigravity to load the plugin. Test with: git status");
        println!(
            "\n  Note: Antigravity checks permissions after hooks rewrite a command.\n  \
             If you use command allowlists, ensure `rtk` commands are permitted,\n  \
             e.g. `command(rtk git status)` or `command(rtk *)`.\n"
        );
    }

    Ok(())
}

pub fn uninstall_antigravity_mode(global: bool, ctx: InitContext) -> Result<()> {
    let base_dir = if global {
        user_dirs::home()
            .context("Could not determine user home directory")?
            .join(".gemini/config")
    } else {
        user_dirs::current_dir().context("Failed to read current directory")?
    };
    let mut removed = uninstall_antigravity_mode_at(&base_dir, global, ctx)?;

    if global
        && let Ok(cwd) = std::env::current_dir()
        && let Some(action) = migrate_legacy_rules_at(&cwd, ctx)?
    {
        match action {
            LegacyRulesAction::RemovedFile(p) => {
                removed.push(format!("legacy rules file: {}", p.display()));
            }
            LegacyRulesAction::StrippedSuffix(p) => {
                removed.push(format!("legacy awareness text in: {}", p.display()));
            }
        }
    }

    if removed.is_empty() {
        println!("RTK Antigravity support was not installed (nothing to remove)");
    } else {
        let header = if ctx.dry_run {
            "[dry-run] would uninstall RTK for Google Antigravity:"
        } else {
            "RTK uninstalled for Google Antigravity:"
        };
        println!("{header}");
        for item in removed {
            println!("  - {item}");
        }
    }

    if ctx.dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

/// Remove RTK's plugin directory, printing nothing. Returns what was removed, or under
/// `--dry-run` what would be.
pub fn uninstall_antigravity_mode_at(
    base_dir: &Path,
    global: bool,
    ctx: InitContext,
) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();
    let plugin_dir = if global {
        base_dir.join("plugins/rtk")
    } else {
        base_dir.join(".agents/plugins/rtk")
    };

    if plugin_dir.exists() {
        if !dry_run {
            // nosemgrep: filesystem-deletion -- uninstall intentionally removes only RTK's Antigravity plugin directory.
            fs::remove_dir_all(&plugin_dir).with_context(|| {
                format!(
                    "Failed to remove Antigravity plugin directory: {}",
                    plugin_dir.display()
                )
            })?;
            if verbose > 0 {
                eprintln!(
                    "Removed Antigravity plugin directory: {}",
                    plugin_dir.display()
                );
            }
        }
        removed.push(format!("Antigravity plugin: {}", plugin_dir.display()));
    }

    if !global && let Some(action) = migrate_legacy_rules_at(base_dir, ctx)? {
        match action {
            LegacyRulesAction::RemovedFile(p) => {
                removed.push(format!("legacy rules file: {}", p.display()));
            }
            LegacyRulesAction::StrippedSuffix(p) => {
                removed.push(format!("legacy awareness text in: {}", p.display()));
            }
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_antigravity_mode_creates_plugin_files_local() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();

        let plugin_dir = temp.path().join(".agents/plugins/rtk");
        let manifest_path = plugin_dir.join("plugin.json");
        let hooks_path = plugin_dir.join("hooks.json");

        assert!(manifest_path.exists(), "plugin.json should exist");
        assert!(hooks_path.exists(), "hooks.json should exist");

        let manifest = fs::read_to_string(&manifest_path).unwrap();
        assert!(manifest.contains(r#""name": "rtk""#));

        let hooks = fs::read_to_string(&hooks_path).unwrap();
        assert!(hooks.contains(r#""rtk-rewrite""#));
        assert!(hooks.contains(r#""command": "rtk hook antigravity""#));
    }

    #[test]
    fn test_antigravity_mode_creates_plugin_files_global() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(temp.path(), true, InitContext::default()).unwrap();

        let plugin_dir = temp.path().join("plugins/rtk");
        let manifest_path = plugin_dir.join("plugin.json");
        let hooks_path = plugin_dir.join("hooks.json");

        assert!(manifest_path.exists(), "global plugin.json should exist");
        assert!(hooks_path.exists(), "global hooks.json should exist");
        assert!(
            plugin_dir
                .join("rules")
                .join(ANTIGRAVITY_RULES_FILE)
                .is_file(),
            "global rules/AGENTS.md should exist"
        );
    }

    #[test]
    fn test_antigravity_mode_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(
            temp.path(),
            false,
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        )
        .unwrap();

        let plugin_dir = temp.path().join(".agents/plugins/rtk");
        assert!(
            !plugin_dir.exists(),
            "Plugin dir must not exist after dry run"
        );
    }

    #[test]
    fn test_antigravity_mode_reinstall_idempotent() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();
        run_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();

        let plugin_dir = temp.path().join(".agents/plugins/rtk");
        assert!(plugin_dir.join("plugin.json").exists());
        assert!(plugin_dir.join("hooks.json").exists());
    }

    #[test]
    fn test_antigravity_mode_writes_awareness_rules_at_each_level() {
        for (level, expected) in [
            (AwarenessLevel::Default, RTK_AWARENESS_DEFAULT),
            (AwarenessLevel::High, RTK_AWARENESS_HIGH),
            (AwarenessLevel::Full, RTK_AWARENESS_FULL),
        ] {
            let temp = TempDir::new().unwrap();
            run_antigravity_mode_at(
                temp.path(),
                false,
                InitContext {
                    awareness: level,
                    ..InitContext::default()
                },
            )
            .unwrap();
            let rules = fs::read_to_string(
                temp.path()
                    .join(".agents/plugins/rtk/rules")
                    .join(ANTIGRAVITY_RULES_FILE),
            )
            .unwrap();
            assert_eq!(rules, expected, "awareness level {level}");
            assert!(
                !rules.starts_with("---"),
                "plugin rules are plain markdown, no frontmatter"
            );
        }
    }

    #[test]
    fn test_antigravity_mode_reinit_rewrites_rules_for_a_new_level() {
        let temp = TempDir::new().unwrap();
        let rules_path = temp
            .path()
            .join(".agents/plugins/rtk/rules")
            .join(ANTIGRAVITY_RULES_FILE);
        for (level, expected) in [
            (AwarenessLevel::Default, RTK_AWARENESS_DEFAULT),
            (AwarenessLevel::Full, RTK_AWARENESS_FULL),
        ] {
            let ctx = InitContext {
                awareness: level,
                ..InitContext::default()
            };
            run_antigravity_mode_at(temp.path(), false, ctx).unwrap();
            assert_eq!(
                fs::read_to_string(&rules_path).unwrap(),
                expected,
                "{level}"
            );
        }
    }

    #[test]
    fn test_antigravity_mode_uninstall_removes_plugin() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();

        let removed =
            uninstall_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();
        assert_eq!(removed.len(), 1);

        let plugin_dir = temp.path().join(".agents/plugins/rtk");
        assert!(!plugin_dir.exists(), "Plugin dir should be removed");
    }

    #[test]
    fn test_migrate_legacy_rules_exact_match_removed() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        fs::write(&legacy_file, RTK_AWARENESS_FULL).unwrap();

        let action = migrate_legacy_rules_at(temp.path(), InitContext::default()).unwrap();
        assert_eq!(
            action,
            Some(LegacyRulesAction::RemovedFile(legacy_file.clone()))
        );
        assert!(!legacy_file.exists());
    }

    #[test]
    fn test_migrate_legacy_rules_exact_match_v1_removed() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        fs::write(&legacy_file, LEGACY_ANTIGRAVITY_RULES_V1).unwrap();

        let action = migrate_legacy_rules_at(temp.path(), InitContext::default()).unwrap();
        assert_eq!(
            action,
            Some(LegacyRulesAction::RemovedFile(legacy_file.clone()))
        );
        assert!(!legacy_file.exists());
    }

    #[test]
    fn test_migrate_legacy_rules_suffix_stripped() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        let user_content = "# Project Coding Standards\n\nAlways write tests.";
        let full_content = format!("{user_content}\n\n{}", RTK_AWARENESS_FULL);
        fs::write(&legacy_file, &full_content).unwrap();

        let action = migrate_legacy_rules_at(temp.path(), InitContext::default()).unwrap();
        assert_eq!(
            action,
            Some(LegacyRulesAction::StrippedSuffix(legacy_file.clone()))
        );
        assert!(legacy_file.exists());
        let updated = fs::read_to_string(&legacy_file).unwrap();
        assert_eq!(updated, user_content);
    }

    #[test]
    fn test_migrate_legacy_rules_foreign_untouched() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        let foreign_content = "Prefer RTK Query for data fetching; do not add axios.";
        fs::write(&legacy_file, foreign_content).unwrap();

        let action = migrate_legacy_rules_at(temp.path(), InitContext::default()).unwrap();
        assert_eq!(action, None);
        assert!(legacy_file.exists());
        assert_eq!(fs::read_to_string(&legacy_file).unwrap(), foreign_content);
    }

    #[test]
    fn test_migrate_legacy_rules_dry_run_leaves_file() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        fs::write(&legacy_file, RTK_AWARENESS_FULL).unwrap();

        let action = migrate_legacy_rules_at(
            temp.path(),
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        )
        .unwrap();
        assert_eq!(
            action,
            Some(LegacyRulesAction::RemovedFile(legacy_file.clone()))
        );
        assert!(legacy_file.exists(), "Dry run must not delete file");
    }

    #[test]
    fn test_antigravity_uninstall_with_only_legacy_rules_removes_them() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        fs::write(&legacy_file, RTK_AWARENESS_FULL).unwrap();

        let removed =
            uninstall_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(removed[0].contains("legacy rules file"));
        assert!(!legacy_file.exists());
    }
}
