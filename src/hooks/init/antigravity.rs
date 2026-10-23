//! Google Antigravity support: transparent command rewriting via PreToolUse lifecycle hook plugin.

use super::*;
use crate::core::user_dirs;

pub const ANTIGRAVITY_PLUGIN_JSON: &str = r#"{
  "name": "rtk",
  "version": "1.0.0",
  "description": "Antigravity plugin for transparent command rewriting and token optimization using rtk"
}
"#;

const ANTIGRAVITY_HOOK_COMMAND: &str = "rtk hook antigravity";

#[cfg(feature = "engine")]
fn antigravity_entries(global: bool) -> (Vec<super::plan::Entry>, Vec<String>) {
    match super::plan::plan_for("antigravity", global) {
        Some(plan) => (plan.entries(), plan.skipped_lines()),
        None => (legacy_antigravity_entries(), Vec::new()),
    }
}

#[cfg(not(feature = "engine"))]
fn antigravity_entries(_global: bool) -> (Vec<(&'static str, Option<String>)>, Vec<String>) {
    (legacy_antigravity_entries(), Vec::new())
}

fn legacy_antigravity_entries() -> Vec<(&'static str, Option<String>)> {
    vec![("PreToolUse", Some("run_command".to_string()))]
}

pub(super) fn antigravity_hooks_json(entries: &[(&'static str, Option<String>)]) -> String {
    let mut group = serde_json::Map::new();
    group.insert("enabled".into(), serde_json::json!(true));
    for (event, matcher) in entries {
        let session_start = *event == crate::hooks::constants::SESSION_START_KEY;
        let command = if session_start {
            format!("{ANTIGRAVITY_HOOK_COMMAND} --event {event}")
        } else {
            ANTIGRAVITY_HOOK_COMMAND.to_string()
        };
        let handler = serde_json::json!({
            "type": "command",
            "command": command,
            "timeout": 10
        });
        let entry = match (session_start, matcher) {
            (true, _) => handler,
            (false, Some(matcher)) => serde_json::json!({ "matcher": matcher, "hooks": [handler] }),
            (false, None) => serde_json::json!({ "hooks": [handler] }),
        };
        if let Some(list) = group
            .entry((*event).to_string())
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
        {
            list.push(entry);
        }
    }
    let root = serde_json::json!({ "rtk-rewrite": group });
    let mut text = serde_json::to_string_pretty(&root).unwrap_or_default();
    text.push('\n');
    text
}

/// The awareness rules inside the plugin bundle. The plugin docs bundled with `agy`
/// recommend `rules/AGENTS.md`: plain markdown, no frontmatter, always on while the
/// plugin is enabled.
const ANTIGRAVITY_RULES_FILE: &str = "AGENTS.md";

pub fn run_antigravity_mode(global: bool, ctx: InitContext) -> Result<()> {
    if global {
        let home = user_dirs::home().context("Could not determine user home directory")?;
        let base_dir = home.join(".gemini/config");
        run_antigravity_mode_at(&base_dir, true, ctx)
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

    let plugin_json_path = plugin_dir.join("plugin.json");
    let hooks_json_path = plugin_dir.join("hooks.json");
    let (entries, skipped) = antigravity_entries(global);
    let hooks_json = antigravity_hooks_json(&entries);
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
            println!("[dry-run] hooks.json content:\n{hooks_json}");
            println!(
                "[dry-run] rules/{ANTIGRAVITY_RULES_FILE} content:\n{}",
                rules_content
            );
        }
        for line in &skipped {
            println!("[dry-run] {line}");
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
        atomic_write(&hooks_json_path, &hooks_json)
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
        let registered: Vec<&str> = entries.iter().map(|(event, _)| *event).collect();
        println!(
            "  Hooks:  {} -> {ANTIGRAVITY_HOOK_COMMAND}",
            registered.join(", ")
        );
        for line in &skipped {
            println!("  {line}");
        }
        println!(
            "  Rules:  rules/{ANTIGRAVITY_RULES_FILE} (awareness level: {})",
            ctx.awareness
        );
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
    let removed = uninstall_antigravity_mode_at(&base_dir, global, ctx)?;

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
}
