//! Codex agent: hook install/uninstall helpers.

use super::*;

pub(crate) fn uninstall_codex(global: bool, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !global {
        anyhow::bail!(
            "Uninstall only works with --global flag. For local projects, manually remove RTK from AGENTS.md"
        );
    }

    let codex_dir = resolve_codex_dir()?;
    let removed = uninstall_codex_at(&codex_dir, ctx)?;

    if removed.is_empty() {
        println!("RTK was not installed for Codex CLI (nothing to remove)");
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK for Codex CLI:"
        } else {
            "RTK uninstalled for Codex CLI:"
        };
        println!("{}", header);
        for item in removed {
            println!("  - {}", item);
        }
    }

    Ok(())
}

pub(crate) fn uninstall_codex_at(codex_dir: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext { verbose, dry_run } = ctx;
    let mut removed = Vec::new();
    let absolute_rtk_md_ref = codex_rtk_md_ref(codex_dir);

    let rtk_md_path = codex_dir.join(RTK_MD);
    if rtk_md_path.exists() {
        if dry_run {
            println!("[dry-run] would remove RTK.md: {}", rtk_md_path.display());
        } else {
            fs::remove_file(&rtk_md_path)
                .with_context(|| format!("Failed to remove RTK.md: {}", rtk_md_path.display()))?;
            if verbose > 0 {
                eprintln!("Removed RTK.md: {}", rtk_md_path.display());
            }
        }
        removed.push(format!("RTK.md: {}", rtk_md_path.display()));
    }

    let agents_md_path = codex_dir.join(AGENTS_MD);
    if agents_md_path.exists() {
        let content = fs::read_to_string(&agents_md_path)
            .with_context(|| format!("Failed to read AGENTS.md: {}", agents_md_path.display()))?;

        let mut working_content = content.clone();
        let mut agents_changed = false;

        if working_content.contains(RTK_BLOCK_START) {
            let (cleaned, did_remove) = remove_rtk_block(&working_content);
            if did_remove {
                working_content = cleaned;
                agents_changed = true;
                removed.push("AGENTS.md: removed rtk-instructions block".to_string());
            }
        }

        if agents_changed {
            atomic_write(&agents_md_path, &working_content).with_context(|| {
                format!("Failed to write AGENTS.md: {}", agents_md_path.display())
            })?;
        }
    }

    if remove_rtk_reference_from_agents(
        &agents_md_path,
        &[RTK_MD_REF, absolute_rtk_md_ref.as_str()],
        ctx,
    )? {
        removed.push("AGENTS.md: removed @RTK.md reference".to_string());
    }

    Ok(removed)
}

pub(crate) fn run_codex_mode(global: bool, ctx: InitContext) -> Result<()> {
    let (agents_md_path, rtk_md_path) = if global {
        let codex_dir = resolve_codex_dir()?;
        (codex_dir.join(AGENTS_MD), codex_dir.join(RTK_MD))
    } else {
        (PathBuf::from(AGENTS_MD), PathBuf::from(RTK_MD))
    };

    run_codex_mode_with_paths(agents_md_path, rtk_md_path, global, ctx)
}

pub(crate) fn run_codex_mode_with_paths(
    agents_md_path: PathBuf,
    rtk_md_path: PathBuf,
    global: bool,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if global && !dry_run {
        if let Some(parent) = agents_md_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create Codex config directory: {}",
                    parent.display()
                )
            })?;
        }
    }

    // ISSUE #892: In global mode, use absolute path so @RTK.md resolves
    // from any CWD (worktrees, nested projects). Codex resolves @ references
    // relative to CWD, not the AGENTS.md file location.
    let rtk_md_ref = if global {
        codex_rtk_md_ref(
            rtk_md_path
                .parent()
                .context("RTK.md path missing parent directory")?,
        )
    } else {
        RTK_MD_REF.to_string()
    };

    write_if_changed(&rtk_md_path, RTK_SLIM_CODEX, RTK_MD, ctx)?;
    let added_ref = patch_agents_md(&agents_md_path, &rtk_md_ref, ctx)?;

    if !dry_run {
        println!("\nRTK configured for Codex CLI.\n");
        println!("  RTK.md:    {}", rtk_md_path.display());
        if added_ref {
            println!("  AGENTS.md: {} reference added", rtk_md_ref);
        } else {
            println!("  AGENTS.md: {} reference already present", rtk_md_ref);
        }
        if global {
            println!(
                "\n  Codex global instructions path: {}",
                agents_md_path.display()
            );
        } else {
            println!(
                "\n  Codex project instructions path: {}",
                agents_md_path.display()
            );
        }
    }

    Ok(())
}

