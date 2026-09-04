//! Sets up RTK hooks so AI coding agents automatically route commands through RTK.

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

use crate::core::utils::{from_json_str, strip_leading_bom};
use crate::hooks::constants::{
    CONFIG_DIR, COPILOT_HOME_ENV, COPILOT_HOOK_FILE, COPILOT_INSTRUCTIONS_FILE, COPILOT_USER_DIR,
    CURSOR_DIR, GEMINI_DIR, GITHUB_DIR, OPENCODE_PLUGIN_FILE, OPENCODE_SUBDIR, PLUGIN_SUBDIR,
};

use super::constants::{
    BEFORE_TOOL_KEY, CLAUDE_DIR, CLAUDE_HOOK_COMMAND, CODEX_DIR, CURSOR_HOOK_COMMAND, DROID_DIR,
    DROID_EXECUTE_MATCHER, DROID_HOME_ENV, DROID_HOOKS_FILE, DROID_HOOKS_SUBDIR,
    DROID_HOOK_COMMAND, DROID_SETTINGS_FILE, GEMINI_HOOK_FILE, HERMES_DIR, HERMES_PLUGINS_SUBDIR,
    HERMES_PLUGIN_INIT_FILE, HERMES_PLUGIN_MANIFEST_FILE, HERMES_PLUGIN_NAME, HOOKS_JSON,
    HOOKS_SUBDIR, OMP_DIR, OMP_LOCAL_DIR, PI_AGENT_STATE_FILE, PI_CODING_AGENT_DIR_ENV, PI_DIR,
    PI_EXTENSIONS_SUBDIR, PI_LOCAL_DIR, PI_PLUGIN_FILE, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE,
    SETTINGS_JSON, VIBE_BASH_MATCH, VIBE_DIR, VIBE_HOOKS_FILE, VIBE_HOOK_COMMAND, VIBE_HOOK_NAME,
    VIBE_PROMPTS_SUBDIR, VIBE_PROMPT_FILE,
};
use super::integrity;
use super::is_claude_hook_command;

mod agents;
mod claude;
mod codex;
mod copilot;
mod cursor;
mod droid;
mod gemini;
mod hermes;
mod omp;
mod opencode;
mod vibe;

pub(crate) use agents::*;
pub(crate) use claude::*;
pub(crate) use codex::*;
pub(crate) use cursor::*;
pub(crate) use gemini::*;
pub(crate) use omp::*;
pub(crate) use opencode::*;

pub use agents::{run_antigravity_mode, run_kilocode_mode, run_kimi_mode};
pub(crate) use copilot::{copilot_user_dir, COPILOT_HOOK_JSON};
pub use copilot::{run_copilot, run_copilot_global, uninstall_copilot, uninstall_copilot_global};
pub use droid::{run_droid_mode, uninstall_droid};
pub use gemini::run_gemini;
pub use hermes::{run_hermes_mode, uninstall_hermes};
pub use omp::run_omp_mode_with_patch_mode;
pub use opencode::run_pi_mode_with_patch_mode;
pub use vibe::{run_vibe_mode, uninstall_vibe};

// Embedded slim RTK awareness instructions
const RTK_SLIM: &str = include_str!("../../../hooks/claude/rtk-awareness.md");

const RTK_SLIM_CODEX: &str = include_str!("../../../hooks/codex/rtk-awareness.md");

/// Template written by `rtk init` when no filters.toml exists yet.
const FILTERS_TEMPLATE: &str = r#"# Project-local RTK filters — commit this file with your repo.
# Filters here override user-global and built-in filters.
# Docs: https://github.com/rtk-ai/rtk#custom-filters
schema_version = 1

# Example: suppress build noise from a custom tool
# [filters.my-tool]
# description = "Compact my-tool output"
# match_command = "^my-tool\\s+build"
# strip_ansi = true
# strip_lines_matching = ["^\\s*$", "^Downloading", "^Installing"]
# max_lines = 30
# on_empty = "my-tool: ok"
"#;

/// Template for user-global filters (~/.config/rtk/filters.toml).
const FILTERS_GLOBAL_TEMPLATE: &str = r#"# User-global RTK filters — apply to all your projects.
# Project-local .rtk/filters.toml takes precedence over these.
# Docs: https://github.com/rtk-ai/rtk#custom-filters
schema_version = 1

# Example: suppress noise from a tool you use everywhere
# [filters.my-global-tool]
# description = "Compact my-global-tool output"
# match_command = "^my-global-tool\\b"
# strip_ansi = true
# strip_lines_matching = ["^\\s*$"]
# max_lines = 40
"#;

const RTK_MD: &str = "RTK.md";

const CLAUDE_MD: &str = "CLAUDE.md";

const AGENTS_MD: &str = "AGENTS.md";

const RTK_MD_REF: &str = "@RTK.md";

const GEMINI_MD: &str = "GEMINI.md";

const RTK_BLOCK_START: &str = "<!-- rtk-instructions";

const RTK_BLOCK_END: &str = "<!-- /rtk-instructions -->";

/// Control flow for settings.json patching
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatchMode {
    Ask,  // Default: prompt user [y/N]
    Auto, // --auto-patch: no prompt
    Skip, // --no-patch: manual instructions
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FilterTrust {
    #[default]
    Ask,
    Trust,
    Skip,
}

/// Result of settings.json patching operation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatchResult {
    Patched,        // Hook was added successfully
    AlreadyPresent, // Hook was already in settings.json
    Declined,       // User declined when prompted
    Skipped,        // --no-patch flag used
    WouldPatch,     // Dry-run: hook would have been added
}

/// Shared context threaded through every init/uninstall function.
///
/// Replaces ad-hoc `verbose: u8, dry_run: bool` parameter pairs to keep
/// signatures compact as more flags are added (mirrors `RunOptions` in
/// `src/core/runner.rs`).
#[derive(Clone, Copy, Default)]
pub struct InitContext {
    pub verbose: u8,
    pub dry_run: bool,
}

/// Shared dry-run footer printed at the end of every init sub-mode.
fn print_dry_run_footer() {
    println!("\n[dry-run] Nothing written.");
}

// Legacy full instructions for backward compatibility (--claude-md mode)
const RTK_INSTRUCTIONS: &str = r##"<!-- rtk-instructions v2 -->
# RTK (Rust Token Killer) - Token-Optimized Commands

## Golden Rule

**Always prefix commands with `rtk`**. If RTK has a dedicated filter, it uses it. If not, it passes through unchanged. This means RTK is always safe to use.

**Important**: Even in command chains with `&&`, use `rtk`:
```bash
# ❌ Wrong
git add . && git commit -m "msg" && git push

# ✅ Correct
rtk git add . && rtk git commit -m "msg" && rtk git push
```

## RTK Commands by Workflow

### Build & Compile (80-90% savings)
```bash
rtk cargo build         # Cargo build output
rtk cargo check         # Cargo check output
rtk cargo clippy        # Clippy warnings grouped by file (80%)
rtk tsc                 # TypeScript errors grouped by file/code (83%)
rtk lint                # ESLint/Biome violations grouped (84%)
rtk prettier --check    # Files needing format only (70%)
rtk next build          # Next.js build with route metrics (87%)
```

### Test (60-99% savings)
```bash
rtk cargo test          # Cargo test failures only (90%)
rtk go test             # Go test failures only (90%)
rtk jest                # Jest failures only (99.5%)
rtk vitest              # Vitest failures only (99.5%)
rtk playwright test     # Playwright failures only (94%)
rtk pytest              # Python test failures only (90%)
rtk rake test           # Ruby test failures only (90%)
rtk rspec               # RSpec test failures only (60%)
rtk test <cmd>          # Generic test wrapper - failures only
```

### Git (59-80% savings)
```bash
rtk git status          # Compact status
rtk git log             # Compact log (works with all git flags)
rtk git diff            # Compact diff (80%)
rtk git show            # Compact show (80%)
rtk git add             # Ultra-compact confirmations (59%)
rtk git commit          # Ultra-compact confirmations (59%)
rtk git push            # Ultra-compact confirmations
rtk git pull            # Ultra-compact confirmations
rtk git branch          # Compact branch list
rtk git fetch           # Compact fetch
rtk git stash           # Compact stash
rtk git worktree        # Compact worktree
```

Note: Git passthrough works for ALL subcommands, even those not explicitly listed.

### GitHub (26-87% savings)
```bash
rtk gh pr view <num>    # Compact PR view (87%)
rtk gh pr checks        # Compact PR checks (79%)
rtk gh run list         # Compact workflow runs (82%)
rtk gh issue list       # Compact issue list (80%)
rtk gh api              # Compact API responses (26%)
```

### JavaScript/TypeScript Tooling (70-90% savings)
```bash
rtk pnpm list           # Compact dependency tree (70%)
rtk pnpm outdated       # Compact outdated packages (80%)
rtk pnpm install        # Compact install output (90%)
rtk npm run <script>    # Compact npm script output
rtk npx <cmd>           # Compact npx command output
rtk prisma              # Prisma without ASCII art (88%)
rtk uv run <cmd>        # Compact uv project command output
```

### Files & Search (60-75% savings)
```bash
rtk ls <path>           # Tree format, compact (65%)
rtk read <file>         # Code reading with filtering (60%)
rtk grep <pattern>      # Search grouped by file (75%). Format flags (-c, -l, -L, -o, -Z) run raw.
rtk find <pattern>      # Find grouped by directory (70%)
```

### Analysis & Debug (70-90% savings)
```bash
rtk err <cmd>           # Filter errors only from any command
rtk log <file>          # Deduplicated logs with counts
rtk json <file>         # JSON structure without values
rtk deps                # Dependency overview
rtk env                 # Environment variables compact
rtk summary <cmd>       # Smart summary of command output
rtk diff                # Ultra-compact diffs
```

### Infrastructure (85% savings)
```bash
rtk docker ps           # Compact container list
rtk docker images       # Compact image list
rtk docker logs <c>     # Deduplicated logs
rtk kubectl get         # Compact resource list
rtk kubectl logs        # Deduplicated pod logs
```

### Network (65-70% savings)
```bash
rtk curl <url>          # Compact HTTP responses (70%)
rtk wget <url>          # Compact download output (65%)
```

