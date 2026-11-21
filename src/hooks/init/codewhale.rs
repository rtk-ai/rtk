//! Codewhale agent: hook install/uninstall helpers.
//!
//! Codewhale reads hooks from `~/.codewhale/config.toml` (the `[hooks]` table,
//! one `[[hooks.hooks]]` entry per hook) and, for a trusted workspace, from
//! `.codewhale/hooks.toml` (top-level `[[hooks]]` entries). Its
//! `tool_call_before` event can allow / deny / ask and rewrite tool input, so
//! `rtk hook codewhale` rewrites shell commands there.

use super::*;
use crate::core::user_dirs;
use crate::hooks::constants::{
    CODEWHALE_BASH_MATCH, CODEWHALE_CONFIG_FILE, CODEWHALE_DIR, CODEWHALE_HOOK_COMMAND,
    CODEWHALE_HOOK_NAME, CODEWHALE_PROJECT_HOOKS_FILE,
};

/// Global hook registry: `~/.codewhale/config.toml`.
fn resolve_global_config() -> Result<PathBuf> {
    let home = user_dirs::home().context("Failed to resolve home directory for Codewhale")?;
    Ok(home.join(CODEWHALE_DIR).join(CODEWHALE_CONFIG_FILE))
}

/// Project hook registry: `.codewhale/hooks.toml`.
fn resolve_project_hooks() -> PathBuf {
    user_dirs::in_working_dir(CODEWHALE_DIR).join(CODEWHALE_PROJECT_HOOKS_FILE)
}

/// Entry point for `rtk init --agent codewhale` (project) and
/// `rtk init -g --agent codewhale` (global).
pub fn run_codewhale_mode(global: bool, patch_mode: PatchMode, ctx: InitContext) -> Result<()> {
    if global {
        let config = resolve_global_config()?;
        run_codewhale_mode_at(&config, true, patch_mode, ctx)
    } else {
        let hooks = resolve_project_hooks();
        run_codewhale_mode_at(&hooks, false, patch_mode, ctx)
    }
}

fn run_codewhale_mode_at(
    path: &Path,
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !dry_run && let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create Codewhale config dir: {}",
                parent.display()
            )
        })?;
    }

    let hook_outcome = patch_codewhale_toml(path, global, patch_mode, ctx)?;

    if dry_run {
        print_dry_run_footer();
    } else if let Some(summary_verb) = hook_outcome.summary_verb() {
        if global {
            println!("\nCodewhale hook {summary_verb} (global).\n");
        } else {
            println!("\nCodewhale hook {summary_verb} (project).\n");
        }
        println!("  Hook registry: {}", path.display());
        if global {
            println!("  Restart Codewhale. Test with: git status\n");
        } else {
            println!("  Review and approve project hooks in Codewhale:");
            println!("    /hooks review");
            println!("    /hooks approve <digest>\n");
            println!("  Restart Codewhale. Test with: git status\n");
        }
    }
    Ok(())
}

/// Outcome of `patch_codewhale_toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodewhaleHookPatchOutcome {
    Installed,
    AlreadyPresent,
    Skipped,
}

impl CodewhaleHookPatchOutcome {
    fn summary_verb(self) -> Option<&'static str> {
        match self {
            Self::Installed => Some("installed"),
            Self::AlreadyPresent => Some("already present"),
            Self::Skipped => None,
        }
    }
}

/// Append the RTK hook entry to a Codewhale hooks TOML file if not present.
///
/// String-level append rather than a parse-serialize round-trip, so existing
/// comments and unrelated configuration in `~/.codewhale/config.toml` survive
/// untouched — the same tradeoff `init/vibe.rs` makes for `hooks.toml`.
fn patch_codewhale_toml(
    path: &Path,
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<CodewhaleHookPatchOutcome> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;

    let existing = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?
    } else {
        String::new()
    };

    if codewhale_toml_has_rtk(&existing) {
        if verbose > 0 {
            eprintln!("{} already has the RTK hook", path.display());
        }
        return Ok(CodewhaleHookPatchOutcome::AlreadyPresent);
    }

    if patch_mode == PatchMode::Skip {
        println!(
            "\nManual setup needed: add the RTK hook to {}\n\
             See: https://www.rtk-ai.app/guide/getting-started/supported-agents#codewhale",
            path.display()
        );
        return Ok(CodewhaleHookPatchOutcome::Skipped);
    }

    if patch_mode == PatchMode::Ask {
        if dry_run {
            println!("[dry-run] would prompt before patching {}", path.display());
        } else {
            print!("Patch {} with the RTK hook? [y/N] ", path.display());
            std::io::stdout().flush().ok();
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer).ok();
            if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                println!(
                    "Skipped. Re-run with --auto-patch, or add the hook manually to {}",
                    path.display()
                );
                return Ok(CodewhaleHookPatchOutcome::Skipped);
            }
        }
    }

    let entry = codewhale_hook_entry(global);
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
        println!("[dry-run] would patch Codewhale hooks: {}", path.display());
        if verbose > 0 {
            println!("[dry-run] appended entry:\n{entry}");
        }
    } else {
        atomic_write(path, &new_content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
    }
    Ok(CodewhaleHookPatchOutcome::Installed)
}

