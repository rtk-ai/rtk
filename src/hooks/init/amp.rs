//! Amp plugin installation helpers.
use super::*;
use crate::core::{user_dirs, user_env};
use crate::hooks::constants::{
    AMP_CONFIG_DIR_ENV, AMP_DIR, AMP_PLUGIN_ENTRY_FILE, AMP_PLUGIN_JS_FILE, AMP_PLUGIN_NAME,
    AMP_PLUGINS_SUBDIR,
};

const AMP_PLUGIN: &str = include_str!("../../../hooks/amp/rtk/index.ts");
const AMP_PLUGIN_MARKER: &str = "shellCommandFromToolCall";

/// Resolve Amp's machine-level configuration directory.
///
/// Amp follows `XDG_CONFIG_HOME` when it is set and otherwise uses
/// `~/.config/amp` on Unix and Windows. Keeping this resolution here also
/// gives the test suite one seam for the platform-specific path contract.
fn resolve_amp_config_dir() -> Result<PathBuf> {
    resolve_amp_config_dir_from(
        user_env::var_os(AMP_CONFIG_DIR_ENV).map(PathBuf::from),
        user_dirs::home(),
    )
}

fn resolve_amp_config_dir_from(
    xdg_config_home: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(xdg_config_home) = xdg_config_home.filter(|path| !path.as_os_str().is_empty()) {
        return Ok(xdg_config_home.join("amp"));
    }

    home_dir
        .map(|home| home.join(AMP_DIR))
        .context("Cannot determine Amp config directory. Set $XDG_CONFIG_HOME or $HOME.")
}

fn amp_plugin_path(config_dir: &Path) -> PathBuf {
    config_dir
        .join(AMP_PLUGINS_SUBDIR)
        .join(AMP_PLUGIN_NAME)
        .join(AMP_PLUGIN_ENTRY_FILE)
}

/// Return the Amp plugin path for the requested scope.
pub(super) fn amp_plugin_path_for_scope(global: bool) -> Result<PathBuf> {
    if global {
        Ok(amp_plugin_path(&resolve_amp_config_dir()?))
    } else {
        Ok(user_dirs::in_working_dir(".amp/plugins/rtk/index.ts"))
    }
}

fn amp_plugin_conflict_path(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return Some(path.to_path_buf());
    }

    let javascript_entry = path.with_file_name(AMP_PLUGIN_JS_FILE);
    javascript_entry.exists().then_some(javascript_entry)
}

fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n")
}

fn is_current_amp_plugin(content: &str) -> bool {
    normalize_line_endings(content).trim_end() == normalize_line_endings(AMP_PLUGIN).trim_end()
}

fn looks_like_rtk_amp_plugin(content: &str) -> bool {
    content.contains(AMP_PLUGIN_MARKER) && content.contains("rtk")
}

/// Check whether installing the managed TypeScript entry would overwrite or
/// shadow an existing Amp plugin. `PatchMode` has the same meaning here as it
/// does for Pi's protected extension: only `--auto-patch` skips the prompt.
fn validate_amp_plugin_path(path: &Path, patch_mode: PatchMode, ctx: InitContext) -> Result<bool> {
    // A single-file plugin has the same Amp name as the managed directory.
    // Do not rely on discovery order when both forms are present.
    if let Some(plugins_dir) = path.parent().and_then(Path::parent) {
        for extension in ["ts", "js"] {
            let single_file = plugins_dir.join(format!("{AMP_PLUGIN_NAME}.{extension}"));
            if single_file.exists() {
                if ctx.dry_run {
                    println!(
                        "[dry-run] conflicting Amp plugin at {}; rename it before installing the directory plugin.",
                        single_file.display()
                    );
                    return Ok(false);
                }
                anyhow::bail!(
                    "Conflicting Amp plugin at {}; rename it before installing the directory plugin.",
                    single_file.display()
                );
            }
        }
    }

    let Some(existing_path) = amp_plugin_conflict_path(path) else {
        return Ok(true);
    };

    if existing_path.is_dir() {
        anyhow::bail!(
            "Amp plugin entry at {} is a directory; remove it manually before retrying.",
            existing_path.display()
        );
    }

    let existing = match fs::read_to_string(&existing_path) {
        Ok(content) => content,
        Err(error) => {
            eprintln!(
                "[warn] Amp plugin entry at {} could not be read; treating it as non-stock: {}",
                existing_path.display(),
                error
            );
            String::new()
        }
    };

    if existing_path == path && is_current_amp_plugin(&existing) {
        return Ok(true);
    }

    let is_shadowing_javascript = existing_path
        .file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new(AMP_PLUGIN_JS_FILE));
    let action = if is_shadowing_javascript {
        "shadowing the existing Amp plugin"
    } else {
        "overwriting the existing Amp plugin"
    };

    if ctx.dry_run {
        match patch_mode {
            PatchMode::Ask => println!(
                "[dry-run] would prompt before {}: {}",
                action,
                existing_path.display()
            ),
            PatchMode::Auto => println!("[dry-run] would {}: {}", action, existing_path.display()),
            PatchMode::Skip => println!(
                "[dry-run] would leave existing Amp plugin unchanged: {}",
                existing_path.display()
            ),
        }
        return Ok(matches!(patch_mode, PatchMode::Auto));
    }

    match patch_mode {
        PatchMode::Auto => Ok(true),
        PatchMode::Skip => Ok(false),
        PatchMode::Ask => {
            let prompt = format!("Allow RTK to {} at {}?", action, existing_path.display());
            prompt_user_confirmation(&prompt)
        }
    }
}