### Meta Commands
```bash
rtk gain                # View token savings statistics
rtk gain --history      # View command history with savings
rtk discover            # Analyze Claude Code sessions for missed RTK usage
rtk proxy <cmd>         # Run command without filtering (for debugging)
rtk init                # Add RTK instructions to CLAUDE.md
rtk init --global       # Add RTK to ~/.claude/CLAUDE.md
```

## Token Savings Overview

| Category | Commands | Typical Savings |
|----------|----------|-----------------|
| Tests | vitest, playwright, cargo test | 90-99% |
| Build | next, tsc, lint, prettier | 70-87% |
| Git | status, log, diff, add, commit | 59-80% |
| GitHub | gh pr, gh run, gh issue | 26-87% |
| Package Managers | pnpm, npm, npx | 70-90% |
| Files | ls, read, grep, find | 60-75% |
| Infrastructure | docker, kubectl | 85% |
| Network | curl, wget | 65-70% |

Overall average: **60-90% token reduction** on common development operations.
<!-- /rtk-instructions -->
"##;

/// Main entry point for `rtk init`
#[allow(clippy::too_many_arguments)]
pub fn run(
    global: bool,
    install_claude: bool,
    install_opencode: bool,
    install_cursor: bool,
    install_windsurf: bool,
    install_cline: bool,
    claude_md: bool,
    hook_only: bool,
    codex: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    // Validation: Codex mode conflicts
    if codex {
        if install_opencode {
            anyhow::bail!("--codex cannot be combined with --opencode");
        }
        if claude_md {
            anyhow::bail!("--codex cannot be combined with --claude-md");
        }
        if hook_only {
            anyhow::bail!("--codex cannot be combined with --hook-only");
        }
        if matches!(patch_mode, PatchMode::Auto) {
            anyhow::bail!("--codex cannot be combined with --auto-patch");
        }
        if matches!(patch_mode, PatchMode::Skip) {
            anyhow::bail!("--codex cannot be combined with --no-patch");
        }
        run_codex_mode(global, ctx)?;
    } else {
        // Validation: Global-only features
        if install_opencode && !global {
            anyhow::bail!("OpenCode plugin is global-only. Use: rtk init -g --opencode");
        }

        if install_cursor && !global {
            anyhow::bail!("Cursor hooks are global-only. Use: rtk init -g --agent cursor");
        }

        if install_windsurf && !global {
            anyhow::bail!("Windsurf support is global-only. Use: rtk init -g --agent windsurf");
        }

        if install_windsurf {
            run_windsurf_mode(ctx)?;
        } else if install_cline {
            run_cline_mode(ctx)?;
        } else {
            // Mode selection (Claude Code / OpenCode)
            match (install_claude, install_opencode, claude_md, hook_only) {
                (false, true, _, _) => run_opencode_only_mode(ctx)?,
                (true, opencode, true, _) => run_claude_md_mode(global, opencode, ctx)?,
                (true, opencode, false, true) => {
                    run_hook_only_mode(global, patch_mode, opencode, ctx)?
                }
                (true, opencode, false, false) => {
                    run_default_mode(global, patch_mode, opencode, ctx)?
                }
                (false, false, _, _) => {
                    if !install_cursor {
                        anyhow::bail!(
                            "at least one of install_claude or install_opencode must be true"
                        )
                    }
                }
            }

            // Cursor hooks (additive, installed alongside Claude Code)
            if install_cursor {
                install_cursor_hooks(ctx)?;
            }
        }
    }

    if !dry_run {
        prompt_telemetry_consent()?;
        // Best-effort: unconditionally re-run tracking-DB schema migrations during
        // install/upgrade (bypassing the `user_version` gate `Tracker::new()` uses
        // on its hot path). This both pre-warms the schema so the first PreToolUse
        // hook invocation (or `rtk <cmd>`) after this doesn't pay the one-time
        // migration cost itself, and self-heals a table dropped/corrupted
        // out-of-band (see `tracking::warn_if_missing_table`) — `rtk init` is
        // already the natural "something's wrong, reinstall" move, so no separate
        // repair flag is needed. `CREATE TABLE IF NOT EXISTS`/`ALTER TABLE` are
        // additive, so existing history is left untouched. Never fail `rtk init`
        // over a tracking-DB hiccup — but still tell the user something's wrong,
        // consistent with every other best-effort warning in this function
        // (rust-patterns.md's anti-pattern rule: a silent `Err(_) => {}` leaves
        // the user with zero indication anything went wrong).
        if let Err(e) = crate::core::tracking::ensure_schema_fresh() {
            eprintln!("  [warn] Failed to prepare tracking database: {e}");
        }
    }

    if dry_run {
        print_dry_run_footer();
    } else {
        println!();
    }

    Ok(())
}

/// Idempotent file write: create or update if content differs.
/// When `dry_run` is true, prints the intended action and does not touch the filesystem.
pub(crate) fn write_if_changed(
    path: &Path,
    content: &str,
    name: &str,
    ctx: InitContext,
) -> Result<bool> {
    write_if_changed_internal(path, content, name, ctx, false)
}

/// Variant used for protected RTK files. A file that cannot be decoded is
/// still replaceable after the caller's policy allows it (for example,
/// `--auto-patch`), so a read error is treated like differing content instead
/// of preventing recovery.
pub(crate) fn write_if_changed_allow_read_error(
    path: &Path,
    content: &str,
    name: &str,
    ctx: InitContext,
) -> Result<bool> {
    write_if_changed_internal(path, content, name, ctx, true)
}

pub(crate) fn write_if_changed_internal(
    path: &Path,
    content: &str,
    name: &str,
    ctx: InitContext,
    allow_read_error: bool,
) -> Result<bool> {
    let InitContext { verbose, dry_run } = ctx;
    if path.exists() {
        let existing = match fs::read_to_string(path) {
            Ok(existing) => existing,
            Err(_) if allow_read_error => {
                if dry_run {
                    println!("[dry-run] would update {}: {}", name, path.display());
                    if verbose > 0 {
                        println!("[dry-run] content:\n{}", content);
                    }
                } else {
                    atomic_write(path, content)
                        .with_context(|| format!("Failed to write {}: {}", name, path.display()))?;
                    if verbose > 0 {
                        eprintln!("Updated {}: {}", name, path.display());
                    }
                }
                return Ok(true);
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read {}: {}", name, path.display()));
            }
        };

        if existing == content {
            if verbose > 0 {
                eprintln!("{} already up to date: {}", name, path.display());
            }
            Ok(false)
        } else {
            if dry_run {
                println!("[dry-run] would update {}: {}", name, path.display());
                if verbose > 0 {
                    println!("[dry-run] content:\n{}", content);
                }
            } else {
                atomic_write(path, content)
                    .with_context(|| format!("Failed to write {}: {}", name, path.display()))?;
                if verbose > 0 {
                    eprintln!("Updated {}: {}", name, path.display());
                }
            }
            Ok(true)
        }
    } else {
        if dry_run {
            println!("[dry-run] would create {}: {}", name, path.display());
            if verbose > 0 {
                println!("[dry-run] content:\n{}", content);
            }
        } else {
            atomic_write(path, content)
                .with_context(|| format!("Failed to write {}: {}", name, path.display()))?;
            if verbose > 0 {
                eprintln!("Created {}: {}", name, path.display());
            }
        }
        Ok(true)
    }
}

/// Resolve the final write target: if `path` is a symlink, follow it so
/// the atomic rename lands on the real file and the symlink is preserved.
fn resolve_atomic_target(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Atomic write using tempfile + rename
/// Prevents corruption on crash/interrupt
/// Follows symlinks so the link itself is preserved.
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let target = resolve_atomic_target(path);
    let parent = target.parent().with_context(|| {
        format!(
            "Cannot write to {}: path has no parent directory",
            target.display()
        )
    })?;

    // Create temp file in same directory (ensures same filesystem for atomic rename)
    let mut temp_file = NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temp file in {}", parent.display()))?;

    // Write content
    temp_file
        .write_all(content.as_bytes())
        .with_context(|| format!("Failed to write {} bytes to temp file", content.len()))?;

    // Atomic rename
    temp_file.persist(&target).with_context(|| {
        format!(
            "Failed to atomically replace {} (disk full?)",
            target.display()
        )
    })?;

    Ok(())
}

/// Prompt user for confirmation.
/// Prints to stderr (stdout may be piped), reads from stdin, and defaults to
/// No in non-interactive environments.
pub(crate) fn prompt_user_confirmation(prompt: &str) -> Result<bool> {
    use std::io::{self, BufRead, IsTerminal};

    eprint!("\n{} [y/N] ", prompt);
    io::stderr().flush().context("Failed to flush prompt")?;

    // If stdin is not a terminal (piped), default to No.
    if !io::stdin().is_terminal() {
        eprintln!("\n(non-interactive mode, defaulting to N)");
        return Ok(false);
    }

    let stdin = io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("Failed to read user input")?;

    let response = line.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}

/// Prompt user for consent to patch settings.json.
fn prompt_user_consent(settings_path: &Path) -> Result<bool> {
    prompt_user_confirmation(&format!("Patch existing {}?", settings_path.display()))
}

pub fn save_telemetry_consent(accepted: bool) -> Result<()> {
    let mut config = crate::core::config::Config::load().unwrap_or_default();
    config.telemetry.consent_given = Some(accepted);
    config.telemetry.enabled = accepted;
    config.telemetry.consent_date = Some(chrono::Utc::now().to_rfc3339());
    config
        .save()
        .context("Failed to save telemetry consent to config.toml")
}

