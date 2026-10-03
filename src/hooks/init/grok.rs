//! Grok Build CLI: install/uninstall a dedicated PreToolUse hook file.

use super::*;
use crate::hooks::constants::{
    GROK_DIR, GROK_HOME_ENV, GROK_HOOK_COMMAND, GROK_HOOK_FILE, GROK_RULES_FILE, GROK_RULES_SUBDIR,
    HOOKS_SUBDIR,
};
use std::ffi::OsString;

pub(crate) fn resolve_grok_home() -> Result<PathBuf> {
    resolve_grok_home_from(std::env::var_os(GROK_HOME_ENV), dirs::home_dir())
}

fn resolve_grok_home_from(
    grok_home: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(value) = grok_home.filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    home_dir
        .map(|h| h.join(GROK_DIR))
        .context("Cannot determine Grok home. Set $GROK_HOME or $HOME.")
}

/// Entry point for `rtk init --agent grok` / `rtk init -g --agent grok`.
pub fn run_grok_mode(global: bool, hook_only: bool, ctx: InitContext) -> Result<()> {
    let grok_home = if global {
        resolve_grok_home()?
    } else {
        PathBuf::from(GROK_DIR)
    };
    run_grok_mode_at(&grok_home, global, hook_only, ctx)
}

fn run_grok_mode_at(
    grok_home: &Path,
    global: bool,
    hook_only: bool,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let hooks_dir = grok_home.join(HOOKS_SUBDIR);
    if !dry_run {
        fs::create_dir_all(&hooks_dir)
            .with_context(|| format!("Failed to create Grok hooks dir: {}", hooks_dir.display()))?;
    }

    let hook_path = hooks_dir.join(GROK_HOOK_FILE);
    write_if_changed(&hook_path, &grok_hook_file_contents(), GROK_HOOK_FILE, ctx)?;

    if !hook_only {
        let rules_dir = grok_home.join(GROK_RULES_SUBDIR);
        if !dry_run {
            fs::create_dir_all(&rules_dir).with_context(|| {
                format!("Failed to create Grok rules dir: {}", rules_dir.display())
            })?;
        }
        let rules_path = rules_dir.join(GROK_RULES_FILE);
        write_if_changed(
            &rules_path,
            awareness_content(ctx.awareness),
            GROK_RULES_FILE,
            ctx,
        )?;
    }

    if dry_run {
        print_dry_run_footer();
    } else {
        let scope = if global { "global" } else { "project" };
        println!("\nGrok Build CLI hook installed ({scope}).\n");
        println!("  Hook: {}", hook_path.display());
        if !hook_only {
            println!(
                "  Rules: {}",
                grok_home
                    .join(GROK_RULES_SUBDIR)
                    .join(GROK_RULES_FILE)
                    .display()
            );
        }
        println!("  Restart Grok. Test with: git status\n");
        if !global {
            println!("  Project hooks require folder trust (`/hooks-trust` or `--trust`).\n");
        }
    }
    Ok(())
}

fn grok_hook_file_contents() -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{
                    "type": "command",
                    "command": GROK_HOOK_COMMAND,
                    "timeout": 10
                }]
            }]
        }
    }))
    .expect("static grok hook JSON is valid")
        + "\n"
}

/// Public entry point for `rtk init --agent grok --uninstall`.
pub fn uninstall_grok(global: bool, ctx: InitContext) -> Result<()> {
    let grok_home = if global {
        match resolve_grok_home() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("RTK Grok uninstall skipped: could not resolve Grok home ({e})");
                return Ok(());
            }
        }
    } else {
        PathBuf::from(GROK_DIR)
    };
    let removed = uninstall_grok_at(&grok_home, ctx)?;

    if removed.is_empty() {
        println!("RTK Grok support was not installed (nothing to remove)");
    } else {
        let header = if ctx.dry_run {
            "[dry-run] would uninstall RTK for Grok Build CLI:"
        } else {
            "RTK uninstalled for Grok Build CLI:"
        };
        println!("{}", header);
        for item in removed {
            println!("  - {}", item);
        }
        if !ctx.dry_run {
            println!("\nRestart Grok to apply changes.");
        }
    }

    if ctx.dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

