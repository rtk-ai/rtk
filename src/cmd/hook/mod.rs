//! Hook protocol for Claude Code and Gemini support.
//!
//! This module provides **shared decision logic** for both Claude Code and Gemini CLI hooks.
//! Protocol-specific I/O handling lives in `hook/claude.rs` and `hook/gemini.rs`.
//!
//! ## Architecture: Separation of Concerns
//!
//! ```text
//! main.rs (CAN use println! - normal RTK behavior)
//!    ↓
//! Commands::Hook match
//!    ├─→ HookCommands::Check → hook::check_for_hook() (THIS MODULE - CAN use println!)
//!    ├─→ HookCommands::Claude → hook::claude::run() [DENY ENFORCED - see hook/claude.rs]
//!    └─→ HookCommands::Gemini → hook::gemini::run() [DENY ENFORCED - see hook/gemini.rs]
//! ```
//!
//! **I/O Policy Scope:**
//! - **This module (hook/mod.rs)**: CAN use `println!`/`eprintln!` (used by `rtk hook check` text protocol)
//! - **main.rs and all command modules**: CAN use `println!`/`eprintln!` (normal RTK behavior)
//! - **hook/claude.rs, hook/gemini.rs ONLY**: CANNOT use `println!`/`eprintln!` (JSON protocols)
//!
//! The `#![deny(clippy::print_stdout, clippy::print_stderr)]` attribute is applied
//! at the **module boundary** (earliest possible stage) — when control enters
//! `claude::run()` or `gemini::run()`, the deny is enforced.
//!
//! ## Protocol Differences
//!
//! **Claude Code** (`rtk hook check` text protocol):
//! - Success: rewritten command on stdout, exit 0
//! - Blocked: error message on stderr, exit 2 (blocking error)
//! - Other exit codes: non-blocking errors
//!
//! **Claude Code** (JSON protocol via `hook/claude.rs`):
//! - See `claude` module documentation
//!
//! **Gemini CLI** (JSON protocol via `hook/gemini.rs`):
//! - See `gemini` module documentation

// LLM protocol adapters
pub(crate) mod claude;

use super::{analysis, lexer};
// PR 2 adds: use super::safety;

/// Hook check result
#[derive(Debug, Clone)]
pub enum HookResult {
    /// Command is safe, rewrite to this
    Rewrite(String),
    /// Command is blocked with this message
    Blocked(String),
}

/// Maximum rewrite depth to prevent infinite recursion from cyclic safety rules.
const MAX_REWRITE_DEPTH: usize = 3;

/// Check a command for the hook protocol.
/// Returns the rewritten command or an error message.
///
/// The `_agent` parameter is reserved for future per-agent behavior.
pub fn check_for_hook(raw: &str, _agent: &str) -> HookResult {
    check_for_hook_inner(raw, 0)
}

fn check_for_hook_inner(raw: &str, depth: usize) -> HookResult {
    if depth >= MAX_REWRITE_DEPTH {
        return HookResult::Blocked("Rewrite loop detected (max depth exceeded)".to_string());
    }
    if raw.trim().is_empty() {
        return HookResult::Rewrite(raw.to_string());
    }
    // PR 2 adds: crate::config::rules::try_remap() alias expansion
    // PR 2 adds: safety::check_raw() and safety::check() dispatch

    let tokens = lexer::tokenize(raw);

    // === SUFFIX-AWARE ROUTING ===
    // Strip known safe redirect/pipe suffixes (2>&1, | tee, | head, etc.) from the
    // end of the command so the core can be routed through an RTK filter.  The suffix
    // is appended verbatim to the rewritten command; the shell applies it to rtk's output.
    //
    // Example: "cargo test 2>&1" → strip suffix → core "cargo test" → "rtk cargo test 2>&1"
    let (core_tokens, suffix) = analysis::split_safe_suffix(tokens);

    if analysis::needs_shell(&core_tokens) {
        // Core needs shell even after suffix stripping — full passthrough.
        // (When suffix is empty, core_tokens == original tokens so this is unchanged.)
        return HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(raw)));
    }

    match analysis::parse_chain(core_tokens) {
        Ok(commands) => {
            // Single command: route to optimized RTK subcommand.
            // Chained commands (&&, ||, ;): wrap entire chain in rtk run -c.
            if commands.len() == 1 {
                let routed = if suffix.is_empty() {
                    // No suffix stripped: use original raw to preserve quoting
                    try_route_native_command(&commands[0], raw)
                } else {
                    // Suffix was stripped: reconstruct core_raw from parsed command.
                    // Quoting is simplified (join with spaces) but acceptable for the
                    // common cases where suffix-bearing commands use simple args.
                    let core_raw = if commands[0].args.is_empty() {
                        commands[0].binary.clone()
                    } else {
                        format!("{} {}", commands[0].binary, commands[0].args.join(" "))
                    };
                    try_route_native_command(&commands[0], &core_raw)
                };

                match routed {
                    Some(rtk_cmd) => {
                        if suffix.is_empty() {
                            HookResult::Rewrite(rtk_cmd)
                        } else {
                            HookResult::Rewrite(format!("{} {}", rtk_cmd, suffix))
                        }
                    }
                    // Unknown command — pass through unchanged (no wrapping)
                    None => HookResult::Rewrite(raw.to_string()),
                }
            } else {
                // Multi-command chain (&&, ||, ;): wrap in shell but substitute each
                // known command with its RTK equivalent for maximum token savings.
                //
                // Example: "cargo test && git log" →
                //   rtk run -c 'rtk cargo test && rtk git log'
                //
                // Unknown commands pass through unchanged — no nested rtk run -c.
                let substituted = reconstruct_with_rtk(&commands);
                let inner = if suffix.is_empty() {
                    substituted
                } else {
                    format!("{} {}", substituted, suffix)
                };
                HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(&inner)))
            }
        }
        Err(_) => HookResult::Rewrite(raw.to_string()),
    }
}

