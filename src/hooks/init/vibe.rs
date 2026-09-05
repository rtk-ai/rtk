//! Vibe agent: hook install/uninstall helpers.

use super::*;

// Vibe integration

pub(crate) fn resolve_vibe_dir() -> Result<PathBuf> {
    resolve_home_subdir(VIBE_DIR)
}

/// Entry point for `rtk init -g --agent vibe`.
///
/// Installs a `pre_tool` hook into `~/.vibe/hooks.toml` (Vibe CLI's hook
/// registry, see https://docs.mistral.ai/vibe/code/cli/hooks) that routes
/// bash tool calls through the native `rtk hook vibe` binary. When not
/// `hook_only`, also drops an `~/.vibe/prompts/rtk.md` system prompt file
/// as a belt-and-suspenders fallback if the hook is disabled.
pub fn run_vibe_mode(
    global: bool,
    hook_only: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    if !global {
        anyhow::bail!("Vibe support is global-only. Use: rtk init -g --agent vibe");
    }
    let vibe_dir = resolve_vibe_dir()?;
    run_vibe_mode_at(&vibe_dir, hook_only, patch_mode, ctx)
}

pub(crate) fn run_vibe_mode_at(
    vibe_dir: &Path,
    hook_only: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !dry_run {
        fs::create_dir_all(vibe_dir)
            .with_context(|| format!("Failed to create Vibe config dir: {}", vibe_dir.display()))?;
    }

    let hooks_path = vibe_dir.join(VIBE_HOOKS_FILE);
    let hook_outcome = patch_vibe_hooks_toml(&hooks_path, patch_mode, ctx)?;

    if !hook_only {
        let prompts_dir = vibe_dir.join(VIBE_PROMPTS_SUBDIR);
        if !dry_run {
            fs::create_dir_all(&prompts_dir).with_context(|| {
                format!("Failed to create prompts dir: {}", prompts_dir.display())
            })?;
        }
        let prompt_path = prompts_dir.join(VIBE_PROMPT_FILE);
        write_if_changed(&prompt_path, RTK_SLIM, VIBE_PROMPT_FILE, ctx)?;
    }

    if dry_run {
        print_dry_run_footer();
    } else if let Some(summary_verb) = hook_outcome.summary_verb() {
        println!("\nMistral Vibe CLI hook {summary_verb} (global).\n");
        println!("  Hook registry: {}", hooks_path.display());
        if !hook_only {
            println!(
                "  Prompt: {}",
                vibe_dir
                    .join(VIBE_PROMPTS_SUBDIR)
                    .join(VIBE_PROMPT_FILE)
                    .display()
            );
        }
        println!("  Restart Vibe. Test with: git status\n");
    }
    Ok(())
}

/// Outcome of `patch_vibe_hooks_toml`. Distinguishes installed / already-present /
/// skipped so the caller can decide whether the "installed" summary is truthful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VibeHookPatchOutcome {
    Installed,
    AlreadyPresent,
    Skipped,
}

impl VibeHookPatchOutcome {
    pub(crate) fn summary_verb(self) -> Option<&'static str> {
        match self {
            Self::Installed => Some("installed"),
            Self::AlreadyPresent => Some("already present"),
            Self::Skipped => None,
        }
    }
}

