//! Agents agent: hook install/uninstall helpers.

use super::*;

// ─── Windsurf support ─────────────────────────────────────────

/// Embedded Windsurf RTK rules
pub(crate) const WINDSURF_RULES: &str = include_str!("../../../hooks/windsurf/rules.md");

/// Embedded Cline RTK rules
pub(crate) const CLINE_RULES: &str = include_str!("../../../hooks/cline/rules.md");

// ─── Cline / Roo Code support ─────────────────────────────────

pub(crate) fn run_cline_mode(ctx: InitContext) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    // Cline reads .clinerules from the project root (workspace-scoped)
    let rules_path = PathBuf::from(".clinerules");

    let existing = fs::read_to_string(&rules_path).unwrap_or_default();
    if existing.contains("RTK") || existing.contains("rtk") {
        if !dry_run {
            println!("\nRTK already configured for Cline in this project.\n");
            println!("  Rules: .clinerules (already present)");
        }
    } else {
        let new_content = if existing.trim().is_empty() {
            CLINE_RULES.to_string()
        } else {
            format!("{}\n\n{}", existing.trim(), CLINE_RULES)
        };
        if dry_run {
            println!(
                "[dry-run] would write .clinerules: {}",
                rules_path.display()
            );
            if verbose > 0 {
                println!("[dry-run] content:\n{}", new_content);
            }
        } else {
            fs::write(&rules_path, &new_content).context("Failed to write .clinerules")?;

            if verbose > 0 {
                eprintln!("Wrote .clinerules");
            }

            println!("\nRTK configured for Cline.\n");
            println!("  Rules: .clinerules (installed)");
        }
    }
    if !dry_run {
        println!("  Cline will now use rtk commands for token savings.");
        println!("  Test with: git status\n");
    }

    Ok(())
}

pub(crate) fn run_windsurf_mode(ctx: InitContext) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    // Windsurf reads .windsurfrules from the project root (workspace-scoped).
    // Global rules (~/.codeium/windsurf/memories/global_rules.md) are unreliable.
    let rules_path = PathBuf::from(".windsurfrules");

    let existing = fs::read_to_string(&rules_path).unwrap_or_default();
    if existing.contains("RTK") || existing.contains("rtk") {
        if !dry_run {
            println!("\nRTK already configured for Windsurf in this project.\n");
            println!("  Rules: .windsurfrules (already present)");
        }
    } else {
        let new_content = if existing.trim().is_empty() {
            WINDSURF_RULES.to_string()
        } else {
            format!("{}\n\n{}", existing.trim(), WINDSURF_RULES)
        };
        if dry_run {
            println!(
                "[dry-run] would write .windsurfrules: {}",
                rules_path.display()
            );
            if verbose > 0 {
                println!("[dry-run] content:\n{}", new_content);
            }
        } else {
            fs::write(&rules_path, &new_content).context("Failed to write .windsurfrules")?;

            if verbose > 0 {
                eprintln!("Wrote .windsurfrules");
            }

            println!("\nRTK configured for Windsurf Cascade.\n");
            println!("  Rules: .windsurfrules (installed)");
        }
    }
    if !dry_run {
        println!("  Cascade will now use rtk commands for token savings.");
        println!("  Restart Windsurf. Test with: git status\n");
    }

    Ok(())
}

// ─── Kilo Code support ────────────────────────────────────────

pub(crate) const KILOCODE_RULES: &str = include_str!("../../../hooks/kilocode/rules.md");

pub fn run_kilocode_mode(ctx: InitContext) -> Result<()> {
    run_kilocode_mode_at(&std::env::current_dir()?, ctx)
}