// --- Shared guard logic (used by both claude_hook.rs and gemini_hook.rs) ---

/// Check if hook processing is disabled by environment.
///
/// Returns true if:
/// - `RTK_HOOK_ENABLED=0` (master toggle off)
/// - `RTK_ACTIVE` is set (recursion prevention — rtk sets this when running commands)
pub fn is_hook_disabled() -> bool {
    std::env::var("RTK_HOOK_ENABLED").as_deref() == Ok("0") || std::env::var("RTK_ACTIVE").is_ok()
}

/// Check if this command should bypass hook processing entirely.
///
/// Returns true for commands that should not be rewritten:
/// - Already routed through rtk (`rtk ...` or `/path/to/rtk ...`)
/// - Contains heredoc (`<<`) which needs raw shell processing
pub fn should_passthrough(cmd: &str) -> bool {
    // Already rtk or heredoc → no-op
    if cmd.starts_with("rtk ") || cmd.contains("/rtk ") || cmd.contains("<<") {
        return true;
    }
    // #196: gh --json/--jq/--template produces structured output that rtk gh
    // would corrupt. Pass through unchanged so callers get raw JSON.
    // Mirrors the guard in registry::rewrite_segment.
    if (cmd.starts_with("gh ") || cmd.contains(" gh "))
        && (cmd.contains("--json") || cmd.contains("--jq") || cmd.contains("--template"))
    {
        return true;
    }
    false
}

/// Replace the command field in a tool_input object, preserving other fields.
///
/// Used by both claude_hook.rs and gemini_hook.rs when rewriting commands.
/// If tool_input is None or not an object, creates a new object with just the command.
///
/// # Arguments
/// * `tool_input` - The original tool_input from the hook payload (may be None)
/// * `new_cmd` - The rewritten command string to replace with
///
/// # Returns
/// A Value with the command field updated, all other fields preserved.
pub fn update_command_in_tool_input(
    tool_input: Option<serde_json::Value>,
    new_cmd: String,
) -> serde_json::Value {
    use serde_json::Value;
    let mut updated = tool_input.unwrap_or_else(|| Value::Object(Default::default()));
    if let Some(obj) = updated.as_object_mut() {
        obj.insert("command".into(), Value::String(new_cmd));
    }
    updated
}

/// Hook output for protocol handlers (claude_hook.rs, gemini_hook.rs).
///
/// This enum separates decision logic from I/O: `run_inner()` returns a
/// `HookResponse`, and `run()` is the single place that writes to stdout/stderr.
/// Combined with `#[deny(clippy::print_stdout, clippy::print_stderr)]` on the
/// hook modules, this prevents any stray output from corrupting the JSON protocol.
#[derive(Debug, Clone, PartialEq)]
pub enum HookResponse {
    /// No opinion — exit 0, no output. Host proceeds normally.
    NoOpinion,
    /// Allow/rewrite — exit 0, JSON to stdout.
    Allow(String),
    /// Deny — exit 2, JSON to stdout + reason to stderr.
    /// Fields: (stdout_json, stderr_reason)
    Deny(String, String),
}

