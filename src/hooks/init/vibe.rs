//! Vibe agent: hook install/uninstall helpers.

use super::*;
use crate::hooks::constants::{
    VIBE_BASH_MATCH, VIBE_DIR, VIBE_HOOK_COMMAND, VIBE_HOOK_NAME, VIBE_HOOKS_FILE,
    VIBE_POSTHOOK_NAME, VIBE_PROMPT_FILE, VIBE_PROMPTS_SUBDIR,
};

// Vibe integration

fn resolve_vibe_dir() -> Result<PathBuf> {
    resolve_home_subdir(VIBE_DIR)
}

/// Entry point for `rtk init -g --agent vibe`.
///
/// Installs a `pre_tool` hook into `~/.vibe/hooks.toml` (Vibe CLI's hook
/// registry, see <https://docs.mistral.ai/vibe/code/cli/hooks>) that routes
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

fn run_vibe_mode_at(
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
    let (entries, skipped) = vibe_entries();
    let hook_outcome = patch_vibe_hooks_toml(&hooks_path, &entries, patch_mode, ctx)?;

    if !hook_only {
        let prompts_dir = vibe_dir.join(VIBE_PROMPTS_SUBDIR);
        if !dry_run {
            fs::create_dir_all(&prompts_dir).with_context(|| {
                format!("Failed to create prompts dir: {}", prompts_dir.display())
            })?;
        }
        let prompt_path = prompts_dir.join(VIBE_PROMPT_FILE);
        write_if_changed(
            &prompt_path,
            awareness_content(ctx.awareness),
            VIBE_PROMPT_FILE,
            ctx,
        )?;
    }

    if dry_run {
        for line in &skipped {
            println!("[dry-run] {line}");
        }
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
        for line in &skipped {
            println!("  {line}");
        }
    }
    Ok(())
}

#[cfg(feature = "engine")]
fn vibe_entries() -> (Vec<super::plan::Entry>, Vec<String>) {
    match super::plan::plan_for("vibe", true) {
        Some(plan) => (plan.entries(), plan.skipped_lines()),
        None => (legacy_vibe_entries(), Vec::new()),
    }
}

#[cfg(not(feature = "engine"))]
fn vibe_entries() -> (Vec<(&'static str, Option<String>)>, Vec<String>) {
    (legacy_vibe_entries(), Vec::new())
}

fn legacy_vibe_entries() -> Vec<(&'static str, Option<String>)> {
    vec![("pre_tool", Some(VIBE_BASH_MATCH.to_string()))]
}

fn vibe_hook_name(event: &str) -> &'static str {
    if event == "post_tool" {
        VIBE_POSTHOOK_NAME
    } else {
        VIBE_HOOK_NAME
    }
}

/// Outcome of `patch_vibe_hooks_toml`. Distinguishes installed / already-present /
/// skipped so the caller can decide whether the "installed" summary is truthful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VibeHookPatchOutcome {
    Installed,
    AlreadyPresent,
    Skipped,
}