fn run_amp_mode_at(
    path: &Path,
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !validate_amp_plugin_path(path, patch_mode, ctx)? {
        if dry_run {
            print_dry_run_footer();
            return Ok(());
        }
        anyhow::bail!(
            "Amp plugin at {} was not changed; rerun with --auto-patch to approve the replacement.",
            path.display()
        );
    }

    let installed = write_if_changed_allow_read_error(path, AMP_PLUGIN, "Amp plugin", ctx)?;

    if dry_run {
        print_dry_run_footer();
    } else {
        let status = if installed {
            "installed"
        } else {
            "already up to date"
        };
        let scope = if global { "global" } else { "project" };
        println!("\nRTK plugin configured for Amp ({scope}, {status}).\n");
        println!("  Plugin: {}", path.display());
        println!("  Restart Amp or reload plugins. Test with: git status\n");
    }

    Ok(())
}

/// Install the native Amp plugin for either the project or machine scope.
pub fn run_amp_mode(global: bool, patch_mode: PatchMode, ctx: InitContext) -> Result<()> {
    let path = amp_plugin_path_for_scope(global)?;
    run_amp_mode_at(&path, global, patch_mode, ctx)
}

fn uninstall_amp_mode_at(path: &Path, ctx: InitContext) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;

    if !path.exists() {
        if dry_run {
            print_dry_run_footer();
        } else {
            println!("RTK Amp plugin was not installed (nothing to remove)");
        }
        return Ok(());
    }

    if path.is_dir() {
        anyhow::bail!(
            "Amp plugin entry at {} is a directory; leaving it alone.",
            path.display()
        );
    }

    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            eprintln!(
                "[warn] Amp plugin at {} could not be read; leaving it alone: {}",
                path.display(),
                error
            );
            if dry_run {
                println!(
                    "[dry-run] would leave unreadable Amp plugin unchanged: {}",
                    path.display()
                );
                print_dry_run_footer();
                return Ok(());
            }
            anyhow::bail!(
                "Amp plugin at {} could not be read; leaving it alone.",
                path.display()
            );
        }
    };

    if !is_current_amp_plugin(&content) {
        if looks_like_rtk_amp_plugin(&content) {
            if dry_run {
                println!(
                    "[dry-run] would refuse to remove modified Amp plugin: {}",
                    path.display()
                );
                print_dry_run_footer();
                return Ok(());
            }
            anyhow::bail!(
                "Amp plugin at {} contains RTK content that does not match the stock plugin. Remove the file manually.",
                path.display()
            );
        }
        println!(
            "Amp plugin at {} is not RTK content; leaving it alone.",
            path.display()
        );
        if dry_run {
            print_dry_run_footer();
        }
        return Ok(());
    }

    if dry_run {
        println!("[dry-run] would remove Amp plugin: {}", path.display());
        print_dry_run_footer();
    } else {
        // nosemgrep: filesystem-deletion -- Amp uninstall removes only the exact stock plugin entry.
        fs::remove_file(path)
            .with_context(|| format!("Failed to remove Amp plugin: {}", path.display()))?;
        if verbose > 0 {
            eprintln!("Removed Amp plugin: {}", path.display());
        }
        println!("RTK uninstalled for Amp:");
        println!("  - Plugin: {}", path.display());
        println!("\nRestart Amp to apply changes.");
    }

    Ok(())
}

/// Remove the Amp plugin for either the project or machine scope.
pub fn uninstall_amp_mode(global: bool, ctx: InitContext) -> Result<()> {
    let path = amp_plugin_path_for_scope(global)?;
    uninstall_amp_mode_at(&path, ctx)
}

