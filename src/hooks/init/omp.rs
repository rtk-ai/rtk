//! Oh My Pi (OMP) agent: shared Pi extension install/uninstall helpers.

use super::*;

// Oh My Pi (OMP) support

// OMP ships a `legacy-pi-compat` layer that remaps the Pi coding-agent
// extension API, so it loads the exact same extension file as Pi
// (`hooks/pi/rtk.ts`, embedded as `PI_PLUGIN`). Only the install paths
// differ:
//
//   global=true  -> `$HOME/.omp/agent/extensions/rtk.ts`
//   global=false -> `.omp/extensions/rtk.ts`

/// Return the OMP extension install path for the given scope.
pub(crate) fn omp_extension_path_for_scope(global: bool) -> Result<PathBuf> {
    if global {
        Ok(resolve_omp_dir()?
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE))
    } else {
        Ok(PathBuf::from(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE))
    }
}

/// Resolve OMP's global agent directory. OMP itself uses
/// `PI_CODING_AGENT_DIR` for this relocation, so RTK follows the same
/// override instead of introducing a second path configuration.
pub(crate) fn resolve_omp_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var(PI_CODING_AGENT_DIR_ENV) {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    resolve_home_subdir(OMP_DIR)
}

/// Install the shared Pi extension file for OMP (hook-only; no AGENTS.md
/// injection). OMP loads the file through its `legacy-pi-compat` layer.
///
/// global=true  -> `$HOME/.omp/agent/extensions/rtk.ts`
/// global=false -> `.omp/extensions/rtk.ts`
#[allow(dead_code)] // Kept as the default-policy API for in-crate callers and tests.
pub fn run_omp_mode(global: bool, ctx: InitContext) -> Result<()> {
    run_omp_mode_with_patch_mode(global, PatchMode::Ask, ctx)
}

/// Install the shared Pi extension file for OMP with an explicit
/// confirmation policy for an existing non-stock file.
pub fn run_omp_mode_with_patch_mode(
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let path = omp_extension_path_for_scope(global)?;
    let extension_was_present = path.exists();

    warn_if_extension_shared_on_install(global, &path, PiCompatibleAgent::Omp)?;

    if !validate_stock_pi_plugin_path(&path, "OMP extension", patch_mode, ctx)? {
        if dry_run {
            print_dry_run_footer();
            return Ok(());
        }
        anyhow::bail!(
            "OMP extension at {} was not changed; remove or back up the file manually before retrying.",
            path.display()
        );
    }

    if let Some(parent) = path.parent() {
        ensure_pi_extensions_dir(
            parent,
            if global {
                "OMP extensions directory"
            } else {
                "local OMP extensions directory"
            },
            ctx,
        )?;
    }

    let installed =
        write_if_changed_allow_read_error(path.as_path(), PI_PLUGIN, "OMP extension", ctx)?;
    record_managed_agent(
        global,
        &path,
        PiCompatibleAgent::Omp,
        extension_was_present,
        ctx,
    )?;

    if dry_run {
        print_dry_run_footer();
    } else {
        print_omp_result(&path, installed);
    }

    Ok(())
}

pub(crate) fn print_omp_result(extension_path: &Path, installed: bool) {
    let status = if installed {
        "installed"
    } else {
        "already up to date"
    };
    println!("RTK OMP extension {}:", status);
    println!("  Extension: {}", extension_path.display());
    println!();
    println!("OMP will load the extension automatically on next start.");
}

/// Uninstall the OMP extension for the given scope.
///
/// The installed file is the shared stock Pi extension. Current and known
/// historical stock content is removed; RTK content that no longer matches a
/// known stock revision is left in place with a manual-removal notice.
/// Unrelated content is never touched.
#[allow(dead_code)] // Kept as the default-policy API for in-crate callers and tests.
pub fn uninstall_omp(global: bool, ctx: InitContext) -> Result<()> {
    uninstall_omp_with_patch_mode(global, PatchMode::Ask, ctx)
}

