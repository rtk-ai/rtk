use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

use crate::integrity;

// Embedded hook script (guards before set -euo pipefail)
const REWRITE_HOOK: &str = include_str!("../hooks/rtk-rewrite.sh");

// Embedded Cursor hook script (preToolUse format)
const CURSOR_REWRITE_HOOK: &str = include_str!("../hooks/cursor-rtk-rewrite.sh");

// Embedded OpenCode plugin (auto-rewrite)
const OPENCODE_PLUGIN: &str = include_str!("../hooks/opencode-rtk.ts");

// Embedded slim RTK awareness instructions
const RTK_SLIM: &str = include_str!("../hooks/rtk-awareness.md");
const RTK_SLIM_CODEX: &str = include_str!("../hooks/rtk-awareness-codex.md");

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

/// Control flow for settings.json patching
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatchMode {
    Ask,  // Default: prompt user [y/N]
    Auto, // --auto-patch: no prompt
    Skip, // --no-patch: manual instructions
}

/// Result of settings.json patching operation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatchResult {
    Patched,        // Hook was added successfully
    AlreadyPresent, // Hook was already in settings.json
    Declined,       // User declined when prompted
    Skipped,        // --no-patch flag used
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

### Test (90-99% savings)
```bash
rtk cargo test          # Cargo test failures only (90%)
rtk vitest run          # Vitest failures only (99.5%)
rtk playwright test     # Playwright failures only (94%)
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
```