pub(crate) fn resolve_codex_dir() -> Result<PathBuf> {
    resolve_codex_dir_from(
        std::env::var_os("CODEX_HOME").map(PathBuf::from),
        dirs::home_dir(),
    )
}

pub(crate) fn resolve_codex_dir_from(
    codex_home: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = codex_home.filter(|path| !path.as_os_str().is_empty()) {
        return Ok(path);
    }

    home_dir
        .map(|home| home.join(CODEX_DIR))
        .context("Cannot determine Codex config directory. Set $CODEX_HOME or $HOME.")
}

pub(crate) fn codex_rtk_md_ref(codex_dir: &Path) -> String {
    format!("@{}", codex_dir.join(RTK_MD).display())
}

pub(crate) fn show_codex_config() -> Result<()> {
    let codex_dir = resolve_codex_dir()?;
    let global_agents_md = codex_dir.join(AGENTS_MD);
    let global_rtk_md = codex_dir.join(RTK_MD);
    let global_rtk_md_ref = codex_rtk_md_ref(&codex_dir);
    let local_agents_md = PathBuf::from(AGENTS_MD);
    let local_rtk_md = PathBuf::from(RTK_MD);

    println!("rtk Configuration (Codex CLI):\n");

    if global_rtk_md.exists() {
        println!("[ok] Global RTK.md: {}", global_rtk_md.display());
    } else {
        println!("[--] Global RTK.md: not found");
    }

    if global_agents_md.exists() {
        let content = fs::read_to_string(&global_agents_md)?;
        if has_rtk_reference(&content, &[RTK_MD_REF, global_rtk_md_ref.as_str()]) {
            println!("[ok] Global AGENTS.md: RTK.md reference");
        } else if content.contains(RTK_BLOCK_START) {
            println!("[!!] Global AGENTS.md: old inline RTK block");
        } else {
            println!("[--] Global AGENTS.md: exists but rtk not configured");
        }
    } else {
        println!("[--] Global AGENTS.md: not found");
    }

    if local_rtk_md.exists() {
        println!("[ok] Local RTK.md: {}", local_rtk_md.display());
    } else {
        println!("[--] Local RTK.md: not found");
    }

    if local_agents_md.exists() {
        let content = fs::read_to_string(&local_agents_md)?;
        if has_rtk_reference(&content, &[RTK_MD_REF]) {
            println!("[ok] Local AGENTS.md: @RTK.md reference");
        } else if content.contains(RTK_BLOCK_START) {
            println!("[!!] Local AGENTS.md: old inline RTK block");
        } else {
            println!("[--] Local AGENTS.md: exists but rtk not configured");
        }
    } else {
        println!("[--] Local AGENTS.md: not found");
    }

    println!("\nUsage:");
    println!("  rtk init --codex              # Configure local AGENTS.md + RTK.md");
    println!("  rtk init -g --codex           # Configure $CODEX_HOME/AGENTS.md + $CODEX_HOME/RTK.md (or ~/.codex/)");
    println!("  rtk init -g --codex --uninstall  # Remove global Codex RTK artifacts");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_codex_mode_rejects_auto_patch() {
        let err = run(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            PatchMode::Auto,
            InitContext::default(),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--codex cannot be combined with --auto-patch"
        );
    }

    #[test]
    fn test_codex_mode_rejects_no_patch() {
        let err = run(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            PatchMode::Skip,
            InitContext::default(),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--codex cannot be combined with --no-patch"
        );
    }

    #[test]
    fn test_run_codex_mode_global_writes_absolute_reference_to_codex_dir() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");
        let rtk_md = temp.path().join("RTK.md");

        run_codex_mode_with_paths(
            agents_md.clone(),
            rtk_md.clone(),
            true,
            InitContext::default(),
        )
        .unwrap();

        assert!(rtk_md.exists());
        assert_eq!(fs::read_to_string(&rtk_md).unwrap(), RTK_SLIM_CODEX);
        assert_eq!(
            fs::read_to_string(&agents_md).unwrap(),
            format!("{}\n", codex_rtk_md_ref(temp.path()))
        );
    }

    #[test]
    fn test_resolve_codex_dir_prefers_codex_home_and_ignores_empty_value() {
        let codex_home = PathBuf::from("/tmp/custom-codex-home");
        let home_dir = PathBuf::from("/tmp/home");

        let preferred =
            resolve_codex_dir_from(Some(codex_home.clone()), Some(home_dir.clone())).unwrap();
        let empty_falls_back =
            resolve_codex_dir_from(Some(PathBuf::new()), Some(home_dir.clone())).unwrap();
        let missing_falls_back = resolve_codex_dir_from(None, Some(home_dir.clone())).unwrap();

        assert_eq!(preferred, codex_home);
        assert_eq!(empty_falls_back, home_dir.join(".codex"));
        assert_eq!(missing_falls_back, home_dir.join(".codex"));
    }

    #[test]
    fn test_uninstall_codex_at_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let codex_dir = temp.path();
        let agents_md = codex_dir.join("AGENTS.md");
        let rtk_md = codex_dir.join("RTK.md");

        fs::write(&agents_md, "# Team rules\n\n@RTK.md\n").unwrap();
        fs::write(&rtk_md, "codex config").unwrap();

        let removed_first = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();
        let removed_second = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();

        assert_eq!(removed_first.len(), 2);
        assert!(removed_second.is_empty());
        assert!(!rtk_md.exists());

        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains("@RTK.md"));
        assert!(content.contains("# Team rules"));
    }

    #[test]
    fn test_uninstall_codex_at_removes_absolute_reference() {
        let temp = TempDir::new().unwrap();
        let codex_dir = temp.path();
        let agents_md = codex_dir.join("AGENTS.md");
        let rtk_md = codex_dir.join("RTK.md");
        let absolute_ref = codex_rtk_md_ref(codex_dir);

        fs::write(&agents_md, format!("# Team rules\n\n{}\n", absolute_ref)).unwrap();
        fs::write(&rtk_md, "codex config").unwrap();

        let removed = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();

        assert_eq!(removed.len(), 2);
        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains(&absolute_ref));
        assert!(content.contains("# Team rules"));
    }

    #[test]
    fn test_run_codex_mode_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");
        let rtk_md = temp.path().join("RTK.md");

        run_codex_mode_with_paths(
            agents_md.clone(),
            rtk_md.clone(),
            true,
            InitContext {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(
            !rtk_md.exists(),
            "dry-run must not create RTK.md: {}",
            rtk_md.display()
        );
        assert!(
            !agents_md.exists(),
            "dry-run must not create AGENTS.md: {}",
            agents_md.display()
        );
    }

    #[test]
    fn test_uninstall_codex_at_removes_rtk_instructions_block() {
        let temp = TempDir::new().unwrap();
        let codex_dir = temp.path();
        let agents_md = codex_dir.join("AGENTS.md");
        let rtk_md = codex_dir.join("RTK.md");

        fs::write(
            &agents_md,
            format!(
                "# Team rules\n\n{} v2 -->\nOLD RTK STUFF\n{}\n\nMore content",
                RTK_BLOCK_START, RTK_BLOCK_END
            ),
        )
        .unwrap();
        fs::write(&rtk_md, "codex config").unwrap();

        let removed = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();

        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains("OLD RTK STUFF"));
        assert!(content.contains("# Team rules"));
        assert!(content.contains("More content"));
        assert!(removed.iter().any(|r| r.contains("rtk-instructions block")));
    }
}