/// The TOML entry emitted for the Codewhale `tool_call_before` hook.
///
/// Global config nests the entry under `[hooks]` as `[[hooks.hooks]]`; a
/// project `.codewhale/hooks.toml` carries the same fields as a top-level
/// `[[hooks]]` array. `timeout_secs`, `background` and `continue_on_error`
/// are omitted so Codewhale's defaults apply (30s, foreground, continue).
fn codewhale_hook_entry(global: bool) -> String {
    let header = if global {
        "[[hooks.hooks]]"
    } else {
        "[[hooks]]"
    };
    format!(
        "{header}\nname = \"{name}\"\nevent = \"tool_call_before\"\ncommand = \"{command}\"\ncondition = {{ type = \"tool_name\", name = \"{bash}\" }}\n",
        name = CODEWHALE_HOOK_NAME,
        command = CODEWHALE_HOOK_COMMAND,
        bash = CODEWHALE_BASH_MATCH,
    )
}

/// Detect an existing RTK entry by its command. A substring match is both
/// necessary and sufficient: RTK's own installer only ever writes
/// `command = "rtk hook codewhale"`, and a full-path spelling (`/usr/bin/rtk
/// hook codewhale`) still contains the substring.
fn codewhale_toml_has_rtk(content: &str) -> bool {
    content.contains(CODEWHALE_HOOK_COMMAND)
}

/// Public entry point for `rtk init --agent codewhale --uninstall`.
pub fn uninstall_codewhale(global: bool, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let path = if global {
        resolve_global_config()?
    } else {
        resolve_project_hooks()
    };
    let removed = uninstall_codewhale_at(&path, global, ctx)?;

    if !removed {
        println!("RTK Codewhale support was not installed (nothing to remove)");
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK for Codewhale:"
        } else {
            "RTK uninstalled for Codewhale:"
        };
        println!("{}", header);
        println!("  - {}", path.display());
        if !dry_run && !global {
            println!(
                "\nProject hooks remain approved in Codewhale until revoked. Run\n\
                 `/hooks revoke` to stop future launches of this workspace's hooks."
            );
        }
    }

    if dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