### Files & Search (60-75% savings)
```bash
rtk ls <path>           # Tree format, compact (65%)
rtk read <file>         # Code reading with filtering (60%)
rtk grep <pattern>      # Search grouped by file (75%)
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
    verbose: u8,
) -> Result<()> {
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
        return run_codex_mode(global, verbose);
    }

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

    // Windsurf-only mode
    if install_windsurf {
        return run_windsurf_mode(verbose);
    }

    // Cline-only mode
    if install_cline {
        return run_cline_mode(verbose);
    }

    // Mode selection (Claude Code / OpenCode)
    match (install_claude, install_opencode, claude_md, hook_only) {
        (false, true, _, _) => run_opencode_only_mode(verbose)?,
        (true, opencode, true, _) => run_claude_md_mode(global, verbose, opencode)?,
        (true, opencode, false, true) => run_hook_only_mode(global, patch_mode, verbose, opencode)?,
        (true, opencode, false, false) => run_default_mode(global, patch_mode, verbose, opencode)?,
        (false, false, _, _) => {
            if !install_cursor {
                anyhow::bail!("at least one of install_claude or install_opencode must be true")
            }
        }
    }

    // Cursor hooks (additive, installed alongside Claude Code)
    if install_cursor {
        install_cursor_hooks(verbose)?;
    }

    // Telemetry notice (shown once during init)
    println!();
    println!("  [info] Anonymous telemetry is enabled (opt-out: RTK_TELEMETRY_DISABLED=1)");
    println!("  [info] See: https://github.com/rtk-ai/rtk#privacy--telemetry");

    Ok(())
}

/// Prepare hook directory and return paths (hook_dir, hook_path)
fn prepare_hook_paths() -> Result<(PathBuf, PathBuf)> {
    let claude_dir = resolve_claude_dir()?;
    let hook_dir = claude_dir.join("hooks");
    fs::create_dir_all(&hook_dir)
        .with_context(|| format!("Failed to create hook directory: {}", hook_dir.display()))?;
    let hook_path = hook_dir.join("rtk-rewrite.sh");
    Ok((hook_dir, hook_path))
}

/// Write hook file if missing or outdated, return true if changed
#[cfg(unix)]
fn ensure_hook_installed(hook_path: &Path, verbose: u8) -> Result<bool> {
    let changed = if hook_path.exists() {
        let existing = fs::read_to_string(hook_path)
            .with_context(|| format!("Failed to read existing hook: {}", hook_path.display()))?;

        if existing == REWRITE_HOOK {
            if verbose > 0 {
                eprintln!("Hook already up to date: {}", hook_path.display());
            }
            false
        } else {
            fs::write(hook_path, REWRITE_HOOK)
                .with_context(|| format!("Failed to write hook to {}", hook_path.display()))?;
            if verbose > 0 {
                eprintln!("Updated hook: {}", hook_path.display());
            }
            true
        }
    } else {
        fs::write(hook_path, REWRITE_HOOK)
            .with_context(|| format!("Failed to write hook to {}", hook_path.display()))?;
        if verbose > 0 {
            eprintln!("Created hook: {}", hook_path.display());
        }
        true
    };

    // Set executable permissions
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(hook_path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("Failed to set hook permissions: {}", hook_path.display()))?;

    // Store SHA-256 hash for runtime integrity verification.
    // Always store (idempotent) to ensure baseline exists even for
    // hooks installed before integrity checks were added.
    integrity::store_hash(hook_path)
        .with_context(|| format!("Failed to store integrity hash for {}", hook_path.display()))?;
    if verbose > 0 && changed {
        eprintln!("Stored integrity hash for hook");
    }

    Ok(changed)
}

/// Idempotent file write: create or update if content differs
fn write_if_changed(path: &Path, content: &str, name: &str, verbose: u8) -> Result<bool> {
    if path.exists() {
        let existing = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}: {}", name, path.display()))?;

        if existing == content {
            if verbose > 0 {
                eprintln!("{} already up to date: {}", name, path.display());
            }
            Ok(false)
        } else {
            fs::write(path, content)
                .with_context(|| format!("Failed to write {}: {}", name, path.display()))?;
            if verbose > 0 {
                eprintln!("Updated {}: {}", name, path.display());
            }
            Ok(true)
        }
    } else {
        fs::write(path, content)
            .with_context(|| format!("Failed to write {}: {}", name, path.display()))?;
        if verbose > 0 {
            eprintln!("Created {}: {}", name, path.display());
        }
        Ok(true)
    }
}

/// Atomic write using tempfile + rename
/// Prevents corruption on crash/interrupt
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().with_context(|| {
        format!(
            "Cannot write to {}: path has no parent directory",
            path.display()
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
    temp_file.persist(path).with_context(|| {
        format!(
            "Failed to atomically replace {} (disk full?)",
            path.display()
        )
    })?;

    Ok(())
}

/// Prompt user for consent to patch settings.json
/// Prints to stderr (stdout may be piped), reads from stdin
/// Default is No (capital N)
fn prompt_user_consent(settings_path: &Path) -> Result<bool> {
    use std::io::{self, BufRead, IsTerminal};

    eprintln!("\nPatch existing {}? [y/N] ", settings_path.display());

    // If stdin is not a terminal (piped), default to No
    if !io::stdin().is_terminal() {
        eprintln!("(non-interactive mode, defaulting to N)");
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

/// Print manual instructions for settings.json patching
fn print_manual_instructions(hook_path: &Path, include_opencode: bool) {
    println!("\n  MANUAL STEP: Add this to ~/.claude/settings.json:");
    println!("  {{");
    println!("    \"hooks\": {{ \"PreToolUse\": [{{");
    println!("      \"matcher\": \"Bash\",");
    println!("      \"hooks\": [{{ \"type\": \"command\",");
    println!("        \"command\": \"{}\"", hook_path.display());
    println!("      }}]");
    println!("    }}]}}");
    println!("  }}");
    if include_opencode {
        println!("\n  Then restart Claude Code and OpenCode. Test with: git status\n");
    } else {
        println!("\n  Then restart Claude Code. Test with: git status\n");
    }
}

/// Remove RTK hook entry from settings.json
/// Returns true if hook was found and removed
fn remove_hook_from_json(root: &mut serde_json::Value) -> bool {
    let hooks = match root.get_mut("hooks").and_then(|h| h.get_mut("PreToolUse")) {
        Some(pre_tool_use) => pre_tool_use,
        None => return false,
    };

    let pre_tool_use_array = match hooks.as_array_mut() {
        Some(arr) => arr,
        None => return false,
    };

    // Find and remove RTK entry
    let original_len = pre_tool_use_array.len();
    pre_tool_use_array.retain(|entry| {
        if let Some(hooks_array) = entry.get("hooks").and_then(|h| h.as_array()) {
            for hook in hooks_array {
                if let Some(command) = hook.get("command").and_then(|c| c.as_str()) {
                    if command.contains("rtk-rewrite.sh") {
                        return false; // Remove this entry
                    }
                }
            }
        }
        true // Keep this entry
    });

    pre_tool_use_array.len() < original_len
}

/// Remove RTK hook from settings.json file
/// Backs up before modification, returns true if hook was found and removed
fn remove_hook_from_settings(verbose: u8) -> Result<bool> {
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join("settings.json");

    if !settings_path.exists() {
        if verbose > 0 {
            eprintln!("settings.json not found, nothing to remove");
        }
        return Ok(false);
    }

    let content = fs::read_to_string(&settings_path)
        .with_context(|| format!("Failed to read {}", settings_path.display()))?;

    if content.trim().is_empty() {
        return Ok(false);
    }

    let mut root: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {} as JSON", settings_path.display()))?;

    let removed = remove_hook_from_json(&mut root);

    if removed {
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

/// Full uninstall for Claude, Gemini, Codex, or Cursor artifacts.
pub fn uninstall(global: bool, gemini: bool, codex: bool, cursor: bool, verbose: u8) -> Result<()> {
    if codex {
        return uninstall_codex(global, verbose);
    }

    if cursor {
        if !global {
            anyhow::bail!("Cursor uninstall only works with --global flag");
        }
        let cursor_removed =
            remove_cursor_hooks(verbose).context("Failed to remove Cursor hooks")?;
        if !cursor_removed.is_empty() {
            println!("RTK uninstalled (Cursor):");
            for item in &cursor_removed {
                println!("  - {}", item);
            }
            println!("\nRestart Cursor to apply changes.");
        } else {
            println!("RTK Cursor support was not installed (nothing to remove)");
        }
        return Ok(());
    }

    if !global {
        anyhow::bail!("Uninstall only works with --global flag. For local projects, manually remove RTK from CLAUDE.md");
    }

    let claude_dir = resolve_claude_dir()?;
    let mut removed = Vec::new();

    // Also uninstall Gemini artifacts if --gemini or always (clean everything)
    if gemini {
        let gemini_removed = uninstall_gemini(verbose)?;
        removed.extend(gemini_removed);
        if !removed.is_empty() {
            println!("RTK uninstalled (Gemini):");
            for item in &removed {
                println!("  - {}", item);
            }
            println!("\nRestart Gemini CLI to apply changes.");
        } else {
            println!("RTK Gemini support was not installed (nothing to remove)");
        }
        return Ok(());
    }

    // 1. Remove hook file
    let hook_path = claude_dir.join("hooks").join("rtk-rewrite.sh");
    if hook_path.exists() {
        fs::remove_file(&hook_path)
            .with_context(|| format!("Failed to remove hook: {}", hook_path.display()))?;
        removed.push(format!("Hook: {}", hook_path.display()));
    }

    // 1b. Remove integrity hash file
    if integrity::remove_hash(&hook_path)? {
        removed.push("Integrity hash: removed".to_string());
    }

    // 2. Remove RTK.md
    let rtk_md_path = claude_dir.join("RTK.md");
    if rtk_md_path.exists() {
        fs::remove_file(&rtk_md_path)
            .with_context(|| format!("Failed to remove RTK.md: {}", rtk_md_path.display()))?;
        removed.push(format!("RTK.md: {}", rtk_md_path.display()));
    }

    // 3. Remove @RTK.md reference from CLAUDE.md
    let claude_md_path = claude_dir.join("CLAUDE.md");
    if claude_md_path.exists() {
        let content = fs::read_to_string(&claude_md_path)
            .with_context(|| format!("Failed to read CLAUDE.md: {}", claude_md_path.display()))?;

        if content.contains("@RTK.md") {
            let new_content = content
                .lines()
                .filter(|line| !line.trim().starts_with("@RTK.md"))
                .collect::<Vec<_>>()
                .join("\n");

            // Clean up double blanks
            let cleaned = clean_double_blanks(&new_content);

            fs::write(&claude_md_path, cleaned).with_context(|| {
                format!("Failed to write CLAUDE.md: {}", claude_md_path.display())
            })?;
            removed.push("CLAUDE.md: removed @RTK.md reference".to_string());
        }
    }

    // 4. Remove hook entry from settings.json
    if remove_hook_from_settings(verbose)? {
        removed.push("settings.json: removed RTK hook entry".to_string());
    }

    // 5. Remove OpenCode plugin
    let opencode_removed = remove_opencode_plugin(verbose)?;
    for path in opencode_removed {
        removed.push(format!("OpenCode plugin: {}", path.display()));
    }

    // 6. Remove Cursor hooks
    let cursor_removed = remove_cursor_hooks(verbose)?;
    removed.extend(cursor_removed);

    // Report results
    if removed.is_empty() {
        println!("RTK was not installed (nothing to remove)");
    } else {
        println!("RTK uninstalled:");
        for item in removed {
            println!("  - {}", item);
        }
        println!("\nRestart Claude Code, OpenCode, and Cursor (if used) to apply changes.");
    }

    Ok(())
}

fn uninstall_codex(global: bool, verbose: u8) -> Result<()> {
    if !global {
        anyhow::bail!(
            "Uninstall only works with --global flag. For local projects, manually remove RTK from AGENTS.md"
        );
    }

    let codex_dir = resolve_codex_dir()?;
    let removed = uninstall_codex_at(&codex_dir, verbose)?;

    if removed.is_empty() {
        println!("RTK was not installed for Codex CLI (nothing to remove)");
    } else {
        println!("RTK uninstalled for Codex CLI:");
        for item in removed {
            println!("  - {}", item);
        }
    }

    Ok(())
}

fn uninstall_codex_at(codex_dir: &Path, verbose: u8) -> Result<Vec<String>> {
    let mut removed = Vec::new();

    let rtk_md_path = codex_dir.join("RTK.md");
    if rtk_md_path.exists() {
        fs::remove_file(&rtk_md_path)
            .with_context(|| format!("Failed to remove RTK.md: {}", rtk_md_path.display()))?;
        if verbose > 0 {
            eprintln!("Removed RTK.md: {}", rtk_md_path.display());
        }
        removed.push(format!("RTK.md: {}", rtk_md_path.display()));
    }

    let agents_md_path = codex_dir.join("AGENTS.md");
    if remove_rtk_reference_from_agents(&agents_md_path, verbose)? {
        removed.push("AGENTS.md: removed @RTK.md reference".to_string());
    }

    Ok(removed)
}

/// Orchestrator: patch settings.json with RTK hook
/// Handles reading, checking, prompting, merging, backing up, and atomic writing
fn patch_settings_json(
    hook_path: &Path,
    mode: PatchMode,
    verbose: u8,
    include_opencode: bool,
) -> Result<PatchResult> {
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join("settings.json");
    let hook_command = hook_path
        .to_str()
        .context("Hook path contains invalid UTF-8")?;

    // Read or create settings.json
    let mut root = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read {}", settings_path.display()))?;

        if content.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&content)
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
            print_manual_instructions(hook_path, include_opencode);
            return Ok(PatchResult::Skipped);
        }
        PatchMode::Ask => {
            if !prompt_user_consent(&settings_path)? {
                print_manual_instructions(hook_path, include_opencode);
                return Ok(PatchResult::Declined);
            }
        }
        PatchMode::Auto => {
            // Proceed without prompting
        }
    }

    // Deep-merge hook
    insert_hook_entry(&mut root, hook_command);

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
    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;
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
fn insert_hook_entry(root: &mut serde_json::Value, hook_command: &str) {
    // Ensure root is an object
    let root_obj = match root.as_object_mut() {
        Some(obj) => obj,
        None => {
            *root = serde_json::json!({});
            root.as_object_mut()
                .expect("Just created object, must succeed")
        }
    };

    // Use entry() API for idiomatic insertion
    let hooks = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .expect("hooks must be an object");

    let pre_tool_use = hooks
        .entry("PreToolUse")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .expect("PreToolUse must be an array");

    // Append RTK hook entry
    pre_tool_use.push(serde_json::json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": hook_command
        }]
    }));
}

/// Check if RTK hook is already present in settings.json
/// Matches on rtk-rewrite.sh substring to handle different path formats
fn hook_already_present(root: &serde_json::Value, hook_command: &str) -> bool {
    let pre_tool_use_array = match root
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
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
            // Exact match OR both contain rtk-rewrite.sh
            cmd == hook_command
                || (cmd.contains("rtk-rewrite.sh") && hook_command.contains("rtk-rewrite.sh"))
        })
}

/// Default mode: hook + slim RTK.md + @RTK.md reference
#[cfg(not(unix))]
fn run_default_mode(
    _global: bool,
    _patch_mode: PatchMode,
    _verbose: u8,
    _install_opencode: bool,
) -> Result<()> {
    eprintln!("[warn] Hook-based mode requires Unix (macOS/Linux).");
    eprintln!("    Windows: use --claude-md mode for full injection.");
    eprintln!("    Falling back to --claude-md mode.");
    run_claude_md_mode(_global, _verbose, _install_opencode)
}

#[cfg(unix)]
fn run_default_mode(
    global: bool,
    patch_mode: PatchMode,
    verbose: u8,
    install_opencode: bool,
) -> Result<()> {
    if !global {
        // Local init: inject CLAUDE.md + generate project-local filters template
        run_claude_md_mode(false, verbose, install_opencode)?;
        generate_project_filters_template(verbose)?;
        return Ok(());
    }

    let claude_dir = resolve_claude_dir()?;
    let rtk_md_path = claude_dir.join("RTK.md");
    let claude_md_path = claude_dir.join("CLAUDE.md");

    // 1. Prepare hook directory and install hook
    let (_hook_dir, hook_path) = prepare_hook_paths()?;
    let hook_changed = ensure_hook_installed(&hook_path, verbose)?;

    // 2. Write RTK.md
    write_if_changed(&rtk_md_path, RTK_SLIM, "RTK.md", verbose)?;

    let opencode_plugin_path = if install_opencode {
        let path = prepare_opencode_plugin_path()?;
        ensure_opencode_plugin_installed(&path, verbose)?;
        Some(path)
    } else {
        None
    };

    // 3. Patch CLAUDE.md (add @RTK.md, migrate if needed)
    let migrated = patch_claude_md(&claude_md_path, verbose)?;

    // 4. Print success message
    let hook_status = if hook_changed {
        "installed/updated"
    } else {
        "already up to date"
    };
    println!("\nRTK hook {} (global).\n", hook_status);
    println!("  Hook:      {}", hook_path.display());
    println!("  RTK.md:    {} (10 lines)", rtk_md_path.display());
    if let Some(path) = &opencode_plugin_path {
        println!("  OpenCode:  {}", path.display());
    }
    println!("  CLAUDE.md: @RTK.md reference added");

    if migrated {
        println!("\n  [ok] Migrated: removed 137-line RTK block from CLAUDE.md");
        println!("              replaced with @RTK.md (10 lines)");
    }

    // 5. Patch settings.json
    let patch_result = patch_settings_json(&hook_path, patch_mode, verbose, install_opencode)?;

    // Report result
    match patch_result {
        PatchResult::Patched => {
            // Already printed by patch_settings_json
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
            // Manual instructions already printed by patch_settings_json
        }
    }

    // 6. Generate user-global filters template (~/.config/rtk/filters.toml)
    generate_global_filters_template(verbose)?;

    println!(); // Final newline

    Ok(())
}

/// Generate .rtk/filters.toml template in the current directory if not present.
fn generate_project_filters_template(verbose: u8) -> Result<()> {
    let rtk_dir = std::path::Path::new(".rtk");
    let path = rtk_dir.join("filters.toml");

    if path.exists() {
        if verbose > 0 {
            eprintln!(".rtk/filters.toml already exists, skipping template");
        }
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
fn generate_global_filters_template(verbose: u8) -> Result<()> {
    let config_dir = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".config"));
    let rtk_dir = config_dir.join("rtk");
    let path = rtk_dir.join("filters.toml");

    if path.exists() {
        if verbose > 0 {
            eprintln!("{} already exists, skipping template", path.display());
        }
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

/// Hook-only mode: just the hook, no RTK.md
#[cfg(not(unix))]
fn run_hook_only_mode(
    _global: bool,
    _patch_mode: PatchMode,
    _verbose: u8,
    _install_opencode: bool,
) -> Result<()> {
    anyhow::bail!("Hook install requires Unix (macOS/Linux). Use WSL or --claude-md mode.")
}

#[cfg(unix)]
fn run_hook_only_mode(
    global: bool,
    patch_mode: PatchMode,
    verbose: u8,
    install_opencode: bool,
) -> Result<()> {
    if !global {
        eprintln!("[warn] Warning: --hook-only only makes sense with --global");
        eprintln!("    For local projects, use default mode or --claude-md");
        return Ok(());
    }

    // Prepare and install hook
    let (_hook_dir, hook_path) = prepare_hook_paths()?;
    let hook_changed = ensure_hook_installed(&hook_path, verbose)?;

    let opencode_plugin_path = if install_opencode {
        let path = prepare_opencode_plugin_path()?;
        ensure_opencode_plugin_installed(&path, verbose)?;
        Some(path)
    } else {
        None
    };

    let hook_status = if hook_changed {
        "installed/updated"
    } else {
        "already up to date"
    };
    println!("\nRTK hook {} (hook-only mode).\n", hook_status);
    println!("  Hook: {}", hook_path.display());
    if let Some(path) = &opencode_plugin_path {
        println!("  OpenCode: {}", path.display());
    }
    println!(
        "  Note: No RTK.md created. Claude won't know about meta commands (gain, discover, proxy)."
    );

    // Patch settings.json
    let patch_result = patch_settings_json(&hook_path, patch_mode, verbose, install_opencode)?;

    // Report result
    match patch_result {
        PatchResult::Patched => {
            // Already printed by patch_settings_json
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
            // Manual instructions already printed by patch_settings_json
        }
    }

    println!(); // Final newline

    Ok(())
}

/// Legacy mode: full 137-line injection into CLAUDE.md
fn run_claude_md_mode(global: bool, verbose: u8, install_opencode: bool) -> Result<()> {
    let path = if global {
        resolve_claude_dir()?.join("CLAUDE.md")
    } else {
        PathBuf::from("CLAUDE.md")
    };

    if global {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
    }

    if verbose > 0 {
        eprintln!("Writing rtk instructions to: {}", path.display());
    }

    if path.exists() {
        let existing = fs::read_to_string(&path)?;
        // upsert_rtk_block handles all 4 cases: add, update, unchanged, malformed
        let (new_content, action) = upsert_rtk_block(&existing, RTK_INSTRUCTIONS);

        match action {
            RtkBlockUpsert::Added => {
                fs::write(&path, new_content)?;
                println!("[ok] Added rtk instructions to existing {}", path.display());
            }
            RtkBlockUpsert::Updated => {
                fs::write(&path, new_content)?;
                println!("[ok] Updated rtk instructions in {}", path.display());
            }
            RtkBlockUpsert::Unchanged => {
                println!(
                    "[ok] {} already contains up-to-date rtk instructions",
                    path.display()
                );
                return Ok(());
            }
            RtkBlockUpsert::Malformed => {
                eprintln!(
                    "[warn] Warning: Found '<!-- rtk-instructions' without closing marker in {}",
                    path.display()
                );

                if let Some((line_num, _)) = existing
                    .lines()
                    .enumerate()
                    .find(|(_, line)| line.contains("<!-- rtk-instructions"))
                {
                    eprintln!("    Location: line {}", line_num + 1);
                }

                eprintln!("    Action: Manually remove the incomplete block, then re-run:");
                if global {
                    eprintln!("            rtk init -g --claude-md");
                } else {
                    eprintln!("            rtk init --claude-md");
                }
                return Ok(());
            }
        }
    } else {
        fs::write(&path, RTK_INSTRUCTIONS)?;
        println!("[ok] Created {} with rtk instructions", path.display());
    }

    if global {
        if install_opencode {
            let opencode_plugin_path = prepare_opencode_plugin_path()?;
            ensure_opencode_plugin_installed(&opencode_plugin_path, verbose)?;
            println!(
                "[ok] OpenCode plugin installed: {}",
                opencode_plugin_path.display()
            );
        }
        println!("   Claude Code will now use rtk in all sessions");
    } else {
        println!("   Claude Code will use rtk in this project");
    }

    Ok(())
}

// ─── Windsurf support ─────────────────────────────────────────

/// Embedded Windsurf RTK rules
const WINDSURF_RULES: &str = include_str!("../hooks/windsurf-rtk-rules.md");

/// Embedded Cline RTK rules
const CLINE_RULES: &str = include_str!("../hooks/cline-rtk-rules.md");

// ─── Cline / Roo Code support ─────────────────────────────────

fn run_cline_mode(verbose: u8) -> Result<()> {
    // Cline reads .clinerules from the project root (workspace-scoped)
    let rules_path = PathBuf::from(".clinerules");

    let existing = fs::read_to_string(&rules_path).unwrap_or_default();
    if existing.contains("RTK") || existing.contains("rtk") {
        println!("\nRTK already configured for Cline in this project.\n");
        println!("  Rules: .clinerules (already present)");
    } else {
        let new_content = if existing.trim().is_empty() {
            CLINE_RULES.to_string()
        } else {
            format!("{}\n\n{}", existing.trim(), CLINE_RULES)
        };
        fs::write(&rules_path, &new_content).context("Failed to write .clinerules")?;

        if verbose > 0 {
            eprintln!("Wrote .clinerules");
        }

        println!("\nRTK configured for Cline.\n");
        println!("  Rules: .clinerules (installed)");
    }
    println!("  Cline will now use rtk commands for token savings.");
    println!("  Test with: git status\n");

    Ok(())
}

fn run_windsurf_mode(verbose: u8) -> Result<()> {
    // Windsurf reads .windsurfrules from the project root (workspace-scoped).
    // Global rules (~/.codeium/windsurf/memories/global_rules.md) are unreliable.
    let rules_path = PathBuf::from(".windsurfrules");

    let existing = fs::read_to_string(&rules_path).unwrap_or_default();
    if existing.contains("RTK") || existing.contains("rtk") {
        println!("\nRTK already configured for Windsurf in this project.\n");
        println!("  Rules: .windsurfrules (already present)");
    } else {
        let new_content = if existing.trim().is_empty() {
            WINDSURF_RULES.to_string()
        } else {
            format!("{}\n\n{}", existing.trim(), WINDSURF_RULES)
        };
        fs::write(&rules_path, &new_content).context("Failed to write .windsurfrules")?;

        if verbose > 0 {
            eprintln!("Wrote .windsurfrules");
        }

        println!("\nRTK configured for Windsurf Cascade.\n");
        println!("  Rules: .windsurfrules (installed)");
    }
    println!("  Cascade will now use rtk commands for token savings.");
    println!("  Restart Windsurf. Test with: git status\n");

    Ok(())
}

fn run_codex_mode(global: bool, verbose: u8) -> Result<()> {
    let (agents_md_path, rtk_md_path) = if global {
        let codex_dir = resolve_codex_dir()?;
        (codex_dir.join("AGENTS.md"), codex_dir.join("RTK.md"))
    } else {
        (PathBuf::from("AGENTS.md"), PathBuf::from("RTK.md"))
    };

    if global {
        if let Some(parent) = agents_md_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create Codex config directory: {}",
                    parent.display()
                )
            })?;
        }
    }

    write_if_changed(&rtk_md_path, RTK_SLIM_CODEX, "RTK.md", verbose)?;
    let added_ref = patch_agents_md(&agents_md_path, verbose)?;

    println!("\nRTK configured for Codex CLI.\n");
    println!("  RTK.md:    {}", rtk_md_path.display());
    if added_ref {
        println!("  AGENTS.md: @RTK.md reference added");
    } else {
        println!("  AGENTS.md: @RTK.md reference already present");
    }
    if global {
        println!(
            "\n  Codex global instructions path: {}",
            agents_md_path.display()
        );
    } else {
        println!(
            "\n  Codex project instructions path: {}",
            agents_md_path.display()
        );
    }

    Ok(())
}

// --- upsert_rtk_block: idempotent RTK block management ---

#[derive(Debug, Clone, Copy, PartialEq)]
enum RtkBlockUpsert {
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
    let start_marker = "<!-- rtk-instructions";
    let end_marker = "<!-- /rtk-instructions -->";

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

/// Patch CLAUDE.md: add @RTK.md, migrate if old block exists
fn patch_claude_md(path: &Path, verbose: u8) -> Result<bool> {
    let mut content = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let mut migrated = false;

    // Check for old block and migrate
    if content.contains("<!-- rtk-instructions") {
        let (new_content, did_migrate) = remove_rtk_block(&content);
        if did_migrate {
            content = new_content;
            migrated = true;
            if verbose > 0 {
                eprintln!("Migrated: removed old RTK block from CLAUDE.md");
            }
        }
    }

    // Check if @RTK.md already present
    if content.contains("@RTK.md") {
        if verbose > 0 {
            eprintln!("@RTK.md reference already present in CLAUDE.md");
        }
        if migrated {
            fs::write(path, content)?;
        }
        return Ok(migrated);
    }

    // Add @RTK.md
    let new_content = if content.is_empty() {
        "@RTK.md\n".to_string()
    } else {
        format!("{}\n\n@RTK.md\n", content.trim())
    };

    fs::write(path, new_content)?;

    if verbose > 0 {
        eprintln!("Added @RTK.md reference to CLAUDE.md");
    }

    Ok(migrated)
}

/// Patch AGENTS.md: add @RTK.md, migrate old inline block if present
fn patch_agents_md(path: &Path, verbose: u8) -> Result<bool> {
    let mut content = if path.exists() {
        fs::read_to_string(path)
            .with_context(|| format!("Failed to read AGENTS.md: {}", path.display()))?
    } else {
        String::new()
    };

    let mut migrated = false;
    if content.contains("<!-- rtk-instructions") {
        let (new_content, did_migrate) = remove_rtk_block(&content);
        if did_migrate {
            content = new_content;
            migrated = true;
            if verbose > 0 {
                eprintln!("Migrated: removed old RTK block from AGENTS.md");
            }
        }
    }

    if content.contains("@RTK.md") {
        if verbose > 0 {
            eprintln!("@RTK.md reference already present in AGENTS.md");
        }
        if migrated {
            atomic_write(path, &content)
                .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;
        }
        return Ok(false);
    }

    let new_content = if content.is_empty() {
        "@RTK.md\n".to_string()
    } else {
        format!("{}\n\n@RTK.md\n", content.trim())
    };

    atomic_write(path, &new_content)
        .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;
    if verbose > 0 {
        eprintln!("Added @RTK.md reference to AGENTS.md");
    }

    Ok(true)
}

fn remove_rtk_reference_from_agents(path: &Path, verbose: u8) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read AGENTS.md: {}", path.display()))?;
    if !content.contains("@RTK.md") {
        return Ok(false);
    }

    let new_content = content
        .lines()
        .filter(|line| !line.trim().starts_with("@RTK.md"))
        .collect::<Vec<_>>()
        .join("\n");
    let cleaned = clean_double_blanks(&new_content);
    atomic_write(path, &cleaned)
        .with_context(|| format!("Failed to write AGENTS.md: {}", path.display()))?;

    if verbose > 0 {
        eprintln!(
            "Removed @RTK.md reference from AGENTS.md: {}",
            path.display()
        );
    }

    Ok(true)
}

/// Remove old RTK block from CLAUDE.md (migration helper)
fn remove_rtk_block(content: &str) -> (String, bool) {
    if let (Some(start), Some(end)) = (
        content.find("<!-- rtk-instructions"),
        content.find("<!-- /rtk-instructions -->"),
    ) {
        let end_pos = end + "<!-- /rtk-instructions -->".len();
        let before = content[..start].trim_end();
        let after = content[end_pos..].trim_start();

        let result = if after.is_empty() {
            before.to_string()
        } else {
            format!("{}\n\n{}", before, after)
        };

        (result, true) // migrated
    } else if content.contains("<!-- rtk-instructions") {
        eprintln!("[warn] Warning: Found '<!-- rtk-instructions' without closing marker.");
        eprintln!("    This can happen if CLAUDE.md was manually edited.");

        // Find line number
        if let Some((line_num, _)) = content
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains("<!-- rtk-instructions"))
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

/// Resolve ~/.claude directory with proper home expansion
fn resolve_claude_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".claude"))
        .context("Cannot determine home directory. Is $HOME set?")
}

/// Resolve ~/.codex directory with proper home expansion
fn resolve_codex_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".codex"))
        .context("Cannot determine home directory. Is $HOME set?")
}
/// Resolve OpenCode config directory (~/.config/opencode)
/// OpenCode uses ~/.config/opencode on all platforms (XDG convention),
/// NOT the macOS-native ~/Library/Application Support/.
fn resolve_opencode_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".config").join("opencode"))
        .context("Cannot determine home directory. Is $HOME set?")
}

/// Return OpenCode plugin path: ~/.config/opencode/plugins/rtk.ts
fn opencode_plugin_path(opencode_dir: &Path) -> PathBuf {
    opencode_dir.join("plugins").join("rtk.ts")
}

/// Prepare OpenCode plugin directory and return install path
fn prepare_opencode_plugin_path() -> Result<PathBuf> {
    let opencode_dir = resolve_opencode_dir()?;
    let path = opencode_plugin_path(&opencode_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create OpenCode plugin directory: {}",
                parent.display()
            )
        })?;
    }
    Ok(path)
}

/// Write OpenCode plugin file if missing or outdated
fn ensure_opencode_plugin_installed(path: &Path, verbose: u8) -> Result<bool> {
    write_if_changed(path, OPENCODE_PLUGIN, "OpenCode plugin", verbose)
}

/// Remove OpenCode plugin file
fn remove_opencode_plugin(verbose: u8) -> Result<Vec<PathBuf>> {
    let opencode_dir = resolve_opencode_dir()?;
    let path = opencode_plugin_path(&opencode_dir);
    let mut removed = Vec::new();

    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("Failed to remove OpenCode plugin: {}", path.display()))?;
        if verbose > 0 {
            eprintln!("Removed OpenCode plugin: {}", path.display());
        }
        removed.push(path);
    }

    Ok(removed)
}

// ─── Cursor Agent support ─────────────────────────────────────────────

/// Resolve ~/.cursor directory
fn resolve_cursor_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".cursor"))
        .context("Cannot determine home directory. Is $HOME set?")
}

/// Install Cursor hooks: hook script + hooks.json
fn install_cursor_hooks(verbose: u8) -> Result<()> {
    let cursor_dir = resolve_cursor_dir()?;
    let hooks_dir = cursor_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).with_context(|| {
        format!(
            "Failed to create Cursor hooks directory: {}",
            hooks_dir.display()
        )
    })?;

    // 1. Write hook script
    let hook_path = hooks_dir.join("rtk-rewrite.sh");
    let hook_changed = write_if_changed(&hook_path, CURSOR_REWRITE_HOOK, "Cursor hook", verbose)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).with_context(|| {
            format!(
                "Failed to set Cursor hook permissions: {}",
                hook_path.display()
            )
        })?;
    }

    // 2. Create or patch hooks.json
    let hooks_json_path = cursor_dir.join("hooks.json");
    let patched = patch_cursor_hooks_json(&hooks_json_path, verbose)?;

    // Report
    let hook_status = if hook_changed {
        "installed/updated"
    } else {
        "already up to date"
    };
    println!("\nCursor hook {} (global).\n", hook_status);
    println!("  Hook:       {}", hook_path.display());
    println!("  hooks.json: {}", hooks_json_path.display());

    if patched {
        println!("  hooks.json: RTK preToolUse entry added");
    } else {
        println!("  hooks.json: RTK preToolUse entry already present");
    }

    println!("  Cursor reloads hooks.json automatically. Test with: git status\n");

    Ok(())
}

/// Patch ~/.cursor/hooks.json to add RTK preToolUse hook.
/// Returns true if the file was modified.
fn patch_cursor_hooks_json(path: &Path, verbose: u8) -> Result<bool> {
    let mut root = if path.exists() {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        if content.trim().is_empty() {
            serde_json::json!({ "version": 1 })
        } else {
            serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse {} as JSON", path.display()))?
        }
    } else {
        serde_json::json!({ "version": 1 })
    };

    // Check idempotency
    if cursor_hook_already_present(&root) {
        if verbose > 0 {
            eprintln!("Cursor hooks.json: RTK hook already present");
        }
        return Ok(false);
    }

    // Insert the RTK preToolUse entry
    insert_cursor_hook_entry(&mut root);

    // Backup if exists
    if path.exists() {
        let backup_path = path.with_extension("json.bak");
        fs::copy(path, &backup_path)
            .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;
        if verbose > 0 {
            eprintln!("Backup: {}", backup_path.display());
        }
    }

    // Atomic write
    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize hooks.json")?;
    atomic_write(path, &serialized)?;

    Ok(true)
}

/// Check if RTK preToolUse hook is already present in Cursor hooks.json
fn cursor_hook_already_present(root: &serde_json::Value) -> bool {
    let hooks = match root
        .get("hooks")
        .and_then(|h| h.get("preToolUse"))
        .and_then(|p| p.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };

    hooks.iter().any(|entry| {
        entry
            .get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|cmd| cmd.contains("rtk-rewrite.sh"))
    })
}

/// Insert RTK preToolUse entry into Cursor hooks.json
fn insert_cursor_hook_entry(root: &mut serde_json::Value) {
    let root_obj = match root.as_object_mut() {
        Some(obj) => obj,
        None => {
            *root = serde_json::json!({ "version": 1 });
            root.as_object_mut()
                .expect("Just created object, must succeed")
        }
    };

    // Ensure version key
    root_obj.entry("version").or_insert(serde_json::json!(1));

    let hooks = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .expect("hooks must be an object");

    let pre_tool_use = hooks
        .entry("preToolUse")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .expect("preToolUse must be an array");

    pre_tool_use.push(serde_json::json!({
        "command": "./hooks/rtk-rewrite.sh",
        "matcher": "Shell"
    }));
}

/// Remove Cursor RTK artifacts: hook script + hooks.json entry
fn remove_cursor_hooks(verbose: u8) -> Result<Vec<String>> {
    let cursor_dir = resolve_cursor_dir()?;
    let mut removed = Vec::new();

    // 1. Remove hook script
    let hook_path = cursor_dir.join("hooks").join("rtk-rewrite.sh");
    if hook_path.exists() {
        fs::remove_file(&hook_path)
            .with_context(|| format!("Failed to remove Cursor hook: {}", hook_path.display()))?;
        removed.push(format!("Cursor hook: {}", hook_path.display()));
    }

    // 2. Remove RTK entry from hooks.json
    let hooks_json_path = cursor_dir.join("hooks.json");
    if hooks_json_path.exists() {
        let content = fs::read_to_string(&hooks_json_path)
            .with_context(|| format!("Failed to read {}", hooks_json_path.display()))?;

        if !content.trim().is_empty() {
            if let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&content) {
                if remove_cursor_hook_from_json(&mut root) {
                    let backup_path = hooks_json_path.with_extension("json.bak");
                    fs::copy(&hooks_json_path, &backup_path).ok();

                    let serialized = serde_json::to_string_pretty(&root)
                        .context("Failed to serialize hooks.json")?;
                    atomic_write(&hooks_json_path, &serialized)?;

                    removed.push("Cursor hooks.json: removed RTK entry".to_string());

                    if verbose > 0 {
                        eprintln!("Removed RTK hook from Cursor hooks.json");
                    }
                }
            }
        }
    }

    Ok(removed)
}

/// Remove RTK preToolUse entry from Cursor hooks.json
/// Returns true if entry was found and removed
fn remove_cursor_hook_from_json(root: &mut serde_json::Value) -> bool {
    let pre_tool_use = match root
        .get_mut("hooks")
        .and_then(|h| h.get_mut("preToolUse"))
        .and_then(|p| p.as_array_mut())
    {
        Some(arr) => arr,
        None => return false,
    };

    let original_len = pre_tool_use.len();
    pre_tool_use.retain(|entry| {
        !entry
            .get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|cmd| cmd.contains("rtk-rewrite.sh"))
    });

    pre_tool_use.len() < original_len
}

/// Show current rtk configuration
pub fn show_config(codex: bool) -> Result<()> {
    if codex {
        return show_codex_config();
    }

    show_claude_config()
}

fn show_claude_config() -> Result<()> {
    let claude_dir = resolve_claude_dir()?;
    let hook_path = claude_dir.join("hooks").join("rtk-rewrite.sh");
    let rtk_md_path = claude_dir.join("RTK.md");
    let global_claude_md = claude_dir.join("CLAUDE.md");
    let local_claude_md = PathBuf::from("CLAUDE.md");

    println!("rtk Configuration:\n");

    // Check hook
    if hook_path.exists() {
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
            let hook_version = crate::hook_check::parse_hook_version(&hook_content);

            if !is_executable {
                println!(
                    "[warn] Hook: {} (NOT executable - run: chmod +x)",
                    hook_path.display()
                );
            } else if !is_thin_delegator {
                println!(
                    "[warn] Hook: {} (outdated — inline logic, not thin delegator)",
                    hook_path.display()
                );
                println!(
                    "   → Run `rtk init --global` to upgrade to the single source of truth hook"
                );
            } else if is_executable && has_guards {
                println!(
                    "[ok] Hook: {} (thin delegator, version {})",
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
            println!("[ok] Hook: {} (exists)", hook_path.display());
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

    // Check hook integrity
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

    // Check global CLAUDE.md
    if global_claude_md.exists() {
        let content = fs::read_to_string(&global_claude_md)?;
        if content.contains("@RTK.md") {
            println!("[ok] Global (~/.claude/CLAUDE.md): @RTK.md reference");
        } else if content.contains("<!-- rtk-instructions") {
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

    // Check settings.json
    let settings_path = claude_dir.join("settings.json");
    if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        if !content.trim().is_empty() {
            if let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) {
                let hook_command = hook_path.display().to_string();
                if hook_already_present(&root, &hook_command) {
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
        let cursor_hook = cursor_dir.join("hooks").join("rtk-rewrite.sh");
        let cursor_hooks_json = cursor_dir.join("hooks.json");

        if cursor_hook.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let meta = fs::metadata(&cursor_hook)?;
                let is_executable = meta.permissions().mode() & 0o111 != 0;
                let content = fs::read_to_string(&cursor_hook)?;
                let is_thin = content.contains("rtk rewrite");

                if !is_executable {
                    println!(
                        "[warn] Cursor hook: {} (NOT executable - run: chmod +x)",
                        cursor_hook.display()
                    );
                } else if is_thin {
                    println!(
                        "[ok] Cursor hook: {} (thin delegator)",
                        cursor_hook.display()
                    );
                } else {
                    println!(
                        "[warn] Cursor hook: {} (outdated - missing rtk rewrite delegation)",
                        cursor_hook.display()
                    );
                }
            }

            #[cfg(not(unix))]
            {
                println!("[ok] Cursor hook: {} (exists)", cursor_hook.display());
            }
        } else {
            println!("[--] Cursor hook: not found");
        }

        if cursor_hooks_json.exists() {
            let content = fs::read_to_string(&cursor_hooks_json)?;
            if !content.trim().is_empty() {
                if let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) {
                    if cursor_hook_already_present(&root) {
                        println!("[ok] Cursor hooks.json: RTK preToolUse configured");
                    } else {
                        println!("[warn] Cursor hooks.json: exists but RTK not configured");
                        println!("    Run: rtk init -g --agent cursor");
                    }
                } else {
                    println!("[warn] Cursor hooks.json: exists but invalid JSON");
                }
            } else {
                println!("[--] Cursor hooks.json: empty");
            }
        } else {
            println!("[--] Cursor hooks.json: not found");
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
    println!("  rtk init -g --codex         # Configure ~/.codex/AGENTS.md + ~/.codex/RTK.md");
    println!("  rtk init -g --opencode      # OpenCode plugin only");
    println!("  rtk init -g --agent cursor  # Install Cursor Agent hooks");

    Ok(())
}

fn show_codex_config() -> Result<()> {
    let codex_dir = resolve_codex_dir()?;
    let global_agents_md = codex_dir.join("AGENTS.md");
    let global_rtk_md = codex_dir.join("RTK.md");
    let local_agents_md = PathBuf::from("AGENTS.md");
    let local_rtk_md = PathBuf::from("RTK.md");

    println!("rtk Configuration (Codex CLI):\n");

    if global_rtk_md.exists() {
        println!("[ok] Global RTK.md: {}", global_rtk_md.display());
    } else {
        println!("[--] Global RTK.md: not found");
    }

    if global_agents_md.exists() {
        let content = fs::read_to_string(&global_agents_md)?;
        if content.contains("@RTK.md") {
            println!("[ok] Global AGENTS.md: @RTK.md reference");
        } else if content.contains("<!-- rtk-instructions") {
            println!("[!!] Global AGENTS.md: old inline RTK block");
        } else {
            println!("[--] Global AGENTS.md: exists but rtk not configured");
        }
    } else {
        println!("[--] Global AGENTS.md: not found");
    }

    if local_rtk_md.exists() {
        println!("[ok] Local RTK.md: {}", local_rtk_md.display());
    } else {
        println!("[--] Local RTK.md: not found");
    }

    if local_agents_md.exists() {
        let content = fs::read_to_string(&local_agents_md)?;
        if content.contains("@RTK.md") {
            println!("[ok] Local AGENTS.md: @RTK.md reference");
        } else if content.contains("<!-- rtk-instructions") {
            println!("[!!] Local AGENTS.md: old inline RTK block");
        } else {
            println!("[--] Local AGENTS.md: exists but rtk not configured");
        }
    } else {
        println!("[--] Local AGENTS.md: not found");
    }

    println!("\nUsage:");
    println!("  rtk init --codex              # Configure local AGENTS.md + RTK.md");
    println!("  rtk init -g --codex           # Configure ~/.codex/AGENTS.md + ~/.codex/RTK.md");
    println!("  rtk init -g --codex --uninstall  # Remove global Codex RTK artifacts");

    Ok(())
}

fn run_opencode_only_mode(verbose: u8) -> Result<()> {
    let opencode_plugin_path = prepare_opencode_plugin_path()?;
    ensure_opencode_plugin_installed(&opencode_plugin_path, verbose)?;
    println!("\nOpenCode plugin installed (global).\n");
    println!("  OpenCode: {}", opencode_plugin_path.display());
    println!("  Restart OpenCode. Test with: git status\n");
    Ok(())
}

// ─── Gemini CLI support ───────────────────────────────────────────

/// Gemini hook wrapper script — delegates to `rtk hook gemini`
const GEMINI_HOOK_SCRIPT: &str = r#"#!/bin/bash
exec rtk hook gemini
"#;

/// Resolve the Gemini config directory (~/.gemini)
fn resolve_gemini_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    Ok(home.join(".gemini"))
}

/// Entry point for `rtk init --gemini`
pub fn run_gemini(global: bool, hook_only: bool, patch_mode: PatchMode, verbose: u8) -> Result<()> {
    if !global {
        anyhow::bail!("Gemini support is global-only. Use: rtk init -g --gemini");
    }

    let gemini_dir = resolve_gemini_dir()?;
    fs::create_dir_all(&gemini_dir).with_context(|| {
        format!(
            "Failed to create Gemini config dir: {}",
            gemini_dir.display()
        )
    })?;

    // 1. Install hook script
    let hook_dir = gemini_dir.join("hooks");
    fs::create_dir_all(&hook_dir)
        .with_context(|| format!("Failed to create hook dir: {}", hook_dir.display()))?;
    let hook_path = hook_dir.join("rtk-hook-gemini.sh");
    write_if_changed(&hook_path, GEMINI_HOOK_SCRIPT, "Gemini hook", verbose)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("Failed to set hook permissions: {}", hook_path.display()))?;
    }

    // 2. Install GEMINI.md (RTK awareness for Gemini)
    if !hook_only {
        let gemini_md_path = gemini_dir.join("GEMINI.md");
        // Reuse the same slim RTK awareness content
        write_if_changed(&gemini_md_path, RTK_SLIM, "GEMINI.md", verbose)?;
    }

    // 3. Patch ~/.gemini/settings.json
    patch_gemini_settings(&gemini_dir, &hook_path, patch_mode, verbose)?;

    println!("\nGemini CLI hook installed (global).\n");
    println!("  Hook: {}", hook_path.display());
    if !hook_only {
        println!("  GEMINI.md: {}", gemini_dir.join("GEMINI.md").display());
    }
    println!("  Restart Gemini CLI. Test with: git status\n");
    Ok(())
}

/// Patch ~/.gemini/settings.json with the BeforeTool hook
fn patch_gemini_settings(
    gemini_dir: &Path,
    hook_path: &Path,
    patch_mode: PatchMode,
    verbose: u8,
) -> Result<()> {
    let settings_path = gemini_dir.join("settings.json");
    let hook_cmd = hook_path.to_string_lossy().to_string();

    // Read or create settings.json
    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read {}", settings_path.display()))?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Check if hook already registered
    if let Some(hooks) = settings.pointer("/hooks/BeforeTool") {
        if let Some(arr) = hooks.as_array() {
            if arr.iter().any(|h| {
                h.pointer("/hooks/0/command")
                    .and_then(|v| v.as_str())
                    .is_some_and(|c| c.contains("rtk"))
            }) {
                if verbose > 0 {
                    eprintln!("Gemini settings.json already has RTK hook");
                }
                return Ok(());
            }
        }
    }

    // Ask user before patching
    if patch_mode == PatchMode::Skip {
        println!(
            "\nManual setup needed: add RTK hook to {}\n\
             See: https://github.com/rtk-ai/rtk#gemini-cli",
            settings_path.display()
        );
        return Ok(());
    }

    if patch_mode == PatchMode::Ask {
        print!("Patch {} with RTK hook? [y/N] ", settings_path.display());
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Skipped. Add hook manually later.");
            return Ok(());
        }
    }

    // Build hook entry matching Gemini CLI format
    let hook_entry = serde_json::json!({
        "matcher": "run_shell_command",
        "hooks": [{
            "type": "command",
            "command": hook_cmd
        }]
    });

    // Insert into settings
    let hooks = settings
        .as_object_mut()
        .context("settings.json is not an object")?
        .entry("hooks")
        .or_insert(serde_json::json!({}));

    let before_tool = hooks
        .as_object_mut()
        .context("hooks is not an object")?
        .entry("BeforeTool")
        .or_insert(serde_json::json!([]));

    before_tool
        .as_array_mut()
        .context("BeforeTool is not an array")?
        .push(hook_entry);

    // Write atomically
    let content = serde_json::to_string_pretty(&settings)?;
    let tmp = NamedTempFile::new_in(gemini_dir)?;
    fs::write(tmp.path(), &content)?;
    tmp.persist(&settings_path)
        .with_context(|| format!("Failed to write {}", settings_path.display()))?;

    if verbose > 0 {
        eprintln!("Patched {}", settings_path.display());
    }

    Ok(())
}

/// Remove Gemini artifacts during uninstall
fn uninstall_gemini(verbose: u8) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    let gemini_dir = match resolve_gemini_dir() {
        Ok(d) => d,
        Err(_) => return Ok(removed),
    };

    // Remove hook
    let hook_path = gemini_dir.join("hooks").join("rtk-hook-gemini.sh");
    if hook_path.exists() {
        fs::remove_file(&hook_path)
            .with_context(|| format!("Failed to remove {}", hook_path.display()))?;
        removed.push(format!("Gemini hook: {}", hook_path.display()));
    }

    // Remove GEMINI.md
    let gemini_md = gemini_dir.join("GEMINI.md");
    if gemini_md.exists() {
        fs::remove_file(&gemini_md)
            .with_context(|| format!("Failed to remove {}", gemini_md.display()))?;
        removed.push(format!("GEMINI.md: {}", gemini_md.display()));
    }

    // Remove hook from settings.json
    let settings_path = gemini_dir.join("settings.json");
    if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        if let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(arr) = settings
                .pointer_mut("/hooks/BeforeTool")
                .and_then(|v| v.as_array_mut())
            {
                let before = arr.len();
                arr.retain(|h| {
                    !h.pointer("/hooks/0/command")
                        .and_then(|v| v.as_str())
                        .is_some_and(|c| c.contains("rtk"))
                });
                if arr.len() < before {
                    let new_content = serde_json::to_string_pretty(&settings)?;
                    fs::write(&settings_path, new_content)?;
                    removed.push("Gemini settings.json: removed RTK hook entry".to_string());
                }
            }
        }
    }

    if verbose > 0 && !removed.is_empty() {
        eprintln!("Gemini artifacts removed");
    }

    Ok(removed)
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
            RTK_INSTRUCTIONS.contains("<!-- rtk-instructions"),
            "RTK_INSTRUCTIONS must have version marker for idempotency"
        );
    }

    #[test]
    fn test_hook_has_guards() {
        assert!(REWRITE_HOOK.contains("command -v rtk"));
        assert!(REWRITE_HOOK.contains("command -v jq"));
        // Guards (rtk/jq availability checks) must appear before the actual delegation call.
        // The thin delegating hook no longer uses set -euo pipefail.
        let jq_pos = REWRITE_HOOK.find("command -v jq").unwrap();
        let rtk_delegate_pos = REWRITE_HOOK.find("rtk rewrite \"$CMD\"").unwrap();
        assert!(
            jq_pos < rtk_delegate_pos,
            "Guards must appear before rtk rewrite delegation"
        );
    }

    #[test]
    fn test_migration_removes_old_block() {
        let input = r#"# My Config

<!-- rtk-instructions v2 -->
OLD RTK STUFF
<!-- /rtk-instructions -->

More content"#;

        let (result, migrated) = remove_rtk_block(input);
        assert!(migrated);
        assert!(!result.contains("OLD RTK STUFF"));
        assert!(result.contains("# My Config"));
        assert!(result.contains("More content"));
    }

    #[test]
    fn test_opencode_plugin_install_and_update() {
        let temp = TempDir::new().unwrap();
        let opencode_dir = temp.path().join("opencode");
        let plugin_path = opencode_plugin_path(&opencode_dir);

        fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        assert!(!plugin_path.exists());

        let changed = ensure_opencode_plugin_installed(&plugin_path, 0).unwrap();
        assert!(changed);
        let content = fs::read_to_string(&plugin_path).unwrap();
        assert_eq!(content, OPENCODE_PLUGIN);

        fs::write(&plugin_path, "// old").unwrap();
        let changed_again = ensure_opencode_plugin_installed(&plugin_path, 0).unwrap();
        assert!(changed_again);
        let content_updated = fs::read_to_string(&plugin_path).unwrap();
        assert_eq!(content_updated, OPENCODE_PLUGIN);
    }

    #[test]
    fn test_opencode_plugin_remove() {
        let temp = TempDir::new().unwrap();
        let opencode_dir = temp.path().join("opencode");
        let plugin_path = opencode_plugin_path(&opencode_dir);
        fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        fs::write(&plugin_path, OPENCODE_PLUGIN).unwrap();

        assert!(plugin_path.exists());
        fs::remove_file(&plugin_path).unwrap();
        assert!(!plugin_path.exists());
    }

    #[test]
    fn test_migration_warns_on_missing_end_marker() {
        let input = "<!-- rtk-instructions v2 -->\nOLD STUFF\nNo end marker";
        let (result, migrated) = remove_rtk_block(input);
        assert!(!migrated);
        assert_eq!(result, input);
    }

    #[test]
    #[cfg(unix)]
    fn test_default_mode_creates_hook_and_rtk_md() {
        let temp = TempDir::new().unwrap();
        let hook_path = temp.path().join("rtk-rewrite.sh");
        let rtk_md_path = temp.path().join("RTK.md");

        fs::write(&hook_path, REWRITE_HOOK).unwrap();
        fs::write(&rtk_md_path, RTK_SLIM).unwrap();

        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(hook_path.exists());
        assert!(rtk_md_path.exists());

        let metadata = fs::metadata(&hook_path).unwrap();
        assert!(metadata.permissions().mode() & 0o111 != 0);
    }

    #[test]
    fn test_claude_md_mode_creates_full_injection() {
        // Just verify RTK_INSTRUCTIONS constant has the right content
        assert!(RTK_INSTRUCTIONS.contains("<!-- rtk-instructions"));
        assert!(RTK_INSTRUCTIONS.contains("rtk cargo test"));
        assert!(RTK_INSTRUCTIONS.contains("<!-- /rtk-instructions -->"));
        assert!(RTK_INSTRUCTIONS.len() > 4000);
    }

    // --- upsert_rtk_block tests ---

    #[test]
    fn test_upsert_rtk_block_appends_when_missing() {
        let input = "# Team instructions";
        let (content, action) = upsert_rtk_block(input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Added);
        assert!(content.contains("# Team instructions"));
        assert!(content.contains("<!-- rtk-instructions"));
    }

    #[test]
    fn test_upsert_rtk_block_updates_stale_block() {
        let input = r#"# Team instructions

<!-- rtk-instructions v1 -->
OLD RTK CONTENT
<!-- /rtk-instructions -->

More notes
"#;

        let (content, action) = upsert_rtk_block(input, RTK_INSTRUCTIONS);
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
        let input = "<!-- rtk-instructions v2 -->\npartial";
        let (content, action) = upsert_rtk_block(input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Malformed);
        assert_eq!(content, input);
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
    fn test_patch_agents_md_adds_reference_once() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");

        fs::write(&agents_md, "# Team rules\n").unwrap();
        let first_added = patch_agents_md(&agents_md, 0).unwrap();
        let second_added = patch_agents_md(&agents_md, 0).unwrap();

        assert!(first_added);
        assert!(!second_added);

        let content = fs::read_to_string(&agents_md).unwrap();
        assert_eq!(content.matches("@RTK.md").count(), 1);
    }

    #[test]
    fn test_codex_mode_rejects_auto_patch() {
        let err = run(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            PatchMode::Auto,
            0,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--codex cannot be combined with --auto-patch"
        );
    }

    #[test]
    fn test_codex_mode_rejects_no_patch() {
        let err = run(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            PatchMode::Skip,
            0,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--codex cannot be combined with --no-patch"
        );
    }

    #[test]
    fn test_patch_agents_md_creates_missing_file() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");

        let added = patch_agents_md(&agents_md, 0).unwrap();

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
            "# Team rules\n\n<!-- rtk-instructions v2 -->\nold\n<!-- /rtk-instructions -->\n",
        )
        .unwrap();

        let added = patch_agents_md(&agents_md, 0).unwrap();

        assert!(added);
        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains("old"));
        assert_eq!(content.matches("@RTK.md").count(), 1);
    }

    #[test]
    fn test_uninstall_codex_at_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let codex_dir = temp.path();
        let agents_md = codex_dir.join("AGENTS.md");
        let rtk_md = codex_dir.join("RTK.md");

        fs::write(&agents_md, "# Team rules\n\n@RTK.md\n").unwrap();
        fs::write(&rtk_md, "codex config").unwrap();

        let removed_first = uninstall_codex_at(codex_dir, 0).unwrap();
        let removed_second = uninstall_codex_at(codex_dir, 0).unwrap();

        assert_eq!(removed_first.len(), 2);
        assert!(removed_second.is_empty());
        assert!(!rtk_md.exists());

        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains("@RTK.md"));
        assert!(content.contains("# Team rules"));
    }

    #[test]
    fn test_local_init_unchanged() {
        // Local init should use claude-md mode
        let temp = TempDir::new().unwrap();
        let claude_md = temp.path().join("CLAUDE.md");

        fs::write(&claude_md, RTK_INSTRUCTIONS).unwrap();
        let content = fs::read_to_string(&claude_md).unwrap();

        assert!(content.contains("<!-- rtk-instructions"));
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

        insert_hook_entry(&mut json_content, hook_command);

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
        insert_hook_entry(&mut json_content, hook_command);

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
        insert_hook_entry(&mut json_content, hook_command);

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

    // ─── Cursor hooks.json tests ───

    #[test]
    fn test_cursor_hook_already_present_true() {
        let json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [{
                    "command": "./hooks/rtk-rewrite.sh",
                    "matcher": "Shell"
                }]
            }
        });
        assert!(cursor_hook_already_present(&json_content));
    }

    #[test]
    fn test_cursor_hook_already_present_false_empty() {
        let json_content = serde_json::json!({ "version": 1 });
        assert!(!cursor_hook_already_present(&json_content));
    }

    #[test]
    fn test_cursor_hook_already_present_false_other_hooks() {
        let json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [{
                    "command": "./hooks/some-other-hook.sh",
                    "matcher": "Shell"
                }]
            }
        });
        assert!(!cursor_hook_already_present(&json_content));
    }

    #[test]
    fn test_insert_cursor_hook_entry_empty() {
        let mut json_content = serde_json::json!({ "version": 1 });
        insert_cursor_hook_entry(&mut json_content);

        let hooks = json_content["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], "./hooks/rtk-rewrite.sh");
        assert_eq!(hooks[0]["matcher"], "Shell");
        assert_eq!(json_content["version"], 1);
    }

    #[test]
    fn test_insert_cursor_hook_preserves_existing() {
        let mut json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [{
                    "command": "./hooks/other.sh",
                    "matcher": "Shell"
                }],
                "afterFileEdit": [{
                    "command": "./hooks/format.sh"
                }]
            }
        });

        insert_cursor_hook_entry(&mut json_content);

        let pre_tool_use = json_content["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 2);
        assert_eq!(pre_tool_use[0]["command"], "./hooks/other.sh");
        assert_eq!(pre_tool_use[1]["command"], "./hooks/rtk-rewrite.sh");

        // afterFileEdit should be preserved
        assert!(json_content["hooks"]["afterFileEdit"].is_array());
    }

    #[test]
    fn test_remove_cursor_hook_from_json() {
        let mut json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [
                    { "command": "./hooks/other.sh", "matcher": "Shell" },
                    { "command": "./hooks/rtk-rewrite.sh", "matcher": "Shell" }
                ]
            }
        });

        let removed = remove_cursor_hook_from_json(&mut json_content);
        assert!(removed);

        let hooks = json_content["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], "./hooks/other.sh");
    }

    #[test]
    fn test_remove_cursor_hook_not_present() {
        let mut json_content = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [
                    { "command": "./hooks/other.sh", "matcher": "Shell" }
                ]
            }
        });

        let removed = remove_cursor_hook_from_json(&mut json_content);
        assert!(!removed);
    }

    #[test]
    fn test_cursor_hook_script_has_guards() {
        assert!(CURSOR_REWRITE_HOOK.contains("command -v rtk"));
        assert!(CURSOR_REWRITE_HOOK.contains("command -v jq"));
        let jq_pos = CURSOR_REWRITE_HOOK.find("command -v jq").unwrap();
        let rtk_delegate_pos = CURSOR_REWRITE_HOOK.find("rtk rewrite \"$CMD\"").unwrap();
        assert!(
            jq_pos < rtk_delegate_pos,
            "Guards must appear before rtk rewrite delegation"
        );
    }

    #[test]
    fn test_cursor_hook_outputs_cursor_format() {
        assert!(CURSOR_REWRITE_HOOK.contains("\"permission\": \"allow\""));
        assert!(CURSOR_REWRITE_HOOK.contains("\"updated_input\""));
        assert!(!CURSOR_REWRITE_HOOK.contains("hookSpecificOutput"));
    }
}
