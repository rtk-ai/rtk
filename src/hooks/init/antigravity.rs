//! Google Antigravity support: transparent command rewriting via PreToolUse lifecycle hook plugin.

use super::*;
use crate::core::user_dirs;
use std::io;

pub const ANTIGRAVITY_PLUGIN_JSON: &str = r#"{
  "name": "rtk",
  "version": "1.0.0",
  "description": "Antigravity plugin for transparent command rewriting and token optimization using rtk"
}
"#;

pub const ANTIGRAVITY_HOOKS_JSON: &str = r#"{
  "rtk-rewrite": {
    "enabled": true,
    "PreToolUse": [
      {
        "matcher": "run_command",
        "hooks": [
          {
            "type": "command",
            "command": "rtk hook antigravity",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
"#;

/// The awareness rules inside the plugin bundle. The plugin docs bundled with `agy`
/// recommend `rules/AGENTS.md`: plain markdown, no frontmatter, always on while the
/// plugin is enabled.
const ANTIGRAVITY_RULES_FILE: &str = "AGENTS.md";

pub const LEGACY_RULES_REL_PATH: &str = ".agents/rules/antigravity-rtk-rules.md";

/// Shipped by v0.36.0..=v0.43.0 (`hooks/antigravity/rules.md`, md5 da893019).
const LEGACY_ANTIGRAVITY_RULES_V036: &str = r#"# RTK - Rust Token Killer (Google Antigravity)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk` to minimize token consumption.

Examples:

```bash
rtk git status
rtk cargo test
rtk ls src/
rtk grep "pattern" src/
rtk find "*.rs" .
rtk docker ps
rtk gh pr list
```

## Meta Commands

```bash
rtk gain              # Show token savings
rtk gain --history    # Command history with savings
rtk discover          # Find missed RTK opportunities
rtk proxy <cmd>       # Run raw (no filtering, for debugging)
```

## Why

RTK filters and compresses command output before it reaches the LLM context, saving 60-90% tokens on common operations. Always use `rtk <cmd>` instead of raw commands.
"#;

/// Shipped by v0.44.0..=v0.48.0 (`hooks/antigravity/rules.md`, md5 837fb66e).
const LEGACY_ANTIGRAVITY_RULES_V044: &str = r#"# RTK - Rust Token Killer (Google Antigravity)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk` to minimize token consumption.

Examples:

```bash
rtk git status
rtk cargo test
rtk ls src/
rtk grep "pattern" src/
rtk find "*.rs" .
rtk docker ps
rtk gh pr list
```

## Meta Commands

```bash
rtk gain              # Show token savings
rtk gain --history    # Command history with savings
rtk discover          # Find missed RTK opportunities
rtk proxy <cmd>       # Run raw (no filtering, for debugging)
```

## Why

RTK filters and compresses command output before it reaches the LLM context, cutting up to 90% of the bash output on common operations. Always use `rtk <cmd>` instead of raw commands.
"#;

/// Shipped by v0.49.0..=v0.50.0 (`hooks/rtk-awareness-full.md`, md5 3e20b5a4).
const LEGACY_ANTIGRAVITY_RULES_V049: &str = r#"# RTK

Prefix every shell command with `rtk`: `rtk git status`, `rtk cargo test`,
`rtk npm run build`, `rtk ls src/`. Keep the prefix inside chains:
`rtk git add . && rtk git commit -m "msg"`. Commands RTK has no filter for
run as-is, so the prefix is always safe.

# Command output

Command output here is condensed to save tokens, keeping every signal and
dropping costly noise. Treat it as the complete result: run commands
normally, and batch related commands into one call to avoid extra turns.
Truncated results state their recovery path in their own output. Re-run a
command as `rtk proxy <cmd>` only when its result is unusable: empty when
output was clearly expected, contradicting its exit code, or garbled.

## About RTK

RTK (Rust Token Killer) is a CLI proxy that filters command output to save
tokens; behavior and exit code are unchanged.

- `rtk gain` / `rtk gain --history` — token savings, overall and per command.
- `rtk proxy <cmd>` — run a command unfiltered, still tracked.
- `RTK_DISABLED=1 <cmd>` — skip RTK for one command.
- `rtk discover` — find past commands RTK could have condensed.
"#;

const LEGACY_ANTIGRAVITY_TEXTS: [&str; 3] = [
    LEGACY_ANTIGRAVITY_RULES_V036,
    LEGACY_ANTIGRAVITY_RULES_V044,
    LEGACY_ANTIGRAVITY_RULES_V049,
];

#[derive(Debug, PartialEq, Eq)]
pub enum LegacyRulesAction {
    RemovedFile(PathBuf),
    StrippedSuffix(PathBuf),
}

/// Detect and migrate legacy `.agents/rules/antigravity-rtk-rules.md` files written
/// by earlier RTK releases (best-effort).
///
/// - If the file content is solely one of the three historical awareness texts, remove it.
/// - If the file has that awareness text appended after user content following a blank line,
///   remove only the RTK suffix, keeping the user content byte-identical.
/// - Leave any other file at that path (including non-UTF-8 files) untouched and silent.
/// - Skip silently, without opening it, a path that is not a regular file (a FIFO, a
///   directory, a dangling link) or that cannot be stat'ed.
/// - When reading, removing or rewriting the file fails (other than the file vanishing before
///   it is read, or not being UTF-8), print a `[warn]` message to `stderr` naming the file and
///   return `None` so the primary install/uninstall command still succeeds.
pub fn migrate_legacy_rules_at(
    workspace_root: &Path,
    ctx: InitContext,
) -> Option<LegacyRulesAction> {
    let legacy_file = workspace_root.join(LEGACY_RULES_REL_PATH);
    let is_regular_file = match fs::metadata(&legacy_file) {
        Ok(meta) => meta.is_file(),
        Err(_) => false,
    };
    if !is_regular_file {
        return None;
    }
    if let Err(e) = ensure_project_file_inside(&legacy_file, "Antigravity") {
        eprintln!("  [warn] Failed to clean legacy Antigravity rules file: {e:#}");
        return None;
    }

    let raw = match fs::read_to_string(&legacy_file) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound || e.kind() == io::ErrorKind::InvalidData => {
            return None;
        }
        Err(e) => {
            eprintln!(
                "  [warn] Failed to clean legacy Antigravity rules file {}: {e}",
                legacy_file.display()
            );
            return None;
        }
    };

    let normalized = raw.replace("\r\n", "\n");
    let trimmed = normalized.trim();

    // 1. Exact match: file content is solely an awareness text an earlier release wrote
    for known in &LEGACY_ANTIGRAVITY_TEXTS {
        if trimmed == known.trim() {
            if !ctx.dry_run {
                // nosemgrep: filesystem-deletion -- legacy migration removes only RTK's legacy awareness rules file.
                if let Err(e) = fs::remove_file(&legacy_file) {
                    eprintln!(
                        "  [warn] Failed to clean legacy Antigravity rules file {}: {e}",
                        legacy_file.display()
                    );
                    return None;
                }
                if ctx.verbose > 0 {
                    eprintln!("Removed legacy rules file: {}", legacy_file.display());
                }
            }
            return Some(LegacyRulesAction::RemovedFile(legacy_file));
        }
    }

    // 2. Suffix removal: awareness text follows user content after a blank line
    let raw_trimmed_end = raw.trim_end();
    for known in &LEGACY_ANTIGRAVITY_TEXTS {
        let target_trimmed = known.trim();
        let crlf_text = target_trimmed.replace('\n', "\r\n");
        let crlf_suffix = format!("\r\n\r\n{crlf_text}");
        let win_append_suffix = format!("\n\n{crlf_text}");
        let lf_suffix = format!("\n\n{target_trimmed}");

        let remaining_bytes = if let Some(prefix) = raw_trimmed_end.strip_suffix(&crlf_suffix) {
            prefix
        } else if let Some(prefix) = raw_trimmed_end.strip_suffix(&win_append_suffix) {
            prefix
        } else if let Some(prefix) = raw_trimmed_end.strip_suffix(&lf_suffix) {
            prefix
        } else {
            continue;
        };

        if remaining_bytes.contains("RTK") || remaining_bytes.contains("rtk") {
            continue;
        }

        if ctx.dry_run {
            if !dry_run_would_write(&legacy_file, WriteKind::Instructions) {
                return None;
            }
        } else {
            if let Err(e) = write_file(&legacy_file, WriteKind::Instructions, remaining_bytes) {
                eprintln!(
                    "  [warn] Failed to clean legacy Antigravity rules file {}: {e}",
                    legacy_file.display()
                );
                return None;
            }
            if ctx.verbose > 0 {
                eprintln!(
                    "Removed legacy awareness text from: {}",
                    legacy_file.display()
                );
            }
        }
        return Some(LegacyRulesAction::StrippedSuffix(legacy_file));
    }

    // 3. Foreign / unrelated file: leave alone and say nothing
    None
}

fn print_legacy_install_action(action: &Option<LegacyRulesAction>, dry_run: bool) {
    match action {
        Some(LegacyRulesAction::RemovedFile(p)) => {
            if dry_run {
                println!("[dry-run] would remove legacy rules file: {}", p.display());
            } else {
                println!("  Legacy: removed {}", p.display());
            }
        }
        Some(LegacyRulesAction::StrippedSuffix(p)) => {
            if dry_run {
                println!(
                    "[dry-run] would remove legacy awareness text from {}",
                    p.display()
                );
            } else {
                println!("  Legacy: removed awareness text from {}", p.display());
            }
        }
        None => {}
    }
}

pub fn run_antigravity_mode(global: bool, ctx: InitContext) -> Result<()> {
    let _scope = (!global).then(|| ProjectScope::enter(ctx));
    if global {
        let home = user_dirs::home().context("Could not determine user home directory")?;
        let base_dir = home.join(".gemini/config");
        let cwd = user_dirs::current_dir().ok();
        run_antigravity_mode_with_workspace(&base_dir, true, cwd.as_deref(), ctx)
    } else {
        let cwd = user_dirs::current_dir().context("Failed to read current directory")?;
        run_antigravity_mode_at(&cwd, false, ctx)
    }
}

pub fn run_antigravity_mode_at(base_dir: &Path, global: bool, ctx: InitContext) -> Result<()> {
    let workspace = if !global { Some(base_dir) } else { None };
    run_antigravity_mode_with_workspace(base_dir, global, workspace, ctx)
}

fn run_antigravity_mode_with_workspace(
    base_dir: &Path,
    global: bool,
    workspace_dir: Option<&Path>,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let plugin_dir = if global {
        base_dir.join("plugins/rtk")
    } else {
        base_dir.join(".agents/plugins/rtk")
    };

    let plugin_json_path = plugin_dir.join("plugin.json");
    let hooks_json_path = plugin_dir.join("hooks.json");
    // Rules under `rules/` apply whenever the plugin is active, as plain markdown
    // with no frontmatter (Antigravity's plugin docs); `awareness.level` picks
    // the text, as it does for every agent with a command hook.
    let rules_path = plugin_dir.join("rules").join(ANTIGRAVITY_RULES_FILE);
    let rules_content = awareness_content(ctx.awareness);
    // The plugin makes Antigravity run `rtk hook antigravity`, so a project install keeps
    // every file of it inside the project.
    for path in [&rules_path, &hooks_json_path, &plugin_json_path] {
        ensure_project_file_inside(path, "Antigravity")?;
    }

    if dry_run {
        println!(
            "[dry-run] would create plugin directory: {}",
            plugin_dir.display()
        );
    }
    // plugin.json is what makes Antigravity discover the directory, so it goes last: a first
    // install that fails halfway leaves no plugin rather than one missing its rules. A re-run
    // over an existing plugin has no such guarantee.
    let rules_name = format!("rules/{ANTIGRAVITY_RULES_FILE}");
    for (path, content, name, what) in [
        (
            &rules_path,
            rules_content,
            rules_name.as_str(),
            "plugin rules",
        ),
        (
            &hooks_json_path,
            ANTIGRAVITY_HOOKS_JSON,
            "hooks.json",
            "hooks.json",
        ),
        (
            &plugin_json_path,
            ANTIGRAVITY_PLUGIN_JSON,
            "plugin.json",
            "plugin.json",
        ),
    ] {
        let report = Report::new(format!("[dry-run] would write {}", path.display()))
            .with_detail(format!("[dry-run] {name} content:\n{content}"))
            .done_verbose(format!("Wrote {}", path.display()));
        write_reported(path, WriteKind::Owned, content, ctx, report)
            .with_context(|| format!("Failed to write Antigravity {what}"))?;
    }
    let legacy_action = workspace_dir.and_then(|ws| migrate_legacy_rules_at(ws, ctx));

    if dry_run {
        print_legacy_install_action(&legacy_action, true);
        print_dry_run_footer();
    } else {
        println!("\nRTK plugin configured for Google Antigravity.\n");
        println!("  Plugin: {} (installed)", plugin_dir.display());
        println!("  Hooks:  PreToolUse -> rtk hook antigravity");
        println!(
            "  Rules:  rules/{ANTIGRAVITY_RULES_FILE} (awareness level: {})",
            ctx.awareness
        );
        print_legacy_install_action(&legacy_action, false);
        println!("  Restart Antigravity to load the plugin. Test with: git status");
        println!(
            "\n  Note: Antigravity checks permissions after hooks rewrite a command.\n  \
             If you use command allowlists, ensure `rtk` commands are permitted,\n  \
             e.g. `command(rtk git status)` or `command(rtk *)`.\n"
        );
    }

    Ok(())
}

pub fn uninstall_antigravity_mode(global: bool, ctx: InitContext) -> Result<()> {
    let _scope = (!global).then(|| ProjectScope::enter(ctx));
    let base_dir = if global {
        user_dirs::home()
            .context("Could not determine user home directory")?
            .join(".gemini/config")
    } else {
        user_dirs::current_dir().context("Failed to read current directory")?
    };
    let mut removed = uninstall_antigravity_mode_at(&base_dir, global, ctx)?;

    if global
        && let Ok(cwd) = user_dirs::current_dir()
        && let Some(action) = migrate_legacy_rules_at(&cwd, ctx)
    {
        match action {
            LegacyRulesAction::RemovedFile(p) => {
                removed.push(format!("legacy rules file: {}", p.display()));
            }
            LegacyRulesAction::StrippedSuffix(p) => {
                removed.push(format!("legacy awareness text in: {}", p.display()));
            }
        }
    }

    if removed.is_empty() {
        println!("RTK Antigravity support was not installed (nothing to remove)");
    } else {
        let header = if ctx.dry_run {
            "[dry-run] would uninstall RTK for Google Antigravity:"
        } else {
            "RTK uninstalled for Google Antigravity:"
        };
        println!("{header}");
        for item in removed {
            println!("  - {item}");
        }
    }

    if ctx.dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

/// Remove RTK's plugin directory, printing nothing. Returns what was removed, or under
/// `--dry-run` what would be.
pub fn uninstall_antigravity_mode_at(
    base_dir: &Path,
    global: bool,
    ctx: InitContext,
) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();
    let plugin_dir = if global {
        base_dir.join("plugins/rtk")
    } else {
        base_dir.join(".agents/plugins/rtk")
    };
    // Removed whole, so a project uninstall must not reach a directory outside the project.
    ensure_project_file_inside(&plugin_dir, "Antigravity")?;

    if plugin_dir.exists() {
        if !dry_run {
            // nosemgrep: filesystem-deletion -- uninstall intentionally removes only RTK's Antigravity plugin directory.
            fs::remove_dir_all(&plugin_dir).with_context(|| {
                format!(
                    "Failed to remove Antigravity plugin directory: {}",
                    plugin_dir.display()
                )
            })?;
            if verbose > 0 {
                eprintln!(
                    "Removed Antigravity plugin directory: {}",
                    plugin_dir.display()
                );
            }
        }
        removed.push(format!("Antigravity plugin: {}", plugin_dir.display()));
    }

    if !global && let Some(action) = migrate_legacy_rules_at(base_dir, ctx) {
        match action {
            LegacyRulesAction::RemovedFile(p) => {
                removed.push(format!("legacy rules file: {}", p.display()));
            }
            LegacyRulesAction::StrippedSuffix(p) => {
                removed.push(format!("legacy awareness text in: {}", p.display()));
            }
        }
    }

    Ok(removed)
}

/// Returns true if an Antigravity RTK plugin is configured (workspace or global).
pub fn is_configured() -> bool {
    // 1. Workspace plugin
    if let Ok(cwd) = user_dirs::current_dir() {
        let ws = cwd.join(".agents/plugins/rtk");
        if ws.join("plugin.json").is_file() || ws.join("hooks.json").is_file() {
            return true;
        }
    }
    // 2. Global plugin
    if let Some(home) = user_dirs::home() {
        let global = home.join(".gemini/config/plugins/rtk");
        if global.join("plugin.json").is_file() || global.join("hooks.json").is_file() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_antigravity_mode_creates_plugin_files_local() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();

        let plugin_dir = temp.path().join(".agents/plugins/rtk");
        let manifest_path = plugin_dir.join("plugin.json");
        let hooks_path = plugin_dir.join("hooks.json");

        assert!(manifest_path.exists(), "plugin.json should exist");
        assert!(hooks_path.exists(), "hooks.json should exist");

        let manifest = fs::read_to_string(&manifest_path).unwrap();
        assert!(manifest.contains(r#""name": "rtk""#));

        let hooks = fs::read_to_string(&hooks_path).unwrap();
        assert!(hooks.contains(r#""rtk-rewrite""#));
        assert!(hooks.contains(r#""command": "rtk hook antigravity""#));
    }

    #[test]
    fn test_antigravity_mode_creates_plugin_files_global() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(temp.path(), true, InitContext::default()).unwrap();

        let plugin_dir = temp.path().join("plugins/rtk");
        let manifest_path = plugin_dir.join("plugin.json");
        let hooks_path = plugin_dir.join("hooks.json");

        assert!(manifest_path.exists(), "global plugin.json should exist");
        assert!(hooks_path.exists(), "global hooks.json should exist");
        assert!(
            plugin_dir
                .join("rules")
                .join(ANTIGRAVITY_RULES_FILE)
                .is_file(),
            "global rules/AGENTS.md should exist"
        );
    }

    #[test]
    fn test_antigravity_mode_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(
            temp.path(),
            false,
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        )
        .unwrap();

        let plugin_dir = temp.path().join(".agents/plugins/rtk");
        assert!(
            !plugin_dir.exists(),
            "Plugin dir must not exist after dry run"
        );
    }

    #[test]
    fn test_antigravity_mode_reinstall_idempotent() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();
        run_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();

        let plugin_dir = temp.path().join(".agents/plugins/rtk");
        assert!(plugin_dir.join("plugin.json").exists());
        assert!(plugin_dir.join("hooks.json").exists());
    }

    #[test]
    fn test_antigravity_mode_writes_awareness_rules_at_each_level() {
        for (level, expected) in [
            (AwarenessLevel::Default, RTK_AWARENESS_DEFAULT),
            (AwarenessLevel::High, RTK_AWARENESS_HIGH),
            (AwarenessLevel::Full, RTK_AWARENESS_FULL),
        ] {
            let temp = TempDir::new().unwrap();
            run_antigravity_mode_at(
                temp.path(),
                false,
                InitContext {
                    awareness: level,
                    ..InitContext::default()
                },
            )
            .unwrap();
            let rules = fs::read_to_string(
                temp.path()
                    .join(".agents/plugins/rtk/rules")
                    .join(ANTIGRAVITY_RULES_FILE),
            )
            .unwrap();
            assert_eq!(rules, expected, "awareness level {level}");
            assert!(
                !rules.starts_with("---"),
                "plugin rules are plain markdown, no frontmatter"
            );
        }
    }

    #[test]
    fn test_antigravity_mode_reinit_rewrites_rules_for_a_new_level() {
        let temp = TempDir::new().unwrap();
        let rules_path = temp
            .path()
            .join(".agents/plugins/rtk/rules")
            .join(ANTIGRAVITY_RULES_FILE);
        for (level, expected) in [
            (AwarenessLevel::Default, RTK_AWARENESS_DEFAULT),
            (AwarenessLevel::Full, RTK_AWARENESS_FULL),
        ] {
            let ctx = InitContext {
                awareness: level,
                ..InitContext::default()
            };
            run_antigravity_mode_at(temp.path(), false, ctx).unwrap();
            assert_eq!(
                fs::read_to_string(&rules_path).unwrap(),
                expected,
                "{level}"
            );
        }
    }

    #[test]
    fn test_antigravity_mode_uninstall_removes_plugin() {
        let temp = TempDir::new().unwrap();
        run_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();

        let removed =
            uninstall_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();
        assert_eq!(removed.len(), 1);

        let plugin_dir = temp.path().join(".agents/plugins/rtk");
        assert!(!plugin_dir.exists(), "Plugin dir should be removed");
    }

    #[test]
    fn test_migrate_legacy_rules_exact_match_removed_for_all_historical_releases() {
        for text in [
            LEGACY_ANTIGRAVITY_RULES_V036,
            LEGACY_ANTIGRAVITY_RULES_V044,
            LEGACY_ANTIGRAVITY_RULES_V049,
        ] {
            let temp = TempDir::new().unwrap();
            let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
            fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
            fs::write(&legacy_file, text).unwrap();

            let action = migrate_legacy_rules_at(temp.path(), InitContext::default());
            assert_eq!(
                action,
                Some(LegacyRulesAction::RemovedFile(legacy_file.clone()))
            );
            assert!(!legacy_file.exists());
        }
    }

    #[test]
    fn test_migrate_legacy_rules_crlf_exact_match_removed() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        let crlf_text = LEGACY_ANTIGRAVITY_RULES_V036.replace('\n', "\r\n");
        fs::write(&legacy_file, &crlf_text).unwrap();

        let action = migrate_legacy_rules_at(temp.path(), InitContext::default());
        assert_eq!(
            action,
            Some(LegacyRulesAction::RemovedFile(legacy_file.clone()))
        );
        assert!(!legacy_file.exists());
    }

    #[test]
    fn test_migrate_legacy_rules_suffix_stripped_for_all_historical_releases() {
        let user_content = "# Team rules\n\nAlways write tests.";
        for text in [
            LEGACY_ANTIGRAVITY_RULES_V036,
            LEGACY_ANTIGRAVITY_RULES_V044,
            LEGACY_ANTIGRAVITY_RULES_V049,
        ] {
            let temp = TempDir::new().unwrap();
            let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
            fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
            let full_content = format!("{user_content}\n\n{text}");
            fs::write(&legacy_file, &full_content).unwrap();

            let action = migrate_legacy_rules_at(temp.path(), InitContext::default());
            assert_eq!(
                action,
                Some(LegacyRulesAction::StrippedSuffix(legacy_file.clone()))
            );
            assert!(legacy_file.exists());
            let updated = fs::read_to_string(&legacy_file).unwrap();
            assert_eq!(updated, user_content);
        }
    }

    #[test]
    fn test_migrate_legacy_rules_crlf_suffix_preserves_crlf_user_bytes() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        let user_content = "# Team rules\r\n\r\nAlways write tests.";
        let crlf_legacy = LEGACY_ANTIGRAVITY_RULES_V036.replace('\n', "\r\n");
        let full_content = format!("{user_content}\r\n\r\n{crlf_legacy}");
        fs::write(&legacy_file, &full_content).unwrap();

        let action = migrate_legacy_rules_at(temp.path(), InitContext::default());
        assert_eq!(
            action,
            Some(LegacyRulesAction::StrippedSuffix(legacy_file.clone()))
        );
        let updated = fs::read_to_string(&legacy_file).unwrap();
        assert_eq!(updated, user_content);
    }

    #[test]
    fn test_migrate_legacy_rules_windows_crlf_suffix_preserves_crlf_user_bytes() {
        let user_content = "# Team rules\r\n\r\nAlways write tests.";
        for text in [
            LEGACY_ANTIGRAVITY_RULES_V036,
            LEGACY_ANTIGRAVITY_RULES_V044,
            LEGACY_ANTIGRAVITY_RULES_V049,
        ] {
            let temp = TempDir::new().unwrap();
            let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
            fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
            let crlf_legacy = text.replace('\n', "\r\n");
            // Windows writer appended "\n\n" followed by CRLF text
            let full_content = format!("{user_content}\n\n{crlf_legacy}");
            fs::write(&legacy_file, &full_content).unwrap();

            let action = migrate_legacy_rules_at(temp.path(), InitContext::default());
            assert_eq!(
                action,
                Some(LegacyRulesAction::StrippedSuffix(legacy_file.clone()))
            );
            let updated = fs::read_to_string(&legacy_file).unwrap();
            assert_eq!(updated, user_content);
        }
    }

    #[test]
    fn test_migrate_legacy_rules_single_newline_separator_untouched() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        let single_nl = format!(
            "# Team rules\n\nAlways write tests.\n{}",
            LEGACY_ANTIGRAVITY_RULES_V036
        );
        fs::write(&legacy_file, &single_nl).unwrap();

        let action = migrate_legacy_rules_at(temp.path(), InitContext::default());
        assert_eq!(action, None);
        assert_eq!(fs::read_to_string(&legacy_file).unwrap(), single_nl);
    }

    /// Earlier releases appended nothing to a file already mentioning `RTK` or `rtk`, so a
    /// prefix that does was written by hand and keeps the block that follows it.
    #[test]
    fn test_migrate_legacy_rules_prefix_mentioning_rtk_untouched() {
        for (prefix, text) in [
            (
                "Prefer RTK Query for data fetching.",
                LEGACY_ANTIGRAVITY_RULES_V036,
            ),
            ("Use rtk for git commands.", LEGACY_ANTIGRAVITY_RULES_V049),
        ] {
            let temp = TempDir::new().unwrap();
            let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
            fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
            let content = format!("{prefix}\n\n{text}");
            fs::write(&legacy_file, &content).unwrap();

            let action = migrate_legacy_rules_at(temp.path(), InitContext::default());
            assert_eq!(action, None, "{prefix}");
            assert_eq!(fs::read_to_string(&legacy_file).unwrap(), content);
        }
    }

    /// A FIFO is skipped without being opened: opening it would block until a writer shows up.
    /// The writer here would hand over the exact legacy text, so a migration that opened the
    /// FIFO would remove it.
    #[cfg(unix)]
    #[test]
    fn test_migrate_legacy_rules_skips_fifo_without_opening_it() {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;

        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        // Shelled out rather than called through libc: `unsafe` is not allowed outside proxy
        // mode's signal handling.
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&legacy_file)
                .status()
                .unwrap()
                .success(),
            "mkfifo failed"
        );

        let writer_path = legacy_file.clone();
        let writer = std::thread::spawn(move || {
            if let Ok(mut handle) = fs::OpenOptions::new().write(true).open(&writer_path) {
                let _ = handle.write_all(LEGACY_ANTIGRAVITY_RULES_V036.as_bytes());
            }
        });

        let action = migrate_legacy_rules_at(temp.path(), InitContext::default());
        assert_eq!(action, None);
        assert!(legacy_file.exists(), "the FIFO must be left in place");

        // Open a non-blocking reader so the writer's open returns and the thread ends.
        let _reader = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&legacy_file)
            .unwrap();
        writer.join().unwrap();
    }

    #[test]
    fn test_migrate_legacy_rules_does_not_match_high_default_or_post_bypass_full() {
        for text in [
            RTK_AWARENESS_HIGH,
            RTK_AWARENESS_DEFAULT,
            RTK_AWARENESS_FULL,
        ] {
            let temp = TempDir::new().unwrap();
            let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
            fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
            fs::write(&legacy_file, text).unwrap();

            let action = migrate_legacy_rules_at(temp.path(), InitContext::default());
            assert_eq!(action, None);
            assert!(legacy_file.exists());
        }
    }

    #[test]
    fn test_migrate_legacy_rules_foreign_untouched() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        let foreign_content = "Prefer RTK Query for data fetching; do not add axios.";
        fs::write(&legacy_file, foreign_content).unwrap();

        let action = migrate_legacy_rules_at(temp.path(), InitContext::default());
        assert_eq!(action, None);
        assert!(legacy_file.exists());
        assert_eq!(fs::read_to_string(&legacy_file).unwrap(), foreign_content);
    }

    #[test]
    fn test_migrate_legacy_rules_dry_run_leaves_file() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        fs::write(&legacy_file, LEGACY_ANTIGRAVITY_RULES_V036).unwrap();

        let action = migrate_legacy_rules_at(
            temp.path(),
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        );
        assert_eq!(
            action,
            Some(LegacyRulesAction::RemovedFile(legacy_file.clone()))
        );
        assert!(legacy_file.exists(), "Dry run must not delete file");
    }

    #[test]
    fn test_antigravity_uninstall_with_only_legacy_rules_removes_them() {
        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        fs::write(&legacy_file, LEGACY_ANTIGRAVITY_RULES_V036).unwrap();

        let removed =
            uninstall_antigravity_mode_at(temp.path(), false, InitContext::default()).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(removed[0].contains("legacy rules file"));
        assert!(!legacy_file.exists());
    }

    #[test]
    fn test_antigravity_global_uninstall_cleans_workspace_legacy_rules() {
        let _guard = crate::core::test_isolation::enter(&crate::core::test_isolation::project());
        let workspace = crate::core::user_dirs::current_dir().unwrap();
        let legacy_file = workspace.join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        fs::write(&legacy_file, LEGACY_ANTIGRAVITY_RULES_V036).unwrap();

        uninstall_antigravity_mode(true, InitContext::default()).unwrap();
        assert!(
            !legacy_file.exists(),
            "global uninstall must remove workspace legacy rules file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_project_install_refuses_a_plugin_dir_linked_outside_the_project() {
        use crate::core::test_isolation;
        use std::os::unix::fs::symlink;

        let project = test_isolation::tempdir();
        let elsewhere = TempDir::new().expect("elsewhere");
        symlink(elsewhere.path(), project.path().join(".agents")).expect("symlink");

        let _entered = test_isolation::enter(project.path());
        let error = run_antigravity_mode(false, InitContext::default())
            .expect_err("a plugin outside the project is refused");

        assert!(error.to_string().contains("outside the project"), "{error}");
        assert!(
            !elsewhere.path().join("plugins").exists(),
            "nothing is written through the link"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_project_uninstall_leaves_a_plugin_dir_linked_outside_the_project_alone() {
        use crate::core::test_isolation;
        use std::os::unix::fs::symlink;

        let project = test_isolation::tempdir();
        let elsewhere = TempDir::new().expect("elsewhere");
        let outside_plugin = elsewhere.path().join("plugins/rtk");
        fs::create_dir_all(&outside_plugin).expect("outside plugin dir");
        fs::write(outside_plugin.join("plugin.json"), ANTIGRAVITY_PLUGIN_JSON).expect("write");
        symlink(elsewhere.path(), project.path().join(".agents")).expect("symlink");

        let _entered = test_isolation::enter(project.path());
        let error = uninstall_antigravity_mode(false, InitContext::default())
            .expect_err("a plugin outside the project is refused");

        assert!(error.to_string().contains("outside the project"), "{error}");
        assert!(
            outside_plugin.join("plugin.json").exists(),
            "a directory outside the project is not removed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_migration_dry_run_does_not_announce_a_strip_the_real_run_refuses() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let legacy_file = temp.path().join(LEGACY_RULES_REL_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        let content = format!("# Mine\n\n{LEGACY_ANTIGRAVITY_RULES_V036}");
        fs::write(&legacy_file, &content).unwrap();
        fs::set_permissions(&legacy_file, fs::Permissions::from_mode(0o444)).unwrap();
        // Root ignores the file's mode, and then the strip is simply made.
        let locked_for_us = fs::OpenOptions::new()
            .write(true)
            .open(&legacy_file)
            .is_err();

        let dry_run = migrate_legacy_rules_at(
            temp.path(),
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        );
        let real_run = migrate_legacy_rules_at(temp.path(), InitContext::default());

        assert_eq!(dry_run.is_some(), real_run.is_some());
        if locked_for_us {
            assert_eq!(real_run, None);
            assert_eq!(fs::read_to_string(&legacy_file).unwrap(), content);
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_project_migration_leaves_legacy_rules_linked_outside_the_project_alone() {
        use crate::core::test_isolation;
        use std::os::unix::fs::symlink;

        let project = test_isolation::tempdir();
        let elsewhere = TempDir::new().expect("elsewhere");
        fs::create_dir(project.path().join(".agents")).expect("mkdir");
        symlink(elsewhere.path(), project.path().join(".agents/rules")).expect("symlink");
        let outside_file = elsewhere.path().join("antigravity-rtk-rules.md");
        let _entered = test_isolation::enter(project.path());

        for content in [
            LEGACY_ANTIGRAVITY_RULES_V036.to_string(),
            format!("# Mine\n\n{LEGACY_ANTIGRAVITY_RULES_V036}"),
        ] {
            fs::write(&outside_file, &content).expect("write");

            uninstall_antigravity_mode(false, InitContext::default()).expect("uninstall");

            assert_eq!(fs::read_to_string(&outside_file).expect("read"), content);
        }
    }
}
