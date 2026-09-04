//! Claude agent: hook install/uninstall helpers.

use super::*;

/// Legacy mode: full 137-line injection into CLAUDE.md
pub(crate) fn run_claude_md_mode(
    global: bool,
    install_opencode: bool,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    let path = if global {
        resolve_claude_dir()?.join(CLAUDE_MD)
    } else {
        PathBuf::from(CLAUDE_MD)
    };

    if global && !dry_run {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
    }

    if verbose > 0 {
        eprintln!("Writing rtk instructions to: {}", path.display());
    }

    let recovery_cmd = if global {
        "rtk init -g --claude-md"
    } else {
        "rtk init --claude-md"
    };

    let action = write_rtk_block(
        &path,
        RTK_INSTRUCTIONS,
        "rtk instructions",
        recovery_cmd,
        ctx,
    )?;

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

// --- upsert_rtk_block: idempotent RTK block management ---

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RtkBlockUpsert {
    /// No existing block found — appended new block
    Added,
    /// Existing block found with different content — replaced
    Updated,
    /// Existing block found with identical content — no-op
    Unchanged,
    /// Opening marker found without closing marker — not safe to rewrite
    Malformed,
}

/// Insert or replace the RTK instructions block in `content`.
///
/// Returns `(new_content, action)` describing what happened.
/// The caller decides whether to write `new_content` based on `action`.
pub(crate) fn upsert_rtk_block(content: &str, block: &str) -> (String, RtkBlockUpsert) {
    let start_marker = RTK_BLOCK_START;
    let end_marker = RTK_BLOCK_END;

    if let Some(start) = content.find(start_marker) {
        if let Some(relative_end) = content[start..].find(end_marker) {
            let end = start + relative_end;
            let end_pos = end + end_marker.len();
            let current_block = content[start..end_pos].trim();
            let desired_block = block.trim();

            if current_block == desired_block {
                return (content.to_string(), RtkBlockUpsert::Unchanged);
            }

            // Replace stale block with desired block
            let before = content[..start].trim_end();
            let after = content[end_pos..].trim_start();

            let result = match (before.is_empty(), after.is_empty()) {
                (true, true) => desired_block.to_string(),
                (true, false) => format!("{desired_block}\n\n{after}"),
                (false, true) => format!("{before}\n\n{desired_block}"),
                (false, false) => format!("{before}\n\n{desired_block}\n\n{after}"),
            };

            return (result, RtkBlockUpsert::Updated);
        }

        // Opening marker without closing marker — malformed
        return (content.to_string(), RtkBlockUpsert::Malformed);
    }

    // No existing block — append
    let trimmed = content.trim();
    if trimmed.is_empty() {
        (block.to_string(), RtkBlockUpsert::Added)
    } else {
        (
            format!("{trimmed}\n\n{}", block.trim()),
            RtkBlockUpsert::Added,
        )
    }
}

/// Idempotently write an RTK-owned marker block into `path`, preserving user content.
///
/// Reads the file (if any), passes it through [`upsert_rtk_block`], and writes the
/// result back via [`atomic_write`]. Refuses to modify files containing an opening
/// marker without a matching closing marker (bails with a diagnostic and the exact
/// `recovery_cmd` to re-run after manual cleanup).
///
/// Returns the [`RtkBlockUpsert`] action so callers can branch on whether anything
/// was actually changed (e.g., to skip post-install steps on `Unchanged`).
///
/// `label` is shown in user-facing messages (e.g., `"rtk instructions"`,
/// `"Copilot instructions"`).
pub(crate) fn write_rtk_block(
    path: &Path,
    block: &str,
    label: &str,
    recovery_cmd: &str,
    ctx: InitContext,
) -> Result<RtkBlockUpsert> {
    let InitContext { dry_run, .. } = ctx;

    let existing = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?
    } else {
        String::new()
    };

    let (new_content, action) = upsert_rtk_block(&existing, block);

    match action {
        RtkBlockUpsert::Added => {
            if dry_run {
                println!("[dry-run] would add {} to {}", label, path.display());
            } else {
                atomic_write(path, &new_content)
                    .with_context(|| format!("Failed to write {}", path.display()))?;
                println!("[ok] Added {} to {}", label, path.display());
            }
        }
        RtkBlockUpsert::Updated => {
            if dry_run {
                println!("[dry-run] would update {} in {}", label, path.display());
            } else {
                atomic_write(path, &new_content)
                    .with_context(|| format!("Failed to write {}", path.display()))?;
                println!("[ok] Updated {} in {}", label, path.display());
            }
        }
        RtkBlockUpsert::Unchanged => {
            if !dry_run {
                println!("[ok] {} already up to date in {}", label, path.display());
            }
        }
        RtkBlockUpsert::Malformed => {
            eprintln!(
                "[warn] Found '{}' without closing marker in {}",
                RTK_BLOCK_START,
                path.display()
            );
            if let Some((line_num, _)) = existing
                .lines()
                .enumerate()
                .find(|(_, line)| line.contains(RTK_BLOCK_START))
            {
                eprintln!("    Location: line {}", line_num + 1);
            }
            eprintln!("    Action: Manually remove the incomplete block, then re-run:");
            eprintln!("            {recovery_cmd}");
            anyhow::bail!(
                "Refusing to modify malformed {} at {}",
                label,
                path.display()
            );
        }
    }

    Ok(action)
}