/// Append the RTK `[[hooks]]` entry to `~/.vibe/hooks.toml` if not already present.
///
/// Uses append-based patching (string level) rather than parse-serialize round-trip
/// to preserve any user comments and formatting in the file.
pub(crate) fn patch_vibe_hooks_toml(
    hooks_path: &Path,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<VibeHookPatchOutcome> {
    let InitContext { verbose, dry_run } = ctx;

    let existing = if hooks_path.exists() {
        fs::read_to_string(hooks_path)
            .with_context(|| format!("Failed to read {}", hooks_path.display()))?
    } else {
        String::new()
    };

    if vibe_hooks_toml_has_rtk(&existing) {
        if verbose > 0 {
            eprintln!("Vibe hooks.toml already has RTK hook");
        }
        return Ok(VibeHookPatchOutcome::AlreadyPresent);
    }

    if patch_mode == PatchMode::Skip {
        println!(
            "\nManual setup needed: add RTK hook to {}\n\
             See: https://www.rtk-ai.app/guide/getting-started/supported-agents#mistral-vibe",
            hooks_path.display()
        );
        return Ok(VibeHookPatchOutcome::Skipped);
    }

    if patch_mode == PatchMode::Ask {
        if dry_run {
            println!(
                "[dry-run] would prompt before patching {}",
                hooks_path.display()
            );
        } else {
            print!("Patch {} with RTK hook? [y/N] ", hooks_path.display());
            std::io::stdout().flush().ok();
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer).ok();
            if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                println!(
                    "Skipped. Re-run with --auto-patch, or add the hook manually to {}",
                    hooks_path.display()
                );
                return Ok(VibeHookPatchOutcome::Skipped);
            }
        }
    }

    let entry = vibe_hook_entry();
    let new_content = if existing.is_empty() {
        entry.clone()
    } else if existing.ends_with("\n\n") {
        format!("{existing}{entry}")
    } else if existing.ends_with('\n') {
        format!("{existing}\n{entry}")
    } else {
        format!("{existing}\n\n{entry}")
    };

    if dry_run {
        println!(
            "[dry-run] would patch Vibe hooks.toml: {}",
            hooks_path.display()
        );
        if verbose > 0 {
            println!("[dry-run] appended entry:\n{entry}");
        }
    } else {
        atomic_write(hooks_path, &new_content)
            .with_context(|| format!("Failed to write {}", hooks_path.display()))?;
    }
    Ok(VibeHookPatchOutcome::Installed)
}

/// TOML entry emitted for the Vibe pre_tool hook. Mirrors the shape documented
/// at https://docs.mistral.ai/vibe/code/cli/hooks.
pub(crate) fn vibe_hook_entry() -> String {
    format!(
        r#"[[hooks]]
name = "{name}"
type = "pre_tool"
match = "{match_glob}"
command = "{command}"
timeout = 10.0
strict = false
description = "Rewrite bash commands through the rtk proxy to save tokens."
"#,
        name = VIBE_HOOK_NAME,
        match_glob = VIBE_BASH_MATCH,
        command = VIBE_HOOK_COMMAND,
    )
}

/// Detect an existing RTK entry by looking for the hook `name` field. Scanning
/// the raw string is enough because `name` is required by Vibe and must be
/// unique, so a substring match is both necessary and sufficient.
///
/// Tradeoff: matches the exact spacing `name = "rtk-rewrite"`. A reformatted
/// file (`name="rtk-rewrite"` or extra whitespace) would defeat idempotency
/// and cause a duplicate append on re-install. Acceptable because our own
/// installer only ever writes the canonical spacing, and the alternative
/// (parse-serialize round-trip via toml_edit) would clobber user comments
/// and formatting in the file.
pub(crate) fn vibe_hooks_toml_has_rtk(content: &str) -> bool {
    let needle = format!(r#"name = "{VIBE_HOOK_NAME}""#);
    content.contains(&needle)
}

/// Public entry point for `rtk init -g --agent vibe --uninstall`.
pub fn uninstall_vibe(ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let vibe_dir = match resolve_vibe_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("RTK Vibe uninstall skipped: could not resolve ~/.vibe/ ({e})");
            return Ok(());
        }
    };
    let removed = uninstall_vibe_at(&vibe_dir, ctx)?;

    if removed.is_empty() {
        println!("RTK Vibe support was not installed (nothing to remove)");
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK for Mistral Vibe CLI:"
        } else {
            "RTK uninstalled for Mistral Vibe CLI:"
        };
        println!("{}", header);
        for item in removed {
            println!("  - {}", item);
        }
        if !dry_run {
            println!("\nRestart Vibe CLI to apply changes.");
        }
    }

    if dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

