# Grok Hook Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `rtk init --grok` and `rtk hook grok` so RTK can intercept Bash tool calls from xAI's Grok Build TUI and suggest token-optimized rewrites.

**Architecture:** Mirror the Copilot CLI integration's *deny + suggest* output protocol (Grok's documented PreToolUse contract is allow/deny only), and mirror the Gemini integration's global-only install flow. Hook is delivered as a standalone JSON file at `~/.grok/hooks/rtk.json` (no settings.json patching). The runtime handler reads Grok's camelCase payload (`toolName`/`toolInput`), reuses `permissions::check_command` + `get_rewritten`, and emits `{"decision":"deny","reason":"Token savings: use \`rtk …\` instead …"}`.

**Tech Stack:** Rust 1.x, anyhow, serde_json, lazy_static, insta (snapshots), tempfile (test isolation). No new dependencies.

**Spec reference:** `docs/superpowers/specs/2026-05-17-grok-hook-integration-design.md`

---

## File Structure

**New code lives entirely inside existing files** (no new modules). This matches the layout used by every other agent integration (Gemini/Codex/Copilot all extend `init.rs` + `hook_cmd.rs`).

| File | Responsibility | Change |
|------|----------------|--------|
| `src/hooks/constants.rs` | Path/filename constants | Add `GROK_DIR`, `GROK_HOOK_FILENAME`, `GROK_MD` |
| `src/hooks/hook_cmd.rs` | Runtime hook handlers | Add `run_grok()` + tests |
| `src/hooks/init.rs` | Install/uninstall flows | Add `run_grok()`, `uninstall_grok()`, helpers + tests |
| `src/main.rs` | CLI surface + dispatch | Add `--grok` flag, `HookCommands::Grok`, dispatch + uninstall wiring |
| `README.md` | User docs | Add Grok to supported-agents list |

---

## Task 1: Add Grok constants

**Files:**
- Modify: `src/hooks/constants.rs`

- [ ] **Step 1: Add the three constants at the end of the file**

Append to `src/hooks/constants.rs`:

```rust

pub const GROK_DIR: &str = ".grok";
pub const GROK_HOOK_FILENAME: &str = "rtk.json";
pub const GROK_MD: &str = "GROK.md";
```

- [ ] **Step 2: Verify the crate still builds**

Run: `cargo check`
Expected: `Finished` with no errors.

- [ ] **Step 3: Commit**

```bash
git add src/hooks/constants.rs
git commit -m "feat(grok): add path constants for Grok hook integration"
```

---

## Task 2: Add `HookCommands::Grok` CLI variant

**Files:**
- Modify: `src/main.rs` around lines 768-786 (the `HookCommands` enum)

- [ ] **Step 1: Add the `Grok` variant**

Inside `enum HookCommands` (after `Copilot,` and before `Check { ... }`), add:

```rust
    /// Process Grok Build TUI PreToolUse hook (reads JSON from stdin)
    Grok,
```

The full enum should now read:

```rust
#[derive(Debug, Subcommand)]
enum HookCommands {
    /// Process Claude Code PreToolUse hook (reads JSON from stdin)
    Claude,
    /// Process Cursor Agent hook (reads JSON from stdin)
    Cursor,
    /// Process Gemini CLI BeforeTool hook (reads JSON from stdin)
    Gemini,
    /// Process Copilot preToolUse hook (VS Code + Copilot CLI, reads JSON from stdin)
    Copilot,
    /// Process Grok Build TUI PreToolUse hook (reads JSON from stdin)
    Grok,
    /// Check how a command would be rewritten by the hook engine (dry-run)
    Check {
        /// Target agent
        #[arg(long, default_value = "claude")]
        agent: String,
        /// Command to check
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
}
```