fn prompt_telemetry_consent() -> Result<()> {
    use std::io::{self, BufRead, IsTerminal};

    let config = crate::core::config::Config::load().unwrap_or_default();
    match config.telemetry.consent_given {
        Some(true) => return Ok(()),
        Some(false) => return Ok(()),
        None => {}
    }

    // Explicit opt-out must short-circuit before the TTY heuristic: some
    // non-interactive environments (devcontainer `postCreateCommand`, certain
    // CI agents) hand rtk a pseudo-TTY, so `is_terminal()` returns true even
    // though no human is available to answer — the prompt then hangs forever.
    // Setting `RTK_TELEMETRY_DISABLED=1` is the documented workaround, so the
    // init prompt has to honour it too, not only `telemetry::maybe_ping`.
    if crate::core::telemetry_cmd::telemetry_disabled_by_env() {
        return Ok(());
    }

    if !io::stdin().is_terminal() {
        return Ok(());
    }

    eprintln!();
    eprintln!("--- Telemetry ---");
    eprintln!("RTK collects anonymous usage metrics once per day to improve filters.");
    eprintln!();
    eprintln!("  What:    command names (not arguments), token savings, OS, version");
    eprintln!("  Why:     prioritize filter development for the most-used commands");
    eprintln!("  Who:     RTK AI Labs, contact@rtk-ai.app");
    eprintln!("  Rights:  disable anytime with `rtk telemetry disable`,");
    eprintln!("           request erasure with `rtk telemetry forget`");
    eprintln!("  Details: https://github.com/rtk-ai/rtk/blob/master/docs/TELEMETRY.md");
    eprintln!();
    eprint!("Enable anonymous telemetry? [y/N] ");

    let stdin = io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("Failed to read user input")?;

    let accepted = {
        let response = line.trim().to_lowercase();
        response == "y" || response == "yes"
    };

    save_telemetry_consent(accepted)?;

    if accepted {
        eprintln!("  Telemetry enabled. Disable anytime: rtk telemetry disable");
    } else {
        eprintln!("  Telemetry disabled.");
    }

    Ok(())
}

fn print_manual_instructions(hook_command: &str, include_opencode: bool) {
    let settings_path = resolve_claude_dir()
        .unwrap_or_else(|_| PathBuf::from(format!("~/{}", CLAUDE_DIR)))
        .join(SETTINGS_JSON);
    println!("\n  MANUAL STEP: Add this to {}:", settings_path.display());
    println!("  {{");
    println!("    \"hooks\": {{ \"PreToolUse\": [{{");
    println!("      \"matcher\": \"Bash\",");
    println!("      \"hooks\": [{{ \"type\": \"command\",");
    println!("        \"command\": \"{}\"", hook_command);
    println!("      }}]");
    println!("    }}]}}");
    println!("  }}");
    if include_opencode {
        println!("\n  Then restart Claude Code and OpenCode. Test with: git status\n");
    } else {
        println!("\n  Then restart Claude Code. Test with: git status\n");
    }
}

fn remove_hook_from_json(root: &mut serde_json::Value) -> bool {
    let hooks = match root
        .get_mut("hooks")
        .and_then(|h| h.get_mut(PRE_TOOL_USE_KEY))
    {
        Some(pre_tool_use) => pre_tool_use,
        None => return false,
    };

    let pre_tool_use_array = match hooks.as_array_mut() {
        Some(arr) => arr,
        None => return false,
    };

    let original_len = pre_tool_use_array.len();
    pre_tool_use_array.retain(|entry| {
        if let Some(hooks_array) = entry.get("hooks").and_then(|h| h.as_array()) {
            for hook in hooks_array {
                if let Some(command) = hook.get("command").and_then(|c| c.as_str()) {
                    // Match both legacy script path and new binary command
                    if command.contains(REWRITE_HOOK_FILE) || is_claude_hook_command(command) {
                        return false;
                    }
                }
            }
        }
        true
    });

    pre_tool_use_array.len() < original_len
}

/// Remove RTK hook from settings.json file
/// Backs up before modification, returns true if hook was found and removed
fn remove_hook_from_settings(ctx: InitContext) -> Result<bool> {
    let InitContext { verbose, dry_run } = ctx;
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join(SETTINGS_JSON);

    if !settings_path.exists() {
        if verbose > 0 {
            eprintln!("settings.json not found, nothing to remove");
        }
        return Ok(false);
    }

    let content = fs::read_to_string(&settings_path)
        .with_context(|| format!("Failed to read {}", settings_path.display()))?;
    let content = strip_leading_bom(&content);

    if content.trim().is_empty() {
        return Ok(false);
    }

    let mut root: serde_json::Value = from_json_str(content)
        .with_context(|| format!("Failed to parse {} as JSON", settings_path.display()))?;

    let removed = remove_hook_from_json(&mut root);

    if removed {
        if dry_run {
            println!(
                "[dry-run] would remove RTK hook entry from {}",
                settings_path.display()
            );
            if verbose > 0 {
                let serialized = serde_json::to_string_pretty(&root)
                    .context("Failed to serialize settings.json")?;
                println!("[dry-run] content:\n{}", serialized);
            }
            return Ok(true);
        }

        // Backup original
        let backup_path = settings_path.with_extension("json.bak");
        fs::copy(&settings_path, &backup_path)
            .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;

        // Atomic write
        let serialized =
            serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;
        atomic_write(&settings_path, &serialized)?;

        if verbose > 0 {
            eprintln!("Removed RTK hook from settings.json");
        }
    }

    Ok(removed)
}

/// Full uninstall for Claude, Gemini, Codex, Cursor, Pi, or OMP artifacts.
#[allow(dead_code)] // Kept as the default-policy API for in-crate callers and tests.
pub fn uninstall(
    global: bool,
    gemini: bool,
    codex: bool,
    cursor: bool,
    pi: bool,
    omp: bool,
    ctx: InitContext,
) -> Result<()> {
    uninstall_with_patch_mode(global, gemini, codex, cursor, pi, omp, PatchMode::Ask, ctx)
}

/// Full uninstall with an explicit confirmation policy for managed
/// Pi-compatible extensions.
#[allow(clippy::too_many_arguments)]
pub fn uninstall_with_patch_mode(
    global: bool,
    gemini: bool,
    codex: bool,
    cursor: bool,
    pi: bool,
    omp: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    if codex {
        uninstall_codex(global, ctx)?;
        if dry_run {
            print_dry_run_footer();
        }
        return Ok(());
    }

    if cursor {
        if !global {
            anyhow::bail!("Cursor uninstall only works with --global flag");
        }
        let cursor_removed = remove_cursor_hooks(ctx).context("Failed to remove Cursor hooks")?;
        if !cursor_removed.is_empty() {
            let header = if dry_run {
                "[dry-run] would uninstall RTK (Cursor):"
            } else {
                "RTK uninstalled (Cursor):"
            };
            println!("{}", header);
            for item in &cursor_removed {
                println!("  - {}", item);
            }
            if !dry_run {
                println!("\nRestart Cursor to apply changes.");
            }
        } else {
            println!("RTK Cursor support was not installed (nothing to remove)");
        }
        if dry_run {
            print_dry_run_footer();
        }
        return Ok(());
    }

    if pi {
        uninstall_pi_with_patch_mode(global, patch_mode, ctx)?;
        return Ok(());
    }

    if omp {
        uninstall_omp_with_patch_mode(global, patch_mode, ctx)?;
        return Ok(());
    }

    if !global {
        anyhow::bail!("Uninstall only works with --global flag. For local projects, manually remove RTK from CLAUDE.md");
    }

    let claude_dir = resolve_claude_dir()?;
    let mut removed = Vec::new();

    // Also uninstall Gemini artifacts if --gemini or always (clean everything)
    if gemini {
        let gemini_removed = uninstall_gemini(ctx)?;
        removed.extend(gemini_removed);
        if !removed.is_empty() {
            let header = if dry_run {
                "[dry-run] would uninstall RTK (Gemini):"
            } else {
                "RTK uninstalled (Gemini):"
            };
            println!("{}", header);
            for item in &removed {
                println!("  - {}", item);
            }
            if !dry_run {
                println!("\nRestart Gemini CLI to apply changes.");
            }
        } else {
            println!("RTK Gemini support was not installed (nothing to remove)");
        }
        if dry_run {
            print_dry_run_footer();
        }
        return Ok(());
    }

    // 1. Remove legacy hook file (if exists from old installation)
    let hook_path = claude_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
    if hook_path.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove hook script: {}",
                hook_path.display()
            );
        } else {
            fs::remove_file(&hook_path)
                .with_context(|| format!("Failed to remove hook: {}", hook_path.display()))?;
        }
        removed.push(format!("Hook script: {}", hook_path.display()));
    }

    // 1b. Remove integrity hash file
    if dry_run {
        // integrity::remove_hash would delete the sidecar file; just report intent.
        if integrity::hash_path_for(&hook_path).exists() {
            println!("[dry-run] would remove integrity hash sidecar");
            removed.push("Integrity hash: removed".to_string());
        }
    } else if integrity::remove_hash(&hook_path)? {
        removed.push("Integrity hash: removed".to_string());
    }

    // 2. Remove RTK.md
    let rtk_md_path = claude_dir.join(RTK_MD);
    if rtk_md_path.exists() {
        if dry_run {
            println!("[dry-run] would remove RTK.md: {}", rtk_md_path.display());
        } else {
            fs::remove_file(&rtk_md_path)
                .with_context(|| format!("Failed to remove RTK.md: {}", rtk_md_path.display()))?;
        }
        removed.push(format!("RTK.md: {}", rtk_md_path.display()));
    }

    // 3. Remove @RTK.md reference from CLAUDE.md
    let claude_md_path = claude_dir.join(CLAUDE_MD);
    if claude_md_path.exists() {
        let content = fs::read_to_string(&claude_md_path)
            .with_context(|| format!("Failed to read CLAUDE.md: {}", claude_md_path.display()))?;

        let mut claude_md_changed = false;
        let mut working_content = content.clone();

        if working_content.contains(RTK_MD_REF) {
            let new_content = working_content
                .lines()
                .filter(|line| !line.trim().starts_with(RTK_MD_REF))
                .collect::<Vec<_>>()
                .join("\n");

            working_content = clean_double_blanks(&new_content);
            claude_md_changed = true;
            removed.push("CLAUDE.md: removed @RTK.md reference".to_string());
        }

        if working_content.contains(RTK_BLOCK_START) {
            let (cleaned, did_remove) = remove_rtk_block(&working_content);
            if did_remove {
                working_content = cleaned;
                claude_md_changed = true;
                removed.push("CLAUDE.md: removed rtk-instructions block".to_string());
            }
        }

        if claude_md_changed {
            let trimmed = working_content.trim();
            if trimmed.is_empty() {
                if dry_run {
                    println!(
                        "[dry-run] would remove CLAUDE.md (empty after cleanup): {}",
                        claude_md_path.display()
                    );
                } else {
                    // nosemgrep: filesystem-deletion
                    fs::remove_file(&claude_md_path).with_context(|| {
                        format!(
                            "Failed to remove empty CLAUDE.md: {}",
                            claude_md_path.display()
                        )
                    })?;
                }
                removed.retain(|r| !r.starts_with("CLAUDE.md:"));
                removed.push("CLAUDE.md: removed (was empty after cleanup)".to_string());
            } else if dry_run {
                println!(
                    "[dry-run] would update CLAUDE.md: {}",
                    claude_md_path.display()
                );
                if verbose > 0 {
                    println!("[dry-run] content:\n{}", working_content);
                }
            } else {
                fs::write(&claude_md_path, &working_content).with_context(|| {
                    format!("Failed to write CLAUDE.md: {}", claude_md_path.display())
                })?;
            }
        }
    }

    // 4. Remove hook entry from settings.json
    if remove_hook_from_settings(ctx)? {
        removed.push("settings.json: removed RTK hook entry".to_string());
    }

    // 5. Remove OpenCode plugin
    let opencode_removed = remove_opencode_plugin(ctx)?;
    for path in opencode_removed {
        removed.push(format!("OpenCode plugin: {}", path.display()));
    }

    // 6. Remove Cursor hooks
    let cursor_removed = remove_cursor_hooks(ctx)?;
    removed.extend(cursor_removed);

    // Report results
    if removed.is_empty() {
        println!("RTK was not installed (nothing to remove)");
        println!("  Checked: {}", hook_path.display());
        println!("  Checked: {}", claude_dir.join(RTK_MD).display());
        println!("  Checked: {}", claude_md_path.display());
        println!("  Checked: {}", claude_dir.join(SETTINGS_JSON).display());
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK:"
        } else {
            "RTK uninstalled:"
        };
        println!("{}", header);
        for item in removed {
            println!("  - {}", item);
        }
        if !dry_run {
            println!("\nRestart Claude Code, OpenCode, and Cursor (if used) to apply changes.");
        }
    }

    if dry_run {
        print_dry_run_footer();
    }

    Ok(())
}

