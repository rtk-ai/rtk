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

pub fn run_antigravity_mode(global: bool, ctx: InitContext) -> Result<()> {
    let _scope = (!global).then(|| ProjectScope::enter(ctx));
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
    let InitContext { dry_run, .. } = ctx;
    let plugin_dir = if global {
        base_dir.join("plugins/rtk")
    } else {
        base_dir.join(".agents/plugins/rtk")
    };

    let plugin_json_path = plugin_dir.join("plugin.json");
    let hooks_json_path = plugin_dir.join("hooks.json");
    // Rules under `rules/` apply whenever the plugin is active, as plain markdown
    // with no frontmatter (Antigravity's plugin docs); `awareness.level` picks
    // the text, as it does for every agent with a command hook.
    let rules_path = plugin_dir.join("rules").join(ANTIGRAVITY_RULES_FILE);
    let rules_content = awareness_content(ctx.awareness);
    // The plugin makes Antigravity run `rtk hook antigravity`, so a project install keeps
    // every file of it inside the project.
    for path in [&rules_path, &hooks_json_path, &plugin_json_path] {
        ensure_project_file_inside(path, "Antigravity")?;
    }

    if dry_run {
        println!(
            "[dry-run] would create plugin directory: {}",
            plugin_dir.display()
        );
    }
    // plugin.json is what makes Antigravity discover the directory, so it goes last: a first
    // install that fails halfway leaves no plugin rather than one missing its rules. A re-run
    // over an existing plugin has no such guarantee.
    let rules_name = format!("rules/{ANTIGRAVITY_RULES_FILE}");
    for (path, content, name, what) in [
        (
            &rules_path,
            rules_content,
            rules_name.as_str(),
            "plugin rules",
        ),
        (
            &hooks_json_path,
            ANTIGRAVITY_HOOKS_JSON,
            "hooks.json",
            "hooks.json",
        ),
        (
            &plugin_json_path,
            ANTIGRAVITY_PLUGIN_JSON,
            "plugin.json",
            "plugin.json",
        ),
    ] {
        let report = Report::new(format!("[dry-run] would write {}", path.display()))
            .with_detail(format!("[dry-run] {name} content:\n{content}"))
            .done_verbose(format!("Wrote {}", path.display()));
        write_reported(path, WriteKind::Owned, content, ctx, report)
            .with_context(|| format!("Failed to write Antigravity {what}"))?;
    }

    if dry_run {
        print_dry_run_footer();
    } else {
        println!("\nRTK plugin configured for Google Antigravity.\n");
        println!("  Plugin: {} (installed)", plugin_dir.display());
        println!("  Hooks:  PreToolUse -> rtk hook antigravity");
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
    let _scope = (!global).then(|| ProjectScope::enter(ctx));
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
    // Removed whole, so a project uninstall must not reach a directory outside the project.
    ensure_project_file_inside(&plugin_dir, "Antigravity")?;

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

    #[cfg(unix)]
    #[test]
    fn test_project_install_refuses_a_plugin_dir_linked_outside_the_project() {
        use crate::core::test_isolation;
        use std::os::unix::fs::symlink;

        let project = test_isolation::tempdir();
        let elsewhere = TempDir::new().expect("elsewhere");
        symlink(elsewhere.path(), project.path().join(".agents")).expect("symlink");

        let _entered = test_isolation::enter(project.path());
        let error = run_antigravity_mode(false, InitContext::default())
            .expect_err("a plugin outside the project is refused");

        assert!(error.to_string().contains("outside the project"), "{error}");
        assert!(
            !elsewhere.path().join("plugins").exists(),
            "nothing is written through the link"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_project_uninstall_leaves_a_plugin_dir_linked_outside_the_project_alone() {
        use crate::core::test_isolation;
        use std::os::unix::fs::symlink;

        let project = test_isolation::tempdir();
        let elsewhere = TempDir::new().expect("elsewhere");
        let outside_plugin = elsewhere.path().join("plugins/rtk");
        fs::create_dir_all(&outside_plugin).expect("outside plugin dir");
        fs::write(outside_plugin.join("plugin.json"), ANTIGRAVITY_PLUGIN_JSON).expect("write");
        symlink(elsewhere.path(), project.path().join(".agents")).expect("symlink");

        let _entered = test_isolation::enter(project.path());
        let error = uninstall_antigravity_mode(false, InitContext::default())
            .expect_err("a plugin outside the project is refused");

        assert!(error.to_string().contains("outside the project"), "{error}");
        assert!(
            outside_plugin.join("plugin.json").exists(),
            "a directory outside the project is not removed"
        );
    }
}