/// Uninstall the OMP extension with an explicit confirmation policy for a
/// global path shared with Pi.
pub fn uninstall_omp_with_patch_mode(
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    let path = omp_extension_path_for_scope(global)?;

    if !path.exists() {
        if dry_run {
            print_dry_run_footer();
        } else {
            println!("RTK OMP extension was not installed (nothing to remove)");
        }
        return Ok(());
    }

    let ownership_state_path = shared_agent_state_path(&path);
    let Some(content) = read_extension_for_uninstall(&path, "OMP extension", ctx)? else {
        return Ok(());
    };

    if is_known_stock_pi_plugin(&content) {
        if !confirm_shared_extension_uninstall(
            global,
            &path,
            PiCompatibleAgent::Omp,
            patch_mode,
            ctx,
        )? {
            if dry_run {
                print_dry_run_footer();
                return Ok(());
            }
            anyhow::bail!(
                "Shared Pi/OMP extension at {} was not removed; rerun with --auto-patch to approve the removal.",
                path.display()
            );
        }

        if dry_run {
            println!("[dry-run] would remove OMP extension: {}", path.display());
            remove_managed_agent_state(&ownership_state_path, ctx)?;
            print_dry_run_footer();
        } else {
            // nosemgrep: filesystem-deletion -- OMP uninstall removes only the RTK-managed extension file.
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove OMP extension: {}", path.display()))?;
            remove_managed_agent_state(&ownership_state_path, ctx)?;
            if verbose > 0 {
                eprintln!("Removed OMP extension: {}", path.display());
            }
            println!("RTK uninstalled (OMP):");
            println!("  - Extension: {}", path.display());
            println!("\nRestart OMP to apply changes.");
        }
    } else if looks_like_rtk_pi_plugin(&content) {
        if dry_run {
            println!(
                "[dry-run] would refuse to remove OMP extension: {}",
                path.display()
            );
            print_dry_run_footer();
            return Ok(());
        }
        anyhow::bail!(
            "OMP extension at {} contains RTK content that does not match the stock extension. Remove the file manually.",
            path.display()
        );
    } else {
        println!(
            "OMP extension at {} is not RTK content; leaving it alone.",
            path.display()
        );
        if dry_run {
            print_dry_run_footer();
        }
    }

    Ok(())
}

/// Show OMP configuration status.
pub(crate) fn show_omp_config() -> Result<()> {
    let global_extension = omp_extension_path_for_scope(true)?;
    let project_extension = omp_extension_path_for_scope(false)?;

    println!("rtk Configuration (Oh My Pi):\n");
    print_omp_extension_status("Global extension", &global_extension)?;
    print_omp_extension_status("Project extension", &project_extension)?;

    println!("\nUsage:");
    println!("  rtk init --agent omp                 # Configure ./.omp/extensions/rtk.ts");
    println!(
        "  rtk init -g --agent omp              # Configure {}",
        global_extension.display()
    );
    println!("  rtk init --agent omp --uninstall     # Remove project OMP RTK extension");
    println!("  rtk init -g --agent omp --uninstall  # Remove global OMP RTK extension");

    Ok(())
}

