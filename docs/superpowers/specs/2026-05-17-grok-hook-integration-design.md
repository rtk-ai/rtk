# RTK Grok Hook Integration — Design Spec

**Date**: 2026-05-17
**Status**: Approved (pending implementation plan)

## 1. Goal

Add native support for [Grok Build TUI](https://github.com/xai-org/grok-cli) (xAI's coding agent CLI) so that when Grok executes shell commands (e.g. `git status`), RTK intercepts via Grok's hook system and suggests a token-optimized rewrite (e.g. `rtk git status`), saving 60-90% tokens on common dev operations.

## 2. Non-Goals

- Project-scoped hook (`<project>/.grok/hooks/`) — global-only in this iteration, matches Gemini pattern.
- HTTP-type hook (`{"type":"http",...}`) — no user demand.
- Installing RTK as a Grok skill (`~/.grok/skills/rtk/`) — RTK awareness is delivered via `GROK.md` like `GEMINI.md`.
- Transparent rewrite via `updatedInput` field — Grok docs do not promise this; reserved for Phase 2 after empirical verification.

## 3. Background

### 3.1 Grok hook system (relevant facts)

Source: `~/.grok/docs/user-guide/10-hooks.md`.

- Hooks are discovered from `~/.grok/hooks/*.json` (global, always trusted) and `<project>/.grok/hooks/*.json` (per-project, requires explicit `/hooks-trust`).
- Grok also reads `~/.claude/settings.json` for Claude Code compatibility.
- Hook JSON schema is identical to Claude Code:
  ```json
  {
    "hooks": {
      "PreToolUse": [
        { "matcher": "Bash", "hooks": [{ "type": "command", "command": "...", "timeout": 5 }] }
      ]
    }
  }
  ```
- Tool name aliases: `Bash` → `run_terminal_cmd`, `Edit` → `search_replace`, `Read` → `read_file`. The matcher accepts the Claude-style name.
- Stdin payload uses **camelCase** keys:
  ```json
  {
    "hookEventName": "pre_tool_use",
    "toolName": "run_terminal_cmd",
    "toolInput": { "command": "npm test" },
    "sessionId": "...", "cwd": "...", "workspaceRoot": "...", "timestamp": "..."
  }
  ```
- Output protocol (documented):
  - Allow: `{"decision":"allow"}` (or empty stdout)
  - Deny: `{"decision":"deny","reason":"..."}`
  - Any other failure (timeout, crash, malformed) is **fail-open** — recorded for UI scrollback but does not block.
  - Only an explicit `{"decision":"deny",...}` blocks a tool call.

### 3.2 Existing RTK agent integrations

| Agent | Init flag | Runtime command | Wire format | Hook file location |
|-------|-----------|-----------------|-------------|--------------------|
| Claude Code | `rtk init` (default) | `rtk hook claude` | snake_case keys, `hookSpecificOutput.updatedInput` rewrite | `~/.claude/settings.json` |
| Cursor | `rtk init --agent cursor` | `rtk hook cursor` | snake_case `tool_input`, `updated_input` rewrite | `.cursor/.../settings.json` |
| Gemini CLI | `rtk init -g --gemini` | `rtk hook gemini` | snake_case, `decision:allow + hookSpecificOutput.tool_input` rewrite | `~/.gemini/settings.json` |
| Copilot CLI | `rtk init --copilot` | `rtk hook copilot` | camelCase `toolName` + `toolArgs` (JSON string), `deny + suggest` | (per Copilot docs) |
| Codex | `rtk init --codex` | (no runtime hook; uses AGENTS.md) | n/a | `~/.codex/` |

Grok slots between Gemini and Copilot CLI:
- Like Gemini: global-only, hook file lives under agent's home dir.
- Like Copilot CLI: documented protocol is allow/deny only — no `updatedInput` rewrite.
- Unlike both: hook file is a standalone `~/.grok/hooks/*.json`, not a patch to `settings.json`. No JSON-merge logic needed.

## 4. Design

### 4.1 User-facing surface

```bash
# Install Grok hook + GROK.md (global)
rtk init -g --grok

# Hook only (skip GROK.md)
rtk init -g --grok --hook-only

# Dry-run
rtk init -g --grok --dry-run -v

# Uninstall
rtk init -g --grok --uninstall

# Runtime (invoked by Grok)
rtk hook grok            # reads JSON payload from stdin

# Dry-run a command rewrite
rtk hook check --agent grok git status
```

Flag rules:
- `--grok` is global-only. `rtk init --grok` (no `-g`) errors with: *"Grok support is global-only. Use: rtk init -g --grok"*.
- `--grok` mutually exclusive with `--gemini`, `--codex`, `--copilot`, `--opencode`, `--agent <name>`. Conflict produces the same style error as existing flags.
- `--hook-only`, `--dry-run`, `--uninstall` are honored as with other agents.
- `--auto-patch` / `--no-patch` are accepted but ignored for Grok (no settings.json patching). Documented in `--show` output.

### 4.2 Hook JSON installed at `~/.grok/hooks/rtk.json`

```json
{
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
}
```

- File is owned exclusively by RTK — `rtk init --grok` always overwrites if content drifts (via `write_if_changed`), and `rtk init --grok --uninstall` removes the entire file.
- Inline `command: "rtk hook grok"` rather than a wrapper script. This matches Copilot CLI's pattern; Gemini's wrapper-script approach is historical (it gives Gemini a stable path to patch into `settings.json`). Grok writes its own dedicated `rtk.json`, so the indirection is unnecessary. PATH dependency is documented in §7 risks.
- `matcher: "Bash"` relies on Grok's documented alias mapping (`Bash` → `run_terminal_cmd`); runtime handler also accepts the canonical `run_terminal_cmd` tool name for forward-compatibility.

### 4.3 Runtime handler — `src/hooks/hook_cmd.rs::run_grok`

Behavior (mirrors `handle_copilot_cli`, adapted to Grok payload schema):

1. Read stdin (cap 1 MiB, reuse `read_stdin_limited`).
2. Parse JSON. On parse failure, log to stderr and exit 0 — fail-open allow.
3. Extract `toolName`. If not `"Bash"` or `"run_terminal_cmd"`, exit silently — pass-through.
4. Extract `toolInput.command`. If missing/empty, exit silently — pass-through.
5. Run `permissions::check_command`. If `Deny`, emit `{"decision":"deny","reason":"Blocked by RTK permission rule"}` and exit. Audit-log as `deny`.
6. Run `get_rewritten` (already wraps `has_heredoc` check, exclude list, transparent prefixes). If no rewrite or rewrite equals input, exit silently — pass-through.
7. Audit-log as `rewrite`.
8. Emit:
   ```json
   {"decision":"deny","reason":"Token savings: use `<rewritten>` instead (rtk saves 60-90% tokens)"}
   ```
   to stdout. Exit 0.

Error-handling rule: any error path that is not an explicit deny **must** exit without emitting `{"decision":"deny",...}` on stdout, so Grok's fail-open semantics let the original command through. This matches the RTK fallback principle (never block the user on a filter bug).

### 4.4 Install flow — `src/hooks/init.rs::run_grok`

Pseudo-code:

```rust
pub fn run_grok(global: bool, hook_only: bool, ctx: InitContext) -> Result<()> {
    if !global {
        anyhow::bail!("Grok support is global-only. Use: rtk init -g --grok");
    }
    let InitContext { dry_run, .. } = ctx;

    let grok_dir = resolve_home_subdir(GROK_DIR)?;            // ~/.grok
    let hooks_dir = grok_dir.join(HOOKS_SUBDIR);               // ~/.grok/hooks
    if !dry_run { fs::create_dir_all(&hooks_dir)?; }

    // 1) Write ~/.grok/hooks/rtk.json
    let hook_json_path = hooks_dir.join(GROK_HOOK_FILENAME);   // "rtk.json"
    let content = serde_json::to_string_pretty(&grok_hook_payload())?;
    write_if_changed(&hook_json_path, &content, "Grok hook", ctx)?;

    // 2) Optionally install GROK.md
    if !hook_only {
        let grok_md_path = grok_dir.join(GROK_MD);             // "GROK.md"
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

### 4.5 Uninstall flow — `src/hooks/init.rs::uninstall_grok`

Remove (existence check first, ignore not-found):
- `~/.grok/hooks/rtk.json`
- `~/.grok/GROK.md`
- Integrity hash entry (if any)

Do **not** remove `~/.grok/hooks/` directory itself (user may have other hooks).

### 4.6 Constants added to `src/hooks/constants.rs`

```rust
pub const GROK_DIR: &str = ".grok";
pub const GROK_HOOK_FILENAME: &str = "rtk.json";
pub const GROK_MD: &str = "GROK.md";
```

`HOOKS_SUBDIR` (`"hooks"`) already exists and is reused.

### 4.7 CLI plumbing — `src/main.rs`

- Add field to `Commands::Init { ... }`:
  ```rust
  /// Install Grok Build TUI integration (writes ~/.grok/hooks/rtk.json + GROK.md)
  #[arg(long)]
  grok: bool,
  ```
- Add variant to `HookCommands`:
  ```rust
  /// Process Grok PreToolUse hook (reads JSON from stdin)
  Grok,
  ```
- Dispatch in `Commands::Init`:
  - Before the `gemini` branch: if `grok` is true, validate mutual exclusion with all other agent flags (`gemini`, `codex`, `copilot`, `opencode`, `claude_md`, `agent.is_some()`), then call `hooks::init::run_grok(global, hook_only, ctx)`.
  - Note: `auto_patch` / `no_patch` are accepted silently; Grok needs no settings.json patch.
- Dispatch in `Commands::Hook`: `HookCommands::Grok => hooks::hook_cmd::run_grok()?`.
- Dispatch in `HookCommands::Check`: existing logic uses `rewrite_command` agnostic of agent — no change needed.
- `uninstall_init_dispatch`: extend to handle `grok` flag analogous to `gemini`/`codex`.

## 5. Testing

All tests follow existing patterns (`src/hooks/hook_cmd.rs::tests` and `src/hooks/init.rs::tests`).

### 5.1 Unit tests — `run_grok` (hook_cmd.rs)

| Test | Setup | Expect |
|------|-------|--------|
| `test_grok_rewrites_bash` | payload `{toolName: "Bash", toolInput: {command: "git status"}}` | stdout = `{"decision":"deny","reason":"Token savings: use `rtk git status` instead..."}` |
| `test_grok_accepts_run_terminal_cmd_alias` | `toolName: "run_terminal_cmd"`, same command | same output as above |
| `test_grok_passes_non_bash_tool` | `toolName: "read_file"` | empty stdout, exit 0 |
| `test_grok_passes_excluded_command` | `command: "rtk git status"` (already prefixed) | empty stdout, exit 0 |
| `test_grok_passes_heredoc` | `command: "cat <<EOF\nfoo\nEOF"` | empty stdout, exit 0 |
| `test_grok_deny_rule_blocks` | install deny rule for `git push --force`; payload contains that | stdout = `{"decision":"deny","reason":"Blocked by RTK permission rule"}` |
| `test_grok_empty_command_ignored` | `toolInput: {command: ""}` | empty stdout |
| `test_grok_malformed_payload` | stdin = `"not json"` | empty stdout (fail-open), exit 0 |
| `test_grok_missing_tool_input` | `toolName: "Bash"`, no `toolInput` | empty stdout |

### 5.2 Unit tests — `init::run_grok`

| Test | Setup | Expect |
|------|-------|--------|
| `test_init_grok_writes_hook_json` | `run_grok(global=true, hook_only=true, ctx)` | `~/.grok/hooks/rtk.json` exists with expected JSON |
| `test_init_grok_writes_grok_md` | `hook_only=false` | `~/.grok/GROK.md` exists with RTK_SLIM content |
| `test_init_grok_idempotent` | call twice | no second write, no error |
| `test_init_grok_local_errors` | `global=false` | returns error containing `"global-only"` |
| `test_uninstall_grok_removes_files` | install then uninstall | both files removed, `~/.grok/hooks/` retained |
| `test_uninstall_grok_missing_files_ok` | uninstall without prior install | succeeds, no error |
| `test_init_grok_dry_run` | `dry_run=true` | files **not** written, footer printed |

### 5.3 Integration / smoke

- `bash scripts/test-all.sh` continues to pass (no Grok-specific assertion; just confirm no regression).
- Manual verification: install `rtk init -g --grok`, start `grok`, run `git status`, expect deny+suggest message in TUI scrollback. Document in PR description, not as automated test.

### 5.4 Token accuracy

Not applicable — hook handler doesn't compress output; it only emits a deny+suggest JSON. Token savings are realized when the user actually re-runs `rtk git status`, which is already covered by `git` filter tests.

## 6. Performance

- `run_grok` is invoked per Bash tool call; latency target is the same as other hooks: <10ms cold start.
- Reuses already-loaded regex (lazy_static), permissions cache (loaded once per invocation), and config.
- No new dependencies.

## 7. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Grok renames the `Bash` matcher alias | Low | High (hook stops firing) | Runtime handler accepts both `Bash` and `run_terminal_cmd`; integration test runbook in PR description includes alias verification. |
| Grok adds undocumented `hookSpecificOutput.updatedInput` support and users expect transparent rewrite | Medium | Low | Phase 2 task carved out in implementation plan; current behavior matches documented protocol. |
| User has both Claude Code and Grok hooks active (since Grok reads `~/.claude/settings.json`) | High | Low | Document precedence in README. RTK rewrite is idempotent (`rtk rtk git status` exclude-listed), and `deny+suggest` from one path is benign if the other path also fires — Grok only blocks on the first explicit deny. |
| User has unrelated JSON files in `~/.grok/hooks/` | High | None | RTK uses a dedicated `rtk.json` filename; never merges with other JSON. |
| `rtk` not on PATH for Grok's child process | Low | High (hook command not found) | Install footer reminds user that Grok must inherit a PATH containing `rtk`; same constraint as Gemini integration today. |

## 8. Phase 2 (out of scope, but planned)

After this lands, run a manual probe:
1. Modify a local build of `run_grok` to also emit `hookSpecificOutput.updatedInput`.
2. Test whether Grok respects it (the tool call becomes `rtk <cmd>` transparently).
3. If yes, switch the wire format to Claude-style and remove the deny+suggest text. If no, keep current behavior.

This task is **not** part of the implementation plan for this spec — it requires empirical data first.

## 9. Documentation updates

- `README.md` — add Grok to supported-agents matrix.
- `CLAUDE.md` (project) — no change (RTK code).
- New section in user docs (if any agent-specific doc tree exists) — defer until other agents have one.
- `--show` output of `rtk init` — list Grok if installed (extend `show_config` to detect `~/.grok/hooks/rtk.json`).

## 10. Rollout

Single PR. No feature flag — install path is opt-in (`rtk init -g --grok`). Existing users unaffected.
