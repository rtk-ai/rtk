//! Sets up RTK hooks so AI coding agents automatically route commands through RTK.
//!
//! `run` dispatches on the target agent; each agent's install and uninstall live in its own
//! submodule, and the helpers shared between them stay here.
use crate::hooks::constants::{
    CLAUDE_DIR, CLAUDE_HOOK_COMMAND, HOOKS_JSON, HOOKS_SUBDIR, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE,
    SETTINGS_JSON,
};
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

use crate::core::utils::{from_json_str, strip_leading_bom};

use super::integrity;
use super::is_claude_hook_command;
use crate::core::config::AwarenessLevel;

mod agents_md;
mod claude;
mod codex;
mod copilot;
mod cursor;
mod droid;
mod gemini;
mod hermes;
mod instructions_agents;
mod opencode;
mod pi;
mod symlinks;
mod trae;
mod vibe;
mod write;

// `agents_md` and `pi` hold helpers that several submodules share, so they are
// glob-imported and reach siblings through `use super::*`; every other submodule is used only
// from here and is imported by name.
use agents_md::*;
use claude::{
    hook_already_present, remove_hook_from_settings, run_claude_md_mode, run_default_mode,
    run_hook_only_mode,
};
use codex::{run_codex_mode, show_codex_config, uninstall_codex};
use cursor::{
    cursor_hook_already_present, install_cursor_hooks, remove_cursor_hooks, resolve_cursor_dir,
};
use gemini::uninstall_gemini;
use instructions_agents::{run_cline_mode, run_windsurf_mode};
use opencode::{
    opencode_plugin_path, remove_opencode_plugin, resolve_opencode_dir, run_opencode_only_mode,
};
use pi::*;
use symlinks::*;
use write::*;

pub(crate) use copilot::{COPILOT_HOOK_JSON, copilot_user_dir};
pub use copilot::{run_copilot, run_copilot_global, uninstall_copilot, uninstall_copilot_global};
pub use droid::{run_droid_mode, uninstall_droid};
pub use gemini::run_gemini;
pub use hermes::{run_hermes_mode, uninstall_hermes};
pub use instructions_agents::{run_antigravity_mode, run_kilocode_mode, run_kimi_mode};
pub use pi::{run_omp_mode_with_patch_mode, run_pi_mode_with_patch_mode};
pub use trae::{run_trae_mode, uninstall_trae_mode};
pub use vibe::{run_vibe_mode, uninstall_vibe};

// Embedded agent-neutral RTK awareness instructions, one file per `awareness.level`.
pub(super) const RTK_AWARENESS_DEFAULT: &str = include_str!("../../../hooks/rtk-awareness.md");
pub(super) const RTK_AWARENESS_HIGH: &str = include_str!("../../../hooks/rtk-awareness-high.md");
pub(super) const RTK_AWARENESS_FULL: &str = include_str!("../../../hooks/rtk-awareness-full.md");

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

pub(super) const RTK_MD: &str = "RTK.md";

pub(super) const CLAUDE_MD: &str = "CLAUDE.md";

pub(super) const AGENTS_MD: &str = "AGENTS.md";

pub(super) const RTK_MD_REF: &str = "@RTK.md";

pub(super) const RTK_BLOCK_START: &str = "<!-- rtk-instructions";
const RTK_BLOCK_VERSION: &str = "v2";
pub(super) const RTK_BLOCK_END: &str = "<!-- /rtk-instructions -->";

/// Control flow for settings.json patching
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum PatchMode {
    #[default]
    Ask, // Default: prompt user [y/N]
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
    /// `awareness.level` from config.toml. Agents with a command hook receive the matching
    /// awareness file; agents without a hook always receive `full` (see `print_instructions_agents_awareness_note`).
    pub awareness: AwarenessLevel,
    /// `--auto-patch` / `--no-patch`, for the decisions that are not about one agent's
    /// settings file, such as a project write that resolves outside the project.
    pub patch_mode: PatchMode,
}

pub(super) fn awareness_content(level: AwarenessLevel) -> &'static str {
    match level {
        AwarenessLevel::Default => RTK_AWARENESS_DEFAULT,
        AwarenessLevel::High => RTK_AWARENESS_HIGH,
        AwarenessLevel::Full => RTK_AWARENESS_FULL,
    }
}

/// Wrap an awareness file in the `<!-- rtk-instructions -->` markers used by
/// `write_rtk_block`, so it can be upserted into a shared file like AGENTS.md.
pub(super) fn rtk_block(body: &str) -> String {
    format!("{RTK_BLOCK_START} {RTK_BLOCK_VERSION} -->\n{body}{RTK_BLOCK_END}\n")
}

/// Shared dry-run footer printed at the end of every init sub-mode.
pub(super) fn print_dry_run_footer() {
    println!("\n[dry-run] Nothing written.");
}

// Legacy full instructions for backward compatibility (--claude-md mode)
pub(super) const RTK_INSTRUCTIONS: &str = r##"<!-- rtk-instructions v2 -->
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
    let _scope = (!global).then(|| ProjectScope::enter(ctx));
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

            // Cursor hooks: on their own, or next to the OpenCode plugin with --opencode
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

    if crate::core::tee_file::legacy_tee_migration_pending() {
        println!("{}", crate::core::tee_file::LEGACY_TEE_NOTICE);
        println!();
    } else if crate::core::tee_file::legacy_tee_config_in_use() {
        println!("{}", crate::core::tee_file::LEGACY_TEE_CONFIG_NOTICE);
        println!();
    } else if crate::core::tee_file::legacy_tee_fields_merged_in_use() {
        println!("{}", crate::core::tee_file::LEGACY_TEE_MERGED_NOTICE);
        println!();
    }

    Ok(())
}

/// Idempotent file write: create or update if content differs.
/// When `dry_run` is true, prints the intended action and does not touch the filesystem.
pub(super) fn write_if_changed(
    path: &Path,
    content: &str,
    name: &str,
    ctx: InitContext,
) -> Result<bool> {
    write_if_changed_internal(path, content, name, ctx, false, WriteKind::Owned)
}

/// [`write_if_changed`] for a config file RTK edits but does not own: the existing file is
/// backed up, then replaced atomically (see [`WriteKind::Config`]).
pub(super) fn patch_if_changed(
    path: &Path,
    content: &str,
    name: &str,
    ctx: InitContext,
) -> Result<bool> {
    write_if_changed_internal(path, content, name, ctx, false, WriteKind::Config)
}