/// Escape single quotes for shell
fn escape_quotes(s: &str) -> String {
    s.replace("'", "'\\''")
}

/// Returns true if `s` looks like a shell env-var assignment: `IDENT=VALUE`.
///
/// Accepts: `FOO=bar`, `FOO=`, `_FOO=123`, `FOO_BAR=baz`
/// Rejects: `=value`, `123=abc`, plain args, flag args like `--foo=bar`
fn is_env_assign(s: &str) -> bool {
    if let Some(eq_pos) = s.find('=') {
        let key = &s[..eq_pos];
        !key.is_empty()
            && key
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    } else {
        false
    }
}

/// Replace the first occurrence of `old_prefix` in `raw` with `new_prefix`.
///
/// Preserves everything after the prefix (including original quoting).
/// Falls back to `rtk run -c '<raw>'` if prefix not found (safe degradation).
///
/// # Examples
/// - `replace_first_word("grep -r p src/", "grep", "rtk grep")` → `"rtk grep -r p src/"`
/// - `replace_first_word("rg pattern", "rg", "rtk grep")` → `"rtk grep pattern"`
fn replace_first_word(raw: &str, old_prefix: &str, new_prefix: &str) -> String {
    raw.strip_prefix(old_prefix)
        .map(|rest| format!("{new_prefix}{rest}"))
        .unwrap_or_else(|| format!("rtk run -c '{}'", escape_quotes(raw)))
}

/// Route pnpm subcommands to RTK equivalents.
///
/// Uses `cmd.args` (parsed, quote-stripped) for routing decisions.
/// Uses `raw` or reconstructed args for output to preserve original quoting.
fn route_pnpm(cmd: &analysis::NativeCommand, raw: &str) -> String {
    let sub = cmd.args.first().map(String::as_str).unwrap_or("");
    match sub {
        "list" | "ls" | "outdated" | "install" => format!("rtk {raw}"),

        // pnpm vitest [run] [flags] → rtk vitest run [flags]
        // Shell script sed bug: 's/^(pnpm )?vitest/rtk vitest run/' on
        // "pnpm vitest run --coverage" produces "rtk vitest run run --coverage".
        // Binary hook corrects this by stripping the leading "run" from parsed args.
        "vitest" => {
            let after_vitest: Vec<&str> = cmd.args[1..]
                .iter()
                .map(String::as_str)
                .skip_while(|&a| a == "run")
                .collect();
            if after_vitest.is_empty() {
                "rtk vitest run".to_string()
            } else {
                format!("rtk vitest run {}", after_vitest.join(" "))
            }
        }

        // pnpm test [flags] → rtk vitest run [flags]
        "test" => {
            let after_test: Vec<&str> = cmd.args[1..].iter().map(String::as_str).collect();
            if after_test.is_empty() {
                "rtk vitest run".to_string()
            } else {
                format!("rtk vitest run {}", after_test.join(" "))
            }
        }

        "tsc" => replace_first_word(raw, "pnpm tsc", "rtk tsc"),
        "lint" => replace_first_word(raw, "pnpm lint", "rtk lint"),
        "eslint" => replace_first_word(raw, "pnpm eslint", "rtk lint"),
        "playwright" => replace_first_word(raw, "pnpm playwright", "rtk playwright"),

        _ => format!("rtk run -c '{}'", escape_quotes(raw)),
    }
}

/// Route npx subcommands to RTK equivalents.
fn route_npx(cmd: &analysis::NativeCommand, raw: &str) -> String {
    let sub = cmd.args.first().map(String::as_str).unwrap_or("");
    match sub {
        "tsc" | "typescript" => replace_first_word(raw, &format!("npx {sub}"), "rtk tsc"),
        "eslint" => replace_first_word(raw, "npx eslint", "rtk lint"),
        "prettier" => replace_first_word(raw, "npx prettier", "rtk prettier"),
        "playwright" => replace_first_word(raw, "npx playwright", "rtk playwright"),
        "prisma" => replace_first_word(raw, "npx prisma", "rtk prisma"),

        // npx vitest [run] [flags] → rtk vitest run [flags]
        // Mirrors pnpm vitest handling: strip double-"run" if user writes "npx vitest run".
        "vitest" => {
            let after_vitest: Vec<&str> = cmd.args[1..]
                .iter()
                .map(String::as_str)
                .skip_while(|&a| a == "run")
                .collect();
            if after_vitest.is_empty() {
                "rtk vitest run".to_string()
            } else {
                format!("rtk vitest run {}", after_vitest.join(" "))
            }
        }

        _ => format!("rtk run -c '{}'", escape_quotes(raw)),
    }
}

