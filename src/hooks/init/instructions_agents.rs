//! Instruction-only agents: configured by writing an instructions file, no hook installed.

use super::*;

/// Agents without a command hook must prefix `rtk` themselves, which only the `full` level
/// teaches, so they always receive `RTK_AWARENESS_FULL`. This prints the one-line note that
/// tells the user why their configured `awareness.level` was not applied.
fn print_instructions_agents_awareness_note(agent: &str, ctx: InitContext) {
    let prefix = if ctx.dry_run { "[dry-run] " } else { "  " };
    let ignored = if ctx.awareness == AwarenessLevel::Full {
        String::new()
    } else {
        format!(
            " (config awareness.level = \"{}\" does not apply here)",
            ctx.awareness
        )
    };
    println!(
        "{prefix}Awareness: full — {agent} has no command hook, so the agent is told to prefix rtk itself{ignored}"
    );
}

// Cline / Roo Code support

pub(super) fn run_cline_mode(ctx: InitContext) -> Result<()> {
    let _scope = ProjectScope::enter(ctx);
    let InitContext { dry_run, .. } = ctx;
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
            RTK_AWARENESS_FULL.to_string()
        } else {
            format!("{}\n\n{}", existing.trim(), RTK_AWARENESS_FULL)
        };
        let report = Report::new(format!(
            "[dry-run] would write .clinerules: {}",
            rules_path.display()
        ))
        .with_content()
        .done_verbose("Wrote .clinerules");
        write_reported(
            &rules_path,
            WriteKind::Instructions,
            &new_content,
            ctx,
            report,
        )
        .context("Failed to write .clinerules")?;
        if !dry_run {
            println!("\nRTK configured for Cline.\n");
            println!("  Rules: .clinerules (installed)");
        }
    }
    print_instructions_agents_awareness_note("Cline", ctx);
    if !dry_run {
        println!("  Cline will now use rtk commands for token savings.");
        println!("  Test with: git status\n");
    }

    Ok(())
}

pub(super) fn run_windsurf_mode(ctx: InitContext) -> Result<()> {
    let _scope = ProjectScope::enter(ctx);
    let InitContext { dry_run, .. } = ctx;
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
            RTK_AWARENESS_FULL.to_string()
        } else {
            format!("{}\n\n{}", existing.trim(), RTK_AWARENESS_FULL)
        };
        let report = Report::new(format!(
            "[dry-run] would write .windsurfrules: {}",
            rules_path.display()
        ))
        .with_content()
        .done_verbose("Wrote .windsurfrules");
        write_reported(
            &rules_path,
            WriteKind::Instructions,
            &new_content,
            ctx,
            report,
        )
        .context("Failed to write .windsurfrules")?;
        if !dry_run {
            println!("\nRTK configured for Windsurf Cascade.\n");
            println!("  Rules: .windsurfrules (installed)");
        }
    }
    print_instructions_agents_awareness_note("Windsurf", ctx);
    if !dry_run {
        println!("  Cascade will now use rtk commands for token savings.");
        println!("  Restart Windsurf. Test with: git status\n");
    }

    Ok(())
}

// Kilo Code support

pub fn run_kilocode_mode(ctx: InitContext) -> Result<()> {
    let _scope = ProjectScope::enter(ctx);
    run_kilocode_mode_at(&std::env::current_dir()?, ctx)
}

fn run_kilocode_mode_at(base_dir: &Path, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
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
            RTK_AWARENESS_FULL.to_string()
        } else {
            format!("{}\n\n{}", existing.trim(), RTK_AWARENESS_FULL)
        };
        let report = Report::new(format!(
            "[dry-run] would write {}: (and create parent dir if missing)",
            rules_path.display()
        ))
        .with_content()
        .done_verbose("Wrote .kilocode/rules/rtk-rules.md");
        write_reported(&rules_path, WriteKind::Owned, &new_content, ctx, report)
            .context("Failed to write .kilocode/rules/rtk-rules.md")?;
        if !dry_run {
            println!("\nRTK configured for Kilo Code.\n");
            println!("  Rules: .kilocode/rules/rtk-rules.md (installed)");
        }
    }
    print_instructions_agents_awareness_note("Kilo Code", ctx);
    if dry_run {
        print_dry_run_footer();
    } else {
        println!("  Kilo Code will now use rtk commands for token savings.");
        println!("  Test with: git status\n");
    }

    Ok(())
}

// Google Antigravity support

pub fn run_antigravity_mode(ctx: InitContext) -> Result<()> {
    let _scope = ProjectScope::enter(ctx);
    run_antigravity_mode_at(&std::env::current_dir()?, ctx)
}

