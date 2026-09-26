//! Instructions-file block helpers shared by every agent that writes one (RTK block upsert and
//! removal), and the AGENTS.md reference helpers the Codex flow uses.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum RtkBlockUpsert {
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
fn upsert_rtk_block(content: &str, block: &str) -> (String, RtkBlockUpsert) {
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
/// result back via [`write_reported`]. Refuses to modify files containing an opening
/// marker without a matching closing marker (bails with a diagnostic and the exact
/// `recovery_cmd` to re-run after manual cleanup).
///
/// Returns the [`RtkBlockUpsert`] action so callers can branch on whether anything
/// was actually changed (e.g., to skip post-install steps on `Unchanged`).
///
/// `label` is shown in user-facing messages (e.g., `"rtk instructions"`,
/// `"Copilot instructions"`).
pub(super) fn write_rtk_block(
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
            let report = Report::new(format!(
                "[dry-run] would add {} to {}",
                label,
                path.display()
            ))
            .done(format!("[ok] Added {} to {}", label, path.display()));
            write_reported(path, WriteKind::Instructions, &new_content, ctx, report)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        }
        RtkBlockUpsert::Updated => {
            let report = Report::new(format!(
                "[dry-run] would update {} in {}",
                label,
                path.display()
            ))
            .done(format!("[ok] Updated {} in {}", label, path.display()));
            write_reported(path, WriteKind::Instructions, &new_content, ctx, report)
                .with_context(|| format!("Failed to write {}", path.display()))?;
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

/// Patch AGENTS.md: add @RTK.md (or absolute path), migrate old inline block if present
pub(super) fn patch_agents_md(path: &Path, rtk_md_ref: &str, ctx: InitContext) -> Result<bool> {
    let InitContext { verbose, .. } = ctx;
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
            let report = Report::new(format!(
                "[dry-run] would migrate {} to {} in {}",
                RTK_MD_REF,
                rtk_md_ref,
                path.display()
            ))
            .done_verbose(format!("Migrated {} to {}", RTK_MD_REF, rtk_md_ref));
            write_reported(path, WriteKind::Instructions, &content, ctx, report)
                .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;
            return Ok(true);
        }
        if migrated {
            let report = Report::new(format!(
                "[dry-run] would write migrated AGENTS.md: {}",
                path.display()
            ));
            write_reported(path, WriteKind::Instructions, &content, ctx, report)
                .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;
        }
        return Ok(false);
    }

    let new_content = if content.is_empty() {
        format!("{}\n", rtk_md_ref)
    } else {
        format!("{}\n\n{}\n", content.trim(), rtk_md_ref)
    };

    let report = Report::new(format!(
        "[dry-run] would add {} reference to AGENTS.md: {}",
        rtk_md_ref,
        path.display()
    ))
    .with_content()
    .done_verbose(format!("Added {} reference to AGENTS.md", rtk_md_ref));
    write_reported(path, WriteKind::Instructions, &new_content, ctx, report)
        .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;

    Ok(true)
}

pub(super) fn has_rtk_reference(content: &str, refs: &[&str]) -> bool {
    content
        .lines()
        .map(str::trim)
        .any(|line| refs.contains(&line))
}

pub(super) fn remove_rtk_reference_from_agents(
    path: &Path,
    refs: &[&str],
    ctx: InitContext,
) -> Result<bool> {
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

    let report = Report::new(format!(
        "[dry-run] would remove RTK.md reference from AGENTS.md: {}",
        path.display()
    ))
    .with_content()
    .done_verbose(format!(
        "Removed RTK.md reference from AGENTS.md: {}",
        path.display()
    ));
    write_reported(path, WriteKind::Instructions, &cleaned, ctx, report)
        .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;

    Ok(true)
}

/// Strip the inline RTK block from an instructions file's content, returning the cleaned
/// text and whether a block was removed.
pub(super) fn remove_rtk_block(content: &str) -> (String, bool) {
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
}