/// Route a single parsed native command to its optimized RTK subcommand.
///
/// ## Design
/// - Uses `cmd.binary`/`cmd.args` (lexer→parse_chain output) for routing DECISIONS.
/// - Uses `raw: &str` with `replace_first_word` for string REPLACEMENT (preserves quoting).
/// - `format!("rtk {raw}")` works when the binary name equals the RTK subcommand.
/// - `replace_first_word` handles renames: `rg → rtk grep`, `cat → rtk read`.
///
/// ## Fallback
/// Unknown binaries or unrecognized subcommands → `rtk run -c '<raw>'` (safe passthrough).
///
/// ## Mirrors
/// `~/.claude/hooks/rtk-rewrite.sh` routing table. Corrects the shell script's
/// `vitest run` double-"run" bug by using parsed args rather than regex substitution.
///
/// ## Safety interaction
/// `safety::check` runs BEFORE this function. Blocked commands (cat, head, sed)
/// never reach here. The `cat` arm is defensive for when `RTK_BLOCK_TOKEN_WASTE=0`.

/// Subcommand-aware routing table for the binary hook.
/// Returns (rtk_cmd_full, prefix_to_replace) when a command should be routed to an RTK subcommand.
/// Conservative whitelist — excludes commands that are better handled by `rtk run -c`.
fn hook_lookup<'a>(binary: &'a str, sub: &str) -> Option<(&'static str, &'a str)> {
    // Extract basename for full-path binaries: /opt/homebrew/bin/gh → gh
    let base = binary.rsplit('/').next().unwrap_or(binary);
    // Match on basename but return original `binary` as prefix for replace_first_word
    match base {
        "git" => {
            // Only well-supported subcommands; others (checkout, rebase, cherry-pick) → rtk run
            match sub {
                "status" | "log" | "diff" | "show" | "add" | "commit" | "push" | "pull"
                | "fetch" | "stash" => Some(("rtk git", binary)),
                _ => None,
            }
        }
        "gh" => match sub {
            "pr" | "issue" | "run" => Some(("rtk gh", binary)),
            _ => None,
        },
        "cargo" => match sub {
            "test" | "build" | "clippy" | "check" | "install" | "fmt" => {
                Some(("rtk cargo", binary))
            }
            _ => None,
        },
        "docker" => match sub {
            "ps" | "images" | "logs" => Some(("rtk docker", binary)),
            _ => None,
        },
        "kubectl" => match sub {
            "get" | "logs" => Some(("rtk kubectl", binary)),
            _ => None,
        },
        "go" => match sub {
            "test" | "build" | "vet" => Some(("rtk go", binary)),
            _ => None,
        },
        "ruff" => match sub {
            "check" | "format" => Some(("rtk ruff", binary)),
            _ => None,
        },
        "pip" | "pip3" => match sub {
            "list" | "outdated" | "install" | "show" => Some(("rtk pip", binary)),
            _ => None,
        },
        // Rename routes: binary → rtk subcommand (different name)
        "grep" => Some(("rtk grep", binary)),
        "rg" => Some(("rtk grep", binary)),
        "ls" => Some(("rtk ls", binary)),
        "eslint" => Some(("rtk lint", binary)),
        "biome" => Some(("rtk lint", binary)),
        "tsc" => Some(("rtk tsc", binary)),
        "prettier" => Some(("rtk prettier", binary)),
        "golangci-lint" | "golangci" => Some(("rtk golangci-lint", binary)),
        "mypy" => Some(("rtk mypy", binary)),
        // Any-subcommand direct routes
        "playwright" => Some(("rtk playwright", binary)),
        "prisma" => Some(("rtk prisma", binary)),
        "curl" => Some(("rtk curl", binary)),
        "pytest" => Some(("rtk pytest", binary)),
        "wc" => Some(("rtk wc", binary)),
        // Graphite CLI — all subcommands route through RTK for token optimization
        "gt" => Some(("rtk gt", binary)),
        "wget" | "diff" | "tree" | "find" => None, // passthrough: builtins_not_blocked
        _ => None,
    }
}