pub(crate) fn run_kilocode_mode_at(base_dir: &Path, ctx: InitContext) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    // Kilo Code reads .kilocode/rules/ from the project root (workspace-scoped)
    let target_dir = base_dir.join(".kilocode/rules");
    let rules_path = target_dir.join("rtk-rules.md");

    let existing = fs::read_to_string(&rules_path).unwrap_or_default();
    if existing.contains("RTK") || existing.contains("rtk") {
        if !dry_run {
            println!("\nRTK already configured for Kilo Code in this project.\n");
            println!("  Rules: .kilocode/rules/rtk-rules.md (already present)");
        }
    } else {
        let new_content = if existing.trim().is_empty() {
            KILOCODE_RULES.to_string()
        } else {
            format!("{}\n\n{}", existing.trim(), KILOCODE_RULES)
        };
        if dry_run {
            println!(
                "[dry-run] would write {}: (and create parent dir if missing)",
                rules_path.display()
            );
            if verbose > 0 {
                println!("[dry-run] content:\n{}", new_content);
            }
        } else {
            fs::create_dir_all(&target_dir)
                .context("Failed to create .kilocode/rules directory")?;
            fs::write(&rules_path, &new_content)
                .context("Failed to write .kilocode/rules/rtk-rules.md")?;

            if verbose > 0 {
                eprintln!("Wrote .kilocode/rules/rtk-rules.md");
            }

            println!("\nRTK configured for Kilo Code.\n");
            println!("  Rules: .kilocode/rules/rtk-rules.md (installed)");
        }
    }
    if dry_run {
        print_dry_run_footer();
    } else {
        println!("  Kilo Code will now use rtk commands for token savings.");
        println!("  Test with: git status\n");
    }

    Ok(())
}

// ─── Google Antigravity support ───────────────────────────────

pub(crate) const ANTIGRAVITY_RULES: &str = include_str!("../../../hooks/antigravity/rules.md");

pub fn run_antigravity_mode(ctx: InitContext) -> Result<()> {
    run_antigravity_mode_at(&std::env::current_dir()?, ctx)
}

pub(crate) fn run_antigravity_mode_at(base_dir: &Path, ctx: InitContext) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    // Antigravity reads .agents/rules/ from the project root (workspace-scoped)
    let target_dir = base_dir.join(".agents/rules");
    let rules_path = target_dir.join("antigravity-rtk-rules.md");

    let existing = fs::read_to_string(&rules_path).unwrap_or_default();
    if existing.contains("RTK") || existing.contains("rtk") {
        if !dry_run {
            println!("\nRTK already configured for Antigravity in this project.\n");
            println!("  Rules: .agents/rules/antigravity-rtk-rules.md (already present)");
        }
    } else {
        let new_content = if existing.trim().is_empty() {
            ANTIGRAVITY_RULES.to_string()
        } else {
            format!("{}\n\n{}", existing.trim(), ANTIGRAVITY_RULES)
        };
        if dry_run {
            println!(
                "[dry-run] would write {}: (and create parent dir if missing)",
                rules_path.display()
            );
            if verbose > 0 {
                println!("[dry-run] content:\n{}", new_content);
            }
        } else {
            fs::create_dir_all(&target_dir).context("Failed to create .agents/rules directory")?;
            fs::write(&rules_path, &new_content)
                .context("Failed to write .agents/rules/antigravity-rtk-rules.md")?;

            if verbose > 0 {
                eprintln!("Wrote .agents/rules/antigravity-rtk-rules.md");
            }

            println!("\nRTK configured for Google Antigravity.\n");
            println!("  Rules: .agents/rules/antigravity-rtk-rules.md (installed)");
        }
    }
    if dry_run {
        print_dry_run_footer();
    } else {
        println!("  Antigravity will now use rtk commands for token savings.");
        println!("  Test with: git status\n");
    }

    Ok(())
}

// ─── Kimi AI support ──────────────────────────────────────────
//
// Kimi Code CLI has NO `.kimirules` convention — that file is never read.
// It loads project-level instructions from `AGENTS.md` in the project root
// (docs: kimi.com/help/kimi-code/cli-customization). Its PreToolUse hooks are
// gate-only (allow/deny + feedback string) and cannot rewrite a command, so
// `git status` -> `rtk git status` is impossible via a hook. We therefore
// inject an RTK instructions block into AGENTS.md — same mechanism as Codex.

