//! Instruction-only agents: configured by writing an instructions file, no hook installed.

use super::*;
use crate::core::user_dirs;

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

/// Last lines of every rules text RTK wrote before it used a marker block.
const LEGACY_RULES_LAST_LINES: [&str; 3] = [
    "- `rtk discover` — find past commands RTK could have condensed.",
    "RTK filters and compresses command output before it reaches the LLM context, cutting up to 90% of the bash output on common operations. Always use `rtk <cmd>` instead of raw commands.",
    "RTK filters and compresses command output before it reaches the LLM context, saving 60-90% tokens on common operations. Always use `rtk <cmd>` instead of raw commands.",
];

fn is_legacy_rules_heading(line: &str) -> bool {
    let line = line.trim_end();
    line == "# RTK" || line.starts_with("# RTK - Rust Token Killer")
}

/// Earlier `rtk init` appended its rules to the end of the file without markers. Returns the
/// file with that trailing section removed, or `None` when it has none. The section has to open
/// with one of RTK's headings and end the file with the last line of an RTK text, so a user's
/// own mention of RTK, or anything they wrote after RTK's text, is never removed.
fn strip_legacy_rules(content: &str) -> Option<String> {
    let last_line = content.lines().rev().find(|line| !line.trim().is_empty())?;
    if !LEGACY_RULES_LAST_LINES.contains(&last_line.trim_end()) {
        return None;
    }
    let mut offset = 0;
    let mut start = None;
    for line in content.split_inclusive('\n') {
        if is_legacy_rules_heading(line) {
            start = Some(offset);
        }
        offset += line.len();
    }
    Some(content[..start?].trim_end().to_string())
}

/// Writes RTK's rules into a rules file as a marker block: added when missing, refreshed in
/// place when stale, and migrated from the unmarked text earlier versions appended, keeping the
/// user's own text around it.
fn write_rules_file(path: &Path, recovery_cmd: &str, ctx: InitContext) -> Result<RtkBlockUpsert> {
    let existing = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?
    } else {
        String::new()
    };
    let legacy = if existing.contains(RTK_BLOCK_START) {
        None
    } else {
        strip_legacy_rules(&existing)
    };
    let rewrote = legacy.is_some();
    let existing = legacy.unwrap_or(existing);
    write_rtk_block_into(
        path,
        &existing,
        rewrote,
        &rtk_block(RTK_AWARENESS_FULL),
        "RTK rules",
        recovery_cmd,
        ctx,
    )
}

// Cline / Roo Code support

pub(super) fn run_cline_mode(ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    // Cline reads .clinerules from the project root (workspace-scoped)
    let rules_path = PathBuf::from(".clinerules");

    write_rules_file(&rules_path, "rtk init --agent cline", ctx)?;
    if !dry_run {
        println!("\nRTK configured for Cline.\n");
        println!("  Rules: .clinerules");
    }
    print_instructions_agents_awareness_note("Cline", ctx);
    if !dry_run {
        println!("  Cline will now use rtk commands for token savings.");
        println!("  Test with: git status\n");
    }

    Ok(())
}