/// Variant used for protected RTK files. A file that cannot be decoded is
/// still replaceable after the caller's policy allows it (for example,
/// `--auto-patch`), so a read error is treated like differing content instead
/// of preventing recovery.
pub(super) fn write_if_changed_allow_read_error(
    path: &Path,
    content: &str,
    name: &str,
    ctx: InitContext,
) -> Result<bool> {
    write_if_changed_internal(path, content, name, ctx, true, WriteKind::Owned)
}

fn write_if_changed_internal(
    path: &Path,
    content: &str,
    name: &str,
    ctx: InitContext,
    allow_read_error: bool,
    kind: WriteKind,
) -> Result<bool> {
    let verb = if path.exists() {
        match fs::read_to_string(path) {
            Ok(existing) if existing == content => {
                if ctx.verbose > 0 {
                    eprintln!("{} already up to date: {}", name, path.display());
                }
                return Ok(false);
            }
            Ok(_) => "update",
            Err(_) if allow_read_error => "update",
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read {}: {}", name, path.display()));
            }
        }
    } else {
        "create"
    };
    let done = if verb == "create" {
        "Created"
    } else {
        "Updated"
    };
    let report = Report::new(format!(
        "[dry-run] would {verb} {}: {}",
        name,
        path.display()
    ))
    .with_content()
    .done_verbose(format!("{done} {}: {}", name, path.display()));
    write_reported(path, kind, content, ctx, report)
        .with_context(|| format!("Failed to write {}: {}", name, path.display()))?;
    Ok(true)
}

/// Read a JSON file with path-aware errors. Missing files return `None` and
/// empty files are treated as an empty JSON object.
pub(super) fn read_json_file(path: &Path) -> Result<Option<serde_json::Value>> {
    if !path.exists() {
        return Ok(None);
    }

    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let content = strip_leading_bom(&content);
    if content.trim().is_empty() {
        return Ok(Some(serde_json::json!({})));
    }

    from_json_str(content)
        .map(Some)
        .with_context(|| format!("Failed to parse {} as JSON", path.display()))
}

/// Canonicalize a path even when its final file has not been
/// created yet. This detects agent directories connected by symlinks while
/// retaining a literal-path fallback for genuinely unresolved paths.
pub(super) fn canonicalize_path_for_comparison(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }

    let mut missing_components = Vec::new();
    let mut candidate = path;
    loop {
        if let Ok(mut canonical) = fs::canonicalize(candidate) {
            for component in missing_components.iter().rev() {
                canonical.push(component);
            }
            return canonical;
        }

        let Some(file_name) = candidate.file_name() else {
            return path.to_path_buf();
        };
        missing_components.push(file_name.to_os_string());

        let Some(parent) = candidate.parent() else {
            return path.to_path_buf();
        };
        if parent == candidate {
            return path.to_path_buf();
        }
        candidate = parent;
    }
}

/// Where [`write_file`] puts the copy it takes of a config or instructions file before writing
/// `path`. Named so a caller that has to vouch for where it writes can vouch for this one too.
pub(super) fn backup_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

/// Serialise `root` and write it to `path` as a config RTK edits, reported by `report`.
/// `label` names the file in the serialisation error.
pub(super) fn update_json_file(
    path: &Path,
    root: &serde_json::Value,
    ctx: InitContext,
    label: &str,
    report: Report,
) -> Result<()> {
    let serialized = serde_json::to_string_pretty(root)
        .with_context(|| format!("Failed to serialize {label}"))?;
    write_reported(path, WriteKind::Config, &serialized, ctx, report)?;
    Ok(())
}