impl VibeHookPatchOutcome {
    fn summary_verb(self) -> Option<&'static str> {
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
fn patch_vibe_hooks_toml(
    hooks_path: &Path,
    entries: &[(&'static str, Option<String>)],
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<VibeHookPatchOutcome> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;

    let existing = if hooks_path.exists() {
        fs::read_to_string(hooks_path)
            .with_context(|| format!("Failed to read {}", hooks_path.display()))?
    } else {
        String::new()
    };

    let wanted: Vec<(&'static str, String)> = entries
        .iter()
        .map(|(event, matcher)| {
            (
                vibe_hook_name(event),
                vibe_hook_entry(event, matcher.as_deref()),
            )
        })
        .collect();
    let stale: Vec<&str> = wanted
        .iter()
        .filter(|(name, text)| {
            vibe_hooks_toml_has_entry(&existing, name) && !existing.contains(text.as_str())
        })
        .map(|(name, _)| *name)
        .collect();
    let missing: Vec<&(&'static str, String)> = wanted
        .iter()
        .filter(|(name, _)| !vibe_hooks_toml_has_entry(&existing, name) || stale.contains(name))
        .collect();
    if missing.is_empty() {
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

    let mut existing = existing;
    for name in &stale {
        if let Some(stripped) = strip_vibe_entry(&existing, name) {
            existing = stripped;
        }
    }
    let entry = missing
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let new_content = if existing.trim().is_empty() {
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

fn vibe_hook_entry(event: &str, matcher: Option<&str>) -> String {
    let description = if event == "post_tool" {
        "Compress tool output through rtk before the model reads it."
    } else {
        "Rewrite bash commands through rtk to save tokens."
    };
    format!(
        r#"[[hooks]]
name = "{name}"
type = "{event}"
match = '{matcher}'
command = "{command}"
timeout = 10.0
strict = false
description = "{description}"
"#,
        name = vibe_hook_name(event),
        matcher = matcher.unwrap_or("*"),
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
fn vibe_hooks_toml_has_entry(content: &str, name: &str) -> bool {
    let needle = format!(r#"name = "{name}""#);
    content.contains(&needle)
}

fn vibe_hooks_toml_has_rtk(content: &str) -> bool {
    vibe_hooks_toml_has_entry(content, VIBE_HOOK_NAME)
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
fn uninstall_vibe_at(vibe_dir: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
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
        if let Some(new_content) = strip_vibe_rtk_entries(&content) {
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
fn strip_vibe_rtk_entries(content: &str) -> Option<String> {
    let mut out: Option<String> = None;
    for name in [VIBE_HOOK_NAME, VIBE_POSTHOOK_NAME] {
        if let Some(stripped) = strip_vibe_entry(out.as_deref().unwrap_or(content), name) {
            out = Some(stripped);
        }
    }
    out
}

#[cfg(test)]
fn strip_vibe_rtk_entry(content: &str) -> Option<String> {
    strip_vibe_entry(content, VIBE_HOOK_NAME)
}

fn strip_vibe_entry(content: &str, name: &str) -> Option<String> {
    let needle = format!(r#"name = "{name}""#);
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

/// Returns true if Vibe CLI RTK hook is configured.
pub fn is_configured() -> bool {
    let Ok(vibe_dir) = resolve_vibe_dir() else {
        return false;
    };
    let hooks_path = vibe_dir.join(VIBE_HOOKS_FILE);
    if let Ok(content) = fs::read_to_string(&hooks_path)
        && vibe_hooks_toml_has_rtk(&content)
    {
        return true;
    }
    false
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
        let entry = vibe_hook_entry("pre_tool", Some("bash"));
        assert!(entry.contains("[[hooks]]"));
        assert!(entry.contains(r#"name = "rtk-rewrite""#));
        assert!(entry.contains(r#"type = "pre_tool""#));
        assert!(entry.contains("match = 'bash'"));
        assert!(entry.contains(r#"command = "rtk hook vibe""#));
        assert!(entry.contains("strict = false"));

        let post = vibe_hook_entry("post_tool", Some(r"re:^(?:bash|file_system\.bash)$"));
        assert!(post.contains(r#"name = "rtk-posthook""#));
        assert!(post.contains(r#"type = "post_tool""#));
        assert!(post.contains(r"match = 're:^(?:bash|file_system\.bash)$'"));
        let parsed: toml::Value = toml::from_str(&post).expect("valid toml");
        assert_eq!(
            parsed["hooks"][0]["match"].as_str(),
            Some(r"re:^(?:bash|file_system\.bash)$")
        );
    }

    #[test]
    fn test_vibe_patch_widens_a_stale_match_and_adds_post_tool() {
        let temp = TempDir::new().unwrap();
        let hooks_path = temp.path().join(VIBE_HOOKS_FILE);
        let user_hook = "[[hooks]]\nname = \"user-audit\"\ntype = \"post_tool\"\nmatch = \"*\"\ncommand = \"audit.py\"\n";
        fs::write(
            &hooks_path,
            format!("{user_hook}\n{}", vibe_hook_entry("pre_tool", Some("bash"))),
        )
        .unwrap();

        let entries = vec![
            ("pre_tool", Some("re:^(?:bash|edit)$".to_string())),
            ("post_tool", Some("re:^(?:bash)$".to_string())),
        ];
        let outcome = patch_vibe_hooks_toml(
            &hooks_path,
            &entries,
            PatchMode::Auto,
            InitContext::default(),
        )
        .unwrap();
        assert_eq!(outcome, VibeHookPatchOutcome::Installed);
        let content = fs::read_to_string(&hooks_path).unwrap();
        assert!(content.contains(r#"name = "user-audit""#));
        assert_eq!(content.matches(r#"name = "rtk-rewrite""#).count(), 1);
        assert!(content.contains("match = 're:^(?:bash|edit)$'"));
        assert!(!content.contains("match = 'bash'"));
        assert!(content.contains(r#"name = "rtk-posthook""#));
        let parsed: toml::Value = toml::from_str(&content).expect("valid toml");
        assert_eq!(parsed["hooks"].as_array().unwrap().len(), 3);

        let outcome = patch_vibe_hooks_toml(
            &hooks_path,
            &entries,
            PatchMode::Auto,
            InitContext::default(),
        )
        .unwrap();
        assert_eq!(outcome, VibeHookPatchOutcome::AlreadyPresent);

        assert!(
            strip_vibe_rtk_entries(&content)
                .unwrap()
                .contains("user-audit")
        );
        assert!(
            !strip_vibe_rtk_entries(&content)
                .unwrap()
                .contains("rtk-posthook")
        );
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
        assert!(
            !vibe_dir
                .join(VIBE_PROMPTS_SUBDIR)
                .join(VIBE_PROMPT_FILE)
                .exists()
        );
    }

    #[test]
    fn test_vibe_uninstall_removes_only_rtk_entry_and_prompt() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        fs::create_dir_all(&vibe_dir).unwrap();
        let user_hook = "[[hooks]]\nname = \"user-audit\"\ntype = \"post_tool\"\nmatch = \"*\"\ncommand = \"audit.py\"\n";
        fs::write(vibe_dir.join(VIBE_HOOKS_FILE), user_hook).unwrap();

        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();
        assert!(
            vibe_dir
                .join(VIBE_PROMPTS_SUBDIR)
                .join(VIBE_PROMPT_FILE)
                .exists()
        );

        let removed_first = uninstall_vibe_at(&vibe_dir, InitContext::default()).unwrap();
        let removed_second = uninstall_vibe_at(&vibe_dir, InitContext::default()).unwrap();

        assert_eq!(removed_first.len(), 2);
        assert!(removed_second.is_empty());
        assert!(
            !vibe_dir
                .join(VIBE_PROMPTS_SUBDIR)
                .join(VIBE_PROMPT_FILE)
                .exists()
        );

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

#[cfg(test)]
mod onramp_report_tests {
    use super::*;
    use crate::hooks::hook_check::HookStatus;

    #[cfg(feature = "engine")]
    const BASE: &str = "http://127.0.0.1:8080";

    #[cfg(feature = "engine")]
    #[test]
    fn a_daemon_nothing_points_at_is_not_configured() {
        let lines = proxy_verification_lines(BASE, true, None, None).join("\n");
        assert!(lines.contains("env.ANTHROPIC_BASE_URL missing"), "{lines}");
        assert!(lines.contains("NOT fully configured"), "{lines}");
    }

    #[cfg(feature = "engine")]
    #[test]
    fn an_env_var_aimed_at_a_dead_daemon_is_not_configured() {
        let lines = proxy_verification_lines(BASE, false, Some(BASE), None).join("\n");
        assert!(lines.contains("NOT responding"), "{lines}");
        assert!(lines.contains("NOT fully configured"), "{lines}");
    }

    #[cfg(feature = "engine")]
    #[test]
    fn an_env_var_aimed_elsewhere_is_named_rather_than_accepted() {
        let other = "http://127.0.0.1:9999";
        let lines = proxy_verification_lines(BASE, true, Some(other), None).join("\n");
        assert!(lines.contains("not this daemon"), "{lines}");
        assert!(lines.contains("NOT fully configured"), "{lines}");
    }

    #[cfg(feature = "engine")]
    #[test]
    fn configured_state_warns_when_nothing_restarts_the_daemon() {
        let lines = proxy_verification_lines(BASE, true, Some(BASE), None).join("\n");
        assert!(lines.contains("nothing restarts the daemon"), "{lines}");
        let lines =
            proxy_verification_lines(BASE, true, Some(BASE), Some("SessionStart autostart"))
                .join("\n");
        assert!(
            lines.contains("restart persistence — SessionStart autostart"),
            "{lines}"
        );
        assert!(!lines.contains("nothing restarts"), "{lines}");
    }

    #[cfg(feature = "engine")]
    #[test]
    fn session_start_entry_roundtrip() {
        let mut root = serde_json::json!({});
        assert!(!session_start_entry_present(&root));
        insert_session_start_entry(&mut root).expect("insert");
        assert!(session_start_entry_present(&root));
        let entry = root
            .pointer("/hooks/SessionStart/0/hooks/0/command")
            .and_then(|v| v.as_str())
            .expect("command written");
        assert_eq!(entry, "rtk hook claude");
        assert!(
            root.pointer("/hooks/SessionStart/0/matcher").is_none(),
            "SessionStart entries carry no matcher"
        );
        assert!(claude::remove_hook_from_json(&mut root));
        assert!(!session_start_entry_present(&root));
        assert!(
            root.get("hooks").is_none(),
            "uninstall must drop the emptied hooks object: {root}"
        );
    }

    #[cfg(feature = "engine")]
    #[test]
    fn foreign_session_start_hooks_survive_uninstall() {
        let mut root = serde_json::json!({
            "hooks": { "SessionStart": [
                { "hooks": [{ "type": "command", "command": "my-own-tool --hello" }] }
            ]}
        });
        insert_session_start_entry(&mut root).expect("insert");
        assert!(claude::remove_hook_from_json(&mut root));
        let arr = root
            .pointer("/hooks/SessionStart")
            .and_then(|v| v.as_array())
            .expect("foreign entry must survive");
        assert_eq!(arr.len(), 1, "{root}");
        assert!(arr[0].to_string().contains("my-own-tool"), "{root}");
    }

    #[cfg(feature = "engine")]
    #[test]
    fn both_halves_holding_is_the_only_configured_state() {
        let lines = proxy_verification_lines(BASE, true, Some(BASE), None).join("\n");
        assert!(lines.contains("proxy: configured"), "{lines}");
        assert!(!lines.contains("NOT"), "{lines}");
        // The prompt must not promise what rtk:proxy does not enable.
        assert!(lines.contains("CCR ships off"), "{lines}");
    }

    #[test]
    fn the_project_footer_reports_global_state_and_points_at_g() {
        let missing = project_scope_footer_lines(HookStatus::Missing, false, false).join("\n");
        assert!(missing.contains("NOT registered"), "{missing}");
        assert!(missing.contains("not set up"), "{missing}");
        assert_eq!(missing.matches("rtk init -g").count(), 2, "{missing}");

        let done = project_scope_footer_lines(HookStatus::Ok, true, true).join("\n");
        assert!(done.contains("registered globally"), "{done}");
        assert!(done.contains("configured and running"), "{done}");
        assert!(!done.contains("rtk init -g"), "{done}");
    }

    /// The state that leaves `rtk gain` empty: daemon up, nothing aimed at it.
    #[test]
    fn a_running_but_unused_daemon_is_called_out_in_project_scope() {
        let lines = project_scope_footer_lines(HookStatus::Ok, false, true).join("\n");
        assert!(lines.contains("nothing routed to it"), "{lines}");
    }
}