/// Patch CLAUDE.md: add @RTK.md, migrate if old block exists
pub(crate) fn patch_claude_md(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext { verbose, dry_run } = ctx;
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
pub(crate) fn patch_agents_md(path: &Path, rtk_md_ref: &str, ctx: InitContext) -> Result<bool> {
    let InitContext { verbose, dry_run } = ctx;
    let mut content = if path.exists() {
        fs::read_to_string(path)
            .with_context(|| format!("Failed to read AGENTS.md: {}", path.display()))?
    } else {
        String::new()
    };

    let mut migrated = false;
    if content.contains(RTK_BLOCK_START) {
        let (new_content, did_migrate) = remove_rtk_block(&content);
        if did_migrate {
            content = new_content;
            migrated = true;
            if verbose > 0 {
                eprintln!("Migrated: removed old RTK block from AGENTS.md");
            }
        }
    }

    // ISSUE #892: Check for both relative and absolute @RTK.md references
    if content.contains(RTK_MD_REF) || content.contains(rtk_md_ref) {
        if verbose > 0 {
            eprintln!("{} reference already present in AGENTS.md", rtk_md_ref);
        }
        // ISSUE #892: Migrate old relative @RTK.md to absolute path if needed
        if rtk_md_ref != RTK_MD_REF && content.contains(RTK_MD_REF) && !content.contains(rtk_md_ref)
        {
            content = content.replace(RTK_MD_REF, rtk_md_ref);
            if dry_run {
                println!(
                    "[dry-run] would migrate {} to {} in {}",
                    RTK_MD_REF,
                    rtk_md_ref,
                    path.display()
                );
            } else {
                atomic_write(path, &content)
                    .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;
                if verbose > 0 {
                    eprintln!("Migrated {} to {}", RTK_MD_REF, rtk_md_ref);
                }
            }
            return Ok(true);
        }
        if migrated {
            if dry_run {
                println!(
                    "[dry-run] would write migrated AGENTS.md: {}",
                    path.display()
                );
            } else {
                atomic_write(path, &content)
                    .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;
            }
        }
        return Ok(false);
    }

    let new_content = if content.is_empty() {
        format!("{}\n", rtk_md_ref)
    } else {
        format!("{}\n\n{}\n", content.trim(), rtk_md_ref)
    };

    if dry_run {
        println!(
            "[dry-run] would add {} reference to AGENTS.md: {}",
            rtk_md_ref,
            path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{}", new_content);
        }
    } else {
        atomic_write(path, &new_content)
            .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;
        if verbose > 0 {
            eprintln!("Added {} reference to AGENTS.md", rtk_md_ref);
        }
    }

    Ok(true)
}

pub(crate) fn has_rtk_reference(content: &str, refs: &[&str]) -> bool {
    content
        .lines()
        .map(str::trim)
        .any(|line| refs.contains(&line))
}

pub(crate) fn remove_rtk_reference_from_agents(
    path: &Path,
    refs: &[&str],
    ctx: InitContext,
) -> Result<bool> {
    let InitContext { verbose, dry_run } = ctx;
    if !path.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read AGENTS.md: {}", path.display()))?;
    if !has_rtk_reference(&content, refs) {
        return Ok(false);
    }

    let new_content = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !refs.contains(&trimmed)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let cleaned = clean_double_blanks(&new_content);

    if dry_run {
        println!(
            "[dry-run] would remove RTK.md reference from AGENTS.md: {}",
            path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{}", cleaned);
        }
        return Ok(true);
    }

    atomic_write(path, &cleaned)
        .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;

    if verbose > 0 {
        eprintln!(
            "Removed RTK.md reference from AGENTS.md: {}",
            path.display()
        );
    }

    Ok(true)
}

/// Remove old RTK block from CLAUDE.md (migration helper)
pub(crate) fn remove_rtk_block(content: &str) -> (String, bool) {
    if let (Some(start), Some(end)) = (content.find(RTK_BLOCK_START), content.find(RTK_BLOCK_END)) {
        let end_pos = end + RTK_BLOCK_END.len();
        let before = content[..start].trim_end();
        let after = content[end_pos..].trim_start();

        let result = if after.is_empty() {
            format!("{}\n", before)
        } else {
            format!("{}\n\n{}", before, after)
        };

        (result, true) // migrated
    } else if content.contains(RTK_BLOCK_START) {
        eprintln!(
            "[warn] Warning: Found '{}' without closing marker.",
            RTK_BLOCK_START
        );
        eprintln!("    This can happen if CLAUDE.md was manually edited.");

        if let Some((line_num, _)) = content
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains(RTK_BLOCK_START))
        {
            eprintln!("    Location: line {}", line_num + 1);
        }

        eprintln!("    Action: Manually remove the incomplete block, then re-run:");
        eprintln!("            rtk init -g");
        (content.to_string(), false)
    } else {
        (content.to_string(), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_mode_creates_rtk_md() {
        let temp = TempDir::new().unwrap();
        let rtk_md_path = temp.path().join("RTK.md");

        fs::write(&rtk_md_path, RTK_SLIM).unwrap();
        assert!(rtk_md_path.exists());

        let content = fs::read_to_string(&rtk_md_path).unwrap();
        assert_eq!(content, RTK_SLIM);
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
    fn test_upsert_rtk_block_appends_when_missing() {
        let input = "# Team instructions";
        let (content, action) = upsert_rtk_block(input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Added);
        assert!(content.contains("# Team instructions"));
        assert!(content.contains(RTK_BLOCK_START));
    }

    #[test]
    fn test_upsert_rtk_block_updates_stale_block() {
        let input = format!(
            "# Team instructions\n\n{} v1 -->\nOLD RTK CONTENT\n{}\n\nMore notes\n",
            RTK_BLOCK_START, RTK_BLOCK_END
        );

        let (content, action) = upsert_rtk_block(&input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Updated);
        assert!(!content.contains("OLD RTK CONTENT"));
        assert!(content.contains("rtk cargo test")); // from current RTK_INSTRUCTIONS
        assert!(content.contains("# Team instructions"));
        assert!(content.contains("More notes"));
    }

    #[test]
    fn test_upsert_rtk_block_noop_when_already_current() {
        let input = format!(
            "# Team instructions\n\n{}\n\nMore notes\n",
            RTK_INSTRUCTIONS
        );
        let (content, action) = upsert_rtk_block(&input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Unchanged);
        assert_eq!(content, input);
    }

    #[test]
    fn test_upsert_rtk_block_detects_malformed_block() {
        let input = format!("{} v2 -->\npartial", RTK_BLOCK_START);
        let (content, action) = upsert_rtk_block(&input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Malformed);
        assert_eq!(content, input);
    }

    #[test]
    fn test_patch_agents_md_adds_reference_once() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");

        fs::write(&agents_md, "# Team rules\n").unwrap();
        let first_added = patch_agents_md(&agents_md, RTK_MD_REF, InitContext::default()).unwrap();
        let second_added = patch_agents_md(&agents_md, RTK_MD_REF, InitContext::default()).unwrap();

        assert!(first_added);
        assert!(!second_added);

        let content = fs::read_to_string(&agents_md).unwrap();
        assert_eq!(content.matches("@RTK.md").count(), 1);
    }

    #[test]
    fn test_patch_agents_md_creates_missing_file() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");

        let added = patch_agents_md(&agents_md, RTK_MD_REF, InitContext::default()).unwrap();

        assert!(added);
        let content = fs::read_to_string(&agents_md).unwrap();
        assert_eq!(content, "@RTK.md\n");
    }

    #[test]
    fn test_patch_agents_md_migrates_inline_block() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");
        fs::write(
            &agents_md,
            format!(
                "# Team rules\n\n{} v2 -->\nold\n{}\n",
                RTK_BLOCK_START, RTK_BLOCK_END
            ),
        )
        .unwrap();

        let added = patch_agents_md(&agents_md, RTK_MD_REF, InitContext::default()).unwrap();

        assert!(added);
        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains("old"));
        assert_eq!(content.matches("@RTK.md").count(), 1);
    }

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