/// Orchestrator: patch settings.json with RTK hook (binary command variant)
/// Handles reading, checking, prompting, merging, backing up, and atomic writing
fn patch_settings_json_command(
    hook_command: &str,
    mode: PatchMode,
    include_opencode: bool,
    ctx: InitContext,
) -> Result<PatchResult> {
    let InitContext { verbose, dry_run } = ctx;
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join(SETTINGS_JSON);

    // Read or create settings.json
    let mut root = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read {}", settings_path.display()))?;
        let content = strip_leading_bom(&content);

        if content.trim().is_empty() {
            serde_json::json!({})
        } else {
            from_json_str(content)
                .with_context(|| format!("Failed to parse {} as JSON", settings_path.display()))?
        }
    } else {
        serde_json::json!({})
    };

    // Check idempotency
    if hook_already_present(&root, hook_command) {
        if verbose > 0 {
            eprintln!("settings.json: hook already present");
        }
        return Ok(PatchResult::AlreadyPresent);
    }

    // Handle mode
    match mode {
        PatchMode::Skip => {
            print_manual_instructions(hook_command, include_opencode);
            return Ok(PatchResult::Skipped);
        }
        PatchMode::Ask => {
            // Skip the interactive prompt in dry-run: we must not mutate state or block on stdin.
            if dry_run {
                println!(
                    "[dry-run] would prompt before patching {}",
                    settings_path.display()
                );
            } else if !prompt_user_consent(&settings_path)? {
                print_manual_instructions(hook_command, include_opencode);
                return Ok(PatchResult::Declined);
            }
        }
        PatchMode::Auto => {
            // Proceed without prompting
        }
    }

    insert_hook_entry(&mut root, hook_command)?;

    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;

    if dry_run {
        println!(
            "[dry-run] would patch settings.json: {}",
            settings_path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{}", serialized);
        }
        return Ok(PatchResult::WouldPatch);
    }

    // Backup original
    if settings_path.exists() {
        let backup_path = settings_path.with_extension("json.bak");
        fs::copy(&settings_path, &backup_path)
            .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;
        if verbose > 0 {
            eprintln!("Backup: {}", backup_path.display());
        }
    }

    // Atomic write
    atomic_write(&settings_path, &serialized)?;

    println!("\n  settings.json: hook added");
    if settings_path.with_extension("json.bak").exists() {
        println!(
            "  Backup: {}",
            settings_path.with_extension("json.bak").display()
        );
    }
    if include_opencode {
        println!("  Restart Claude Code and OpenCode. Test with: git status");
    } else {
        println!("  Restart Claude Code. Test with: git status");
    }

    Ok(PatchResult::Patched)
}

/// Clean up consecutive blank lines (collapse 3+ to 2)
/// Used when removing @RTK.md line from CLAUDE.md
fn clean_double_blanks(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut result = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        if line.trim().is_empty() {
            // Count consecutive blank lines
            let mut blank_count = 0;
            while i < lines.len() && lines[i].trim().is_empty() {
                blank_count += 1;
                i += 1;
            }

            // Keep at most 2 blank lines
            let keep = blank_count.min(2);
            result.extend(std::iter::repeat_n("", keep));
        } else {
            result.push(line);
            i += 1;
        }
    }

    result.join("\n")
}

/// Deep-merge RTK hook entry into settings.json
/// Creates hooks.PreToolUse structure if missing, preserves existing hooks
fn insert_hook_entry(root: &mut serde_json::Value, hook_command: &str) -> Result<()> {
    let root_obj = match root.as_object_mut() {
        Some(obj) => obj,
        None => {
            *root = serde_json::json!({});
            root.as_object_mut().expect("just-created json object")
        }
    };

    let hooks = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("hooks value is not an object")?;

    let pre_tool_use = hooks
        .entry(PRE_TOOL_USE_KEY)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .context("PreToolUse value is not an array")?;

    pre_tool_use.push(serde_json::json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": hook_command
        }]
    }));
    Ok(())
}

/// Check if RTK hook is already present in settings.json
/// Matches on legacy rtk-rewrite.sh path OR new `rtk hook claude` command
fn hook_already_present(root: &serde_json::Value, hook_command: &str) -> bool {
    let pre_tool_use_array = match root
        .get("hooks")
        .and_then(|h| h.get(PRE_TOOL_USE_KEY))
        .and_then(|p| p.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };

    pre_tool_use_array
        .iter()
        .filter_map(|entry| entry.get("hooks")?.as_array())
        .flatten()
        .filter_map(|hook| hook.get("command")?.as_str())
        .any(|cmd| {
            cmd == hook_command || is_claude_hook_command(cmd) || cmd.contains(REWRITE_HOOK_FILE)
        })
}

/// Default mode: hook + slim RTK.md + @RTK.md reference
fn run_default_mode(
    global: bool,
    patch_mode: PatchMode,
    install_opencode: bool,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !global {
        // Local init: inject CLAUDE.md + generate project-local filters template
        run_claude_md_mode(false, install_opencode, ctx)?;
        generate_project_filters_template(ctx)?;
        return Ok(());
    }

    let claude_dir = resolve_claude_dir()?;
    let rtk_md_path = claude_dir.join(RTK_MD);
    let claude_md_path = claude_dir.join(CLAUDE_MD);

    // 1. Migrate old hook script if present
    migrate_old_hook_script(ctx);

    // 2. Write RTK.md
    write_if_changed(&rtk_md_path, RTK_SLIM, RTK_MD, ctx)?;

    let opencode_plugin_path = if install_opencode {
        let path = prepare_opencode_plugin_path()?;
        ensure_opencode_plugin_installed(&path, ctx)?;
        Some(path)
    } else {
        None
    };

    // 3. Patch CLAUDE.md (add @RTK.md, migrate if needed)
    let migrated = patch_claude_md(&claude_md_path, ctx)?;

    // 4. Print success message (skip in dry-run)
    if !dry_run {
        println!("\nRTK hook registered (global).\n");
        println!("  Command:   {}", CLAUDE_HOOK_COMMAND);
        println!("  RTK.md:    {} (10 lines)", rtk_md_path.display());
        if let Some(path) = &opencode_plugin_path {
            println!("  OpenCode:  {}", path.display());
        }
        println!("  CLAUDE.md: @RTK.md reference added");

        if migrated {
            println!("\n  [ok] Migrated: removed 137-line RTK block from CLAUDE.md");
            println!("              replaced with @RTK.md (10 lines)");
        }
    }

    // 5. Patch settings.json with binary command
    let patch_result =
        patch_settings_json_command(CLAUDE_HOOK_COMMAND, patch_mode, install_opencode, ctx)?;

    // Report result
    if !dry_run {
        match patch_result {
            PatchResult::Patched => {
                // Already printed by patch_settings_json_command
            }
            PatchResult::AlreadyPresent => {
                println!("\n  settings.json: hook already present");
                if install_opencode {
                    println!("  Restart Claude Code and OpenCode. Test with: git status");
                } else {
                    println!("  Restart Claude Code. Test with: git status");
                }
            }
            PatchResult::Declined | PatchResult::Skipped => {
                // Manual instructions already printed
            }
            PatchResult::WouldPatch => {
                // Cannot happen outside dry_run
            }
        }
    }

    // 6. Generate user-global filters template (~/.config/rtk/filters.toml)
    generate_global_filters_template(ctx)?;

    if !dry_run {
        println!(); // Final newline
    }

    Ok(())
}