pub fn run_kimi_mode(ctx: InitContext) -> Result<()> {
    run_kimi_mode_at(&std::env::current_dir()?, ctx)
}

pub(crate) fn run_kimi_mode_at(base_dir: &Path, ctx: InitContext) -> Result<()> {
    // Kimi reads AGENTS.md from the project root (workspace-scoped).
    let agents_md_path = base_dir.join(AGENTS_MD);

    write_rtk_block(
        &agents_md_path,
        RTK_INSTRUCTIONS,
        "RTK instructions",
        "rtk init --agent kimi",
        ctx,
    )?;

    if !ctx.dry_run {
        println!("\nRTK configured for Kimi AI.\n");
        println!("  AGENTS.md: {}", agents_md_path.display());
        println!("  Kimi AI will now use rtk commands for token savings.");
        println!("  Test with: git status\n");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_kilocode_mode_creates_rules_file() {
        let temp = TempDir::new().unwrap();
        run_kilocode_mode_at(temp.path(), InitContext::default()).unwrap();

        let rules_path = temp.path().join(".kilocode/rules/rtk-rules.md");
        assert!(rules_path.exists(), "Rules file should be created");
        let content = fs::read_to_string(&rules_path).unwrap();
        assert!(content.contains("RTK"), "Rules file should contain RTK");
    }

    #[test]
    fn test_kilocode_mode_is_idempotent() {
        let temp = TempDir::new().unwrap();
        run_kilocode_mode_at(temp.path(), InitContext::default()).unwrap();

        let path = temp.path().join(".kilocode/rules/rtk-rules.md");
        let first = fs::read_to_string(&path).unwrap();

        // Second run should not overwrite
        run_kilocode_mode_at(temp.path(), InitContext::default()).unwrap();
        let second = fs::read_to_string(&path).unwrap();
        assert_eq!(first, second, "Idempotent: content should not change");
    }

    #[test]
    fn test_antigravity_mode_creates_rules_file() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(temp.path(), InitContext::default()).unwrap();

        let rules_path = temp.path().join(".agents/rules/antigravity-rtk-rules.md");
        assert!(rules_path.exists(), "Rules file should be created");
        let content = fs::read_to_string(&rules_path).unwrap();
        assert!(content.contains("RTK"), "Rules file should contain RTK");
    }

    #[test]
    fn test_antigravity_mode_is_idempotent() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(temp.path(), InitContext::default()).unwrap();

        let path = temp.path().join(".agents/rules/antigravity-rtk-rules.md");
        let first = fs::read_to_string(&path).unwrap();

        // Second run should not overwrite
        run_antigravity_mode_at(temp.path(), InitContext::default()).unwrap();
        let second = fs::read_to_string(&path).unwrap();
        assert_eq!(first, second, "Idempotent: content should not change");
    }

    #[test]
    fn test_kimi_mode_writes_agents_md() {
        let temp = TempDir::new().unwrap();
        run_kimi_mode_at(temp.path(), InitContext::default()).unwrap();

        // Kimi reads AGENTS.md, NOT .kimirules (which it does not support).
        let agents_md = temp.path().join("AGENTS.md");
        assert!(agents_md.exists(), "AGENTS.md should be created");
        assert!(
            !temp.path().join(".kimirules").exists(),
            ".kimirules must not be created (unsupported by kimi-cli)"
        );
        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(
            content.contains(RTK_BLOCK_START),
            "AGENTS.md should contain the RTK instructions block"
        );
    }

    #[test]
    fn test_kimi_mode_is_idempotent() {
        let temp = TempDir::new().unwrap();
        run_kimi_mode_at(temp.path(), InitContext::default()).unwrap();

        let path = temp.path().join("AGENTS.md");
        let first = fs::read_to_string(&path).unwrap();

        // Second run is an upsert no-op.
        run_kimi_mode_at(temp.path(), InitContext::default()).unwrap();
        let second = fs::read_to_string(&path).unwrap();
        assert_eq!(first, second, "Idempotent: content should not change");
    }
}