/// Prompt user for confirmation.
/// Prints to stderr (stdout may be piped), reads from stdin, and defaults to
/// No in non-interactive environments.
pub(super) fn prompt_user_confirmation(prompt: &str) -> Result<bool> {
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
pub(super) fn prompt_user_consent(settings_path: &Path) -> Result<bool> {
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

pub(super) fn print_manual_instructions(hook_command: &str, include_opencode: bool) {
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
    let _scope = (!global).then(|| ProjectScope::enter(ctx));
    let InitContext { dry_run, .. } = ctx;
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
        anyhow::bail!(
            "Uninstall only works with --global flag. For local projects, manually remove RTK from CLAUDE.md"
        );
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
            // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
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
            // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
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
            } else {
                let report = Report::new(format!(
                    "[dry-run] would update CLAUDE.md: {}",
                    claude_md_path.display()
                ))
                .with_content();
                write_reported(
                    &claude_md_path,
                    WriteKind::Instructions,
                    &working_content,
                    ctx,
                    report,
                )
                .with_context(|| {
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

/// Clean up consecutive blank lines (collapse 3+ to 2)
/// Used when removing @RTK.md line from CLAUDE.md
pub(super) fn clean_double_blanks(content: &str) -> String {
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

/// Only command hooks belong to RTK; prompt/agent entries are user-owned.
pub(super) fn is_command_hook(hook: &serde_json::Value, matches: impl Fn(&str) -> bool) -> bool {
    hook.get("type").is_none_or(|kind| kind == "command")
        && hook
            .get("command")
            .and_then(serde_json::Value::as_str)
            .is_some_and(matches)
}

/// Codex/Cursor match tool names with regular expressions.
pub(super) fn group_covers_tool(group: &serde_json::Value, tool: &str) -> bool {
    match group.get("matcher") {
        None | Some(serde_json::Value::Null) => true,
        Some(matcher) => matcher.as_str().is_some_and(|pattern| {
            pattern.is_empty()
                || pattern == "*"
                || regex::Regex::new(pattern).is_ok_and(|regex| regex.is_match(tool))
        }),
    }
}

#[derive(Clone, Copy)]
pub(super) enum HookEntries {
    Grouped,
    Flat,
}

/// Share traversal, while callers retain their host's matcher and ownership rules.
pub(super) fn hook_present(
    root: &serde_json::Value,
    event: &str,
    layout: HookEntries,
    covers: impl Fn(&serde_json::Value) -> bool,
    owns: impl Fn(&serde_json::Value) -> bool,
) -> bool {
    root.get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                covers(entry)
                    && match layout {
                        HookEntries::Grouped => entry
                            .get("hooks")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|hooks| hooks.iter().any(&owns)),
                        HookEntries::Flat => owns(entry),
                    }
            })
        })
}

pub(super) fn append_hook_entry(
    root: &mut serde_json::Value,
    event: &str,
    entry: serde_json::Value,
) -> Result<()> {
    let hooks = root
        .as_object_mut()
        .context("hook config root is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("hooks value is not an object")?;
    hooks
        .entry(event)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .with_context(|| format!("{event} value is not an array"))?
        .push(entry);
    Ok(())
}

/// Remove only owned entries, pruning a group only when this removal empties it.
pub(super) fn remove_hook_entries(
    root: &mut serde_json::Value,
    event: &str,
    layout: HookEntries,
    owns: impl Fn(&serde_json::Value) -> bool,
) -> bool {
    let Some(entries) = root
        .get_mut("hooks")
        .and_then(|hooks| hooks.get_mut(event))
        .and_then(serde_json::Value::as_array_mut)
    else {
        return false;
    };
    let mut removed = false;
    entries.retain_mut(|entry| match layout {
        HookEntries::Flat => {
            let matched = owns(entry);
            removed |= matched;
            !matched
        }
        HookEntries::Grouped => {
            let Some(hooks) = entry
                .get_mut("hooks")
                .and_then(serde_json::Value::as_array_mut)
            else {
                return true;
            };
            let before = hooks.len();
            hooks.retain(|hook| !owns(hook));
            if hooks.len() == before {
                return true;
            }
            removed = true;
            !hooks.is_empty()
        }
    });
    removed
}

/// Deep-merge RTK hook entry into settings.json
/// Creates hooks.PreToolUse structure if missing, preserves existing hooks
pub(super) fn insert_hook_entry(root: &mut serde_json::Value, hook_command: &str) -> Result<()> {
    if !root.is_object() {
        *root = serde_json::json!({});
    }
    append_hook_entry(
        root,
        PRE_TOOL_USE_KEY,
        serde_json::json!({
            "matcher": "Bash",
            "hooks": [{"type": "command", "command": hook_command}]
        }),
    )
}

/// Generate .rtk/filters.toml template in the current directory if not present.
pub(super) fn project_filters_template_path() -> PathBuf {
    Path::new(".rtk").join("filters.toml")
}

pub(super) fn generate_project_filters_template(ctx: InitContext) -> Result<()> {
    let InitContext { verbose, .. } = ctx;
    let path = project_filters_template_path();

    if path.exists() {
        if verbose > 0 {
            eprintln!(".rtk/filters.toml already exists, skipping template");
        }
        return Ok(());
    }

    let report = Report::new(format!(
        "[dry-run] would create .rtk/filters.toml template: {}",
        path.display()
    ))
    .done(format!(
        "  filters:   {} (template, edit to add project filters)",
        path.display()
    ));
    write_reported(&path, WriteKind::Owned, FILTERS_TEMPLATE, ctx, report)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Generate ~/.config/rtk/filters.toml template if not present.
pub(super) fn generate_global_filters_template(ctx: InitContext) -> Result<()> {
    let InitContext { verbose, .. } = ctx;
    let config_dir = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".config"));
    let rtk_dir = config_dir.join(crate::core::constants::RTK_DATA_DIR);
    let path = rtk_dir.join("filters.toml");

    if path.exists() {
        if verbose > 0 {
            eprintln!("{} already exists, skipping template", path.display());
        }
        return Ok(());
    }

    let report = Report::new(format!(
        "[dry-run] would create global filters template: {}",
        path.display()
    ))
    .done(format!(
        "  filters:   {} (template, edit to add user-global filters)",
        path.display()
    ));
    write_reported(
        &path,
        WriteKind::Owned,
        FILTERS_GLOBAL_TEMPLATE,
        ctx,
        report,
    )
    .with_context(|| format!("Failed to write {}", path.display()))?;
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

/// An agent's config directory: `override_dir` when set and non-empty, else `home/subdir`.
/// `error` names the agent and its variable when neither is available.
pub(super) fn resolve_config_dir(
    override_dir: Option<OsString>,
    home_dir: Option<PathBuf>,
    subdir: &str,
    error: &'static str,
) -> Result<PathBuf> {
    if let Some(dir) = override_dir.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    home_dir.map(|home| home.join(subdir)).context(error)
}

pub(super) fn resolve_home_subdir(subdir: &str) -> Result<PathBuf> {
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

pub(super) fn resolve_claude_dir_from(
    claude_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    resolve_config_dir(
        claude_dir.map(PathBuf::into_os_string),
        home_dir,
        CLAUDE_DIR,
        "Cannot determine Claude config directory. Set $CLAUDE_CONFIG_DIR or $HOME.",
    )
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
    match resolve_opencode_dir() {
        Ok(opencode_dir) => {
            let plugin = opencode_plugin_path(&opencode_dir);
            if plugin.exists() {
                println!("[ok] OpenCode: plugin installed ({})", plugin.display());
            } else {
                println!("[--] OpenCode: plugin not found");
            }
        }
        _ => println!("[--] OpenCode: config dir not found"),
    }

    // Check Cursor hooks
    match resolve_cursor_dir() {
        Ok(cursor_dir) => {
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
                    println!(
                        "[warn] Cursor hook: {} (legacy script — run `rtk init -g --agent cursor` to upgrade)",
                        cursor_hook.display()
                    );
                }
            } else {
                println!("[--] Cursor hook: not found");
            }
        }
        _ => println!("[--] Cursor: home dir not found"),
    }

    println!("\nUsage:");
    println!("  rtk init              # Full injection into local CLAUDE.md");
    println!("  rtk init -g           # Hook + RTK.md + @RTK.md + settings.json (recommended)");
    println!("  rtk init -g --auto-patch    # Same as above but no prompt");
    println!("  rtk init -g --no-patch      # Skip settings.json (manual setup)");
    println!("  rtk init -g --uninstall     # Remove all RTK artifacts");
    println!("  rtk init -g --claude-md     # Legacy: full injection into ~/.claude/CLAUDE.md");
    println!("  rtk init -g --hook-only     # Hook only, no RTK.md");
    println!("  rtk init --codex            # Configure local AGENTS.md + RTK.md + hooks.json");
    println!("  rtk init -g --codex         # Configure global AGENTS.md + RTK.md + hooks.json");
    println!("  rtk init -g --opencode      # OpenCode plugin only");
    println!("  rtk init -g --agent cursor  # Install Cursor Agent hooks");

    Ok(())
}

#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use tempfile::TempDir;
/// Serialises all tests that mutate the process-wide working directory.
#[cfg(test)]
pub(super) static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Holds the cwd lock and puts the working directory back when it goes out of scope.
///
/// Restoring by hand needs the call under test to return rather than panic, so one failing
/// assertion used to leave every later test inside a deleted `TempDir`, burying the real
/// failure under unrelated ones.
#[cfg(test)]
pub(super) struct CwdGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    original: PathBuf,
}