fn uninstall_grok_at(grok_home: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();

    let hook_path = grok_home.join(HOOKS_SUBDIR).join(GROK_HOOK_FILE);
    if hook_path.exists() {
        if dry_run {
            println!("[dry-run] would remove Grok hook: {}", hook_path.display());
        } else {
            // nosemgrep: filesystem-deletion -- uninstall removes only RTK's dedicated Grok hook file
            fs::remove_file(&hook_path)
                .with_context(|| format!("Failed to remove {}", hook_path.display()))?;
        }
        removed.push(format!("Grok hook: {}", hook_path.display()));
    }

    let rules_path = grok_home.join(GROK_RULES_SUBDIR).join(GROK_RULES_FILE);
    if rules_path.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove Grok rules: {}",
                rules_path.display()
            );
        } else {
            // nosemgrep: filesystem-deletion -- uninstall removes only RTK's Grok awareness file
            fs::remove_file(&rules_path)
                .with_context(|| format!("Failed to remove {}", rules_path.display()))?;
        }
        removed.push(format!("Grok rules: {}", rules_path.display()));
    }

    if verbose > 0 && !removed.is_empty() {
        eprintln!("Grok artifacts removed");
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_grok_home_uses_env_then_dot_grok() {
        let temp = TempDir::new().unwrap();
        let override_home = temp.path().join("custom-grok");
        assert_eq!(
            resolve_grok_home_from(Some(override_home.clone().into()), None).unwrap(),
            override_home
        );
        let home = temp.path().join("user");
        assert_eq!(
            resolve_grok_home_from(None, Some(home.clone())).unwrap(),
            home.join(GROK_DIR)
        );
    }

    #[test]
    fn test_grok_hook_file_shape() {
        let content = grok_hook_file_contents();
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            v.pointer("/hooks/PreToolUse/0/matcher")
                .and_then(|m| m.as_str()),
            Some("Bash")
        );
        assert_eq!(
            v.pointer("/hooks/PreToolUse/0/hooks/0/command")
                .and_then(|c| c.as_str()),
            Some(GROK_HOOK_COMMAND)
        );
    }

    #[test]
    fn test_grok_install_creates_hook_and_rules() {
        let temp = TempDir::new().unwrap();
        let grok_home = temp.path().join(".grok");
        run_grok_mode_at(&grok_home, true, false, InitContext::default()).unwrap();

        let hook = fs::read_to_string(grok_home.join(HOOKS_SUBDIR).join(GROK_HOOK_FILE)).unwrap();
        assert!(hook.contains(GROK_HOOK_COMMAND));
        assert!(
            grok_home
                .join(GROK_RULES_SUBDIR)
                .join(GROK_RULES_FILE)
                .exists()
        );
    }

    #[test]
    fn test_grok_hook_only_skips_rules() {
        let temp = TempDir::new().unwrap();
        let grok_home = temp.path().join(".grok");
        run_grok_mode_at(&grok_home, true, true, InitContext::default()).unwrap();
        assert!(grok_home.join(HOOKS_SUBDIR).join(GROK_HOOK_FILE).exists());
        assert!(
            !grok_home
                .join(GROK_RULES_SUBDIR)
                .join(GROK_RULES_FILE)
                .exists()
        );
    }

    #[test]
    fn test_grok_install_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let grok_home = temp.path().join(".grok");
        run_grok_mode_at(&grok_home, true, false, InitContext::default()).unwrap();
        run_grok_mode_at(&grok_home, true, false, InitContext::default()).unwrap();
        let hook = fs::read_to_string(grok_home.join(HOOKS_SUBDIR).join(GROK_HOOK_FILE)).unwrap();
        assert_eq!(hook.matches(GROK_HOOK_COMMAND).count(), 1);
    }

    #[test]
    fn test_grok_uninstall_removes_hook_and_rules() {
        let temp = TempDir::new().unwrap();
        let grok_home = temp.path().join(".grok");
        run_grok_mode_at(&grok_home, true, false, InitContext::default()).unwrap();
        uninstall_grok_at(&grok_home, InitContext::default()).unwrap();
        assert!(!grok_home.join(HOOKS_SUBDIR).join(GROK_HOOK_FILE).exists());
        assert!(
            !grok_home
                .join(GROK_RULES_SUBDIR)
                .join(GROK_RULES_FILE)
                .exists()
        );
    }
}