- [ ] **Step 2: Verify it compiles (dispatch will fail until Task 4 — that's expected)**

Run: `cargo check`
Expected: a single error about a non-exhaustive `match` on `HookCommands` in `run_cli`. Do **not** fix it yet — Task 4 wires the dispatch with the real handler.

- [ ] **Step 3: Don't commit yet**

This task is a stub; commit together with Task 4 to keep the tree compiling at each commit.

---

## Task 3: Implement `run_grok` runtime handler (TDD)

**Files:**
- Modify: `src/hooks/hook_cmd.rs` (add handler near other `run_*` functions, e.g. immediately after `run_gemini`)
- Test: same file, in the existing `mod tests`

### Step 3.1 — Write failing tests

- [ ] **Step 1: Add a Grok-format payload helper in the existing `mod tests`**

Inside `mod tests` (after `vscode_input` / `copilot_cli_input` helpers near the top of the test module), add:

```rust
    fn grok_input(tool: &str, cmd: &str) -> Value {
        json!({
            "hookEventName": "pre_tool_use",
            "sessionId": "test-session",
            "cwd": "/tmp/proj",
            "workspaceRoot": "/tmp/proj",
            "toolName": tool,
            "toolInput": { "command": cmd },
            "timestamp": "2026-05-17T00:00:00Z"
        })
    }
```

- [ ] **Step 2: Add a test-only inner runner that returns the stdout string**

In `hook_cmd.rs` (outside `mod tests`, gated by `#[cfg(test)]`, near `run_claude_inner`), add:

```rust
#[cfg(test)]
fn run_grok_inner(input: &str) -> Option<String> {
    let v: Value = serde_json::from_str(input).ok()?;
    process_grok_payload(&v).map(|out| out.to_string())
}
```

This calls `process_grok_payload`, a pure function that mirrors `process_claude_payload`. The pure function is defined in step 3.3 below.

- [ ] **Step 3: Write the failing test cases**

Append to `mod tests` in `hook_cmd.rs`:

```rust
    #[test]
    fn test_grok_rewrites_bash_alias() {
        let input = grok_input("Bash", "git status");
        let out = run_grok_inner(&input.to_string()).expect("rewrite expected");
        assert!(out.contains(r#""decision":"deny""#), "got: {}", out);
        assert!(out.contains("rtk git status"), "got: {}", out);
    }

    #[test]
    fn test_grok_rewrites_run_terminal_cmd() {
        let input = grok_input("run_terminal_cmd", "git status");
        let out = run_grok_inner(&input.to_string()).expect("rewrite expected");
        assert!(out.contains("rtk git status"), "got: {}", out);
    }

    #[test]
    fn test_grok_passes_non_bash_tool() {
        let input = grok_input("read_file", "git status");
        assert!(
            run_grok_inner(&input.to_string()).is_none(),
            "non-Bash tool must pass through"
        );
    }

    #[test]
    fn test_grok_passes_already_rtk() {
        let input = grok_input("Bash", "rtk git status");
        assert!(
            run_grok_inner(&input.to_string()).is_none(),
            "already-rewritten commands must pass through"
        );
    }

    #[test]
    fn test_grok_passes_heredoc() {
        let input = grok_input("Bash", "cat <<EOF\nhello\nEOF");
        assert!(
            run_grok_inner(&input.to_string()).is_none(),
            "heredoc commands must pass through"
        );
    }

    #[test]
    fn test_grok_passes_empty_command() {
        let input = grok_input("Bash", "");
        assert!(run_grok_inner(&input.to_string()).is_none());
    }

    #[test]
    fn test_grok_passes_missing_tool_input() {
        let input = json!({
            "hookEventName": "pre_tool_use",
            "toolName": "Bash"
        });
        assert!(run_grok_inner(&input.to_string()).is_none());
    }

    #[test]
    fn test_grok_malformed_payload_returns_none() {
        assert!(run_grok_inner("not json").is_none());
    }
```

- [ ] **Step 4: Run the new tests to verify they fail**

Run: `cargo test --lib hooks::hook_cmd::tests::test_grok`
Expected: all eight tests fail with **compile errors** because `process_grok_payload` doesn't exist yet. (Clean red.)

### Step 3.2 — Implement the pure helper + public entry point

- [ ] **Step 5: Add `process_grok_payload` and `run_grok`**

Inside `src/hooks/hook_cmd.rs`, after the existing Gemini section (after `fn print_rewrite`), insert a new section:

```rust
// ── Grok hook ────────────────────────────────────────────────

/// Pure decision function. Returns `Some(json_value)` when a deny+suggest
/// response should be written to stdout, `None` when the hook should pass
/// through silently (Grok treats empty stdout as fail-open allow).
fn process_grok_payload(v: &Value) -> Option<Value> {
    let tool_name = v.get("toolName").and_then(|t| t.as_str()).unwrap_or("");
    if !matches!(tool_name, "Bash" | "run_terminal_cmd") {
        return None;
    }

    let cmd = v
        .pointer("/toolInput/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())?;

    if permissions::check_command(cmd) == PermissionVerdict::Deny {
        return Some(json!({
            "decision": "deny",
            "reason": "Blocked by RTK permission rule",
        }));
    }

    let rewritten = get_rewritten(cmd)?;

    Some(json!({
        "decision": "deny",
        "reason": format!(
            "Token savings: use `{}` instead (rtk saves 60-90% tokens)",
            rewritten
        ),
    }))
}

/// Run the Grok Build TUI PreToolUse hook.
///
/// Wire protocol (Grok docs `~/.grok/docs/user-guide/10-hooks.md`):
/// - Stdin: camelCase payload `{ toolName, toolInput: { command }, ... }`.
/// - Stdout (deny): `{"decision":"deny","reason":"..."}`.
/// - Empty stdout or any non-2 exit code is fail-open allow.
pub fn run_grok() -> Result<()> {
    let input = read_stdin_limited()?;
    let input = input.trim();
    if input.is_empty() {
        return Ok(());
    }

    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(io::stderr(), "[rtk hook] Failed to parse JSON input: {e}");
            return Ok(());
        }
    };

    let cmd_for_audit = v
        .pointer("/toolInput/command")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    match process_grok_payload(&v) {
        Some(output) => {
            // Distinguish deny-by-rule from deny-by-rewrite for the audit log.
            let action = output
                .get("reason")
                .and_then(|r| r.as_str())
                .map(|r| {
                    if r.starts_with("Token savings") {
                        "rewrite"
                    } else {
                        "deny"
                    }
                })
                .unwrap_or("deny");
            let rewritten = output
                .get("reason")
                .and_then(|r| r.as_str())
                .and_then(extract_suggestion_from_reason)
                .unwrap_or_default();
            audit_log(action, &cmd_for_audit, &rewritten);
            let _ = writeln!(io::stdout(), "{output}");
        }
        None => {
            // Pass-through: empty stdout = Grok fail-open allow.
        }
    }
    Ok(())
}

/// Pull the suggested rewrite out of a `Token savings: use \`X\` instead ...` reason.
/// Used only for audit logging; failure-tolerant.
fn extract_suggestion_from_reason(reason: &str) -> Option<String> {
    let start = reason.find('`')? + 1;
    let rest = &reason[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}
```

- [ ] **Step 6: Run the eight Grok tests — they should now pass**

Run: `cargo test --lib hooks::hook_cmd::tests::test_grok`
Expected: all 8 tests pass.

- [ ] **Step 7: Run the full hook_cmd test module to confirm no regressions**

Run: `cargo test --lib hooks::hook_cmd`
Expected: all existing tests still pass.

- [ ] **Step 8: Don't commit yet**

Wait for Task 4 so the tree compiles at this commit.

---

## Task 4: Wire `HookCommands::Grok` dispatch in `main.rs`

**Files:**
- Modify: `src/main.rs` around lines 2162-2178 (the `Commands::Hook` match)

- [ ] **Step 1: Add the dispatch arm**

Inside the `match command { ... }` block for `Commands::Hook`, add (after `HookCommands::Copilot`):

```rust
            HookCommands::Grok => {
                hooks::hook_cmd::run_grok()?;
                0
            }
```

The block should read:

```rust
        Commands::Hook { command } => match command {
            HookCommands::Claude => {
                hooks::hook_cmd::run_claude()?;
                0
            }
            HookCommands::Cursor => {
                hooks::hook_cmd::run_cursor()?;
                0
            }
            HookCommands::Gemini => {
                hooks::hook_cmd::run_gemini()?;
                0
            }
            HookCommands::Copilot => {
                hooks::hook_cmd::run_copilot()?;
                0
            }
            HookCommands::Grok => {
                hooks::hook_cmd::run_grok()?;
                0
            }
            HookCommands::Check { agent: _, command } => {
                // ... existing body unchanged
```

- [ ] **Step 2: Build the crate**

Run: `cargo build`
Expected: clean build, no warnings introduced.

- [ ] **Step 3: Smoke-test the new subcommand**

Run:

```bash
echo '{"toolName":"Bash","toolInput":{"command":"git status"}}' | cargo run --quiet -- hook grok
```

Expected stdout (single line):

```json
{"decision":"deny","reason":"Token savings: use `rtk git status` instead (rtk saves 60-90% tokens)"}
```

If the project filters warning shows up first on stderr (`[rtk] WARNING: untrusted project filters`), that's fine — it's pre-existing behavior. Only the stdout JSON matters.

- [ ] **Step 4: Commit Tasks 2-4 together**

```bash
git add src/main.rs src/hooks/hook_cmd.rs
git commit -m "feat(grok): runtime hook handler + CLI dispatch

Implements `rtk hook grok` for Grok Build TUI's PreToolUse hook.
Mirrors Copilot CLI's deny+suggest output protocol because Grok's
documented PreToolUse contract is allow/deny only — no updatedInput
rewrite. Pure decision function (process_grok_payload) drives both
the public entry point and unit tests."
```

---

## Task 5: Implement `init::run_grok` (TDD)

**Files:**
- Modify: `src/hooks/init.rs` (add functions near the Gemini section around line 3398; add tests in the existing `mod tests`)
- Modify: `src/hooks/init.rs` imports (add `GROK_DIR`, `GROK_HOOK_FILENAME`, `GROK_MD` to the `use super::constants::{...}` block)

### Step 5.1 — Add imports

- [ ] **Step 1: Extend the constants `use` block**

Find this near the top of `src/hooks/init.rs`:

```rust
use super::constants::{
    BEFORE_TOOL_KEY, CLAUDE_DIR, CLAUDE_HOOK_COMMAND, CODEX_DIR, CURSOR_HOOK_COMMAND,
    GEMINI_HOOK_FILE, HERMES_DIR, HERMES_PLUGINS_SUBDIR, HERMES_PLUGIN_INIT_FILE,
    HERMES_PLUGIN_MANIFEST_FILE, HERMES_PLUGIN_NAME, HOOKS_JSON, HOOKS_SUBDIR, PRE_TOOL_USE_KEY,
    REWRITE_HOOK_FILE, SETTINGS_JSON,
};
```

Replace with (sorted alphabetically, three new names added):

```rust
use super::constants::{
    BEFORE_TOOL_KEY, CLAUDE_DIR, CLAUDE_HOOK_COMMAND, CODEX_DIR, CURSOR_HOOK_COMMAND,
    GEMINI_HOOK_FILE, GROK_DIR, GROK_HOOK_FILENAME, GROK_MD, HERMES_DIR, HERMES_PLUGINS_SUBDIR,
    HERMES_PLUGIN_INIT_FILE, HERMES_PLUGIN_MANIFEST_FILE, HERMES_PLUGIN_NAME, HOOKS_JSON,
    HOOKS_SUBDIR, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE, SETTINGS_JSON,
};
```

(Note: `GROK_MD` is a *path* constant living in `constants.rs`. The existing `GEMINI_MD` is a local const in `init.rs` — Grok deliberately follows the `CLAUDE_DIR` pattern instead so the path lives next to its siblings.)

- [ ] **Step 2: Verify the imports compile**

Run: `cargo check`
Expected: clean (no usage yet, but the names resolve).

### Step 5.2 — Write failing tests

- [ ] **Step 3: Add tests at the end of the existing `mod tests` in `init.rs`**

Locate the `mod tests` block in `src/hooks/init.rs` (it starts around line 3800 and contains the `test_codex_*` etc. tests). At the **end** of that module (just before its closing `}`), append:

```rust
    // ── Grok integration tests ─────────────────────────────────

    #[test]
    fn test_run_grok_at_writes_hook_json_and_grok_md() {
        let temp = TempDir::new().unwrap();
        run_grok_at(temp.path(), false, InitContext::default()).unwrap();

        let hook_json = temp.path().join("hooks").join("rtk.json");
        assert!(hook_json.exists(), "hook JSON not written");
        let content = fs::read_to_string(&hook_json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed.pointer("/hooks/PreToolUse/0/matcher")
                .and_then(|v| v.as_str()),
            Some("Bash")
        );
        assert_eq!(
            parsed.pointer("/hooks/PreToolUse/0/hooks/0/command")
                .and_then(|v| v.as_str()),
            Some("rtk hook grok")
        );

        let grok_md = temp.path().join("GROK.md");
        assert!(grok_md.exists(), "GROK.md not written");
    }

    #[test]
    fn test_run_grok_at_hook_only_skips_grok_md() {
        let temp = TempDir::new().unwrap();
        run_grok_at(temp.path(), true, InitContext::default()).unwrap();

        assert!(temp.path().join("hooks").join("rtk.json").exists());
        assert!(!temp.path().join("GROK.md").exists());
    }

    #[test]
    fn test_run_grok_at_is_idempotent() {
        let temp = TempDir::new().unwrap();
        run_grok_at(temp.path(), false, InitContext::default()).unwrap();
        // Second call must succeed and leave state untouched.
        run_grok_at(temp.path(), false, InitContext::default()).unwrap();

        let hook_json = temp.path().join("hooks").join("rtk.json");
        let content = fs::read_to_string(&hook_json).unwrap();
        assert!(content.contains("rtk hook grok"));
    }

    #[test]
    fn test_run_grok_at_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        run_grok_at(
            temp.path(),
            false,
            InitContext { verbose: 0, dry_run: true },
        )
        .unwrap();

        assert!(!temp.path().join("hooks").join("rtk.json").exists());
        assert!(!temp.path().join("GROK.md").exists());
    }

    #[test]
    fn test_run_grok_rejects_local_install() {
        let err = run_grok(false, false, InitContext::default()).unwrap_err();
        assert!(
            err.to_string().contains("global-only"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_uninstall_grok_at_removes_files() {
        let temp = TempDir::new().unwrap();
        run_grok_at(temp.path(), false, InitContext::default()).unwrap();

        let removed = uninstall_grok_at(temp.path(), InitContext::default()).unwrap();
        assert_eq!(removed.len(), 2);
        assert!(!temp.path().join("hooks").join("rtk.json").exists());
        assert!(!temp.path().join("GROK.md").exists());
        // The hooks/ directory itself must NOT be removed (user may have other hooks).
        assert!(temp.path().join("hooks").exists());
    }

    #[test]
    fn test_uninstall_grok_at_is_idempotent() {
        let temp = TempDir::new().unwrap();
        // Uninstall without prior install — must succeed.
        let removed = uninstall_grok_at(temp.path(), InitContext::default()).unwrap();
        assert!(removed.is_empty());
    }
```

- [ ] **Step 4: Run the new tests — they should fail to compile**

Run: `cargo test --lib hooks::init::tests::test_run_grok`
Expected: compile errors: `run_grok_at`, `run_grok`, `uninstall_grok_at` not found. That's the red state for Task 5 and 6.

### Step 5.3 — Implement `run_grok` and `run_grok_at`

- [ ] **Step 5: Add Grok section after Gemini in `init.rs`**

Find the end of the Gemini section in `src/hooks/init.rs` (around line 3667, immediately after `fn uninstall_gemini` closes and before the Copilot section starts with `// ── Copilot integration ─────────────────────────────────────`).

Insert a new section there:

```rust

// ── Grok Build TUI support ────────────────────────────────────

fn resolve_grok_dir() -> Result<PathBuf> {
    resolve_home_subdir(GROK_DIR)
}

/// Build the hook JSON payload written to `<grok-dir>/hooks/rtk.json`.
fn grok_hook_payload() -> serde_json::Value {
    serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [
                        { "type": "command", "command": "rtk hook grok", "timeout": 5 }
                    ]
                }
            ]
        }
    })
}

/// Entry point for `rtk init --grok` (global-only).
pub fn run_grok(global: bool, hook_only: bool, ctx: InitContext) -> Result<()> {
    if !global {
        anyhow::bail!("Grok support is global-only. Use: rtk init -g --grok");
    }
    let grok_dir = resolve_grok_dir()?;
    run_grok_at(&grok_dir, hook_only, ctx)
}

/// Testable core: install Grok hook + GROK.md into the given directory.
fn run_grok_at(grok_dir: &Path, hook_only: bool, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;

    if !dry_run {
        fs::create_dir_all(grok_dir).with_context(|| {
            format!("Failed to create Grok config dir: {}", grok_dir.display())
        })?;
    }

    // 1) Write hooks/rtk.json
    let hooks_dir = grok_dir.join(HOOKS_SUBDIR);
    if !dry_run {
        fs::create_dir_all(&hooks_dir)
            .with_context(|| format!("Failed to create hook dir: {}", hooks_dir.display()))?;
    }
    let hook_json_path = hooks_dir.join(GROK_HOOK_FILENAME);
    let payload = serde_json::to_string_pretty(&grok_hook_payload())?;
    write_if_changed(&hook_json_path, &payload, "Grok hook", ctx)?;

    // 2) Install GROK.md (RTK awareness) unless --hook-only
    if !hook_only {
        let grok_md_path = grok_dir.join(GROK_MD);
        write_if_changed(&grok_md_path, RTK_SLIM, GROK_MD, ctx)?;
    }

    if dry_run {
        print_dry_run_footer();
    } else {
        println!("\nGrok hook installed (global).\n");
        println!("  Hook: {}", hook_json_path.display());
        if !hook_only {
            println!("  GROK.md: {}", grok_dir.join(GROK_MD).display());
        }
        println!("  Restart Grok. Press Ctrl+L in TUI to confirm hook loaded.");
        println!("  Test with: git status\n");
    }

    Ok(())
}
```

- [ ] **Step 6: Run installation tests (uninstall tests will still fail)**

Run: `cargo test --lib hooks::init::tests::test_run_grok`
Expected: the four `test_run_grok_*` tests **pass**; the two `test_uninstall_grok_*` tests still fail to compile because the function isn't there yet.

### Step 5.4 — Implement `uninstall_grok` and `uninstall_grok_at`

- [ ] **Step 7: Add the uninstall functions at the end of the Grok section you just created**

In the same Grok section (right after `run_grok_at`), append:

```rust

/// Entry point for `rtk init --grok --uninstall` (global-only).
fn uninstall_grok(global: bool, ctx: InitContext) -> Result<Vec<String>> {
    if !global {
        anyhow::bail!("Grok uninstall is global-only. Use: rtk init -g --grok --uninstall");
    }
    let grok_dir = match resolve_grok_dir() {
        Ok(d) => d,
        Err(_) => return Ok(Vec::new()),
    };
    uninstall_grok_at(&grok_dir, ctx)
}

/// Testable core: remove Grok artifacts from the given directory.
/// Returns a list of removed-item descriptions (empty if nothing was present).
fn uninstall_grok_at(grok_dir: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext { verbose, dry_run } = ctx;
    let mut removed = Vec::new();

    let hook_json_path = grok_dir.join(HOOKS_SUBDIR).join(GROK_HOOK_FILENAME);
    if hook_json_path.exists() {
        if dry_run {
            println!("[dry-run] would remove Grok hook: {}", hook_json_path.display());
        } else {
            fs::remove_file(&hook_json_path)
                .with_context(|| format!("Failed to remove {}", hook_json_path.display()))?;
        }
        removed.push(format!("Grok hook: {}", hook_json_path.display()));
    }

    let grok_md_path = grok_dir.join(GROK_MD);
    if grok_md_path.exists() {
        if dry_run {
            println!("[dry-run] would remove GROK.md: {}", grok_md_path.display());
        } else {
            fs::remove_file(&grok_md_path)
                .with_context(|| format!("Failed to remove {}", grok_md_path.display()))?;
        }
        removed.push(format!("GROK.md: {}", grok_md_path.display()));
    }

    if verbose > 0 && !removed.is_empty() {
        eprintln!("Grok artifacts removed");
    }

    Ok(removed)
}
```

- [ ] **Step 8: Run all Grok init tests**

Run: `cargo test --lib hooks::init::tests::test_run_grok hooks::init::tests::test_uninstall_grok`
Expected: all 7 tests pass.

- [ ] **Step 9: Run the full init test module to guard against regressions**

Run: `cargo test --lib hooks::init`
Expected: every existing test still passes.

- [ ] **Step 10: Don't commit yet**

Tasks 7-9 wire the CLI; commit after those so each commit leaves a usable CLI.

---

## Task 6: Add `--grok` flag to `Commands::Init`

**Files:**
- Modify: `src/main.rs` around lines 320-381 (the `Init { ... }` variant field block)

- [ ] **Step 1: Add the field**

In `enum Commands`, inside the `Init { ... }` variant, after `copilot: bool,` (around line 376), add:

```rust
        /// Install Grok Build TUI integration (writes ~/.grok/hooks/rtk.json + GROK.md)
        #[arg(long)]
        grok: bool,
```

- [ ] **Step 2: Verify clap accepts the new flag**

Run: `cargo build`
Expected: clean build (one new harmless `unused variable: grok` warning is OK at this point — Task 7 consumes it).

- [ ] **Step 3: Don't commit yet**

Continue to Task 7.

---

## Task 7: Wire `--grok` install dispatch in `main.rs`

**Files:**
- Modify: `src/main.rs` around lines 1790-1870 (the `Commands::Init { ... } => { ... }` body)

- [ ] **Step 1: Add `grok` to the destructuring pattern**

In `Commands::Init { ... } => {`, extend the destructuring pattern to include `grok`:

```rust
        Commands::Init {
            global,
            opencode,
            gemini,
            agent,
            show,
            claude_md,
            hook_only,
            auto_patch,
            no_patch,
            uninstall,
            codex,
            copilot,
            grok,
            dry_run,
        } => {
```

- [ ] **Step 2: Add the mutual-exclusion guard and install branch**

Inside that block, after `if show { ... }` and the existing `uninstall` branch (which becomes Task 8 below), and **before** the `else if gemini { ... }` branch, add:

```rust
            } else if grok {
                if codex || copilot || gemini || opencode || claude_md || agent.is_some() {
                    anyhow::bail!(
                        "--grok cannot be combined with another agent flag \
                         (--codex/--copilot/--gemini/--opencode/--claude-md/--agent)"
                    );
                }
                hooks::init::run_grok(global, hook_only, ctx)?;
```

The chain should now read:

```rust
            if show {
                hooks::init::show_config(codex)?;
            } else if uninstall {
                // (updated in Task 8)
            } else if grok {
                if codex || copilot || gemini || opencode || claude_md || agent.is_some() {
                    anyhow::bail!(
                        "--grok cannot be combined with another agent flag \
                         (--codex/--copilot/--gemini/--opencode/--claude-md/--agent)"
                    );
                }
                hooks::init::run_grok(global, hook_only, ctx)?;
            } else if gemini {
                // ... existing branch unchanged
```

- [ ] **Step 3: Verify it builds**

Run: `cargo build`
Expected: clean build, no warnings.

- [ ] **Step 4: Smoke-test the dry-run install**

Run: `cargo run --quiet -- init -g --grok --dry-run -v`
Expected stdout includes:

```
[dry-run] would create Grok hook: <some path>/.grok/hooks/rtk.json
[dry-run] would create GROK.md: <some path>/.grok/GROK.md

[dry-run] Nothing written.
```

If your real `~/.grok/` contains a previous test artifact and the `would create` becomes `would update`, that's still a pass — it means the dry-run sees a content diff and would have written.

- [ ] **Step 5: Don't commit yet**

Task 8 wires the uninstall path; commit after.

---

## Task 8: Extend `uninstall_init_dispatch` and `uninstall()` for Grok

**Files:**
- Modify: `src/main.rs` around lines 1362-1381 (the `uninstall_init_dispatch` helper)
- Modify: `src/main.rs` around lines 1811-1820 (the `uninstall` invocation site)
- Modify: `src/hooks/init.rs` around line 604 (the `pub fn uninstall` signature + body)

### Step 8.1 — Extend `uninstall` in `init.rs`

- [ ] **Step 1: Add a `grok: bool` parameter and dispatch**

Find this in `src/hooks/init.rs`:

```rust
pub fn uninstall(
    global: bool,
    gemini: bool,
    codex: bool,
    cursor: bool,
    ctx: InitContext,
) -> Result<()> {
```

Change the signature to:

```rust
pub fn uninstall(
    global: bool,
    gemini: bool,
    codex: bool,
    cursor: bool,
    grok: bool,
    ctx: InitContext,
) -> Result<()> {
```

Inside the function body, immediately after the existing `if codex { ... return Ok(()); }` block (around line 617, right before the `if cursor { ... }` block), insert:

```rust
    if grok {
        if !global {
            anyhow::bail!("Grok uninstall only works with --global flag");
        }
        let removed = uninstall_grok(global, ctx)?;
        let header = if dry_run {
            "[dry-run] would uninstall RTK (Grok):"
        } else {
            "RTK uninstalled (Grok):"
        };
        if removed.is_empty() {
            println!("RTK Grok support was not installed (nothing to remove)");
        } else {
            println!("{}", header);
            for item in &removed {
                println!("  - {}", item);
            }
            if !dry_run {
                println!("\nRestart Grok to apply changes.");
            }
        }
        if dry_run {
            print_dry_run_footer();
        }
        return Ok(());
    }
```

### Step 8.2 — Update `uninstall_init_dispatch` in `main.rs`

- [ ] **Step 2: Thread `grok` through the dispatch helper**

Find this in `src/main.rs`:

```rust
fn uninstall_init_dispatch<UninstallHermes, UninstallStandard>(
    agent: Option<AgentTarget>,
    global: bool,
    gemini: bool,
    codex: bool,
    ctx: hooks::init::InitContext,
    uninstall_hermes: UninstallHermes,
    uninstall_standard: UninstallStandard,
) -> Result<()>
where
    UninstallHermes: FnOnce(hooks::init::InitContext) -> Result<()>,
    UninstallStandard: FnOnce(bool, bool, bool, bool, hooks::init::InitContext) -> Result<()>,
{
    if agent == Some(AgentTarget::Hermes) {
        uninstall_hermes(ctx)
    } else {
        let cursor = agent == Some(AgentTarget::Cursor);
        uninstall_standard(global, gemini, codex, cursor, ctx)
    }
}
```

Replace with:

```rust
fn uninstall_init_dispatch<UninstallHermes, UninstallStandard>(
    agent: Option<AgentTarget>,
    global: bool,
    gemini: bool,
    codex: bool,
    grok: bool,
    ctx: hooks::init::InitContext,
    uninstall_hermes: UninstallHermes,
    uninstall_standard: UninstallStandard,
) -> Result<()>
where
    UninstallHermes: FnOnce(hooks::init::InitContext) -> Result<()>,
    UninstallStandard: FnOnce(bool, bool, bool, bool, bool, hooks::init::InitContext) -> Result<()>,
{
    if agent == Some(AgentTarget::Hermes) {
        uninstall_hermes(ctx)
    } else {
        let cursor = agent == Some(AgentTarget::Cursor);
        uninstall_standard(global, gemini, codex, cursor, grok, ctx)
    }
}
```

- [ ] **Step 3: Update both call sites**

In `src/main.rs` around line 1811, find:

```rust
                uninstall_init_dispatch(
                    agent,
                    global,
                    gemini,
                    codex,
                    ctx,
                    hooks::init::uninstall_hermes,
                    hooks::init::uninstall,
                )?;
```

Replace with:

```rust
                uninstall_init_dispatch(
                    agent,
                    global,
                    gemini,
                    codex,
                    grok,
                    ctx,
                    hooks::init::uninstall_hermes,
                    hooks::init::uninstall,
                )?;
```

The second call site is in a test block around line 2685 — find it with `rg "uninstall_init_dispatch\(" src/main.rs` and apply the same pattern: insert a `false` (or matching boolean) argument in the same position.

- [ ] **Step 4: Build and run all tests**

Run: `cargo build`
Expected: clean.

Run: `cargo test --lib`
Expected: all tests pass, including the new Grok ones and existing init tests.

- [ ] **Step 5: Smoke-test uninstall**

```bash
cargo run --quiet -- init -g --grok           # install
ls ~/.grok/hooks/rtk.json ~/.grok/GROK.md      # both should exist
cargo run --quiet -- init -g --grok --uninstall  # remove
ls ~/.grok/hooks/rtk.json ~/.grok/GROK.md 2>&1 # both should fail with "No such file"
```

Expected: install creates both files; uninstall removes both and prints `RTK uninstalled (Grok):` followed by the two paths.

- [ ] **Step 6: Commit Tasks 6-8 together**

```bash
git add src/main.rs src/hooks/init.rs
git commit -m "feat(grok): rtk init --grok install/uninstall flow

Adds CLI surface for installing Grok Build TUI integration.
Global-only (matches Gemini pattern). Writes a dedicated
~/.grok/hooks/rtk.json — no settings.json patching needed,
since Grok discovers hooks from the hooks/ directory directly.
GROK.md (RTK awareness) is reused from the Claude slim template."
```

---

## Task 9: Surface Grok in `show_config`

**Files:**
- Modify: `src/hooks/init.rs` around line 3091 (the `show_config` function)

- [ ] **Step 1: Read the current implementation**

Run: `rg -n "pub fn show_config" src/hooks/init.rs -A 40`

Identify the section that prints per-agent installation status (Claude / Gemini / Codex / etc.).

- [ ] **Step 2: Add a Grok status line**

In `show_config`, after the existing Gemini status section, add a parallel block:

```rust
    // Grok
    if let Ok(grok_dir) = resolve_grok_dir() {
        let hook_present = grok_dir.join(HOOKS_SUBDIR).join(GROK_HOOK_FILENAME).exists();
        let md_present = grok_dir.join(GROK_MD).exists();
        match (hook_present, md_present) {
            (true, true) => println!("  Grok:    ✓ hook + GROK.md ({})", grok_dir.display()),
            (true, false) => println!("  Grok:    ✓ hook only ({})", grok_dir.display()),
            (false, true) => println!("  Grok:    ⚠ GROK.md present but hook missing"),
            (false, false) => println!("  Grok:    ✗ not installed"),
        }
    }
```

Match the formatting of the existing Gemini/Codex blocks — if they use a different bullet style or column width in your local copy, copy that style. The intent is "Grok shows up in `rtk init --show` output".

- [ ] **Step 3: Build and smoke-test**

Run: `cargo run --quiet -- init --show`
Expected: output now includes a `Grok:` line reflecting current state (typically `✗ not installed` on a fresh machine).

- [ ] **Step 4: Commit**

```bash
git add src/hooks/init.rs
git commit -m "feat(grok): include Grok status in 'rtk init --show'"
```

---

## Task 10: Update README

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Find the supported-agents section**

Run: `rg -n "Gemini|Copilot|Codex" README.md | head`

Find the table or list that enumerates supported agents and their install commands.

- [ ] **Step 2: Add a Grok entry**

Insert a Grok row matching the existing format. The install command is `rtk init -g --grok`. Example fragment to add to the agents table (adapt to actual table columns):

```markdown
| Grok Build TUI | `rtk init -g --grok` | global-only | deny+suggest |
```

If the README has a longer per-agent prose section, add a Grok subsection mirroring Gemini's, with:

- Install: `rtk init -g --grok`
- Uninstall: `rtk init -g --grok --uninstall`
- Hook file: `~/.grok/hooks/rtk.json`
- RTK awareness: `~/.grok/GROK.md`
- Reload: in Grok TUI press `Ctrl+L` (or run `/hooks-list`) to confirm the hook loaded.

- [ ] **Step 3: Verify rendering**

Run: `rg -n "Grok" README.md`
Expected: at least 2-3 hits (table row + prose section).

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs(grok): document rtk init --grok in README"
```

---

## Task 11: Final gate

- [ ] **Step 1: Run the project's mandatory pre-commit pipeline (per `CLAUDE.md`)**

Run: `cargo fmt --all && cargo clippy --all-targets && cargo test --all`
Expected: zero formatting changes, zero clippy warnings, all tests pass.

If clippy complains about the new code, fix the lints inline (no `#[allow(...)]` workarounds) and add a fixup commit:

```bash
git add -u
git commit -m "chore(grok): satisfy clippy"
```

- [ ] **Step 2: End-to-end manual verification (recorded in PR description, not as automated test)**

```bash
cargo install --path .                       # install local rtk
rtk init -g --grok                            # install Grok integration
grok                                          # start Grok TUI
# In Grok, type a prompt like: "run git status"
# Expected: Grok proposes a `Bash` tool call, RTK hook fires, Grok scrollback
# shows a deny annotation with reason "Token savings: use `rtk git status` ..."
# Press Ctrl+L to confirm "rtk hook grok" appears under Global hooks.
rtk init -g --grok --uninstall                # clean up
```

Document the observed behavior in the PR description. If Grok silently ignores the deny (i.e. the tool call goes through anyway), file a Phase 2 follow-up issue per spec §8.

- [ ] **Step 3: Open the PR**

```bash
git push -u origin <branch>
gh pr create --title "feat(grok): native Grok Build TUI hook integration" --body "$(cat <<'EOF'
## Summary
- Adds `rtk init -g --grok` to install Grok Build TUI hook + GROK.md
- Adds `rtk hook grok` runtime handler (deny+suggest, mirrors Copilot CLI)
- Grok integration is global-only and uses a dedicated `~/.grok/hooks/rtk.json` (no settings.json patching)

## Design
See `docs/superpowers/specs/2026-05-17-grok-hook-integration-design.md`.

## Test plan
- [ ] `cargo test --all` passes
- [ ] Manual: install on a host with `grok` CLI, verify deny+suggest in TUI scrollback
- [ ] Manual: `rtk init --show` lists Grok status
- [ ] Manual: uninstall removes both files, retains `~/.grok/hooks/` directory

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-Review

### 1. Spec coverage

| Spec section | Task(s) covering it |
|--------------|---------------------|
| §4.1 User-facing surface — `rtk init --grok` / `--uninstall` / `--hook-only` / `--dry-run` | Tasks 6, 7, 8 (install/uninstall/flags) |
| §4.1 — `rtk hook grok` runtime | Tasks 3, 4 |
| §4.1 — `rtk hook check --agent grok` | No new task — existing `Check` handler is agent-agnostic; manual smoke step in Task 4 covers it. |
| §4.2 Hook JSON at `~/.grok/hooks/rtk.json` with `matcher: "Bash"`, `command: "rtk hook grok"`, `timeout: 5` | Task 5 (`grok_hook_payload`) |
| §4.3 Runtime handler — accept `Bash` or `run_terminal_cmd`, deny-rule path, rewrite path, pass-through path, fail-open semantics | Task 3 (process_grok_payload + 8 tests) |
| §4.4 Install flow (global-only, `--hook-only` skips GROK.md, dry-run prints footer) | Task 5 (`run_grok` + tests) |
| §4.5 Uninstall flow (remove files, keep `hooks/` dir) | Task 5 (`uninstall_grok_at` + test_uninstall_grok_at_removes_files asserts `hooks/` retained) |
| §4.6 Constants | Task 1 |
| §4.7 CLI plumbing (`--grok` flag, `HookCommands::Grok`, dispatch, uninstall_init_dispatch) | Tasks 2, 4, 6, 7, 8 |
| §5 Testing — 8 unit tests on handler, 6 tests on init/uninstall | Task 3 (8), Task 5 (6 — five `run_grok_*` plus two `uninstall_grok_*`; total 7 actually, exceeds spec) |
| §5.3 Manual verification | Task 11 step 2 |
| §6 Performance — no regression | Task 11 step 1 (full test suite + clippy) catches any obvious regression. |
| §7 Risk: alias rename | Task 3 — handler accepts both names. |
| §7 Risk: `rtk` not on PATH | Task 5 — install footer reminds user. |
| §8 Phase 2 | Task 11 step 2 — note follow-up issue, not implemented in this plan. |
| §9 Docs (README + `--show`) | Tasks 9, 10 |
| §10 Rollout — single PR, no feature flag | Task 11 step 3 (single PR) |

No spec sections without a task.

### 2. Placeholder scan

- Searched for "TBD", "TODO", "later", "appropriate", "edge cases" — none in plan body.
- Every code step includes the full code block.
- Task 9 step 2 says "Match the formatting of the existing Gemini/Codex blocks" — this is a directive to mirror style, with the canonical block provided. Acceptable because the surrounding format may vary between versions of `show_config`, and the snippet is complete.

### 3. Type consistency

- `process_grok_payload(&Value) -> Option<Value>` — used by `run_grok` and `run_grok_inner`. Consistent.
- `run_grok(global: bool, hook_only: bool, ctx: InitContext) -> Result<()>` — declared in Task 5, called in Task 7. Matches.
- `run_grok_at(grok_dir: &Path, hook_only: bool, ctx: InitContext) -> Result<()>` — declared in Task 5, used in 4 tests. Matches.
- `uninstall_grok(global: bool, ctx: InitContext) -> Result<Vec<String>>` — declared in Task 5, called in Task 8's updated `uninstall` body. Matches.
- `uninstall_grok_at(grok_dir: &Path, ctx: InitContext) -> Result<Vec<String>>` — declared in Task 5, used in 2 tests. Matches.
- `uninstall_init_dispatch` and `uninstall` signatures both extended with `grok: bool` in the same task (Task 8) — consistent at every call site.
- Constants `GROK_DIR`, `GROK_HOOK_FILENAME`, `GROK_MD` — declared in Task 1, imported in Task 5, used in Tasks 5 and 9. Consistent.