#[cfg(test)]
impl CwdGuard {
    pub(super) fn enter(dir: &Path) -> Self {
        let _lock = CWD_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let original = std::env::current_dir().expect("read the current directory");
        std::env::set_current_dir(dir).expect("enter the test directory");
        Self { _lock, original }
    }
}

#[cfg(test)]
impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

/// Whether this process is refused writing a read-only file; root is not.
#[cfg(test)]
pub(super) fn read_only_is_enforced(path: &Path) -> bool {
    fs::OpenOptions::new().write(true).open(path).is_err()
}

#[cfg(test)]
pub(super) fn with_claude_dir_override<F: FnOnce(&Path)>(tmp: &TempDir, f: F) {
    let claude_dir = tmp.path().join(CLAUDE_DIR);
    fs::create_dir_all(&claude_dir).unwrap();

    temp_env::with_var("CLAUDE_CONFIG_DIR", Some(&claude_dir), || f(&claude_dir));
}

#[cfg(test)]
pub(super) fn with_missing_claude_dir_override<F: FnOnce(&Path)>(tmp: &TempDir, f: F) {
    let claude_dir = tmp.path().join(CLAUDE_DIR);
    let home_dir = tmp.path().join("home");
    assert!(
        !claude_dir.exists(),
        "test precondition: Claude config dir must be missing"
    );

    temp_env::with_vars(
        [
            ("CLAUDE_CONFIG_DIR", Some(claude_dir.as_os_str())),
            ("HOME", Some(home_dir.as_os_str())),
        ],
        || f(&claude_dir),
    );
}

#[cfg(test)]
mod tests {
    use super::claude::{remove_hook_from_json, remove_legacy_hook_entries_from_json};
    use super::codex::{codex_hook_already_present, remove_codex_hook_from_json};
    use super::cursor::remove_legacy_cursor_hook_entries_from_json;
    use super::trae::{
        insert_trae_hook_entry, remove_trae_hook_from_json, trae_hook_already_present,
    };
    use super::*;
    use crate::hooks::constants::{
        CODEX_HOOK_COMMAND, CURSOR_HOOK_COMMAND, TRAE_HOOK_COMMAND, TRAE_RUN_COMMAND_MATCHER,
    };
    use tempfile::TempDir;

    #[test]
    fn test_awareness_content_maps_each_level_to_its_file() {
        assert_eq!(
            awareness_content(AwarenessLevel::Default),
            RTK_AWARENESS_DEFAULT
        );
        assert_eq!(awareness_content(AwarenessLevel::High), RTK_AWARENESS_HIGH);
        assert_eq!(awareness_content(AwarenessLevel::Full), RTK_AWARENESS_FULL);
    }

    #[test]
    fn test_awareness_levels_nest() {
        let contract = RTK_AWARENESS_DEFAULT.trim_end();
        assert!(
            RTK_AWARENESS_HIGH.starts_with(contract),
            "high must start with the default output contract verbatim"
        );
        assert!(
            RTK_AWARENESS_FULL.contains(contract),
            "full must contain the default output contract verbatim"
        );
    }

    #[test]
    fn test_awareness_activation_rule_only_in_full() {
        const ACTIVATION: &str = "Prefix every shell command with `rtk`";
        assert!(!RTK_AWARENESS_DEFAULT.contains(ACTIVATION));
        assert!(!RTK_AWARENESS_HIGH.contains(ACTIVATION));
        assert!(RTK_AWARENESS_FULL.contains(ACTIVATION));
    }

    #[test]
    fn test_awareness_meta_commands_present_above_default() {
        for (name, content) in [
            ("default", RTK_AWARENESS_DEFAULT),
            ("high", RTK_AWARENESS_HIGH),
            ("full", RTK_AWARENESS_FULL),
        ] {
            assert!(
                content.contains("rtk proxy <cmd>"),
                "{name} must mention rtk proxy"
            );
        }
        for (name, content) in [("high", RTK_AWARENESS_HIGH), ("full", RTK_AWARENESS_FULL)] {
            assert!(
                content.contains("RTK_DISABLED=1"),
                "{name} must mention RTK_DISABLED"
            );
            assert!(content.contains("rtk gain"), "{name} must mention rtk gain");
        }
        assert!(!RTK_AWARENESS_DEFAULT.contains("rtk gain"));
    }

    #[test]
    fn test_awareness_files_stay_within_line_budget() {
        assert!(RTK_AWARENESS_DEFAULT.lines().count() <= 10);
        assert!(RTK_AWARENESS_HIGH.lines().count() <= 21);
        assert!(RTK_AWARENESS_FULL.lines().count() <= 25);
    }

    #[test]
    fn test_rtk_block_wraps_body_in_markers() {
        let block = rtk_block(RTK_AWARENESS_FULL);
        assert!(block.starts_with(&format!("{RTK_BLOCK_START} v2 -->\n")));
        assert!(block.ends_with(&format!("{RTK_BLOCK_END}\n")));
        assert!(block.contains(RTK_AWARENESS_FULL));
    }

    #[test]
    fn test_local_init_block_follows_awareness_level_not_legacy() {
        for level in [
            AwarenessLevel::Default,
            AwarenessLevel::High,
            AwarenessLevel::Full,
        ] {
            let block = rtk_block(awareness_content(level));
            assert!(block.starts_with(RTK_BLOCK_START));
            assert!(block.contains(awareness_content(level).trim_end()));
        }
        // Plain local init must not ship the legacy Golden Rule; --claude-md keeps it.
        assert!(!rtk_block(awareness_content(AwarenessLevel::Default)).contains("Golden Rule"));
        assert!(!rtk_block(awareness_content(AwarenessLevel::High)).contains("Golden Rule"));
        assert!(RTK_INSTRUCTIONS.contains("Golden Rule"));
    }

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
    // Tests for insert_hook_entry()
    #[test]
    fn test_insert_hook_entry_empty_root() {
        let mut json_content = serde_json::json!({});
        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";

        insert_hook_entry(&mut json_content, hook_command).unwrap();

        // Should create full structure
        assert!(json_content.get("hooks").is_some());
        assert!(
            json_content
                .get("hooks")
                .unwrap()
                .get("PreToolUse")
                .is_some()
        );

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

    // Tests for write_file() with RTK-owned files
    #[test]
    fn test_owned_write() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.json");

        let content = r#"{"key": "value"}"#;
        write_file(&file_path, WriteKind::Owned, content).unwrap();

        assert!(file_path.exists());
        let written = fs::read_to_string(&file_path).unwrap();
        assert_eq!(written, content);
    }