/// Remove the RTK hook entry (and, when non-empty, the surrounding blank
/// lines) from `~/.vibe/hooks.toml` and the sibling `~/.vibe/prompts/rtk.md`
/// prompt file. Leaves any other user-declared hooks intact.
pub(crate) fn uninstall_vibe_at(vibe_dir: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext { verbose, dry_run } = ctx;
    let mut removed = Vec::new();

    let prompt_path = vibe_dir.join(VIBE_PROMPTS_SUBDIR).join(VIBE_PROMPT_FILE);
    if prompt_path.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove Vibe RTK prompt: {}",
                prompt_path.display()
            );
        } else {
            // nosemgrep: filesystem-deletion -- uninstall path removes only RTK's own prompt file
            fs::remove_file(&prompt_path)
                .with_context(|| format!("Failed to remove {}", prompt_path.display()))?;
        }
        removed.push(format!("Vibe prompt: {}", prompt_path.display()));
    }

    let hooks_path = vibe_dir.join(VIBE_HOOKS_FILE);
    if hooks_path.exists() {
        let content = fs::read_to_string(&hooks_path)
            .with_context(|| format!("Failed to read {}", hooks_path.display()))?;
        if let Some(new_content) = strip_vibe_rtk_entry(&content) {
            if dry_run {
                println!(
                    "[dry-run] would remove RTK hook from Vibe hooks.toml: {}",
                    hooks_path.display()
                );
            } else if new_content.trim().is_empty() {
                // nosemgrep: filesystem-deletion -- uninstall removes hooks.toml only when it becomes empty after stripping the RTK entry
                fs::remove_file(&hooks_path)
                    .with_context(|| format!("Failed to remove {}", hooks_path.display()))?;
            } else {
                atomic_write(&hooks_path, &new_content)
                    .with_context(|| format!("Failed to write {}", hooks_path.display()))?;
            }
            removed.push(format!(
                "Vibe hooks.toml: removed RTK entry ({})",
                hooks_path.display()
            ));
        }
    }

    if verbose > 0 && !removed.is_empty() {
        eprintln!("Vibe artifacts removed");
    }

    Ok(removed)
}