/// Returns true if the token is a shell prefix builtin that modifies the
/// execution of the following command (e.g. `noglob`, `command`, `nocorrect`).
/// These builtins are NOT standalone executables — they must stay in shell context.
fn is_shell_prefix_builtin(token: &str) -> bool {
    matches!(
        token,
        "noglob" | "command" | "builtin" | "exec" | "nocorrect"
    )
}

pub(crate) fn route_native_command(cmd: &analysis::NativeCommand, raw: &str) -> String {
    // === SHELL PREFIX BUILTIN STRIPPING ===
    // When the "binary" is a shell prefix builtin (noglob, command, etc.),
    // strip it, route the real command, and re-prepend the prefix.
    // These builtins modify execution context — they are NOT executables
    // and cannot be wrapped in `rtk run -c`.
    //
    // Example: "noglob gh release create v0.3.0-rc1 --title ..."
    //   → prefix="noglob", real_binary="gh", args=["release","create",...]
    //   → route "gh release create ..." → "rtk gh release create ..."
    //   → result: "noglob rtk gh release create v0.3.0-rc1 --title ..."
    if is_shell_prefix_builtin(&cmd.binary) {
        if let Some(real_binary) = cmd.args.first() {
            let prefix = &cmd.binary;
            let real_args = cmd.args[1..].to_vec();
            let real_cmd = analysis::NativeCommand {
                binary: real_binary.clone(),
                args: real_args,
                operator: cmd.operator.clone(),
            };
            let core_raw = raw
                .strip_prefix(prefix)
                .map(|s| s.trim_start())
                .unwrap_or(raw);
            return match try_route_native_command(&real_cmd, core_raw) {
                Some(routed) => format!("{} {}", prefix, routed),
                None => raw.to_string(), // Unknown cmd — pass through with prefix intact
            };
        }
        // Bare prefix with no following command — pass through unchanged
        return raw.to_string();
    }

    // === ENV PREFIX STRIPPING ===
    // When the "binary" is actually a VAR=val env assignment (e.g. "GIT_PAGER=cat"),
    // collect all leading env assigns, find the real binary in args, route it, and
    // prepend the env vars so the shell sets them for the rtk subprocess.
    //
    // Example: "GIT_PAGER=cat git status"
    //   → env_prefix="GIT_PAGER=cat", real_binary="git", args=["status"]
    //   → route "git status" → "rtk git status"
    //   → result: "GIT_PAGER=cat rtk git status"
    if is_env_assign(&cmd.binary) {
        let mut env_parts: Vec<&str> = vec![cmd.binary.as_str()];
        let mut arg_idx = 0;
        while arg_idx < cmd.args.len() && is_env_assign(&cmd.args[arg_idx]) {
            env_parts.push(&cmd.args[arg_idx]);
            arg_idx += 1;
        }
        if arg_idx < cmd.args.len() {
            let env_prefix_str = env_parts.join(" ");
            // Strip env prefix from raw to get core_raw, preserving original quoting.
            let core_raw = raw
                .strip_prefix(&env_prefix_str)
                .map(|s| s.trim_start())
                .unwrap_or_else(|| {
                    // Fallback: count the env prefix length and skip past it
                    let skip = env_prefix_str.len();
                    if skip < raw.len() {
                        raw[skip..].trim_start()
                    } else {
                        raw
                    }
                });
            let real_binary = cmd.args[arg_idx].clone();
            let real_args = cmd.args[arg_idx + 1..].to_vec();
            let real_cmd = analysis::NativeCommand {
                binary: real_binary,
                args: real_args,
                operator: cmd.operator.clone(),
            };
            return match try_route_native_command(&real_cmd, core_raw) {
                Some(routed) => format!("{} {}", env_prefix_str, routed),
                None => raw.to_string(), // Unknown cmd — pass through with env prefix intact
            };
        }
        // All tokens are env assigns (no real command) — fall through to passthrough
    }

    let sub = cmd.args.first().map(String::as_str).unwrap_or("");
    let sub2 = cmd.args.get(1).map(String::as_str).unwrap_or("");

    // 1. Static routing table: subcommand-aware whitelist (hook_lookup).
    //    More conservative than classify_command (discovery) — only routes
    //    commands/subcommands that RTK optimizes well.
    if let Some((rtk_full, prefix)) = hook_lookup(&cmd.binary, sub) {
        return replace_first_word(raw, prefix, rtk_full);
    }

    // 2. Complex cases that require Rust logic and cannot be expressed as table entries.

    // cat: blocked by safety rules before reaching here; defensive for RTK_BLOCK_TOKEN_WASTE=0
    if cmd.binary == "cat" {
        return replace_first_word(raw, "cat", "rtk read");
    }

    match cmd.binary.as_str() {
        // vitest: bare invocation → rtk vitest run (not rtk vitest)
        "vitest" if sub.is_empty() => "rtk vitest run".to_string(),
        "vitest" => format!("rtk {raw}"),

        // uv pip: two-word prefix replacement
        "uv" if sub == "pip" && matches!(sub2, "list" | "outdated" | "install" | "show") => {
            replace_first_word(raw, "uv pip", "rtk pip")
        }

        // python/python3 -m pytest: two-arg prefix replacement
        "python" | "python3" if sub == "-m" && sub2 == "pytest" => {
            let prefix = format!("{} -m pytest", cmd.binary);
            replace_first_word(raw, &prefix, "rtk pytest")
        }

        // python/python3 -m mypy: two-arg prefix replacement
        "python" | "python3" if sub == "-m" && sub2 == "mypy" => {
            let prefix = format!("{} -m mypy", cmd.binary);
            replace_first_word(raw, &prefix, "rtk mypy")
        }

        // pnpm / npx: delegated to helpers (complex sub-routing)
        "pnpm" => route_pnpm(cmd, raw),
        "npx" => route_npx(cmd, raw),

        // Fallback: unknown binary or unrecognized subcommand
        _ => format!("rtk run -c '{}'", escape_quotes(raw)),
    }
}
/// Try to route a single command to its optimised RTK subcommand.
///
/// Returns `Some(rtk_cmd)` when the command is natively routable (direct or renamed).
/// Returns `None` when the command would fall back to `rtk run -c '...'` passthrough —
/// the caller should keep the original `raw` string unchanged in that case.
///
/// This avoids embedding nested `rtk run -c` calls inside an outer shell string,
/// which would require double-escaping and never improves token savings.
pub(crate) fn try_route_native_command(cmd: &analysis::NativeCommand, raw: &str) -> Option<String> {
    let routed = route_native_command(cmd, raw);
    if routed.starts_with("rtk run -c") {
        None // passthrough — keep original
    } else {
        Some(routed)
    }
}