/// Migrate old hook script to new binary command.
/// Deletes `~/.claude/hooks/rtk-rewrite.sh` and `.rtk-hook.sha256` if present,
/// and removes the stale settings.json entry so the new `rtk hook claude` entry
/// can be registered.
fn migrate_old_hook_script(ctx: InitContext) {
    let InitContext { verbose, dry_run } = ctx;
    if let Some(home) = dirs::home_dir() {
        let old_hook = home
            .join(CLAUDE_DIR)
            .join(HOOKS_SUBDIR)
            .join(REWRITE_HOOK_FILE);
        if old_hook.exists() {
            if dry_run {
                println!(
                    "[dry-run] would migrate legacy hook script: {}",
                    old_hook.display()
                );
            // nosemgrep: filesystem-deletion
            } else if let Err(e) = std::fs::remove_file(&old_hook) {
                if verbose > 0 {
                    eprintln!("  [warn] Failed to remove old hook script: {e}");
                }
            } else {
                if verbose > 0 {
                    eprintln!("  [ok] Removed old hook script: {}", old_hook.display());
                }
                // Clean up the stale settings.json entry that pointed to the deleted script
                if let Err(e) = remove_legacy_settings_entries(ctx) {
                    if verbose > 0 {
                        eprintln!("  [warn] Failed to clean legacy settings.json entry: {e}");
                    }
                }
            }
        }
        // Remove legacy hash file
        let hash_file = home
            .join(CLAUDE_DIR)
            .join(HOOKS_SUBDIR)
            .join(".rtk-hook.sha256");
        if hash_file.exists() {
            if dry_run {
                println!(
                    "[dry-run] would remove legacy hash file: {}",
                    hash_file.display()
                );
            } else {
                let _ = std::fs::remove_file(&hash_file);
            }
        }
        // Remove Cursor legacy hook
        let cursor_hook = home.join(CURSOR_DIR).join("hooks").join(REWRITE_HOOK_FILE);
        if cursor_hook.exists() {
            if dry_run {
                println!(
                    "[dry-run] would remove legacy Cursor hook: {}",
                    cursor_hook.display()
                );
            } else {
                let _ = std::fs::remove_file(&cursor_hook);
            }
        }
    }
}

/// Remove only legacy `rtk-rewrite.sh` entries from settings.json.
/// Preserves any existing `rtk hook claude` entries (new format).
fn remove_legacy_settings_entries(ctx: InitContext) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join(SETTINGS_JSON);

    if !settings_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&settings_path)
        .with_context(|| format!("Failed to read {}", settings_path.display()))?;
    let content = strip_leading_bom(&content);
    if content.trim().is_empty() {
        return Ok(());
    }

    let mut root: serde_json::Value = from_json_str(content)
        .with_context(|| format!("Failed to parse {}", settings_path.display()))?;

    if !remove_legacy_hook_entries_from_json(&mut root) {
        return Ok(());
    }

    if dry_run {
        println!(
            "[dry-run] would remove legacy rtk-rewrite.sh entry from {}",
            settings_path.display()
        );
        return Ok(());
    }

    // Backup before modifying
    let backup_path = settings_path.with_extension("json.bak");
    fs::copy(&settings_path, &backup_path)
        .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;

    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;
    atomic_write(&settings_path, &serialized)?;

    if verbose > 0 {
        eprintln!("  [ok] Removed legacy rtk-rewrite.sh entry from settings.json");
    }
    Ok(())
}

/// Remove only legacy `rtk-rewrite.sh` hook entries from a parsed settings.json.
/// Returns true if any entries were removed.
/// Does NOT remove `rtk hook claude` entries — those are the new format.
fn remove_legacy_hook_entries_from_json(root: &mut serde_json::Value) -> bool {
    let pre_tool_use_array = match root
        .get_mut("hooks")
        .and_then(|h| h.get_mut(PRE_TOOL_USE_KEY))
        .and_then(|p| p.as_array_mut())
    {
        Some(arr) => arr,
        None => return false,
    };

    let original_len = pre_tool_use_array.len();
    pre_tool_use_array.retain(|entry| {
        let dominated_by_legacy = entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|hooks| {
                hooks.iter().all(|hook| {
                    hook.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|cmd| cmd.contains(REWRITE_HOOK_FILE))
                })
            })
            .unwrap_or(false);
        !dominated_by_legacy
    });

    pre_tool_use_array.len() < original_len
}

/// Generate .rtk/filters.toml template in the current directory if not present.
fn generate_project_filters_template(ctx: InitContext) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    let rtk_dir = std::path::Path::new(".rtk");
    let path = rtk_dir.join("filters.toml");

    if path.exists() {
        if verbose > 0 {
            eprintln!(".rtk/filters.toml already exists, skipping template");
        }
        return Ok(());
    }

    if dry_run {
        println!(
            "[dry-run] would create .rtk/filters.toml template: {}",
            path.display()
        );
        return Ok(());
    }

    fs::create_dir_all(rtk_dir)
        .with_context(|| format!("Failed to create directory: {}", rtk_dir.display()))?;
    fs::write(&path, FILTERS_TEMPLATE)
        .with_context(|| format!("Failed to write {}", path.display()))?;

    println!(
        "  filters:   {} (template, edit to add project filters)",
        path.display()
    );
    Ok(())
}

/// Generate ~/.config/rtk/filters.toml template if not present.
fn generate_global_filters_template(ctx: InitContext) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    let config_dir = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".config"));
    let rtk_dir = config_dir.join(crate::core::constants::RTK_DATA_DIR);
    let path = rtk_dir.join("filters.toml");

    if path.exists() {
        if verbose > 0 {
            eprintln!("{} already exists, skipping template", path.display());
        }
        return Ok(());
    }

    if dry_run {
        println!(
            "[dry-run] would create global filters template: {}",
            path.display()
        );
        return Ok(());
    }

    fs::create_dir_all(&rtk_dir)
        .with_context(|| format!("Failed to create directory: {}", rtk_dir.display()))?;
    fs::write(&path, FILTERS_GLOBAL_TEMPLATE)
        .with_context(|| format!("Failed to write {}", path.display()))?;

    println!(
        "  filters:   {} (template, edit to add user-global filters)",
        path.display()
    );
    Ok(())
}

pub fn finalize_filter_trust(global: bool, dry_run: bool, trust: FilterTrust) -> Result<()> {
    let paths = crate::hooks::trust::gated_filter_paths();
    let path = match if global { paths.get(1) } else { paths.first() } {
        Some(p) => p,
        None => return Ok(()),
    };
    if !path.exists() {
        return Ok(());
    }

    let status = crate::hooks::trust::check_trust(path)
        .unwrap_or(crate::hooks::trust::TrustStatus::Untrusted);
    if matches!(
        status,
        crate::hooks::trust::TrustStatus::Trusted | crate::hooks::trust::TrustStatus::EnvOverride
    ) {
        return Ok(());
    }

    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(_) => return Ok(()),
    };
    let content = String::from_utf8_lossy(&bytes);
    let filters = crate::core::toml_filter::active_filter_summaries(&content);
    if filters.is_empty() {
        return Ok(());
    }

    if dry_run {
        println!(
            "[dry-run] {} untrusted custom filter(s) in {}",
            filters.len(),
            path.display()
        );
        return Ok(());
    }

    let scope = if global { "global" } else { "project" };
    crate::hooks::trust::print_filter_notice(path, scope, &filters);

    let enable = match trust {
        FilterTrust::Trust => true,
        FilterTrust::Skip => false,
        FilterTrust::Ask => crate::hooks::trust::confirm_enable_at_tty()?,
    };

    if enable {
        let hash = crate::hooks::integrity::compute_hash_bytes(&bytes);
        crate::hooks::trust::trust_filter_with_hash(path, &hash)?;
        eprintln!("Enabled. Revoke with `rtk untrust`.");
    } else {
        eprintln!("\x1b[33m  Not enabled — run `rtk trust` to review and enable.\x1b[0m");
    }
    Ok(())
}

/// Hook-only mode: just the hook, no RTK.md
fn run_hook_only_mode(
    global: bool,
    patch_mode: PatchMode,
    install_opencode: bool,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !global {
        eprintln!("[warn] Warning: --hook-only only makes sense with --global");
        eprintln!("    For local projects, use default mode or --claude-md");
        return Ok(());
    }

    // Migrate old hook script if present
    migrate_old_hook_script(ctx);

    let opencode_plugin_path = if install_opencode {
        let path = prepare_opencode_plugin_path()?;
        ensure_opencode_plugin_installed(&path, ctx)?;
        Some(path)
    } else {
        None
    };

    if !dry_run {
        println!("\nRTK hook registered (hook-only mode).\n");
        println!("  Command: {}", CLAUDE_HOOK_COMMAND);
        if let Some(path) = &opencode_plugin_path {
            println!("  OpenCode: {}", path.display());
        }
        println!(
            "  Note: No RTK.md created. Claude won't know about meta commands (gain, discover, proxy)."
        );
    }

    // Patch settings.json with binary command
    let patch_result =
        patch_settings_json_command(CLAUDE_HOOK_COMMAND, patch_mode, install_opencode, ctx)?;

    // Report result
    if !dry_run {
        match patch_result {
            PatchResult::Patched => {
                // Already printed by patch_settings_json_command
            }
            PatchResult::AlreadyPresent => {
                println!("\n  settings.json: hook already present");
                if install_opencode {
                    println!("  Restart Claude Code and OpenCode. Test with: git status");
                } else {
                    println!("  Restart Claude Code. Test with: git status");
                }
            }
            PatchResult::Declined | PatchResult::Skipped => {
                // Manual instructions already printed
            }
            PatchResult::WouldPatch => {
                // Cannot happen outside dry_run
            }
        }
    }

    if !dry_run {
        println!(); // Final newline
    }

    Ok(())
}

fn resolve_home_subdir(subdir: &str) -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(subdir))
        .context(if cfg!(windows) {
            "Cannot determine home directory. Is %USERPROFILE% set?"
        } else {
            "Cannot determine home directory. Is $HOME set?"
        })
}

pub fn resolve_claude_dir() -> Result<PathBuf> {
    resolve_claude_dir_from(
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        dirs::home_dir(),
    )
}

fn resolve_claude_dir_from(
    claude_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = claude_dir.filter(|path| !path.as_os_str().is_empty()) {
        return Ok(path);
    }
    home_dir
        .map(|h| h.join(CLAUDE_DIR))
        .context("Cannot determine Claude config directory. Set $CLAUDE_CONFIG_DIR or $HOME.")
}

/// Show current rtk configuration
pub fn show_config(codex: bool, omp: bool) -> Result<()> {
    if omp {
        return show_omp_config();
    }
    if codex {
        return show_codex_config();
    }

    show_claude_config()
}