pub(crate) fn print_omp_extension_status(label: &str, path: &Path) -> Result<()> {
    if path.exists() {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => {
                println!("  {}: {} (unreadable)", label, path.display());
                return Ok(());
            }
        };
        if is_current_pi_plugin(&content) {
            println!("  {}: {} (up to date)", label, path.display());
        } else if is_known_stock_pi_plugin(&content) {
            println!(
                "  {}: {} (stock version - will be replaced on next rtk init)",
                label,
                path.display()
            );
        } else if looks_like_rtk_pi_plugin(&content) {
            println!(
                "  {}: {} (modified RTK content - rtk init will ask before overwriting; use --auto-patch to replace)",
                label,
                path.display()
            );
        } else {
            println!(
                "  {}: {} (unrelated content - rtk init will ask before overwriting; use --auto-patch to replace)",
                label,
                path.display()
            );
        }
    } else {
        println!("  {}: {} (not installed)", label, path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_omp_extension_path_for_scope_local() {
        let path = omp_extension_path_for_scope(false).unwrap();
        assert_eq!(
            path,
            PathBuf::from(OMP_LOCAL_DIR)
                .join(PI_EXTENSIONS_SUBDIR)
                .join(PI_PLUGIN_FILE)
        );
    }

    #[test]
    fn test_omp_extension_path_for_scope_global_honours_pi_dir_override() {
        let tmp = TempDir::new().unwrap();
        with_omp_dir_override(&tmp, |omp_dir| {
            let path = omp_extension_path_for_scope(true).unwrap();
            assert_eq!(
                path,
                omp_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE)
            );
        });
    }

    #[test]
    fn test_omp_global_install_and_uninstall_use_override() {
        let tmp = TempDir::new().unwrap();
        with_omp_dir_override(&tmp, |omp_dir| {
            run_omp_mode(true, InitContext::default()).unwrap();

            let plugin = omp_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE);
            assert!(plugin.exists(), "global OMP extension must be created");
            let state_path = shared_agent_state_path(&plugin);
            assert_eq!(
                fs::read_to_string(&state_path).unwrap(),
                "omp\n",
                "OMP install must record its ownership"
            );

            uninstall_with_patch_mode(
                true,
                false,
                false,
                false,
                false,
                true,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(!plugin.exists(), "global OMP extension must be removed");
            assert!(!state_path.exists(), "ownership state must be removed");
        });
    }

    #[test]
    fn test_omp_local_install_writes_shared_pi_extension() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        run_omp_mode(false, InitContext::default()).unwrap();
        std::env::set_current_dir(&cwd).unwrap();

        let path = tmp
            .path()
            .join(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content.trim(), PI_PLUGIN.trim());
    }

    #[test]
    fn test_omp_install_refuses_modified_extension() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        let modified = "// user-modified extension\nexport default () => {}\n";
        fs::write(&path, modified).unwrap();

        let result = run_omp_mode_with_patch_mode(false, PatchMode::Skip, InitContext::default());
        std::env::set_current_dir(&cwd).unwrap();

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("was not changed"),
            "unexpected error: {}",
            err
        );
        assert_eq!(fs::read_to_string(path).unwrap(), modified);
    }

    #[test]
    fn test_omp_install_dry_run_reports_refusal_without_error() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        let modified = "// user-modified extension\nexport default () => {}\n";
        fs::write(&path, modified).unwrap();

        let result = run_omp_mode(
            false,
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        );
        std::env::set_current_dir(&cwd).unwrap();

        result.unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), modified);
    }

    #[test]
    fn test_omp_local_install_dry_run_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        run_omp_mode(
            false,
            InitContext {
                verbose: 0,
                dry_run: true,
            },
        )
        .unwrap();
        std::env::set_current_dir(&cwd).unwrap();

        let path = tmp
            .path()
            .join(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        assert!(!path.exists());
        assert!(!tmp.path().join(OMP_LOCAL_DIR).exists());
    }

    #[test]
    fn test_omp_local_uninstall_removes_plugin() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        run_omp_mode(false, InitContext::default()).unwrap();
        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
            InitContext::default(),
        );
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();

        let path = tmp
            .path()
            .join(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        assert!(!path.exists());
    }

    #[test]
    fn test_omp_local_uninstall_dry_run_keeps_plugin() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        run_omp_mode(false, InitContext::default()).unwrap();
        let plugin = tmp
            .path()
            .join(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        assert!(
            plugin.exists(),
            "plugin must exist before uninstall dry-run"
        );

        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
            InitContext {
                verbose: 0,
                dry_run: true,
            },
        );
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();

        assert!(
            plugin.exists(),
            "dry-run uninstall must not remove the local OMP extension"
        );
    }

    #[test]
    fn test_omp_uninstall_modified_extension_bails() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(
            &path,
            "// user-modified extension\nexport default (pi) => { pi.exec(\"rtk\", [\"rewrite\", cmd]) }\n",
        )
        .unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
            InitContext::default(),
        );
        std::env::set_current_dir(&cwd).unwrap();

        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("does not match the stock extension"),
            "unexpected error: {}",
            err
        );
        assert!(path.exists(), "modified extension must not be removed");
    }

    #[test]
    fn test_omp_uninstall_modified_extension_dry_run_is_preview() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(
            &path,
            "// user-modified extension\nexport default (pi) => { pi.exec(\"rtk\", [\"rewrite\", cmd]) }\n",
        )
        .unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        );
        std::env::set_current_dir(&cwd).unwrap();

        result.unwrap();
        assert!(path.exists(), "dry-run must preserve modified extension");
    }

    #[test]
    fn test_omp_uninstall_unreadable_extension_is_left_alone() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
            InitContext::default(),
        );
        std::env::set_current_dir(&cwd).unwrap();

        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("could not be read; leaving it alone"),
            "unreadable extension uninstall should fail clearly: {err}"
        );
        assert!(path.exists(), "unreadable extension must be left alone");
    }

    #[test]
    fn test_omp_uninstall_unrelated_content_dry_run_left_alone() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(
            &path,
            "// rtk rewrite is mentioned here\nexport default () => {}\n",
        )
        .unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        );
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();

        assert!(path.exists(), "non-RTK extension must be left in place");
    }

    #[test]
    fn test_omp_uninstall_missing_dry_run_is_noop() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        );
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();
    }
}