/// Extract and drop the `[[hooks]]` block whose `name = "rtk-rewrite"` field
/// is set. Returns `None` when the entry is absent, `Some(new_content)` after
/// removal (with surrounding blank lines collapsed). The scan walks `[[hooks]]`
/// section boundaries — anything else in the file is preserved verbatim.
pub(crate) fn strip_vibe_rtk_entry(content: &str) -> Option<String> {
    let needle = format!(r#"name = "{VIBE_HOOK_NAME}""#);
    if !content.contains(&needle) {
        return None;
    }

    let lines: Vec<&str> = content.lines().collect();
    let mut sections: Vec<(usize, usize)> = Vec::new();
    let mut current_start: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("[[hooks]]") || trimmed.starts_with('[') {
            if let Some(start) = current_start.take() {
                sections.push((start, i));
            }
            if trimmed.starts_with("[[hooks]]") {
                current_start = Some(i);
            }
        }
    }
    if let Some(start) = current_start {
        sections.push((start, lines.len()));
    }

    let target = sections
        .iter()
        .find(|(start, end)| lines[*start..*end].iter().any(|l| l.contains(&needle)))?;

    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    kept.extend(&lines[..target.0]);
    kept.extend(&lines[target.1..]);

    let mut out = kept.join("\n");
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Vibe tests

    #[test]
    fn test_vibe_detects_rtk_entry_by_name_field() {
        assert!(!vibe_hooks_toml_has_rtk(""));
        assert!(!vibe_hooks_toml_has_rtk("[[hooks]]\nname = \"other\"\n"));
        assert!(vibe_hooks_toml_has_rtk(
            "[[hooks]]\nname = \"rtk-rewrite\"\n"
        ));
    }

    #[test]
    fn test_vibe_hook_entry_shape_matches_docs() {
        let entry = vibe_hook_entry();
        assert!(entry.contains("[[hooks]]"));
        assert!(entry.contains(r#"name = "rtk-rewrite""#));
        assert!(entry.contains(r#"type = "pre_tool""#));
        assert!(entry.contains(r#"match = "bash""#));
        assert!(entry.contains(r#"command = "rtk hook vibe""#));
        assert!(entry.contains("strict = false"));
    }

    #[test]
    fn test_vibe_strip_returns_none_when_entry_absent() {
        let content = "[[hooks]]\nname = \"other\"\ntype = \"post_tool\"\n";
        assert!(strip_vibe_rtk_entry(content).is_none());
    }

    #[test]
    fn test_vibe_strip_removes_only_rtk_entry() {
        let content = "[[hooks]]\nname = \"user-audit\"\ntype = \"post_tool\"\nmatch = \"*\"\ncommand = \"audit.py\"\n\n[[hooks]]\nname = \"rtk-rewrite\"\ntype = \"pre_tool\"\nmatch = \"bash\"\ncommand = \"rtk hook vibe\"\n";
        let stripped = strip_vibe_rtk_entry(content).expect("expected removal");
        assert!(stripped.contains(r#"name = "user-audit""#));
        assert!(!stripped.contains(r#"name = "rtk-rewrite""#));
        assert!(!stripped.contains("rtk hook vibe"));
    }

    #[test]
    fn test_vibe_install_creates_hook_and_prompt() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();

        let hooks_content = fs::read_to_string(vibe_dir.join(VIBE_HOOKS_FILE)).unwrap();
        assert!(hooks_content.contains(r#"name = "rtk-rewrite""#));
        assert!(hooks_content.contains(r#"command = "rtk hook vibe""#));

        let prompt_path = vibe_dir.join(VIBE_PROMPTS_SUBDIR).join(VIBE_PROMPT_FILE);
        assert!(prompt_path.exists());
    }

    #[test]
    fn test_vibe_install_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();
        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();

        let hooks_content = fs::read_to_string(vibe_dir.join(VIBE_HOOKS_FILE)).unwrap();
        assert_eq!(hooks_content.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_vibe_install_preserves_existing_user_hook() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        fs::create_dir_all(&vibe_dir).unwrap();
        let user_hook = "[[hooks]]\nname = \"user-audit\"\ntype = \"post_tool\"\nmatch = \"*\"\ncommand = \"audit.py\"\n";
        fs::write(vibe_dir.join(VIBE_HOOKS_FILE), user_hook).unwrap();

        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();

        let hooks_content = fs::read_to_string(vibe_dir.join(VIBE_HOOKS_FILE)).unwrap();
        assert!(hooks_content.contains(r#"name = "user-audit""#));
        assert!(hooks_content.contains(r#"name = "rtk-rewrite""#));
    }

    #[test]
    fn test_vibe_hook_only_skips_prompt_file() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        run_vibe_mode_at(&vibe_dir, true, PatchMode::Auto, InitContext::default()).unwrap();

        assert!(vibe_dir.join(VIBE_HOOKS_FILE).exists());
        assert!(!vibe_dir
            .join(VIBE_PROMPTS_SUBDIR)
            .join(VIBE_PROMPT_FILE)
            .exists());
    }

    #[test]
    fn test_vibe_uninstall_removes_only_rtk_entry_and_prompt() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        fs::create_dir_all(&vibe_dir).unwrap();
        let user_hook = "[[hooks]]\nname = \"user-audit\"\ntype = \"post_tool\"\nmatch = \"*\"\ncommand = \"audit.py\"\n";
        fs::write(vibe_dir.join(VIBE_HOOKS_FILE), user_hook).unwrap();

        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();
        assert!(vibe_dir
            .join(VIBE_PROMPTS_SUBDIR)
            .join(VIBE_PROMPT_FILE)
            .exists());

        let removed_first = uninstall_vibe_at(&vibe_dir, InitContext::default()).unwrap();
        let removed_second = uninstall_vibe_at(&vibe_dir, InitContext::default()).unwrap();

        assert_eq!(removed_first.len(), 2);
        assert!(removed_second.is_empty());
        assert!(!vibe_dir
            .join(VIBE_PROMPTS_SUBDIR)
            .join(VIBE_PROMPT_FILE)
            .exists());

        let remaining = fs::read_to_string(vibe_dir.join(VIBE_HOOKS_FILE)).unwrap();
        assert!(remaining.contains(r#"name = "user-audit""#));
        assert!(!remaining.contains(r#"name = "rtk-rewrite""#));
    }

    #[test]
    fn test_vibe_uninstall_removes_hooks_file_when_no_other_hooks() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();
        assert!(vibe_dir.join(VIBE_HOOKS_FILE).exists());

        uninstall_vibe_at(&vibe_dir, InitContext::default()).unwrap();

        assert!(!vibe_dir.join(VIBE_HOOKS_FILE).exists());
    }
}