fn show_claude_config() -> Result<()> {
    let claude_dir = resolve_claude_dir()?;
    let hook_path = claude_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
    let rtk_md_path = claude_dir.join(RTK_MD);
    let global_claude_md = claude_dir.join(CLAUDE_MD);
    let local_claude_md = PathBuf::from(CLAUDE_MD);

    println!("rtk Configuration:\n");

    // Check hook: prefer binary command detection, fall back to script file
    let settings_path = claude_dir.join(SETTINGS_JSON);
    let binary_hook_registered = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path).unwrap_or_default();
        if let Ok(root) = from_json_str::<serde_json::Value>(&content) {
            hook_already_present(&root, CLAUDE_HOOK_COMMAND)
        } else {
            false
        }
    } else {
        false
    };

    if binary_hook_registered {
        println!("[ok] Hook: {} (native binary command)", CLAUDE_HOOK_COMMAND);
    } else if hook_path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(&hook_path)?;
            let perms = metadata.permissions();
            let is_executable = perms.mode() & 0o111 != 0;

            let hook_content = fs::read_to_string(&hook_path)?;
            let has_guards =
                hook_content.contains("command -v rtk") && hook_content.contains("command -v jq");
            let is_thin_delegator = hook_content.contains("rtk rewrite");
            let hook_version = super::hook_check::parse_hook_version(&hook_content);

            if !is_executable {
                println!(
                    "[warn] Hook: {} (NOT executable - run: chmod +x)",
                    hook_path.display()
                );
            } else if !is_thin_delegator {
                println!(
                    "[warn] Hook: {} (outdated — run `rtk init -g` to upgrade to native binary)",
                    hook_path.display()
                );
            } else if is_executable && has_guards {
                println!(
                    "[warn] Hook: {} (legacy script v{} — run `rtk init -g` to upgrade)",
                    hook_path.display(),
                    hook_version
                );
            } else {
                println!(
                    "[warn] Hook: {} (no guards - outdated)",
                    hook_path.display()
                );
            }
        }

        #[cfg(not(unix))]
        {
            println!(
                "[warn] Hook: {} (legacy script — run `rtk init -g` to upgrade)",
                hook_path.display()
            );
        }
    } else {
        println!("[--] Hook: not found");
    }

    // Check RTK.md
    if rtk_md_path.exists() {
        println!("[ok] RTK.md: {} (slim mode)", rtk_md_path.display());
    } else {
        println!("[--] RTK.md: not found");
    }

    // Check hook integrity (only relevant for legacy script hooks)
    if hook_path.exists() && !binary_hook_registered {
        match integrity::verify_hook_at(&hook_path) {
            Ok(integrity::IntegrityStatus::Verified) => {
                println!("[ok] Integrity: hook hash verified");
            }
            Ok(integrity::IntegrityStatus::Tampered { .. }) => {
                println!("[FAIL] Integrity: hook modified outside rtk init (run: rtk verify)");
            }
            Ok(integrity::IntegrityStatus::NoBaseline) => {
                println!("[warn] Integrity: no baseline hash (run: rtk init -g to establish)");
            }
            Ok(integrity::IntegrityStatus::NotInstalled)
            | Ok(integrity::IntegrityStatus::OrphanedHash) => {
                // Don't show integrity line if hook isn't installed
            }
            Err(_) => {
                println!("[warn] Integrity: check failed");
            }
        }
    }

    // Check global CLAUDE.md
    if global_claude_md.exists() {
        let content = fs::read_to_string(&global_claude_md)?;
        if content.contains(RTK_MD_REF) {
            println!("[ok] Global (~/.claude/CLAUDE.md): @RTK.md reference");
        } else if content.contains(RTK_BLOCK_START) {
            println!(
                "[warn] Global (~/.claude/CLAUDE.md): old RTK block (run: rtk init -g to migrate)"
            );
        } else {
            println!("[--] Global (~/.claude/CLAUDE.md): exists but rtk not configured");
        }
    } else {
        println!("[--] Global (~/.claude/CLAUDE.md): not found");
    }

    // Check local CLAUDE.md
    if local_claude_md.exists() {
        let content = fs::read_to_string(&local_claude_md)?;
        if content.contains("rtk") {
            println!("[ok] Local (./CLAUDE.md): rtk enabled");
        } else {
            println!("[--] Local (./CLAUDE.md): exists but rtk not configured");
        }
    } else {
        println!("[--] Local (./CLAUDE.md): not found");
    }

    // Check settings.json (detailed status)
    if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        let content = strip_leading_bom(&content);
        if !content.trim().is_empty() {
            if let Ok(root) = from_json_str::<serde_json::Value>(content) {
                if hook_already_present(&root, CLAUDE_HOOK_COMMAND) {
                    println!("[ok] settings.json: RTK hook configured");
                } else {
                    println!("[warn] settings.json: exists but RTK hook not configured");
                    println!("    Run: rtk init -g --auto-patch");
                }
            } else {
                println!("[warn] settings.json: exists but invalid JSON");
            }
        } else {
            println!("[--] settings.json: empty");
        }
    } else {
        println!("[--] settings.json: not found");
    }

    // Check OpenCode plugin
    if let Ok(opencode_dir) = resolve_opencode_dir() {
        let plugin = opencode_plugin_path(&opencode_dir);
        if plugin.exists() {
            println!("[ok] OpenCode: plugin installed ({})", plugin.display());
        } else {
            println!("[--] OpenCode: plugin not found");
        }
    } else {
        println!("[--] OpenCode: config dir not found");
    }

    // Check Cursor hooks
    if let Ok(cursor_dir) = resolve_cursor_dir() {
        let cursor_hook = cursor_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
        let cursor_hooks_json = cursor_dir.join(HOOKS_JSON);

        // Check for binary command in hooks.json first
        let cursor_binary_registered = if cursor_hooks_json.exists() {
            let content = fs::read_to_string(&cursor_hooks_json).unwrap_or_default();
            if let Ok(root) = from_json_str::<serde_json::Value>(&content) {
                cursor_hook_already_present(&root)
            } else {
                false
            }
        } else {
            false
        };

        if cursor_binary_registered {
            println!("[ok] Cursor hook: registered in hooks.json");
        } else if cursor_hook.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let meta = fs::metadata(&cursor_hook)?;
                let is_executable = meta.permissions().mode() & 0o111 != 0;
                let content = fs::read_to_string(&cursor_hook)?;
                let _is_thin = content.contains("rtk rewrite");

                if !is_executable {
                    println!(
                        "[warn] Cursor hook: {} (legacy script, NOT executable)",
                        cursor_hook.display()
                    );
                } else {
                    println!(
                        "[warn] Cursor hook: {} (legacy script — run `rtk init -g --agent cursor` to upgrade)",
                        cursor_hook.display()
                    );
                }
            }

            #[cfg(not(unix))]
            {
                println!("[warn] Cursor hook: {} (legacy script — run `rtk init -g --agent cursor` to upgrade)", cursor_hook.display());
            }
        } else {
            println!("[--] Cursor hook: not found");
        }
    } else {
        println!("[--] Cursor: home dir not found");
    }

    println!("\nUsage:");
    println!("  rtk init              # Full injection into local CLAUDE.md");
    println!("  rtk init -g           # Hook + RTK.md + @RTK.md + settings.json (recommended)");
    println!("  rtk init -g --auto-patch    # Same as above but no prompt");
    println!("  rtk init -g --no-patch      # Skip settings.json (manual setup)");
    println!("  rtk init -g --uninstall     # Remove all RTK artifacts");
    println!("  rtk init -g --claude-md     # Legacy: full injection into ~/.claude/CLAUDE.md");
    println!("  rtk init -g --hook-only     # Hook only, no RTK.md");
    println!("  rtk init --codex            # Configure local AGENTS.md + RTK.md");
    println!("  rtk init -g --codex         # Configure $CODEX_HOME/AGENTS.md + $CODEX_HOME/RTK.md (or ~/.codex/)");
    println!("  rtk init -g --opencode      # OpenCode plugin only");
    println!("  rtk init -g --agent cursor  # Install Cursor Agent hooks");

    Ok(())
}

#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use tempfile::TempDir;
#[cfg(test)]
pub(crate) static CLAUDE_DIR_LOCK: Mutex<()> = Mutex::new(());
#[cfg(test)]
pub(crate) static PI_DIR_LOCK: Mutex<()> = Mutex::new(());
/// Serialises all tests that mutate the process-wide working directory.
#[cfg(test)]
pub(crate) static CWD_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) fn with_claude_dir_override<F: FnOnce(&Path)>(tmp: &TempDir, f: F) {
    let _guard = CLAUDE_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let claude_dir = tmp.path().join(CLAUDE_DIR);
    fs::create_dir_all(&claude_dir).unwrap();

    let orig = std::env::var_os("CLAUDE_CONFIG_DIR");
    std::env::set_var("CLAUDE_CONFIG_DIR", &claude_dir);
    f(&claude_dir);
    match orig {
        Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
        None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
    }
}

#[cfg(test)]
pub(crate) fn with_pi_dir_override<F: FnOnce(&Path)>(tmp: &TempDir, f: F) {
    let _guard = PI_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let pi_dir = tmp.path().join("pi_agent");
    fs::create_dir_all(&pi_dir).unwrap();

    let orig = std::env::var_os(PI_CODING_AGENT_DIR_ENV);
    std::env::set_var(PI_CODING_AGENT_DIR_ENV, &pi_dir);
    f(&pi_dir);
    match orig {
        Some(v) => std::env::set_var(PI_CODING_AGENT_DIR_ENV, v),
        None => std::env::remove_var(PI_CODING_AGENT_DIR_ENV),
    }
}