/// Substitute RTK commands within a multi-command chain string.
///
/// Iterates each command in the parsed chain.  Known commands (those with an RTK
/// subcommand equivalent) are replaced with their `rtk <cmd>` form.  Unknown commands
/// are kept verbatim so the shell can handle them.  Operators (`&&`, `||`, `;`) are
/// preserved between commands.
///
/// # Why this is safe
/// Only `&&`/`||`/`;` chains reach this function (pipe characters trigger `needs_shell`
/// before `parse_chain`, so pipes never appear here).  Each command's stdout is
/// independent — no cross-command parsing is affected by RTK's output format changes.
///
/// # Example
/// ```text
/// "cargo test && git log $BRANCH"
///   cmd[0]: binary="cargo" args=["test"] op=Some("&&")  → "rtk cargo test"
///   cmd[1]: binary="git"   args=["log","$BRANCH"] op=None → "rtk git log $BRANCH"
///   result: "rtk cargo test && rtk git log $BRANCH"
/// ```
fn reconstruct_with_rtk(commands: &[analysis::NativeCommand]) -> String {
    commands
        .iter()
        .map(|cmd| {
            // Reconstruct the core raw string from parsed binary + args.
            // Quote-stripping in parse_chain means we lose original quoting here,
            // but this is acceptable for the common cases (simple args, no spaces).
            let core_raw = if cmd.args.is_empty() {
                cmd.binary.clone()
            } else {
                format!("{} {}", cmd.binary, cmd.args.join(" "))
            };

            // Route if known; otherwise preserve the original core_raw verbatim.
            let part = match try_route_native_command(cmd, &core_raw) {
                Some(routed) => routed,
                None => core_raw,
            };

            // Append operator if present (all but the last command have one).
            match &cmd.operator {
                Some(op) => format!("{} {}", part, op),
                None => part,
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Format hook result for Claude (text output)
///
/// Exit codes:
/// - 0: Success, command rewritten/allowed
/// - 2: Blocking error, command should be denied
pub fn format_for_claude(result: HookResult) -> (String, bool, i32) {
    match result {
        HookResult::Rewrite(cmd) => (cmd, true, 0),
        HookResult::Blocked(msg) => (msg, false, 2), // Exit 2 = blocking error per Claude Code spec
    }
}
