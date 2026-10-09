//! Kiro IDE / CLI support: an always-included steering file plus a PreToolUse hook
//! (`rtk hook kiro`) that blocks raw commands with an `rtk` suggestion.

use super::*;
use crate::core::user_dirs;
use crate::hooks::constants::{
    KIRO_DIR, KIRO_HOOK_COMMAND, KIRO_HOOK_FILE, KIRO_HOOKS_SUBDIR, KIRO_STEERING_FILE,
    KIRO_STEERING_SUBDIR,
};

/// Embedded Kiro steering file (always-included prompt guidance).
const KIRO_STEERING: &str = include_str!("../../../hooks/kiro/rtk.md");

/// Embedded Kiro hook config (agent-hook JSON, v1 `hooks` array format).
const KIRO_HOOK_JSON: &str = include_str!("../../../hooks/kiro/rtk-rewrite.json");

/// Install RTK integration for Kiro IDE/CLI.
///
/// Both artifacts go under one root: `<cwd>/.kiro/` for a project install,
/// `~/.kiro/` with `global`. Kiro CLI reads hooks from both scopes; Kiro IDE
/// reads hooks only from the workspace, so a global install covers the IDE
/// with the steering file alone.
pub fn run_kiro_mode(global: bool, ctx: InitContext) -> Result<()> {
    run_kiro_mode_at(&resolve_kiro_dir(global)?, global, ctx)
}

fn run_kiro_mode_at(kiro_root: &Path, global: bool, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;

    let steering_dir = kiro_root.join(KIRO_STEERING_SUBDIR);
    let hooks_dir = kiro_root.join(KIRO_HOOKS_SUBDIR);

    if !dry_run {
        for dir in [&steering_dir, &hooks_dir] {
            fs::create_dir_all(dir)
                .with_context(|| format!("Failed to create Kiro directory: {}", dir.display()))?;
        }
    }

    let steering_path = steering_dir.join(KIRO_STEERING_FILE);
    let steering_changed = write_if_changed(&steering_path, KIRO_STEERING, "Kiro steering", ctx)?;

    let hook_path = hooks_dir.join(KIRO_HOOK_FILE);
    let hook_changed = write_if_changed(&hook_path, KIRO_HOOK_JSON, "Kiro hook config", ctx)?;

    if dry_run {
        print_dry_run_footer();
        return Ok(());
    }

    let scope = if global { "global" } else { "project" };
    let status = |changed: bool| {
        if changed {
            "(installed)"
        } else {
            "(already up to date)"
        }
    };
    println!(
        "
RTK configured for Kiro ({scope}).
"
    );
    println!(
        "  Steering: {} {}",
        steering_path.display(),
        status(steering_changed)
    );
    println!(
        "  Hook:     {} {}",
        hook_path.display(),
        status(hook_changed)
    );
    if global {
        println!(
            "
  Note: Kiro CLI reads this global hook; Kiro IDE reads hooks only
  \
             from the workspace. Run `rtk init --agent kiro` in a repository
  \
             to add the hook there for the IDE."
        );
    }
    println!(
        "
  Test with: git status
"
    );

    Ok(())
}

/// Uninstall RTK integration for Kiro IDE/CLI from the scope selected by
/// `global`, leaving any other user-managed files in place.
pub fn uninstall_kiro(global: bool, ctx: InitContext) -> Result<()> {
    uninstall_kiro_at(&resolve_kiro_dir(global)?, global, ctx)
}

fn uninstall_kiro_at(kiro_root: &Path, global: bool, ctx: InitContext) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();

    let artifacts = [
        (
            "Steering",
            kiro_root
                .join(KIRO_STEERING_SUBDIR)
                .join(KIRO_STEERING_FILE),
        ),
        (
            "Hook config",
            kiro_root.join(KIRO_HOOKS_SUBDIR).join(KIRO_HOOK_FILE),
        ),
    ];
    for (label, path) in &artifacts {
        if !path.exists() {
            continue;
        }
        if dry_run {
            println!("[dry-run] would remove Kiro {label}: {}", path.display());
        } else {
            // nosemgrep: filesystem-deletion -- Kiro uninstall removes only RTK-managed files.
            fs::remove_file(path)
                .with_context(|| format!("Failed to remove Kiro {label}: {}", path.display()))?;
            if verbose > 0 {
                eprintln!("Removed Kiro {label}: {}", path.display());
            }
        }
        removed.push(format!("{label}: {}", path.display()));
    }

    if removed.is_empty() {
        println!("RTK Kiro support was not installed (nothing to remove)");
    } else {
        let scope = if global { "global" } else { "project" };
        let header = if dry_run {
            format!("[dry-run] would uninstall RTK for Kiro ({scope}):")
        } else {
            format!("RTK uninstalled for Kiro ({scope}):")
        };
        println!("{}", header);
        for item in &removed {
            println!("  - {}", item);
        }
        if !dry_run {
            println!(
                "
Restart Kiro to apply changes."
            );
        }
    }

    if dry_run {
        print_dry_run_footer();
    }

    Ok(())
}