/// Remove the RTK hook entry from a Codewhale hooks file, leaving unrelated
/// configuration intact. A project `hooks.toml` that becomes empty is removed
/// entirely; a global `config.toml` is never removed.
fn uninstall_codewhale_at(path: &Path, global: bool, ctx: InitContext) -> Result<bool> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    if !path.exists() {
        return Ok(false);
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let Some(new_content) = strip_codewhale_rtk_entry(&content) else {
        return Ok(false);
    };

    if dry_run {
        println!(
            "[dry-run] would remove the RTK hook from {}",
            path.display()
        );
        return Ok(true);
    }

    if !global && new_content.trim().is_empty() {
        // nosemgrep: filesystem-deletion -- uninstall removes hooks.toml only when it becomes empty after stripping the RTK entry
        fs::remove_file(path).with_context(|| format!("Failed to remove {}", path.display()))?;
    } else {
        atomic_write(path, &new_content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
    }

    if verbose > 0 {
        eprintln!("Codewhale RTK hook removed");
    }
    Ok(true)
}

/// Extract and drop the `[[hooks.hooks]]` / `[[hooks]]` block whose entry runs
/// `rtk hook codewhale`. Returns `None` when the command is absent. Walks
/// table-section boundaries so a global `config.toml`'s `[hooks]` settings and
/// other `[[...]]` entries are preserved verbatim.
fn strip_codewhale_rtk_entry(content: &str) -> Option<String> {
    if !codewhale_toml_has_rtk(content) {
        return None;
    }

    let lines: Vec<&str> = content.lines().collect();
    let mut sections: Vec<(usize, usize)> = Vec::new();
    let mut current_start: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with('[') {
            if let Some(start) = current_start.take() {
                sections.push((start, i));
            }
            current_start = Some(i);
        }
    }
    if let Some(start) = current_start {
        sections.push((start, lines.len()));
    }

    let target = sections.iter().find(|(start, end)| {
        lines[*start..*end]
            .iter()
            .any(|l| l.contains(CODEWHALE_HOOK_COMMAND))
    })?;

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

/// Returns true if Codewhale has an RTK hook configured in either scope.
pub fn is_configured() -> bool {
    let global = resolve_global_config()
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .is_some_and(|content| codewhale_toml_has_rtk(&content));
    if global {
        return true;
    }
    fs::read_to_string(resolve_project_hooks())
        .is_ok_and(|content| codewhale_toml_has_rtk(&content))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_codewhale_detects_rtk_entry_by_command() {
        assert!(!codewhale_toml_has_rtk(""));
        assert!(!codewhale_toml_has_rtk(
            "[[hooks]]\ncommand = \"echo hi\"\n"
        ));
        assert!(codewhale_toml_has_rtk(
            "[[hooks.hooks]]\ncommand = \"rtk hook codewhale\"\n"
        ));
        assert!(codewhale_toml_has_rtk(
            "[[hooks]]\ncommand = \"/usr/local/bin/rtk hook codewhale\"\n"
        ));
    }

    #[test]
    fn test_codewhale_global_entry_shape_matches_docs() {
        let entry = codewhale_hook_entry(true);
        assert!(entry.contains("[[hooks.hooks]]"));
        assert!(entry.contains(r#"name = "rtk-rewrite""#));
        assert!(entry.contains(r#"event = "tool_call_before""#));
        assert!(entry.contains(r#"command = "rtk hook codewhale""#));
        assert!(entry.contains(r#"condition = { type = "tool_name", name = "bash" }"#));
    }

    #[test]
    fn test_codewhale_project_entry_shape_matches_docs() {
        let entry = codewhale_hook_entry(false);
        assert!(entry.starts_with("[[hooks]]\n"));
        assert!(!entry.contains("[[hooks.hooks]]"));
    }

    #[test]
    fn test_codewhale_strip_returns_none_when_entry_absent() {
        assert!(strip_codewhale_rtk_entry("[[hooks]]\ncommand = \"echo hi\"\n").is_none());
    }

    #[test]
    fn test_codewhale_strip_removes_only_rtk_entry() {
        let content = "[[hooks]]\nname = \"user-audit\"\nevent = \"tool_call_after\"\ncommand = \"audit.py\"\n\n[[hooks]]\nname = \"rtk-rewrite\"\nevent = \"tool_call_before\"\ncommand = \"rtk hook codewhale\"\ncondition = { type = \"tool_name\", name = \"bash\" }\n";
        let stripped = strip_codewhale_rtk_entry(content).expect("expected removal");
        assert!(stripped.contains(r#"name = "user-audit""#));
        assert!(!stripped.contains("rtk hook codewhale"));
        assert!(!stripped.contains(r#"name = "rtk-rewrite""#));
    }

    #[test]
    fn test_codewhale_strip_preserves_global_hooks_table() {
        // A global config.toml's `[hooks]` settings table must survive.
        let content = "[hooks]\nenabled = true\n\n[[hooks.hooks]]\nname = \"rtk-rewrite\"\nevent = \"tool_call_before\"\ncommand = \"rtk hook codewhale\"\ncondition = { type = \"tool_name\", name = \"bash\" }\n\n[models]\nprovider = \"deepseek\"\n";
        let stripped = strip_codewhale_rtk_entry(content).expect("expected removal");
        assert!(stripped.contains("[hooks]\nenabled = true"));
        assert!(stripped.contains("[models]"));
        assert!(!stripped.contains("rtk hook codewhale"));
    }

    #[test]
    fn test_codewhale_global_install_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let config = temp.path().join(".codewhale").join("config.toml");
        run_codewhale_mode_at(&config, true, PatchMode::Auto, InitContext::default()).unwrap();
        run_codewhale_mode_at(&config, true, PatchMode::Auto, InitContext::default()).unwrap();

        let content = fs::read_to_string(&config).unwrap();
        assert_eq!(
            content.matches("rtk hook codewhale").count(),
            1,
            "re-install must not duplicate the entry"
        );
    }

    #[test]
    fn test_codewhale_project_install_uses_top_level_hooks_table() {
        let temp = TempDir::new().unwrap();
        let hooks = temp.path().join(".codewhale").join("hooks.toml");
        run_codewhale_mode_at(&hooks, false, PatchMode::Auto, InitContext::default()).unwrap();

        let content = fs::read_to_string(&hooks).unwrap();
        assert!(content.starts_with("[[hooks]]\n"));
        assert!(!content.contains("[[hooks.hooks]]"));
        assert!(content.contains("rtk hook codewhale"));
    }

    #[test]
    fn test_codewhale_uninstall_removes_hooks_file_when_empty() {
        let temp = TempDir::new().unwrap();
        let hooks = temp.path().join(".codewhale").join("hooks.toml");
        run_codewhale_mode_at(&hooks, false, PatchMode::Auto, InitContext::default()).unwrap();
        assert!(hooks.exists());

        assert!(uninstall_codewhale_at(&hooks, false, InitContext::default()).unwrap());
        assert!(!hooks.exists(), "empty project hooks.toml must be removed");
    }

    #[test]
    fn test_codewhale_uninstall_preserves_user_entries() {
        let temp = TempDir::new().unwrap();
        let hooks = temp.path().join(".codewhale").join("hooks.toml");
        let user = "[[hooks]]\nname = \"user-audit\"\nevent = \"tool_call_after\"\ncommand = \"audit.py\"\n";
        fs::create_dir_all(hooks.parent().unwrap()).unwrap();
        fs::write(&hooks, user).unwrap();

        run_codewhale_mode_at(&hooks, false, PatchMode::Auto, InitContext::default()).unwrap();
        assert!(uninstall_codewhale_at(&hooks, false, InitContext::default()).unwrap());

        let content = fs::read_to_string(&hooks).unwrap();
        assert!(content.contains(r#"name = "user-audit""#));
        assert!(!content.contains("rtk hook codewhale"));
    }
}
