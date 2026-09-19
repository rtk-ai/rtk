//! Claude agent: hook install/uninstall helpers.

use super::*;

/// Legacy mode (--claude-md): inject the full RTK_INSTRUCTIONS block into CLAUDE.md.
pub(crate) fn run_claude_md_mode(
    global: bool,
    install_opencode: bool,
    ctx: InitContext,
) -> Result<()> {
    run_claude_md_mode_with(global, install_opencode, RTK_INSTRUCTIONS, ctx)
}

pub(crate) fn run_claude_md_mode_with(
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

    if global
        && !dry_run
        && let Some(parent) = path.parent()
    {
        fs::create_dir_all(parent)?;
    }

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

pub(crate) fn patch_claude_md(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
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
            if dry_run {
                println!(
                    "[dry-run] would migrate old RTK block in CLAUDE.md: {}",
                    path.display()
                );
            } else {
                fs::write(path, content)?;
            }
        }
        return Ok(migrated);
    }

    // Add @RTK.md
    let new_content = if content.is_empty() {
        "@RTK.md\n".to_string()
    } else {
        format!("{}\n\n@RTK.md\n", content.trim())
    };

    if dry_run {
        println!(
            "[dry-run] would add @RTK.md reference to CLAUDE.md: {}",
            path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{}", new_content);
        }
    } else {
        fs::write(path, new_content)?;

        if verbose > 0 {
            eprintln!("Added @RTK.md reference to CLAUDE.md");
        }
    }

    Ok(migrated)
}

/// Patch AGENTS.md: add @RTK.md (or absolute path), migrate old inline block if present
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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

    // --- upsert_rtk_block tests ---
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
        // Mirrors `test_copilot_init_refuses_malformed_block`: a malformed
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