/// Resolve the Kiro config directory for the given scope.
///
/// - `global=true`:  `~/.kiro` (user-global, applies to all sessions)
/// - `global=false`: `<cwd>/.kiro` (project-scoped, versionable)
fn resolve_kiro_dir(global: bool) -> Result<PathBuf> {
    if global {
        resolve_home_subdir(KIRO_DIR)
    } else {
        Ok(user_dirs::current_dir()
            .context("Failed to read current directory")?
            .join(KIRO_DIR))
    }
}

/// Report the state of Kiro integration for `--show`.
pub fn show_kiro_config() {
    // Project scope
    let project_kiro = PathBuf::from(KIRO_DIR);

    // Global scope
    let global_kiro = resolve_home_subdir(KIRO_DIR).ok();

    show_kiro_config_at(&project_kiro, global_kiro.as_deref());
}

/// Testable variant: reports installed/absent status for both scopes.
/// Returns a list of status lines for programmatic inspection.
fn show_kiro_config_at(project_kiro: &Path, global_kiro: Option<&Path>) -> Vec<String> {
    let mut lines = Vec::new();

    // Project scope
    let project_steering = project_kiro
        .join(KIRO_STEERING_SUBDIR)
        .join(KIRO_STEERING_FILE);
    let project_hook = project_kiro.join(KIRO_HOOKS_SUBDIR).join(KIRO_HOOK_FILE);

    if project_steering.exists() {
        let content = fs::read_to_string(&project_steering).unwrap_or_default();
        if content.contains("rtk") || content.contains("RTK") {
            lines.push("[ok] Kiro (project): steering installed".to_string());
        } else {
            lines.push("[--] Kiro (project): steering exists but rtk not configured".to_string());
        }
    } else {
        lines.push("[--] Kiro (project): steering not found".to_string());
    }

    if project_hook.exists() {
        let content = fs::read_to_string(&project_hook).unwrap_or_default();
        if content.contains(KIRO_HOOK_COMMAND) {
            lines.push("[ok] Kiro (project): hook config installed".to_string());
        } else {
            lines.push("[--] Kiro (project): hook config exists but not RTK".to_string());
        }
    } else {
        lines.push("[--] Kiro (project): hook config not found".to_string());
    }

    // Global scope
    if let Some(global_kiro) = global_kiro {
        let global_steering = global_kiro
            .join(KIRO_STEERING_SUBDIR)
            .join(KIRO_STEERING_FILE);
        let global_hook = global_kiro.join(KIRO_HOOKS_SUBDIR).join(KIRO_HOOK_FILE);

        if global_steering.exists() {
            let content = fs::read_to_string(&global_steering).unwrap_or_default();
            if content.contains("rtk") || content.contains("RTK") {
                lines.push("[ok] Kiro (global): steering installed".to_string());
            } else {
                lines
                    .push("[--] Kiro (global): steering exists but rtk not configured".to_string());
            }
        } else {
            lines.push("[--] Kiro (global): steering not found".to_string());
        }

        if global_hook.exists() {
            let content = fs::read_to_string(&global_hook).unwrap_or_default();
            if content.contains(KIRO_HOOK_COMMAND) {
                lines.push(
                    "[ok] Kiro (global): hook config installed (read by Kiro CLI only)".to_string(),
                );
            } else {
                lines.push("[--] Kiro (global): hook config exists but not RTK".to_string());
            }
        } else {
            lines.push("[--] Kiro (global): hook config not found".to_string());
        }
    } else {
        lines.push("[--] Kiro (global): home directory not found".to_string());
    }

    for line in &lines {
        println!("{line}");
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_kiro_mode_creates_steering_and_hook() {
        let temp = TempDir::new().unwrap();
        let kiro_dir = temp.path().join(".kiro");
        run_kiro_mode_at(&kiro_dir, false, InitContext::default()).unwrap();

        let steering_path = kiro_dir.join("steering").join("rtk.md");
        let hook_path = kiro_dir.join("hooks").join("rtk-rewrite.json");

        assert!(steering_path.exists(), "Steering file should be created");
        assert!(hook_path.exists(), "Hook config should be created");

        let steering_content = fs::read_to_string(&steering_path).unwrap();
        assert!(
            steering_content.contains("rtk"),
            "Steering should contain rtk instructions"
        );
        assert!(
            steering_content.contains("rtk gain"),
            "Steering should contain meta commands"
        );
        assert!(
            steering_content.contains("rtk discover"),
            "Steering should contain discover command"
        );
        assert!(
            steering_content.contains("rtk proxy"),
            "Steering should contain proxy command"
        );

        let hook_content = fs::read_to_string(&hook_path).unwrap();
        assert!(
            hook_content.contains("rtk hook kiro"),
            "Hook should contain rtk hook kiro command"
        );
        assert!(
            hook_content.contains("PreToolUse"),
            "Hook should reference PreToolUse event"
        );
    }

    #[test]
    fn test_kiro_mode_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let kiro_dir = temp.path().join(".kiro");

        run_kiro_mode_at(&kiro_dir, false, InitContext::default()).unwrap();
        let steering_path = kiro_dir.join("steering").join("rtk.md");
        let hook_path = kiro_dir.join("hooks").join("rtk-rewrite.json");
        let first_steering = fs::read_to_string(&steering_path).unwrap();
        let first_hook = fs::read_to_string(&hook_path).unwrap();

        // Second run should be a no-op
        run_kiro_mode_at(&kiro_dir, false, InitContext::default()).unwrap();
        let second_steering = fs::read_to_string(&steering_path).unwrap();
        let second_hook = fs::read_to_string(&hook_path).unwrap();

        assert_eq!(
            first_steering, second_steering,
            "Steering must be idempotent"
        );
        assert_eq!(first_hook, second_hook, "Hook config must be idempotent");
    }

    #[test]
    fn test_kiro_mode_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        let kiro_dir = temp.path().join(".kiro");

        let ctx = InitContext {
            dry_run: true,
            ..Default::default()
        };
        run_kiro_mode_at(&kiro_dir, false, ctx).unwrap();

        assert!(
            !kiro_dir.exists(),
            "dry-run must not create .kiro directory"
        );
    }

    #[test]
    fn test_uninstall_kiro_removes_artifacts() {
        let temp = TempDir::new().unwrap();
        let kiro_dir = temp.path().join(".kiro");

        // Install first
        run_kiro_mode_at(&kiro_dir, false, InitContext::default()).unwrap();
        let steering_path = kiro_dir.join("steering").join("rtk.md");
        let hook_path = kiro_dir.join("hooks").join("rtk-rewrite.json");
        assert!(steering_path.exists());
        assert!(hook_path.exists());

        // Uninstall
        uninstall_kiro_at(&kiro_dir, false, InitContext::default()).unwrap();
        assert!(!steering_path.exists(), "Steering file should be removed");
        assert!(!hook_path.exists(), "Hook config should be removed");
    }

    #[test]
    fn test_uninstall_kiro_preserves_other_files() {
        let temp = TempDir::new().unwrap();
        let kiro_dir = temp.path().join(".kiro");

        // Install RTK
        run_kiro_mode_at(&kiro_dir, false, InitContext::default()).unwrap();

        // Add a user file in the same directories
        let user_steering = kiro_dir.join("steering").join("user-rules.md");
        let user_hook = kiro_dir.join("hooks").join("user-hook.kiro.hook");
        fs::write(
            &user_steering,
            "# User rules
",
        )
        .unwrap();
        fs::write(&user_hook, r#"{"name": "user hook"}"#).unwrap();

        // Uninstall
        uninstall_kiro_at(&kiro_dir, false, InitContext::default()).unwrap();

        // User files must remain
        assert!(
            user_steering.exists(),
            "User steering file must be preserved"
        );
        assert!(user_hook.exists(), "User hook file must be preserved");
        assert_eq!(
            fs::read_to_string(&user_steering).unwrap(),
            "# User rules
"
        );
        assert_eq!(
            fs::read_to_string(&user_hook).unwrap(),
            r#"{"name": "user hook"}"#
        );
    }

    #[test]
    fn test_uninstall_kiro_nothing_to_remove() {
        let temp = TempDir::new().unwrap();
        let kiro_dir = temp.path().join(".kiro");

        // Uninstall on clean filesystem should not error
        uninstall_kiro_at(&kiro_dir, false, InitContext::default()).unwrap();
    }

    #[test]
    fn test_uninstall_kiro_dry_run_preserves_artifacts() {
        let temp = TempDir::new().unwrap();
        let kiro_dir = temp.path().join(".kiro");

        // Install first
        run_kiro_mode_at(&kiro_dir, false, InitContext::default()).unwrap();
        let steering_path = kiro_dir.join("steering").join("rtk.md");
        let hook_path = kiro_dir.join("hooks").join("rtk-rewrite.json");

        let ctx = InitContext {
            dry_run: true,
            ..Default::default()
        };
        uninstall_kiro_at(&kiro_dir, false, ctx).unwrap();

        // Files must still exist
        assert!(
            steering_path.exists(),
            "dry-run uninstall must not remove steering"
        );
        assert!(
            hook_path.exists(),
            "dry-run uninstall must not remove hook config"
        );
    }

    #[test]
    fn test_kiro_round_trip_install_uninstall() {
        let temp = TempDir::new().unwrap();
        let kiro_dir = temp.path().join(".kiro");

        // Add pre-existing user content
        let steering_dir = kiro_dir.join("steering");
        let hooks_dir = kiro_dir.join("hooks");
        fs::create_dir_all(&steering_dir).unwrap();
        fs::create_dir_all(&hooks_dir).unwrap();
        let user_file = steering_dir.join("my-project.md");
        fs::write(&user_file, "custom content").unwrap();

        // Install
        run_kiro_mode_at(&kiro_dir, false, InitContext::default()).unwrap();
        assert!(steering_dir.join("rtk.md").exists());
        assert!(hooks_dir.join("rtk-rewrite.json").exists());

        // Uninstall
        uninstall_kiro_at(&kiro_dir, false, InitContext::default()).unwrap();

        // RTK artifacts gone, user content preserved
        assert!(!steering_dir.join("rtk.md").exists());
        assert!(!hooks_dir.join("rtk-rewrite.json").exists());
        assert!(user_file.exists());
        assert_eq!(fs::read_to_string(&user_file).unwrap(), "custom content");
    }

    #[test]
    fn test_kiro_hook_json_format() {
        let v: serde_json::Value = serde_json::from_str(KIRO_HOOK_JSON).unwrap();
        assert_eq!(v["version"], "v1");
        let hooks = v["hooks"].as_array().expect("hooks must be an array");
        // Kiro reports the shell tool as `execute_bash` or `shell` depending on
        // the configuration, so both need a matcher.
        let matchers: Vec<&str> = hooks
            .iter()
            .map(|h| h["matcher"].as_str().expect("matcher must be a string"))
            .collect();
        assert_eq!(matchers, ["execute_bash", "shell"]);
        for hook in hooks {
            assert_eq!(hook["trigger"], "PreToolUse");
            assert!(hook["name"].as_str().is_some(), "name must be present");
            assert_eq!(hook["action"]["type"], "command");
            assert_eq!(hook["action"]["command"], "rtk hook kiro");
        }
    }

    #[test]
    fn test_kiro_steering_content() {
        // Verify the embedded steering file has key sections
        assert!(
            KIRO_STEERING.contains("inclusion: always"),
            "Steering must have front matter"
        );
        assert!(
            KIRO_STEERING.contains("rtk gain"),
            "Steering must list gain meta command"
        );
        assert!(
            KIRO_STEERING.contains("rtk discover"),
            "Steering must list discover meta command"
        );
        assert!(
            KIRO_STEERING.contains("rtk proxy"),
            "Steering must list proxy meta command"
        );
        assert!(
            KIRO_STEERING.contains("rtk git status"),
            "Steering must have example commands"
        );
    }

    #[test]
    fn test_kiro_mode_global_scope_creates_artifacts() {
        let temp = TempDir::new().unwrap();
        let global_root = temp.path().join("home").join(".kiro");

        run_kiro_mode_at(&global_root, true, InitContext::default()).unwrap();

        let steering = fs::read_to_string(global_root.join("steering").join("rtk.md")).unwrap();
        assert!(steering.contains("rtk gain"));

        let hook = fs::read_to_string(global_root.join("hooks").join("rtk-rewrite.json")).unwrap();
        assert_eq!(hook, KIRO_HOOK_JSON);
    }

    #[test]
    fn test_kiro_uninstall_global_scope_removes_artifacts() {
        let temp = TempDir::new().unwrap();
        let global_root = temp.path().join("home").join(".kiro");
        let project_root = temp.path().join("project").join(".kiro");

        run_kiro_mode_at(&global_root, true, InitContext::default()).unwrap();
        run_kiro_mode_at(&project_root, false, InitContext::default()).unwrap();

        uninstall_kiro_at(&global_root, true, InitContext::default()).unwrap();

        assert!(!global_root.join("steering").join("rtk.md").exists());
        assert!(!global_root.join("hooks").join("rtk-rewrite.json").exists());
        assert!(
            project_root.join("hooks").join("rtk-rewrite.json").exists(),
            "A global uninstall must leave the project install alone"
        );
    }

    #[test]
    fn test_kiro_show_reports_installed_state() {
        let temp = TempDir::new().unwrap();
        let project_kiro = temp.path().join("project").join(".kiro");
        let global_kiro = temp.path().join("global").join(".kiro");

        // Project install (steering + hook) and a global steering install.
        run_kiro_mode_at(&project_kiro, false, InitContext::default()).unwrap();
        run_kiro_mode_at(&global_kiro, true, InitContext::default()).unwrap();

        let lines = show_kiro_config_at(&project_kiro, Some(&global_kiro));

        // Both scopes should report installed
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[ok] Kiro (project): steering installed")),
            "Show should report project steering as installed"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[ok] Kiro (project): hook config installed")),
            "Show should report project hook as installed"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[ok] Kiro (global): steering installed")),
            "Show should report global steering as installed"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[ok] Kiro (global): hook config installed")),
            "Show should report global hook as installed: {lines:?}"
        );
    }

    #[test]
    fn test_kiro_show_reports_absent_state() {
        let temp = TempDir::new().unwrap();
        let project_kiro = temp.path().join("project").join(".kiro");
        let global_kiro = temp.path().join("global").join(".kiro");

        // Neither installed
        let lines = show_kiro_config_at(&project_kiro, Some(&global_kiro));

        assert!(
            lines
                .iter()
                .any(|l| l.contains("[--] Kiro (project): steering not found")),
            "Show should report project steering as not found"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[--] Kiro (project): hook config not found")),
            "Show should report project hook as not found"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[--] Kiro (global): steering not found")),
            "Show should report global steering as not found"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[--] Kiro (global): hook config not found")),
            "Show should report global hook as not found"
        );
    }

    #[test]
    fn test_kiro_show_reports_mixed_state() {
        let temp = TempDir::new().unwrap();
        let project_kiro = temp.path().join("project").join(".kiro");
        let global_kiro = temp.path().join("global").join(".kiro");

        // Only install in project scope
        run_kiro_mode_at(&project_kiro, false, InitContext::default()).unwrap();

        let lines = show_kiro_config_at(&project_kiro, Some(&global_kiro));

        assert!(
            lines
                .iter()
                .any(|l| l.contains("[ok] Kiro (project): steering installed")),
            "Project steering should be reported as installed"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[ok] Kiro (project): hook config installed")),
            "Project hook should be reported as installed"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[--] Kiro (global): steering not found")),
            "Global steering should be reported as not found"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[--] Kiro (global): hook config not found")),
            "Show should report global hook as not found"
        );
    }

    #[test]
    fn test_kiro_steering_has_shell_examples() {
        // Validate the steering includes representative shell command examples
        // (consistent with other agents' rule files)
        assert!(
            KIRO_STEERING.contains("rtk cargo test"),
            "Steering must include cargo test example"
        );
        assert!(
            KIRO_STEERING.contains("rtk ls"),
            "Steering must include ls example"
        );
        assert!(
            KIRO_STEERING.contains("rtk grep"),
            "Steering must include grep example"
        );
        assert!(
            KIRO_STEERING.contains("rtk docker"),
            "Steering must include docker example"
        );
        assert!(
            KIRO_STEERING.contains("rtk gh"),
            "Steering must include gh example"
        );
        // Verify chaining instruction
        assert!(
            KIRO_STEERING.contains("&&") || KIRO_STEERING.contains("cadeia"),
            "Steering must mention command chaining"
        );
    }
}