    #[test]
    fn test_owned_write_creates_missing_parent_dirs() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("agent").join("hooks").join("hooks.json");

        write_file(&file_path, WriteKind::Owned, "{}").unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), "{}");
    }

    #[cfg(unix)]
    #[test]
    fn test_owned_write_through_a_dangling_symlink_creates_its_target() {
        let temp = TempDir::new().unwrap();
        let link_path = temp.path().join("hooks.json");
        let real_path = temp.path().join("dotfiles").join("hooks.json");
        fs::create_dir(temp.path().join("dotfiles")).unwrap();
        std::os::unix::fs::symlink("dotfiles/hooks.json", &link_path).unwrap();

        write_file(&link_path, WriteKind::Owned, "{}").unwrap();

        assert!(fs::symlink_metadata(&link_path).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(&real_path).unwrap(), "{}");
    }

    #[cfg(unix)]
    #[test]
    fn test_owned_write_keeps_the_existing_mode() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        for mode in [0o644, 0o600, 0o755, 0o664] {
            let file_path = temp.path().join(format!("file-{mode:o}"));
            fs::write(&file_path, "old").unwrap();
            fs::set_permissions(&file_path, fs::Permissions::from_mode(mode)).unwrap();

            write_file(&file_path, WriteKind::Owned, "new").unwrap();

            let actual = fs::metadata(&file_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(actual, mode, "mode of {}", file_path.display());
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_patch_file_replaces_a_symlinked_backup_instead_of_following_it() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("hooks.json");
        let other = temp.path().join("CLAUDE.md");
        fs::write(&path, "{}").unwrap();
        fs::write(&other, "notes").unwrap();
        std::os::unix::fs::symlink("CLAUDE.md", backup_path_for(&path)).unwrap();

        write_file(&path, WriteKind::Config, "{\"new\":1}").unwrap();

        assert_eq!(fs::read_to_string(&other).unwrap(), "notes");
        let backup = backup_path_for(&path);
        assert!(!fs::symlink_metadata(&backup).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(backup).unwrap(), "{}");
    }

    #[cfg(unix)]
    #[test]
    fn test_patch_file_does_not_create_a_dangling_backup_target() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("hooks.json");
        fs::write(&path, "{}").unwrap();
        std::os::unix::fs::symlink("outside.json", backup_path_for(&path)).unwrap();

        write_file(&path, WriteKind::Config, "{}").unwrap();

        assert!(!temp.path().join("outside.json").exists());
    }

    #[test]
    fn test_patch_file_leaves_a_file_hard_linked_to_the_backup_alone() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("hooks.json");
        let other = temp.path().join("notes.txt");
        fs::write(&path, "{}").unwrap();
        fs::write(&other, "notes").unwrap();
        fs::hard_link(&other, backup_path_for(&path)).unwrap();

        write_file(&path, WriteKind::Config, "{\"new\":1}").unwrap();

        assert_eq!(fs::read_to_string(&other).unwrap(), "notes");
        assert_eq!(fs::read_to_string(backup_path_for(&path)).unwrap(), "{}");
    }

    #[cfg(unix)]
    #[test]
    fn test_patch_file_writes_in_place_and_keeps_hard_links() {
        use std::os::unix::fs::MetadataExt;
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("CLAUDE.md");
        let other = temp.path().join("other-checkout-CLAUDE.md");
        fs::write(&path, "notes\n").unwrap();
        fs::hard_link(&path, &other).unwrap();
        let inode = fs::metadata(&path).unwrap().ino();

        let backup = write_file(&path, WriteKind::Instructions, "notes\n\n@RTK.md\n")
            .unwrap()
            .unwrap();

        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(fs::read_to_string(&other).unwrap(), "notes\n\n@RTK.md\n");
        assert_eq!(backup, temp.path().join("CLAUDE.md.bak"));
        assert_eq!(fs::read_to_string(backup).unwrap(), "notes\n");
    }

    #[test]
    fn test_owned_write_takes_no_backup() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("RTK.md");
        fs::write(&path, "old").unwrap();

        write_file(&path, WriteKind::Owned, "new").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert!(!temp.path().join("RTK.md.bak").exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_patch_config_replaces_the_file_atomically_after_a_backup() {
        use std::os::unix::fs::MetadataExt;
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        fs::write(&path, "{}").unwrap();
        let inode = fs::metadata(&path).unwrap().ino();

        let backup = write_file(&path, WriteKind::Config, "{\"hooks\":{}}")
            .unwrap()
            .unwrap();

        assert_ne!(fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"hooks\":{}}");
        assert_eq!(fs::read_to_string(backup).unwrap(), "{}");
    }

    #[cfg(unix)]
    #[test]
    fn test_an_existing_backup_link_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("CLAUDE.md");
        let protected = temp.path().join("protected");
        fs::write(&path, "notes\n").unwrap();
        fs::write(&protected, "keep\n").unwrap();
        fs::set_permissions(&protected, fs::Permissions::from_mode(0o444)).unwrap();
        std::os::unix::fs::symlink("protected", backup_path_for(&path)).unwrap();

        write_file(&path, WriteKind::Instructions, "notes\n\n@RTK.md\n").unwrap();

        assert_eq!(fs::read_to_string(&protected).unwrap(), "keep\n");
        assert!(
            fs::symlink_metadata(backup_path_for(&path))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "notes\n\n@RTK.md\n");
    }

    #[cfg(unix)]
    #[test]
    fn test_a_read_only_backup_left_over_does_not_block_creating_the_file() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("CLAUDE.md");
        fs::write(backup_path_for(&path), "old backup\n").unwrap();
        fs::set_permissions(backup_path_for(&path), fs::Permissions::from_mode(0o444)).unwrap();

        write_file(&path, WriteKind::Instructions, "@RTK.md\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "@RTK.md\n");
    }

    #[cfg(unix)]
    #[test]
    fn test_an_instruction_file_with_a_read_only_backup_is_written_without_one() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("CLAUDE.md");
        fs::write(&path, "notes\n").unwrap();
        fs::write(backup_path_for(&path), "kept\n").unwrap();
        fs::set_permissions(backup_path_for(&path), fs::Permissions::from_mode(0o444)).unwrap();
        if !read_only_is_enforced(&backup_path_for(&path)) {
            return;
        }

        let backup = write_file(&path, WriteKind::Instructions, "notes\n\n@RTK.md\n").unwrap();

        assert_eq!(backup, None);
        assert_eq!(fs::read_to_string(&path).unwrap(), "notes\n\n@RTK.md\n");
        assert_eq!(
            fs::read_to_string(backup_path_for(&path)).unwrap(),
            "kept\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_a_config_with_a_read_only_backup_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        fs::write(&path, "{}").unwrap();
        fs::write(backup_path_for(&path), "kept").unwrap();
        fs::set_permissions(backup_path_for(&path), fs::Permissions::from_mode(0o444)).unwrap();
        if !read_only_is_enforced(&backup_path_for(&path)) {
            return;
        }

        let error = write_file(&path, WriteKind::Config, "{\"hooks\":{}}").unwrap_err();

        assert!(format!("{error:#}").contains("backup"), "{error:#}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
    }

    #[cfg(unix)]
    #[test]
    fn test_an_instruction_file_linked_into_a_missing_directory_is_refused() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("CLAUDE.md");
        std::os::unix::fs::symlink("unmounted/dotfiles/CLAUDE.md", &path).unwrap();

        let error = write_file(&path, WriteKind::Instructions, "@RTK.md\n").unwrap_err();

        assert!(format!("{error:#}").contains("does not exist"), "{error:#}");
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn test_patch_config_rewrites_an_existing_backup_when_the_directory_is_locked() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let locked = temp.path().join("claude");
        let dotfiles = temp.path().join("dotfiles");
        fs::create_dir_all(&locked).unwrap();
        fs::create_dir_all(&dotfiles).unwrap();
        fs::write(dotfiles.join("settings.json"), "{}").unwrap();
        let path = locked.join("settings.json");
        std::os::unix::fs::symlink("../dotfiles/settings.json", &path).unwrap();
        fs::write(backup_path_for(&path), "old backup").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        // Root ignores the directory's mode, and then the backup is simply replaced.
        let locked_for_us = NamedTempFile::new_in(&locked).is_err();

        let result = write_file(&path, WriteKind::Config, "{\"hooks\":{}}");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        result.unwrap();
        assert_eq!(fs::read_to_string(backup_path_for(&path)).unwrap(), "{}");
        assert_eq!(
            fs::read_to_string(dotfiles.join("settings.json")).unwrap(),
            "{\"hooks\":{}}"
        );
        if locked_for_us {
            assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        }
    }

    #[test]
    fn test_an_earlier_instruction_backup_is_kept() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("CLAUDE.md");
        fs::write(&path, "# mine\n").unwrap();
        fs::write(backup_path_for(&path), "my hand-made rescue copy\n").unwrap();

        let backup = write_file(&path, WriteKind::Instructions, "# mine\n\n@RTK.md\n").unwrap();

        assert_eq!(backup, None);
        assert_eq!(
            fs::read_to_string(backup_path_for(&path)).unwrap(),
            "my hand-made rescue copy\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_a_config_whose_backup_is_a_hard_link_to_it_is_not_emptied() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let locked = temp.path().join("claude");
        let dotfiles = temp.path().join("dotfiles");
        fs::create_dir_all(&locked).unwrap();
        fs::create_dir_all(&dotfiles).unwrap();
        let real = dotfiles.join("settings.json");
        fs::write(&real, "{\"keep\":1}").unwrap();
        let path = locked.join("settings.json");
        std::os::unix::fs::symlink("../dotfiles/settings.json", &path).unwrap();
        fs::hard_link(&real, backup_path_for(&path)).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let locked_for_us = NamedTempFile::new_in(&locked).is_err();

        let result = write_file(&path, WriteKind::Config, "{}");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        if locked_for_us {
            assert!(result.is_err());
            assert_eq!(fs::read_to_string(&real).unwrap(), "{\"keep\":1}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_a_link_loop_is_reported_as_one() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("CLAUDE.md");
        std::os::unix::fs::symlink("CLAUDE.md", &path).unwrap();

        let error = write_file(&path, WriteKind::Instructions, "x").unwrap_err();

        assert!(
            format!("{error:#}").contains("cannot be followed"),
            "{error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_a_file_under_a_dangling_directory_link_is_refused() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("dotfiles")).unwrap();
        let cursor_dir = temp.path().join(".cursor");
        std::os::unix::fs::symlink("dotfiles/cursor", &cursor_dir).unwrap();

        let error =
            write_file(&cursor_dir.join("hooks.json"), WriteKind::Config, "{}").unwrap_err();

        assert!(format!("{error:#}").contains("does not exist"), "{error:#}");
        assert!(ensure_patchable(&cursor_dir.join("hooks.json")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_a_two_hop_dangling_chain_names_the_missing_directory() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("CLAUDE.md");
        std::os::unix::fs::symlink("link2", &path).unwrap();
        std::os::unix::fs::symlink("missing/dir/CLAUDE.md", temp.path().join("link2")).unwrap();

        let error = write_file(&path, WriteKind::Instructions, "x").unwrap_err();

        assert!(format!("{error:#}").contains("does not exist"), "{error:#}");
    }

    #[cfg(unix)]
    #[test]
    fn test_a_config_whose_backup_is_the_file_itself_is_refused() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        fs::write(&path, "{\"keep\":1}").unwrap();
        std::os::unix::fs::symlink("settings.json", backup_path_for(&path)).unwrap();

        let error = write_file(&path, WriteKind::Config, "{}").unwrap_err();

        assert!(
            format!("{error:#}").contains("the file itself"),
            "{error:#}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"keep\":1}");
    }

    #[test]
    fn test_a_file_patched_twice_in_one_run_is_backed_up_once() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("AGENTS.md");
        fs::write(&path, "original\n").unwrap();

        write_file(&path, WriteKind::Instructions, "first edit\n").unwrap();
        write_file(&path, WriteKind::Instructions, "second edit\n").unwrap();

        assert_eq!(
            fs::read_to_string(backup_path_for(&path)).unwrap(),
            "original\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_patch_instructions_writes_without_a_backup_when_the_directory_is_locked() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("shared");
        fs::create_dir(&dir).unwrap();
        let path = dir.join(".clinerules");
        fs::write(&path, "team rules\n").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        // Root ignores the directory's mode, and then the backup is simply taken.
        let locked = NamedTempFile::new_in(&dir).is_err();

        let result = write_file(&path, WriteKind::Instructions, "team rules\nrtk\n");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

        let backup = result.unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "team rules\nrtk\n");
        if locked {
            assert_eq!(backup, None);
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_backup_keeps_the_source_mode() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        for mode in [0o600, 0o644] {
            let path = temp.path().join(format!("settings-{mode:o}.json"));
            fs::write(&path, "{\"token\":1}").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();

            write_file(&path, WriteKind::Config, "{}").unwrap();

            let backup = fs::metadata(backup_path_for(&path)).unwrap();
            assert_eq!(backup.permissions().mode() & 0o777, mode);
        }
    }

    #[test]
    fn test_patch_file_refuses_a_read_only_file_before_the_backup() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        let backup = backup_path_for(&path);
        fs::write(&path, "current").unwrap();
        fs::write(&backup, "earlier").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions.clone()).unwrap();
        if !read_only_is_enforced(&path) {
            return;
        }

        let err = write_file(&path, WriteKind::Config, "new").unwrap_err();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&path, permissions).unwrap();

        assert!(format!("{err:#}").contains("read-only"), "{err:#}");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "earlier");
    }

    #[test]
    fn test_owned_write_refuses_a_read_only_file() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join(".clinerules");
        fs::write(&file_path, "mine").unwrap();
        let mut permissions = fs::metadata(&file_path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&file_path, permissions.clone()).unwrap();
        if !read_only_is_enforced(&file_path) {
            return;
        }

        let err = write_file(&file_path, WriteKind::Owned, "new").unwrap_err();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&file_path, permissions).unwrap();

        assert!(format!("{err:#}").contains("read-only"), "{err:#}");
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "mine");
    }

    #[cfg(unix)]
    #[test]
    fn test_owned_write_does_not_carry_setuid_over() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("tool");
        fs::write(&file_path, "old").unwrap();
        fs::set_permissions(&file_path, fs::Permissions::from_mode(0o4755)).unwrap();

        write_file(&file_path, WriteKind::Owned, "new").unwrap();

        let actual = fs::metadata(&file_path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(actual, 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn test_owned_write_new_file_gets_the_same_mode_as_fs_write() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let reference = temp.path().join("reference");
        let file_path = temp.path().join("written");
        fs::write(&reference, "x").unwrap();

        write_file(&file_path, WriteKind::Owned, "x").unwrap();

        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file_path), mode(&reference));
    }

    #[cfg(unix)]
    #[test]
    fn test_owned_write_preserves_symlink() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let target_path = temp.path().join("real-settings.json");
        let link_path = temp.path().join("settings.json");

        fs::write(&target_path, "{}").expect("seed target file");
        symlink(&target_path, &link_path).expect("create symlink");

        write_file(&link_path, WriteKind::Owned, "{\"hooks\":{}}").unwrap();

        let meta = fs::symlink_metadata(&link_path).unwrap();
        assert!(meta.file_type().is_symlink(), "symlink must survive");
        let written = fs::read_to_string(&target_path).unwrap();
        assert_eq!(written, "{\"hooks\":{}}");
    }

    #[cfg(unix)]
    #[test]
    fn test_owned_write_preserves_relative_symlink() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let subdir = temp.path().join("real");
        fs::create_dir(&subdir).unwrap();
        let target_path = subdir.join("settings.json");
        let link_path = temp.path().join("settings.json");

        fs::write(&target_path, "{}").expect("seed target file");
        symlink(Path::new("real/settings.json"), &link_path).expect("create relative symlink");

        write_file(&link_path, WriteKind::Owned, "{\"patched\":true}").unwrap();

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
    // Legacy migration tests

    // The next three tests exercise every host's hook registration side by side, which is why
    // the per-host helpers they call are pub(super) rather than private.
    #[test]
    fn test_legacy_migration_preserves_mixed_groups_and_prompt_hooks() {
        let legacy = "/home/user/hooks/rtk-rewrite.sh";
        let mut root = serde_json::json!({"hooks": {"PreToolUse": [
            {"hooks": [
                {"command": legacy},
                {"type": "prompt", "command": legacy},
                {"command": "echo user"},
                {"command": CLAUDE_HOOK_COMMAND}
            ]},
            {"hooks": []}
        ]}});
        assert!(remove_legacy_hook_entries_from_json(&mut root));
        assert!(!remove_legacy_hook_entries_from_json(&mut root));
        assert_eq!(
            root["hooks"]["PreToolUse"],
            serde_json::json!([
                {"hooks": [
                    {"type": "prompt", "command": legacy},
                    {"command": "echo user"},
                    {"command": CLAUDE_HOOK_COMMAND}
                ]},
                {"hooks": []}
            ])
        );
        let mut cursor = serde_json::json!({"hooks": {"preToolUse": [
            {"command": legacy},
            {"type": "prompt", "command": legacy},
            {"command": CURSOR_HOOK_COMMAND}
        ]}});
        assert!(remove_legacy_cursor_hook_entries_from_json(&mut cursor));
        assert!(!remove_legacy_cursor_hook_entries_from_json(&mut cursor));
        assert_eq!(
            cursor["hooks"]["preToolUse"],
            serde_json::json!([
                {"type": "prompt", "command": legacy},
                {"command": CURSOR_HOOK_COMMAND}
            ])
        );
    }

    #[test]
    fn test_hook_presence_respects_host_matcher_forms() {
        for matcher in [
            None,
            Some(""),
            Some("*"),
            Some("Bash"),
            Some("Read|Bash"),
            Some("^Ba"),
        ] {
            let mut root = serde_json::json!({"hooks": {"PreToolUse": [
                {"hooks": [{"command": CODEX_HOOK_COMMAND}]}
            ]}});
            if let Some(matcher) = matcher {
                root["hooks"]["PreToolUse"][0]["matcher"] = serde_json::json!(matcher);
            }
            assert!(codex_hook_already_present(&root), "{matcher:?}");
            root["hooks"]["PreToolUse"][0]["hooks"][0]["command"] =
                serde_json::json!(CLAUDE_HOOK_COMMAND);
            assert!(
                hook_already_present(&root, CLAUDE_HOOK_COMMAND),
                "{matcher:?}"
            );
        }
        for (matcher, expected) in [
            (serde_json::Value::Null, true),
            (serde_json::json!("Read, Bash"), true),
            (serde_json::json!("Ba"), false),
            (serde_json::json!("["), false),
        ] {
            let root = serde_json::json!({"hooks": {"PreToolUse": [
                {"matcher": matcher, "hooks": [{"command": CLAUDE_HOOK_COMMAND}]}
            ]}});
            assert_eq!(hook_already_present(&root, CLAUDE_HOOK_COMMAND), expected);
        }
        for matcher in [
            None,
            Some(""),
            Some("*"),
            Some("Shell"),
            Some("Read|Shell"),
            Some("^Sh"),
        ] {
            let mut root =
                serde_json::json!({"hooks": {"preToolUse": [{"command": CURSOR_HOOK_COMMAND}]}});
            if let Some(matcher) = matcher {
                root["hooks"]["preToolUse"][0]["matcher"] = serde_json::json!(matcher);
            }
            assert!(cursor_hook_already_present(&root), "{matcher:?}");
        }
    }

    #[test]
    fn test_grouped_registration_preserves_user_hooks_and_checks_matcher() {
        for (command, tool) in [
            (CLAUDE_HOOK_COMMAND, "Bash"),
            (CODEX_HOOK_COMMAND, "Bash"),
            (TRAE_HOOK_COMMAND, TRAE_RUN_COMMAND_MATCHER),
        ] {
            let present = |root: &serde_json::Value| match command {
                CLAUDE_HOOK_COMMAND => hook_already_present(root, command),
                CODEX_HOOK_COMMAND => codex_hook_already_present(root),
                _ => trae_hook_already_present(root),
            };
            let mut root = serde_json::json!({"hooks": {"PreToolUse": [
                {"matcher": "Read", "hooks": [{"type": "command", "command": command, "timeout": 99}]},
                {"matcher": tool, "hooks": [{"type": "prompt", "command": command}]},
                {"matcher": tool, "hooks": []}
            ], "Stop": [{"command": "echo stop"}]}, "user": true});
            assert!(
                !present(&root),
                "inactive and prompt hooks do not count: {command}"
            );
            match command {
                CLAUDE_HOOK_COMMAND | CODEX_HOOK_COMMAND => {
                    insert_hook_entry(&mut root, command).unwrap()
                }
                _ => insert_trae_hook_entry(&mut root).unwrap(),
            }
            assert!(present(&root));
            let groups = root["hooks"]["PreToolUse"].as_array_mut().unwrap();
            groups.last_mut().unwrap()["hooks"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"type": "command", "command": "echo user"}));
            let remove = |root: &mut serde_json::Value| match command {
                CLAUDE_HOOK_COMMAND => remove_hook_from_json(root),
                CODEX_HOOK_COMMAND => remove_codex_hook_from_json(root),
                _ => remove_trae_hook_from_json(root),
            };
            assert!(remove(&mut root));
            assert!(!remove(&mut root));
            assert!(!present(&root));
            assert_eq!(
                root["hooks"]["PreToolUse"],
                serde_json::json!([
                    {"matcher": tool, "hooks": [{"type": "prompt", "command": command}]},
                    {"matcher": tool, "hooks": []},
                    {"matcher": tool, "hooks": [{"type": "command", "command": "echo user"}]}
                ])
            );
            assert_eq!(
                root["hooks"]["Stop"],
                serde_json::json!([{"command": "echo stop"}])
            );
            assert_eq!(root["user"], true);
        }
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
    #[test]
    fn test_read_json_file_handles_missing_and_empty_files() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("missing.json");
        let empty = temp.path().join("empty.json");
        fs::write(&empty, "  \n").unwrap();

        assert!(read_json_file(&missing).unwrap().is_none());
        assert_eq!(read_json_file(&empty).unwrap(), Some(serde_json::json!({})));
    }

    #[test]
    fn test_read_json_file_errors_include_the_path() {
        let temp = TempDir::new().unwrap();
        let invalid = temp.path().join("invalid.json");
        fs::write(&invalid, "{").unwrap();

        let parse_error = read_json_file(&invalid).unwrap_err();
        assert!(format!("{parse_error:#}").contains(&invalid.display().to_string()));

        let read_error = read_json_file(temp.path()).unwrap_err();
        assert!(format!("{read_error:#}").contains(&temp.path().display().to_string()));
    }

    #[test]
    fn test_patch_file_preserves_previous_content() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("hooks.json");
        fs::write(&path, "old").unwrap();

        let backup = write_file(&path, WriteKind::Config, "new")
            .unwrap()
            .unwrap();

        assert_eq!(backup, path.with_extension("json.bak"));
        assert_eq!(fs::read_to_string(path).unwrap(), "new");
        assert_eq!(fs::read_to_string(backup).unwrap(), "old");
    }

    #[test]
    fn test_patch_file_failure_preserves_original() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("hooks.json");
        fs::write(&path, "old").unwrap();
        fs::create_dir(path.with_extension("json.bak")).unwrap();

        let err = write_file(&path, WriteKind::Config, "new").unwrap_err();

        assert!(format!("{err:#}").contains("backup"));
        assert_eq!(fs::read_to_string(path).unwrap(), "old");
    }

    #[test]
    fn test_cwd_guard_restores_after_a_panic() {
        let tmp = TempDir::new().expect("tmp");
        // Read under the lock: another cwd-mutating test holding it has the process sitting in
        // its own TempDir, and capturing that would assert against a directory this test never
        // entered.
        let before = {
            let _held = CwdGuard::enter(Path::new("."));
            std::env::current_dir().expect("cwd")
        };
        let panicked = std::panic::catch_unwind(|| {
            let _cwd = CwdGuard::enter(tmp.path());
            panic!("the call under test fails");
        });
        assert!(panicked.is_err(), "the panic must propagate");
        // Observed under the lock as well: the unwind dropped the closure's guard, so without
        // retaking it a concurrent cwd test sitting in its own TempDir is what gets read.
        let after = {
            let _held = CwdGuard::enter(Path::new("."));
            std::env::current_dir().expect("cwd")
        };
        assert_eq!(
            after, before,
            "a panic must not strand the process in the test directory"
        );
    }
}