fn print_amp_plugin_status(label: &str, path: &Path) -> Result<()> {
    if path.exists() {
        if path.is_dir() {
            println!(
                "  [warn] {label}: {} (entry is a directory)",
                path.display()
            );
            return Ok(());
        }
        let content = fs::read_to_string(path).unwrap_or_default();
        if is_current_amp_plugin(&content) {
            println!("  [ok] {label}: {} (up to date)", path.display());
        } else if looks_like_rtk_amp_plugin(&content) {
            println!(
                "  [warn] {label}: {} (modified RTK content - rtk init will ask before overwriting; use --auto-patch to replace)",
                path.display()
            );
        } else {
            println!(
                "  [warn] {label}: {} (unrelated content - rtk init will ask before overwriting; use --auto-patch to replace)",
                path.display()
            );
        }
    } else if let Some(javascript_entry) = amp_plugin_conflict_path(path) {
        println!(
            "  [warn] {label}: {} (index.js exists; rtk init will ask before shadowing it)",
            javascript_entry.display()
        );
    } else {
        println!("  [--] {label}: {} (not installed)", path.display());
    }
    Ok(())
}

pub(super) fn show_amp_config() -> Result<()> {
    let global = amp_plugin_path_for_scope(true)?;
    let project = amp_plugin_path_for_scope(false)?;

    println!("rtk Configuration (Amp):\n");
    print_amp_plugin_status("Global plugin", &global)?;
    print_amp_plugin_status("Project plugin", &project)?;

    println!("\nUsage:");
    println!(
        "  rtk init --agent amp             # Configure {}",
        project.display()
    );
    println!(
        "  rtk init -g --agent amp          # Configure {}",
        global.display()
    );
    println!("  rtk init --uninstall --agent amp # Remove the project plugin");
    println!("  rtk init -g --uninstall --agent amp # Remove the global plugin");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_isolation;

    #[test]
    fn amp_config_dir_prefers_xdg_and_ignores_empty_values() {
        let home = PathBuf::from("/home/example");
        assert_eq!(
            resolve_amp_config_dir_from(Some(PathBuf::from("/tmp/xdg")), Some(home.clone()))
                .unwrap(),
            PathBuf::from("/tmp/xdg/amp")
        );
        assert_eq!(
            resolve_amp_config_dir_from(Some(PathBuf::new()), Some(home.clone())).unwrap(),
            home.join(AMP_DIR)
        );
        assert_eq!(
            resolve_amp_config_dir_from(None, Some(home.clone())).unwrap(),
            home.join(AMP_DIR)
        );
    }

    #[test]
    fn amp_plugin_path_uses_xdg_for_global_scope() {
        let tmp = test_isolation::tempdir();
        let xdg = tmp.path().join("xdg");
        user_env::with_path(AMP_CONFIG_DIR_ENV, Some(&xdg), || {
            assert_eq!(
                amp_plugin_path_for_scope(true).unwrap(),
                xdg.join("amp/plugins/rtk/index.ts")
            );
        });
    }

    #[test]
    fn install_is_idempotent_and_embeds_the_native_plugin() {
        let tmp = test_isolation::tempdir();
        let path = tmp.path().join(".amp/plugins/rtk/index.ts");
        run_amp_mode_at(&path, false, PatchMode::Auto, InitContext::default()).unwrap();
        let first = fs::read_to_string(&path).unwrap();
        assert!(first.contains("amp.helpers.shellCommandFromToolCall"));
        assert!(first.contains("Bun.spawn"));

        run_amp_mode_at(&path, false, PatchMode::Auto, InitContext::default()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), first);
    }

    #[test]
    fn dry_run_does_not_create_amp_plugin_files() {
        let tmp = test_isolation::tempdir();
        let path = tmp.path().join(".amp/plugins/rtk/index.ts");
        run_amp_mode_at(
            &path,
            false,
            PatchMode::Auto,
            InitContext {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn protected_plugin_is_left_unchanged_without_auto_patch() {
        let tmp = test_isolation::tempdir();
        let path = tmp.path().join(".amp/plugins/rtk/index.ts");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "export default () => 'user plugin'\n").unwrap();

        let result = run_amp_mode_at(&path, false, PatchMode::Skip, InitContext::default());
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "export default () => 'user plugin'\n"
        );
    }

    #[test]
    fn uninstall_removes_only_the_current_plugin_and_is_idempotent() {
        let tmp = test_isolation::tempdir();
        let path = tmp.path().join(".amp/plugins/rtk/index.ts");
        run_amp_mode_at(&path, false, PatchMode::Auto, InitContext::default()).unwrap();
        uninstall_amp_mode_at(&path, InitContext::default()).unwrap();
        assert!(!path.exists());
        uninstall_amp_mode_at(&path, InitContext::default()).unwrap();
    }

    #[test]
    fn uninstall_preserves_modified_rtk_content() {
        let tmp = test_isolation::tempdir();
        let path = tmp.path().join(".amp/plugins/rtk/index.ts");
        run_amp_mode_at(&path, false, PatchMode::Auto, InitContext::default()).unwrap();
        let mut content = fs::read_to_string(&path).unwrap();
        content.push_str("\n// user change\n");
        fs::write(&path, content).unwrap();

        let result = uninstall_amp_mode_at(&path, InitContext::default());
        assert!(result.is_err());
        assert!(path.exists());
    }
}