fn run_antigravity_mode_at(base_dir: &Path, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
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
            RTK_AWARENESS_FULL.to_string()
        } else {
            format!("{}\n\n{}", existing.trim(), RTK_AWARENESS_FULL)
        };
        let report = Report::new(format!(
            "[dry-run] would write {}: (and create parent dir if missing)",
            rules_path.display()
        ))
        .with_content()
        .done_verbose("Wrote .agents/rules/antigravity-rtk-rules.md");
        write_reported(&rules_path, WriteKind::Owned, &new_content, ctx, report)
            .context("Failed to write .agents/rules/antigravity-rtk-rules.md")?;
        if !dry_run {
            println!("\nRTK configured for Google Antigravity.\n");
            println!("  Rules: .agents/rules/antigravity-rtk-rules.md (installed)");
        }
    }
    print_instructions_agents_awareness_note("Antigravity", ctx);
    if dry_run {
        print_dry_run_footer();
    } else {
        println!("  Antigravity will now use rtk commands for token savings.");
        println!("  Test with: git status\n");
    }

    Ok(())
}

// Kimi AI support
//
// Kimi Code CLI has NO `.kimirules` convention — that file is never read.
// It loads project-level instructions from `AGENTS.md` in the project root
// (docs: kimi.com/help/kimi-code/cli-customization). Its PreToolUse hooks are
// gate-only (allow/deny + feedback string) and cannot rewrite a command, so
// `git status` -> `rtk git status` is impossible via a hook. We therefore
// inject an RTK instructions block into AGENTS.md — same mechanism as Codex.

pub fn run_kimi_mode(ctx: InitContext) -> Result<()> {
    let _scope = ProjectScope::enter(ctx);
    run_kimi_mode_at(&std::env::current_dir()?, ctx)
}

fn run_kimi_mode_at(base_dir: &Path, ctx: InitContext) -> Result<()> {
    // Kimi reads AGENTS.md from the project root (workspace-scoped).
    let agents_md_path = base_dir.join(AGENTS_MD);

    write_rtk_block(
        &agents_md_path,
        &rtk_block(RTK_AWARENESS_FULL),
        "RTK instructions",
        "rtk init --agent kimi",
        ctx,
    )?;

    print_instructions_agents_awareness_note("Kimi AI", ctx);
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
    use super::codex::{codex_rtk_md_content, run_codex_mode_with_paths};
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_kilocode_rules_file_is_rtks_own_and_takes_no_backup() {
        let temp = TempDir::new().unwrap();
        let rules = temp.path().join(".kilocode/rules/rtk-rules.md");
        fs::create_dir_all(rules.parent().unwrap()).unwrap();
        fs::write(&rules, "team notes\n").unwrap();

        run_kilocode_mode_at(temp.path(), InitContext::default()).unwrap();

        assert!(fs::read_to_string(&rules).unwrap().contains("rtk"));
        assert!(
            !temp
                .path()
                .join(".kilocode/rules/rtk-rules.md.bak")
                .exists()
        );
    }

    #[test]
    fn test_kilocode_mode_creates_rules_file() {
        let temp = TempDir::new().unwrap();
        run_kilocode_mode_at(temp.path(), InitContext::default()).unwrap();

        let rules_path = temp.path().join(".kilocode/rules/rtk-rules.md");
        assert!(rules_path.exists(), "Rules file should be created");
        let content = fs::read_to_string(&rules_path).unwrap();
        assert_eq!(content, RTK_AWARENESS_FULL);
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
    #[test]
    fn test_agents_get_appropriate_awareness_at_every_level() {
        for level in [
            AwarenessLevel::Default,
            AwarenessLevel::High,
            AwarenessLevel::Full,
        ] {
            let ctx = InitContext {
                awareness: level,
                ..Default::default()
            };
            let temp = TempDir::new().unwrap();

            run_kilocode_mode_at(temp.path(), ctx).unwrap();
            assert_eq!(
                fs::read_to_string(temp.path().join(".kilocode/rules/rtk-rules.md")).unwrap(),
                RTK_AWARENESS_FULL,
                "kilocode with level {level}"
            );

            run_antigravity_mode_at(temp.path(), ctx).unwrap();
            assert_eq!(
                fs::read_to_string(temp.path().join(".agents/rules/antigravity-rtk-rules.md"))
                    .unwrap(),
                RTK_AWARENESS_FULL,
                "antigravity with level {level}"
            );

            run_kimi_mode_at(temp.path(), ctx).unwrap();
            let agents_md = fs::read_to_string(temp.path().join(AGENTS_MD)).unwrap();
            assert!(
                agents_md.contains(RTK_AWARENESS_FULL),
                "kimi with level {level}"
            );

            let codex_agents = temp.path().join("codex").join(AGENTS_MD);
            let codex_rtk = temp.path().join("codex").join(RTK_MD);
            fs::create_dir_all(temp.path().join("codex")).unwrap();
            run_codex_mode_with_paths(
                codex_agents,
                codex_rtk.clone(),
                temp.path().join("codex/hooks.json"),
                false,
                ctx,
            )
            .unwrap();
            assert_eq!(
                fs::read_to_string(&codex_rtk).unwrap(),
                codex_rtk_md_content(level),
                "codex with level {level}"
            );
        }
    }
}
