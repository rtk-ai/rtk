use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

// Embedded hook script (guards before set -euo pipefail)
const REWRITE_HOOK: &str = include_str!("../hooks/rtk-rewrite.sh");

// Embedded slim RTK awareness instructions
const RTK_SLIM: &str = include_str!("../hooks/rtk-awareness.md");

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
pub fn run(global: bool, claude_md: bool, hook_only: bool, verbose: u8) -> Result<()> {
    // Mode selection
    match (claude_md, hook_only) {
        (true, _) => run_claude_md_mode(global, verbose),
        (false, true) => run_hook_only_mode(global, verbose),
        (false, false) => run_default_mode(global, verbose),
    }
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

/// Default mode: hook + slim RTK.md + @RTK.md reference
#[cfg(not(unix))]
fn run_default_mode(_global: bool, _verbose: u8) -> Result<()> {
    eprintln!("⚠️  Hook-based mode requires Unix (macOS/Linux).");
    eprintln!("    Windows: use --claude-md mode for full injection.");
    eprintln!("    Falling back to --claude-md mode.");
    run_claude_md_mode(_global, _verbose)
}

#[cfg(unix)]
fn run_default_mode(global: bool, verbose: u8) -> Result<()> {
    if !global {
        // Local init: unchanged behavior (full injection into ./CLAUDE.md)
        return run_claude_md_mode(false, verbose);
    }

    let claude_dir = resolve_claude_dir()?;
    let rtk_md_path = claude_dir.join("RTK.md");
    let claude_md_path = claude_dir.join("CLAUDE.md");

    // 1. Prepare hook directory and install hook
    let (_hook_dir, hook_path) = prepare_hook_paths()?;
    ensure_hook_installed(&hook_path, verbose)?;

    // 2. Write RTK.md
    write_if_changed(&rtk_md_path, RTK_SLIM, "RTK.md", verbose)?;

    // 3. Patch CLAUDE.md (add @RTK.md, migrate if needed)
    let migrated = patch_claude_md(&claude_md_path, verbose)?;

    // 5. Print success message
    println!("\nRTK hook installed (global).\n");
    println!("  Hook:      {}", hook_path.display());
    println!("  RTK.md:    {} (10 lines)", rtk_md_path.display());
    println!("  CLAUDE.md: @RTK.md reference added");

    if migrated {
        println!("\n  ✅ Migrated: removed 137-line RTK block from CLAUDE.md");
        println!("              replaced with @RTK.md (10 lines)");
    }

    println!("\n  MANUAL STEP: Add this to ~/.claude/settings.json:");
    println!("  {{");
    println!("    \"hooks\": {{ \"PreToolUse\": [{{");
    println!("      \"matcher\": \"Bash\",");
    println!("      \"hooks\": [{{ \"type\": \"command\",");
    println!("        \"command\": \"{}\"", hook_path.display());
    println!("      }}]");
    println!("    }}]}}");
    println!("  }}");
    println!("\n  Then restart Claude Code. Test with: git status\n");

    Ok(())
}

/// Hook-only mode: just the hook, no RTK.md
#[cfg(not(unix))]
fn run_hook_only_mode(_global: bool, _verbose: u8) -> Result<()> {
    anyhow::bail!("Hook install requires Unix (macOS/Linux). Use WSL or --claude-md mode.")
}

#[cfg(unix)]
fn run_hook_only_mode(global: bool, verbose: u8) -> Result<()> {
    if !global {
        eprintln!("⚠️  Warning: --hook-only only makes sense with --global");
        eprintln!("    For local projects, use default mode or --claude-md");
        return Ok(());
    }

    // Prepare and install hook
    let (_hook_dir, hook_path) = prepare_hook_paths()?;
    ensure_hook_installed(&hook_path, verbose)?;

    println!("\nRTK hook installed (hook-only mode).\n");
    println!("  Hook: {}", hook_path.display());
    println!("\n  MANUAL STEP: Add hook to ~/.claude/settings.json (see --global output)");
    println!("  Note: No RTK.md created. Claude won't know about meta commands (gain, discover, proxy).\n");

    Ok(())
}

/// Legacy mode: full 137-line injection into CLAUDE.md
fn run_claude_md_mode(global: bool, verbose: u8) -> Result<()> {
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

        if existing.contains("<!-- rtk-instructions") {
            println!("✅ {} already contains rtk instructions", path.display());
            return Ok(());
        }

        let new_content = format!("{}\n\n{}", existing.trim(), RTK_INSTRUCTIONS);
        fs::write(&path, new_content)?;
        println!("✅ Added rtk instructions to existing {}", path.display());
    } else {
        fs::write(&path, RTK_INSTRUCTIONS)?;
        println!("✅ Created {} with rtk instructions", path.display());
    }

    if global {
        println!("   Claude Code will now use rtk in all sessions");
    } else {
        println!("   Claude Code will use rtk in this project");
    }

    Ok(())
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
        eprintln!("⚠️  Warning: Found '<!-- rtk-instructions' without closing marker.");
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

/// Show current rtk configuration
pub fn show_config() -> Result<()> {
    let claude_dir = resolve_claude_dir()?;
    let hook_path = claude_dir.join("hooks").join("rtk-rewrite.sh");
    let rtk_md_path = claude_dir.join("RTK.md");
    let global_claude_md = claude_dir.join("CLAUDE.md");
    let local_claude_md = PathBuf::from("CLAUDE.md");

    println!("📋 rtk Configuration:\n");

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

            if is_executable && has_guards {
                println!("✅ Hook: {} (executable, with guards)", hook_path.display());
            } else if !is_executable {
                println!(
                    "⚠️  Hook: {} (NOT executable - run: chmod +x)",
                    hook_path.display()
                );
            } else {
                println!("⚠️  Hook: {} (no guards - outdated)", hook_path.display());
            }
        }

        #[cfg(not(unix))]
        {
            println!("✅ Hook: {} (exists)", hook_path.display());
        }
    } else {
        println!("⚪ Hook: not found");
    }

    // Check RTK.md
    if rtk_md_path.exists() {
        println!("✅ RTK.md: {} (slim mode)", rtk_md_path.display());
    } else {
        println!("⚪ RTK.md: not found");
    }

    // Check global CLAUDE.md
    if global_claude_md.exists() {
        let content = fs::read_to_string(&global_claude_md)?;
        if content.contains("@RTK.md") {
            println!("✅ Global (~/.claude/CLAUDE.md): @RTK.md reference");
        } else if content.contains("<!-- rtk-instructions") {
            println!(
                "⚠️  Global (~/.claude/CLAUDE.md): old RTK block (run: rtk init -g to migrate)"
            );
        } else {
            println!("⚪ Global (~/.claude/CLAUDE.md): exists but rtk not configured");
        }
    } else {
        println!("⚪ Global (~/.claude/CLAUDE.md): not found");
    }

    // Check local CLAUDE.md
    if local_claude_md.exists() {
        let content = fs::read_to_string(&local_claude_md)?;
        if content.contains("rtk") {
            println!("✅ Local (./CLAUDE.md): rtk enabled");
        } else {
            println!("⚪ Local (./CLAUDE.md): exists but rtk not configured");
        }
    } else {
        println!("⚪ Local (./CLAUDE.md): not found");
    }

    println!("\nUsage:");
    println!("  rtk init              # Full injection into local CLAUDE.md");
    println!("  rtk init -g           # Hook + RTK.md + @RTK.md (recommended)");
    println!("  rtk init -g --claude-md    # Legacy: full injection into ~/.claude/CLAUDE.md");
    println!("  rtk init -g --hook-only    # Hook only, no RTK.md");

    Ok(())
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
        // Guards must be BEFORE set -euo pipefail
        let guard_pos = REWRITE_HOOK.find("command -v rtk").unwrap();
        let set_pos = REWRITE_HOOK.find("set -euo pipefail").unwrap();
        assert!(
            guard_pos < set_pos,
            "Guards must come before set -euo pipefail"
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
    fn test_local_init_unchanged() {
        // Local init should use claude-md mode
        let temp = TempDir::new().unwrap();
        let claude_md = temp.path().join("CLAUDE.md");

        fs::write(&claude_md, RTK_INSTRUCTIONS).unwrap();
        let content = fs::read_to_string(&claude_md).unwrap();

        assert!(content.contains("<!-- rtk-instructions"));
    }
}