pub(super) fn run_windsurf_mode(ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    // Windsurf reads .windsurfrules from the project root (workspace-scoped).
    // Global rules (~/.codeium/windsurf/memories/global_rules.md) are unreliable.
    let rules_path = PathBuf::from(".windsurfrules");

    write_rules_file(&rules_path, "rtk init --agent windsurf", ctx)?;
    if !dry_run {
        println!("\nRTK configured for Windsurf Cascade.\n");
        println!("  Rules: .windsurfrules");
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
    run_kilocode_mode_at(&user_dirs::current_dir()?, ctx)
}

fn run_kilocode_mode_at(base_dir: &Path, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    // Kilo Code reads .kilocode/rules/ from the project root (workspace-scoped)
    let target_dir = base_dir.join(".kilocode/rules");
    let rules_path = target_dir.join("rtk-rules.md");

    if !dry_run {
        fs::create_dir_all(&target_dir).context("Failed to create .kilocode/rules directory")?;
    }
    write_rules_file(&rules_path, "rtk init --agent kilocode", ctx)?;
    if !dry_run {
        println!("\nRTK configured for Kilo Code.\n");
        println!("  Rules: .kilocode/rules/rtk-rules.md");
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

// Kimi AI support
//
// Kimi Code CLI has NO `.kimirules` convention — that file is never read.
// It loads project-level instructions from `AGENTS.md` in the project root
// (docs: kimi.com/help/kimi-code/cli-customization). Its PreToolUse hooks are
// gate-only (allow/deny + feedback string) and cannot rewrite a command, so
// `git status` -> `rtk git status` is impossible via a hook. We therefore
// inject an RTK instructions block into AGENTS.md — same mechanism as Codex.

pub fn run_kimi_mode(ctx: InitContext) -> Result<()> {
    run_kimi_mode_at(&user_dirs::current_dir()?, ctx)
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
    fn test_kilocode_mode_creates_rules_file() {
        let temp = TempDir::new().unwrap();
        run_kilocode_mode_at(temp.path(), InitContext::default()).unwrap();

        let rules_path = temp.path().join(".kilocode/rules/rtk-rules.md");
        assert!(rules_path.exists(), "Rules file should be created");
        let content = fs::read_to_string(&rules_path).unwrap();
        assert_eq!(content, rtk_block(RTK_AWARENESS_FULL));
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

    fn rules_file(temp: &TempDir, content: &str) -> PathBuf {
        let path = temp.path().join(".clinerules");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_rules_file_with_unrelated_rtk_mention_gets_block() {
        let temp = TempDir::new().unwrap();
        let user = "# Project rules\n\nPrefer RTK Query for data fetching.\n";
        let path = rules_file(&temp, user);

        let action = write_rules_file(&path, "rtk init", InitContext::default()).unwrap();

        assert_eq!(action, RtkBlockUpsert::Added);
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.starts_with(user.trim_end()),
            "user rules kept: {content}"
        );
        assert!(content.ends_with(rtk_block(RTK_AWARENESS_FULL).trim()));
    }

    #[test]
    fn test_rules_file_with_current_block_is_unchanged() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join(".clinerules");
        write_rules_file(&path, "rtk init", InitContext::default()).unwrap();
        let first = fs::read_to_string(&path).unwrap();

        let action = write_rules_file(&path, "rtk init", InitContext::default()).unwrap();

        assert_eq!(action, RtkBlockUpsert::Unchanged);
        assert_eq!(fs::read_to_string(&path).unwrap(), first);
    }

    #[test]
    fn test_rules_file_with_stale_block_is_refreshed_in_place() {
        let temp = TempDir::new().unwrap();
        let stale = rtk_block("# RTK\n\nOld wording.\n");
        let path = rules_file(
            &temp,
            &format!("# Before\n\n{stale}\n# After\n\nKeep me.\n"),
        );

        let action = write_rules_file(&path, "rtk init", InitContext::default()).unwrap();

        assert_eq!(action, RtkBlockUpsert::Updated);
        let content = fs::read_to_string(&path).unwrap();
        assert!(!content.contains("Old wording."));
        assert!(content.starts_with("# Before\n\n"));
        assert!(content.contains(rtk_block(RTK_AWARENESS_FULL).trim()));
        assert!(content.ends_with("# After\n\nKeep me.\n"));
    }

    #[test]
    fn test_rules_file_with_legacy_unmarked_rules_is_migrated() {
        let temp = TempDir::new().unwrap();
        let legacy = "# RTK - Rust Token Killer (Cline)\n\nUse rtk.\n\n```bash\nrtk gain\n```\n\nRTK filters and compresses command output before it reaches the LLM context, saving 60-90% tokens on common operations. Always use `rtk <cmd>` instead of raw commands.\n";
        let path = rules_file(&temp, &format!("# My rules\n\nBe brief.\n\n{legacy}"));

        let action = write_rules_file(&path, "rtk init", InitContext::default()).unwrap();

        assert_eq!(action, RtkBlockUpsert::Updated);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(
            content,
            format!(
                "# My rules\n\nBe brief.\n\n{}",
                rtk_block(RTK_AWARENESS_FULL).trim()
            )
        );
    }

    #[test]
    fn test_rules_file_with_current_unmarked_rules_is_migrated() {
        let temp = TempDir::new().unwrap();
        let path = rules_file(&temp, RTK_AWARENESS_FULL);

        assert_eq!(
            write_rules_file(&path, "rtk init", InitContext::default()).unwrap(),
            RtkBlockUpsert::Updated
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            rtk_block(RTK_AWARENESS_FULL)
        );
    }

    #[test]
    fn test_rules_file_keeps_user_text_written_after_legacy_rules() {
        let temp = TempDir::new().unwrap();
        let user = format!("{RTK_AWARENESS_FULL}\n# Mine\n\nDo not delete.\n");
        let path = rules_file(&temp, &user);

        write_rules_file(&path, "rtk init", InitContext::default()).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.starts_with(user.trim_end()),
            "nothing removed: {content}"
        );
        assert!(content.ends_with(rtk_block(RTK_AWARENESS_FULL).trim()));
    }

    #[test]
    fn test_rules_file_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        let path = rules_file(&temp, "Prefer RTK Query.\n");
        let ctx = InitContext {
            dry_run: true,
            ..Default::default()
        };

        write_rules_file(&path, "rtk init", ctx).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "Prefer RTK Query.\n");
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
                rtk_block(RTK_AWARENESS_FULL),
                "kilocode with level {level}"
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