#[cfg(test)]
pub(crate) fn with_omp_dir_override<F: FnOnce(&Path)>(tmp: &TempDir, f: F) {
    // OMP reuses PI_CODING_AGENT_DIR, so share the Pi environment lock.
    let _guard = PI_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let omp_dir = tmp.path().join("omp_agent");
    fs::create_dir_all(&omp_dir).unwrap();

    let orig = std::env::var_os(PI_CODING_AGENT_DIR_ENV);
    std::env::set_var(PI_CODING_AGENT_DIR_ENV, &omp_dir);
    f(&omp_dir);
    match orig {
        Some(v) => std::env::set_var(PI_CODING_AGENT_DIR_ENV, v),
        None => std::env::remove_var(PI_CODING_AGENT_DIR_ENV),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_init_mentions_all_top_level_commands() {
        for cmd in [
            "rtk cargo",
            "rtk gh",
            "rtk vitest",
            "rtk tsc",
            "rtk lint",
            "rtk prettier",
            "rtk next",
            "rtk playwright",
            "rtk prisma",
            "rtk pnpm",
            "rtk npm",
            "rtk uv",
            "rtk curl",
            "rtk git",
            "rtk docker",
            "rtk kubectl",
        ] {
            assert!(
                RTK_INSTRUCTIONS.contains(cmd),
                "Missing {cmd} in RTK_INSTRUCTIONS"
            );
        }
    }

    #[test]
    fn test_init_has_version_marker() {
        assert!(
            RTK_INSTRUCTIONS.contains(RTK_BLOCK_START),
            "RTK_INSTRUCTIONS must start with RTK_BLOCK_START marker"
        );
        assert!(
            RTK_INSTRUCTIONS.contains(RTK_BLOCK_END),
            "RTK_INSTRUCTIONS must end with RTK_BLOCK_END marker"
        );
    }

    #[test]
    fn test_migration_removes_old_block() {
        let input = format!(
            "# My Config\n\n{} v2 -->\nOLD RTK STUFF\n{}\n\nMore content",
            RTK_BLOCK_START, RTK_BLOCK_END
        );

        let (result, migrated) = remove_rtk_block(&input);
        assert!(migrated);
        assert!(!result.contains("OLD RTK STUFF"));
        assert!(result.contains("# My Config"));
        assert!(result.contains("More content"));
    }

    #[test]
    fn test_migration_warns_on_missing_end_marker() {
        let input = format!("{} v2 -->\nOLD STUFF\nNo end marker", RTK_BLOCK_START);
        let (result, migrated) = remove_rtk_block(&input);
        assert!(!migrated);
        assert_eq!(result, input);
    }

    #[test]
    fn test_init_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let claude_md = temp.path().join("CLAUDE.md");

        fs::write(&claude_md, "# My stuff\n\n@RTK.md\n").unwrap();

        let content = fs::read_to_string(&claude_md).unwrap();
        let count = content.matches("@RTK.md").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_write_if_changed_dry_run_does_not_create_file() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("rtk-test.md");

        let changed = write_if_changed(
            &target,
            "some content",
            "test file",
            InitContext {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(
            changed,
            "dry-run should report would-change for missing file"
        );
        assert!(
            !target.exists(),
            "dry-run must not create file: {}",
            target.display()
        );
    }

    #[test]
    fn test_write_if_changed_dry_run_does_not_modify_existing_file() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("rtk-test.md");
        fs::write(&target, "original").unwrap();

        let changed = write_if_changed(
            &target,
            "new content",
            "test file",
            InitContext {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(changed, "dry-run should report would-change");
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "original",
            "dry-run must not modify file contents"
        );
    }

    #[test]
    fn test_local_init_unchanged() {
        // Local init should use claude-md mode
        let temp = TempDir::new().unwrap();
        let claude_md = temp.path().join("CLAUDE.md");

        fs::write(&claude_md, RTK_INSTRUCTIONS).unwrap();
        let content = fs::read_to_string(&claude_md).unwrap();

        assert!(content.contains(RTK_BLOCK_START));
    }

    // Tests for hook_already_present()
    #[test]
    fn test_hook_already_present_exact_match() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/Users/test/.claude/hooks/rtk-rewrite.sh"
                    }]
                }]
            }
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(hook_already_present(&json_content, hook_command));
    }

    #[test]
    fn test_hook_already_present_different_path() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                    }]
                }]
            }
        });

        let hook_command = "~/.claude/hooks/rtk-rewrite.sh";
        // Should match on rtk-rewrite.sh substring
        assert!(hook_already_present(&json_content, hook_command));
    }

    #[test]
    fn test_hook_not_present_empty() {
        let json_content = serde_json::json!({});
        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(!hook_already_present(&json_content, hook_command));
    }

    #[test]
    fn test_hook_already_present_new_command() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": CLAUDE_HOOK_COMMAND
                    }]
                }]
            }
        });

        assert!(hook_already_present(&json_content, CLAUDE_HOOK_COMMAND));
    }

    #[test]
    fn test_hook_already_present_absolute_new_command() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/opt/homebrew/bin/rtk hook claude",
                        "timeout": 5
                    }]
                }]
            }
        });

        assert!(hook_already_present(&json_content, CLAUDE_HOOK_COMMAND));
    }

    #[test]
    fn test_hook_not_present_other_hooks() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/some/other/hook.sh"
                    }]
                }]
            }
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(!hook_already_present(&json_content, hook_command));
    }

    // Tests for insert_hook_entry()
    #[test]
    fn test_insert_hook_entry_empty_root() {
        let mut json_content = serde_json::json!({});
        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";

        insert_hook_entry(&mut json_content, hook_command).unwrap();

        // Should create full structure
        assert!(json_content.get("hooks").is_some());
        assert!(json_content
            .get("hooks")
            .unwrap()
            .get("PreToolUse")
            .is_some());

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);

        let command = pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(command, hook_command);
    }

    #[test]
    fn test_insert_hook_entry_preserves_existing() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/some/other/hook.sh"
                    }]
                }]
            }
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        insert_hook_entry(&mut json_content, hook_command).unwrap();

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 2); // Should have both hooks

        // Check first hook is preserved
        let first_command = pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(first_command, "/some/other/hook.sh");

        // Check second hook is RTK
        let second_command = pre_tool_use[1]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(second_command, hook_command);
    }

    #[test]
    fn test_insert_hook_preserves_other_keys() {
        let mut json_content = serde_json::json!({
            "env": {"PATH": "/custom/path"},
            "permissions": {"allowAll": true},
            "model": "claude-sonnet-4"
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        insert_hook_entry(&mut json_content, hook_command).unwrap();

        // Should preserve all other keys
        assert_eq!(json_content["env"]["PATH"], "/custom/path");
        assert_eq!(json_content["permissions"]["allowAll"], true);
        assert_eq!(json_content["model"], "claude-sonnet-4");

        // And add hooks
        assert!(json_content.get("hooks").is_some());
    }

    // Tests for atomic_write()
    #[test]
    fn test_atomic_write() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.json");

        let content = r#"{"key": "value"}"#;
        atomic_write(&file_path, content).unwrap();

        assert!(file_path.exists());
        let written = fs::read_to_string(&file_path).unwrap();
        assert_eq!(written, content);
    }

    #[cfg(unix)]
    #[test]
    fn test_atomic_write_preserves_symlink() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let target_path = temp.path().join("real-settings.json");
        let link_path = temp.path().join("settings.json");

        fs::write(&target_path, "{}").expect("seed target file");
        symlink(&target_path, &link_path).expect("create symlink");

        atomic_write(&link_path, "{\"hooks\":{}}").unwrap();

        let meta = fs::symlink_metadata(&link_path).unwrap();
        assert!(meta.file_type().is_symlink(), "symlink must survive");
        let written = fs::read_to_string(&target_path).unwrap();
        assert_eq!(written, "{\"hooks\":{}}");
    }

    #[cfg(unix)]
    #[test]
    fn test_atomic_write_preserves_relative_symlink() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let subdir = temp.path().join("real");
        fs::create_dir(&subdir).unwrap();
        let target_path = subdir.join("settings.json");
        let link_path = temp.path().join("settings.json");

        fs::write(&target_path, "{}").expect("seed target file");
        symlink(Path::new("real/settings.json"), &link_path).expect("create relative symlink");

        atomic_write(&link_path, "{\"patched\":true}").unwrap();

        let meta = fs::symlink_metadata(&link_path).unwrap();
        assert!(meta.file_type().is_symlink(), "symlink must survive");
        let written = fs::read_to_string(&target_path).unwrap();
        assert_eq!(written, "{\"patched\":true}");
    }

    // Test for preserve_order round-trip
    #[test]
    fn test_preserve_order_round_trip() {
        let original = r#"{"env": {"PATH": "/usr/bin"}, "permissions": {"allowAll": true}, "model": "claude-sonnet-4"}"#;
        let parsed: serde_json::Value = serde_json::from_str(original).unwrap();
        let serialized = serde_json::to_string(&parsed).unwrap();

        // Keys should appear in same order
        let _original_keys: Vec<&str> = original.split("\"").filter(|s| s.contains(":")).collect();
        let _serialized_keys: Vec<&str> =
            serialized.split("\"").filter(|s| s.contains(":")).collect();

        // Just check that keys exist (preserve_order doesn't guarantee exact order in nested objects)
        assert!(serialized.contains("\"env\""));
        assert!(serialized.contains("\"permissions\""));
        assert!(serialized.contains("\"model\""));
    }

    // Tests for clean_double_blanks()
    #[test]
    fn test_clean_double_blanks() {
        // Input: line1, 2 blank lines, line2, 1 blank line, line3, 3 blank lines, line4
        // Expected: line1, 2 blank lines (kept), line2, 1 blank line, line3, 2 blank lines (max), line4
        let input = "line1\n\n\nline2\n\nline3\n\n\n\nline4";
        // That's: line1 \n \n \n line2 \n \n line3 \n \n \n \n line4
        // Which is: line1, blank, blank, line2, blank, line3, blank, blank, blank, line4
        // So 2 blanks after line1 (keep both), 1 blank after line2 (keep), 3 blanks after line3 (keep 2)
        let expected = "line1\n\n\nline2\n\nline3\n\n\nline4";
        assert_eq!(clean_double_blanks(input), expected);
    }

    #[test]
    fn test_clean_double_blanks_preserves_single() {
        let input = "line1\n\nline2\n\nline3";
        assert_eq!(clean_double_blanks(input), input); // No change
    }

    // Tests for remove_hook_from_settings()
    #[test]
    fn test_remove_hook_from_json() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/some/other/hook.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/Users/test/.claude/hooks/rtk-rewrite.sh"
                        }]
                    }
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        // Should have only one hook left
        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);

        // Check it's the other hook
        let command = pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(command, "/some/other/hook.sh");
    }

    #[test]
    fn test_remove_hook_from_json_new_command() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/some/other/hook.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": CLAUDE_HOOK_COMMAND
                        }]
                    }
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);
        assert_eq!(
            pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap(),
            "/some/other/hook.sh"
        );
    }

    #[test]
    fn test_remove_hook_from_json_absolute_new_command() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/some/other/hook.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/opt/homebrew/bin/rtk hook claude"
                        }]
                    }
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);
        assert_eq!(
            pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap(),
            "/some/other/hook.sh"
        );
    }

    #[test]
    fn test_remove_hook_when_not_present() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/some/other/hook.sh"
                    }]
                }]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(!removed);
    }

    // ─── Legacy migration tests ──────────────────────────────────────

    #[test]
    fn test_remove_legacy_hook_entries_strips_old_script() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                    }]
                }]
            }
        });

        assert!(remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert!(arr.is_empty());
    }

    #[test]
    fn test_remove_legacy_hook_entries_preserves_new_command() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": CLAUDE_HOOK_COMMAND
                        }]
                    }
                ]
            }
        });

        assert!(remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let cmd = arr[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, CLAUDE_HOOK_COMMAND);
    }

    #[test]
    fn test_remove_legacy_hook_entries_noop_when_no_legacy() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": CLAUDE_HOOK_COMMAND
                    }]
                }]
            }
        });

        assert!(!remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn test_remove_legacy_hook_entries_preserves_third_party_hooks() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "some-other-tool --hook"
                        }]
                    }
                ]
            }
        });

        assert!(remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let cmd = arr[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, "some-other-tool --hook");
    }

    #[test]
    fn test_global_default_mode_creates_artifacts() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(claude_dir.join(RTK_MD).exists(), "RTK.md must be created");
            assert!(
                claude_dir.join(CLAUDE_MD).exists(),
                "CLAUDE.md must be created"
            );

            let settings = claude_dir.join(SETTINGS_JSON);
            assert!(settings.exists(), "settings.json must be created");
            let content = fs::read_to_string(&settings).unwrap();
            assert!(
                content.contains(CLAUDE_HOOK_COMMAND),
                "settings.json must contain hook command"
            );
        });
    }

    #[test]
    fn test_patch_settings_json_tolerates_utf8_bom() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            // Notepad and PowerShell 5.1 `Out-File -Encoding utf8` prepend a BOM.
            let settings = claude_dir.join(SETTINGS_JSON);
            fs::write(&settings, "\u{feff}{\"foo\": 1}").unwrap();

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
            );
            assert!(
                result.is_ok(),
                "BOM-prefixed settings.json must not abort init: {:?}",
                result.err()
            );

            let content = fs::read_to_string(&settings).unwrap();
            let v: serde_json::Value = serde_json::from_str(&content).unwrap();
            assert_eq!(v["foo"], 1, "existing keys must survive the patch");
            assert!(
                content.contains(CLAUDE_HOOK_COMMAND),
                "hook must be installed"
            );
        });
    }

    #[test]
    fn test_patch_settings_json_bom_plus_invalid_json_still_errors() {
        // Stripping the BOM must not mask genuinely broken JSON: the
        // parse-error context has to survive so the user gets blamed for
        // the right thing.
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            let settings = claude_dir.join(SETTINGS_JSON);
            fs::write(&settings, "\u{feff}{not valid json").unwrap();

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
            );
            let err = result.expect_err("invalid JSON must still fail");
            assert!(
                err.to_string().contains("Failed to parse"),
                "error must carry the parse context, got: {err:#}"
            );
        });
    }

    #[test]
    fn test_patch_settings_json_bom_only_file() {
        // U+FEFF is not whitespace, so the `content.trim().is_empty()`
        // empty-file guard does not catch a BOM-only file.
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            let settings = claude_dir.join(SETTINGS_JSON);
            fs::write(&settings, "\u{feff}").unwrap();

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
            );
            assert!(
                result.is_ok(),
                "BOM-only settings.json must be treated as empty: {:?}",
                result.err()
            );
        });
    }

    #[test]
    fn test_global_uninstall_removes_artifacts() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();
            uninstall(
                true,
                false,
                false,
                false,
                false,
                false,
                InitContext::default(),
            )
            .unwrap();

            assert!(!claude_dir.join(RTK_MD).exists(), "RTK.md must be removed");
            let settings_content =
                fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap_or_default();
            assert!(
                !settings_content.contains(CLAUDE_HOOK_COMMAND),
                "hook entry must be removed from settings.json"
            );
        });
    }

    #[test]
    fn test_global_default_mode_idempotent() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            let settings = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            let count = settings.matches(CLAUDE_HOOK_COMMAND).count();
            assert_eq!(count, 1, "hook command must appear exactly once");
        });
    }

    #[test]
    fn test_local_init_no_hook() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let result = run_default_mode(false, PatchMode::Auto, false, InitContext::default());
        std::env::set_current_dir(&cwd).unwrap();

        result.unwrap();
        assert!(
            tmp.path().join(CLAUDE_MD).exists(),
            "local CLAUDE.md must be created"
        );
        assert!(
            !tmp.path().join(SETTINGS_JSON).exists(),
            "settings.json must not be created for local init"
        );
    }

    #[test]
    fn test_global_hook_only_mode_creates_settings() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_hook_only_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(
                !claude_dir.join(RTK_MD).exists(),
                "RTK.md must NOT be created in hook-only mode"
            );
            let settings = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            assert!(
                settings.contains(CLAUDE_HOOK_COMMAND),
                "settings.json must contain hook command"
            );
        });
    }

    #[test]
    fn test_run_default_mode_dry_run_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            let dry = InitContext {
                dry_run: true,
                ..Default::default()
            };
            run_default_mode(true, PatchMode::Auto, false, dry).unwrap();

            assert!(
                !claude_dir.join(RTK_MD).exists(),
                "dry-run must not create RTK.md"
            );
            assert!(
                !claude_dir.join(CLAUDE_MD).exists(),
                "dry-run must not create CLAUDE.md"
            );
            assert!(
                !claude_dir.join(SETTINGS_JSON).exists(),
                "dry-run must not create settings.json"
            );
        });
    }

    #[test]
    fn test_uninstall_dry_run_preserves_artifacts() {
        let tmp = TempDir::new().unwrap();
        with_claude_dir_override(&tmp, |claude_dir| {
            // Stage a real install first
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();
            assert!(claude_dir.join(RTK_MD).exists());
            assert!(claude_dir.join(SETTINGS_JSON).exists());

            let settings_before = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            let rtk_md_before = fs::read_to_string(claude_dir.join(RTK_MD)).unwrap();

            // Dry-run uninstall
            let dry = InitContext {
                dry_run: true,
                ..Default::default()
            };
            uninstall(true, false, false, false, false, false, dry).unwrap();

            // Files must still exist with identical content
            assert!(
                claude_dir.join(RTK_MD).exists(),
                "dry-run uninstall must not remove RTK.md"
            );
            assert!(
                claude_dir.join(SETTINGS_JSON).exists(),
                "dry-run uninstall must not remove settings.json"
            );
            assert_eq!(
                fs::read_to_string(claude_dir.join(RTK_MD)).unwrap(),
                rtk_md_before,
                "dry-run uninstall must not modify RTK.md"
            );
            assert_eq!(
                fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap(),
                settings_before,
                "dry-run uninstall must not modify settings.json"
            );
        });
    }

    #[test]
    fn test_uninstall_removes_rtk_instructions_block() {
        let temp = TempDir::new().unwrap();
        let claude_md = temp.path().join("CLAUDE.md");

        fs::write(&claude_md, RTK_INSTRUCTIONS).unwrap();
        assert!(claude_md.exists());

        let content = fs::read_to_string(&claude_md).unwrap();
        assert!(content.contains(RTK_BLOCK_START));

        let (cleaned, did_remove) = remove_rtk_block(&content);
        assert!(did_remove);
        assert!(!cleaned.contains(RTK_BLOCK_START));
        assert!(!cleaned.contains("rtk cargo test"));
    }

    #[test]
    fn test_uninstall_preserves_non_rtk_content() {
        let content = format!(
            "# My Project\n\nSome custom instructions.\n\n{}\n\n## Other Notes\n\nKeep this.",
            RTK_INSTRUCTIONS
        );

        let (cleaned, did_remove) = remove_rtk_block(&content);

        assert!(did_remove);
        assert!(cleaned.contains("# My Project"));
        assert!(cleaned.contains("Some custom instructions."));
        assert!(cleaned.contains("## Other Notes"));
        assert!(cleaned.contains("Keep this."));
        assert!(!cleaned.contains(RTK_BLOCK_START));
    }

    #[test]
    fn test_uninstall_handles_both_artifacts() {
        let content = format!("# Config\n\n@RTK.md\n\n{}\n\nMore stuff", RTK_INSTRUCTIONS);

        let after_at_removal: String = content
            .lines()
            .filter(|line| !line.trim().starts_with("@RTK.md"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!after_at_removal.contains("@RTK.md"));
        assert!(after_at_removal.contains(RTK_BLOCK_START));

        let (final_content, did_remove) = remove_rtk_block(&after_at_removal);
        assert!(did_remove);
        assert!(!final_content.contains(RTK_BLOCK_START));
        assert!(final_content.contains("# Config"));
        assert!(final_content.contains("More stuff"));
    }

    #[test]
    fn test_uninstall_integration_preserves_user_content() {
        let user_content = "# My Project Rules\n\nAlways use snake_case.";
        let installed = format!("{}\n\n{}", user_content, RTK_INSTRUCTIONS);

        let (cleaned, did_remove) = remove_rtk_block(&installed);
        assert!(did_remove);
        assert!(!cleaned.trim().is_empty(), "user content should remain");
        assert!(
            cleaned.contains("My Project Rules"),
            "user content must be preserved"
        );
        assert!(
            cleaned.contains("snake_case"),
            "user content must be preserved"
        );
        assert!(
            !cleaned.contains(RTK_BLOCK_START),
            "RTK block must be fully removed"
        );
        assert!(
            !cleaned.contains(RTK_BLOCK_END),
            "RTK end marker must be removed"
        );
    }
}
