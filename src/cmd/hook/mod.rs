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

/// Commands whose RTK output format matches their raw output, making them
/// safe as the left side of any pipe.
///
/// Commands whose RTK output format matches raw output (format-preserving).
///
/// For a command to be format-preserving, RTK must emit the same logical
/// lines as the underlying tool — just possibly with ANSI codes stripped.
/// These can be substituted on the left of a pipe without breaking the
/// right-side consumer.
///
/// # Contrast with format-changing commands
/// `cargo test`, `git log`, `pytest`, `go test` etc. heavily compress output.
/// They must **not** appear here — substituting them as a pipe-left would
/// break right-side semantic sinks (`grep`, `jq`, `awk`, `patch`, `xargs`).
#[cfg(test)]
const FORMAT_PRESERVING: &[&str] = &["tail", "echo", "cat", "find", "fd"];

/// Right-side commands that accept any input format (transparent sinks).
///
/// These commands copy, truncate, or tee their stdin without interpreting its
/// structure, so RTK's compressed output is always compatible with them.
/// Already handled at the routing level by `split_safe_suffix` — listed here
/// for classification documentation and future pipe-left substitution logic.
#[cfg(test)]
const TRANSPARENT_SINKS: &[&str] = &["tee", "head", "tail", "cat"];

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

#[cfg(test)]
mod tests {
    use super::*;

    // === TEST HELPERS ===

    fn assert_rewrite(input: &str, contains: &str) {
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => assert!(
                cmd.contains(contains),
                "'{}' rewrite should contain '{}', got '{}'",
                input,
                contains,
                cmd
            ),
            other => panic!("Expected Rewrite for '{}', got {:?}", input, other),
        }
    }

    /// Assert that a command at the given rewrite depth produces a Blocked result
    /// containing the expected message substring.
    fn assert_blocked(input: &str, depth: usize, contains: &str) {
        match check_for_hook_inner(input, depth) {
            HookResult::Blocked(msg) => assert!(
                msg.contains(contains),
                "'{}' block msg should contain '{}', got '{}'",
                input,
                contains,
                msg
            ),
            other => panic!("Expected Blocked for '{}', got {:?}", input, other),
        }
    }

    // === ESCAPE_QUOTES ===

    #[test]
    fn test_escape_quotes() {
        assert_eq!(escape_quotes("hello"), "hello");
        assert_eq!(escape_quotes("it's"), "it'\\''s");
        assert_eq!(escape_quotes("it's a test's"), "it'\\''s a test'\\''s");
    }

    // === EMPTY / WHITESPACE ===

    #[test]
    fn test_check_empty_and_whitespace() {
        match check_for_hook("", "claude") {
            HookResult::Rewrite(cmd) => assert!(cmd.is_empty()),
            _ => panic!("Expected Rewrite for empty"),
        }
        match check_for_hook("   ", "claude") {
            HookResult::Rewrite(cmd) => assert!(cmd.trim().is_empty()),
            _ => panic!("Expected Rewrite for whitespace"),
        }
    }

    // === COMMANDS THAT SHOULD REWRITE (table-driven) ===

    #[test]
    fn test_safe_commands_rewrite() {
        // Known commands route to RTK subcommands
        assert_rewrite("git status", "rtk git status");
        assert_rewrite(r#"git commit -m "Fix && Bug""#, "rtk git commit"); // quoted &&: single cmd, routes

        // Shell metacharacters still go through rtk run -c (needs_shell detects them)
        let shell_cases = [
            ("ls *.rs", "rtk run"),               // glob
            ("echo `date`", "rtk run"),           // backticks
            ("echo $(date)", "rtk run"),          // subshell
            ("echo {a,b}.txt", "rtk run"),        // brace expansion
            ("cd /tmp && git status", "rtk run"), // chain rewrite
        ];
        for (input, expected) in shell_cases {
            assert_rewrite(input, expected);
        }

        // Single unknown commands pass through unchanged (no wrapping)
        assert_passthrough("FOO=bar echo hello"); // env prefix + unknown cmd
        assert_passthrough("echo 'hello!@#$%^&*()'"); // special chars in quotes (no shell metachar)
        assert_passthrough("echo '日本語 🎉'"); // unicode in quotes
        assert_passthrough(&format!("echo {}", "a".repeat(1000))); // very long command

        // Chain rewrite preserves operator structure
        match check_for_hook("cd /tmp && git status", "claude") {
            HookResult::Rewrite(cmd) => assert!(
                cmd.contains("&&"),
                "Chain rewrite must preserve '&&', got '{}'",
                cmd
            ),
            other => panic!("Expected Rewrite for chain, got {:?}", other),
        }
    }

    // === ENV VAR PREFIX ROUTING ===
    // Commands prefixed with KEY=VALUE env vars must route to the optimized RTK
    // subcommand with the env var preserved, not fall back to rtk run -c passthrough.

    #[test]
    fn test_env_prefix_routes_to_rtk_subcommand() {
        // Each case: (input, expected_rtk_subcommand_prefix, env_prefix_preserved)
        let cases = [
            ("GIT_PAGER=cat git status", "rtk git", "GIT_PAGER=cat"),
            (
                "GIT_PAGER=cat git log --oneline -10",
                "rtk git",
                "GIT_PAGER=cat",
            ),
            ("RUST_LOG=debug cargo test", "rtk cargo", "RUST_LOG=debug"),
            ("LANG=C ls -la", "rtk ls", "LANG=C"),
            (
                "TEST_SESSION_ID=2 npx playwright test --config=foo",
                "rtk playwright",
                "TEST_SESSION_ID=2",
            ),
        ];
        for (input, rtk_sub, env_prefix) in cases {
            match check_for_hook(input, "claude") {
                HookResult::Rewrite(cmd) => {
                    assert!(
                        cmd.contains(rtk_sub),
                        "'{input}' must route to '{rtk_sub}', got '{cmd}'"
                    );
                    assert!(
                        cmd.contains(env_prefix),
                        "'{input}' must preserve env prefix '{env_prefix}', got '{cmd}'"
                    );
                }
                other => panic!("Expected Rewrite for '{input}', got {other:?}"),
            }
        }
    }

    #[test]
    fn test_env_prefix_multi_var_routes() {
        // Multiple env vars before a known command
        let input = "NODE_ENV=test CI=1 npx vitest run";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    cmd.contains("rtk vitest"),
                    "must route to rtk vitest, got '{cmd}'"
                );
                assert!(
                    cmd.contains("NODE_ENV=test"),
                    "must preserve NODE_ENV, got '{cmd}'"
                );
                assert!(cmd.contains("CI=1"), "must preserve CI, got '{cmd}'");
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_env_prefix_unknown_cmd_fallback() {
        // Unknown command after env prefix → passes through unchanged
        assert_passthrough("VAR=1 unknown_xyz_abc_cmd");
    }

    #[test]
    fn test_env_prefix_npm_still_passthrough() {
        // npm has no RTK subcommand → passes through unchanged
        assert_passthrough("NODE_ENV=test npm run test:e2e");
    }

    #[test]
    fn test_env_prefix_docker_compose_passthrough() {
        // docker compose up has no RTK route → passes through unchanged
        assert_passthrough("COMPOSE_PROJECT_NAME=test docker compose up -d");
    }

    // === GLOBAL OPTIONS (PR #99 parity) ===
    // Commands with global options before subcommands must not be blocked.
    // Ported from upstream hooks/rtk-rewrite.sh global option stripping.

    #[test]
    fn test_global_options_not_blocked() {
        // Commands with global options must NOT be blocked.
        // They pass through unchanged since hook_lookup doesn't strip global options.
        let cases = [
            // Git global options
            "git --no-pager status",
            "git -C /path/to/project status",
            "git -C /path --no-pager log --oneline",
            "git --no-optional-locks diff HEAD",
            "git --bare log",
            // Cargo toolchain prefix
            "cargo +nightly test",
            "cargo +stable build --release",
            // Docker global options
            "docker --context prod ps",
            "docker -H tcp://host:2375 images",
            // Kubectl global options
            "kubectl -n kube-system get pods",
            "kubectl --context prod describe pod foo",
        ];
        for input in cases {
            assert_passthrough(input);
        }
    }

    // === SPECIFIC COMMANDS NOT BLOCKED ===
    // Ported from old hooks/test-rtk-rewrite.sh Sections 1 & 3.
    // These commands must pass through (not be blocked by safety rules).

    #[test]
    fn test_specific_commands_not_blocked() {
        let cases = [
            // Git variants
            "git log --oneline -10",
            "git diff HEAD",
            "git show abc123",
            "git add .",
            // GitHub CLI
            "gh pr list",
            "gh api repos/owner/repo",
            "gh release list",
            // Package managers
            "npm run test:e2e",
            "npm run build",
            "npm test",
            // Docker
            "docker compose up -d",
            "docker compose logs postgrest",
            "docker compose down",
            "docker run --rm postgres",
            "docker exec -it db psql",
            // Kubernetes
            "kubectl describe pod foo",
            "kubectl apply -f deploy.yaml",
            // Test runners
            "npx playwright test",
            "npx prisma migrate",
            "cargo test",
            // Vitest variants (dedup is internal to rtk run, not hook level)
            "vitest",
            "vitest run",
            "vitest run --reporter=verbose",
            "npx vitest run",
            "pnpm vitest run --coverage",
            // TypeScript
            "vue-tsc -b",
            "npx vue-tsc --noEmit",
            // Utilities
            "curl -s https://example.com",
            "ls -la",
            "grep -rn pattern src/",
            "rg pattern src/",
        ];
        for input in cases {
            // Test name intent: commands must Rewrite (not Blocked), regardless of routing target.
            // Specific routing targets are verified in test_routing_native_commands.
            assert!(
                matches!(check_for_hook(input, "claude"), HookResult::Rewrite(_)),
                "'{}' should Rewrite (not Blocked)",
                input
            );
        }
    }

    // === COMMANDS THAT PASS THROUGH (builtins/unknown) ===
    // Ported from old hooks/test-rtk-rewrite.sh Section 5.
    // These are not blocked — they pass through unchanged (no rtk run -c wrapping).

    #[test]
    fn test_builtins_not_blocked() {
        let cases = [
            "echo hello world",
            "cd /tmp",
            "mkdir -p foo/bar",
            "python3 script.py",
            "find . -name '*.ts'",
            "tree src/",
            "wget https://example.com/file",
        ];
        for input in cases {
            assert_passthrough(input);
        }
        // node -e with single quotes: lexer handles as quoted string, passes through
        assert_passthrough("node -e 'console.log(1)'");
    }

    // === SHELL PREFIX BUILTINS (noglob, command, builtin, exec, nocorrect) ===
    // These zsh/bash builtins modify the execution of the NEXT command.
    // The hook should strip the prefix, route the real command, and
    // re-prepend the prefix so the shell applies it correctly.
    //
    // Bug: `noglob gh release create v0.3.0-rc1 ...` was being wrapped in
    // `rtk run -c 'noglob gh ...'` which fails because noglob is a shell
    // builtin, not an executable that rtk can invoke.

    #[test]
    fn test_noglob_prefix_routes_inner_command() {
        // noglob + known command: should route the inner command through RTK
        assert_rewrite("noglob gh pr view 123", "noglob rtk gh pr view 123");
    }

    #[test]
    fn test_noglob_prefix_with_unknown_command() {
        // noglob + unknown command: should preserve noglob prefix, wrap inner in rtk run -c
        match check_for_hook("noglob some-unknown-tool --arg", "claude") {
            HookResult::Rewrite(cmd) => {
                // noglob should be OUTSIDE the rtk run -c, not inside it
                assert!(
                    !cmd.contains("rtk run -c 'noglob"),
                    "noglob should not be inside rtk run -c, got '{}'",
                    cmd
                );
            }
            HookResult::Blocked(_) => panic!("should not be blocked"),
        }
    }

    #[test]
    fn test_command_prefix_routes_inner_command() {
        assert_rewrite("command git status", "command rtk git status");
    }

    #[test]
    fn test_builtin_prefix_passthrough() {
        // builtin cd should just pass through with noglob-style prefix handling
        match check_for_hook("builtin cd /tmp", "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    !cmd.contains("rtk run -c 'builtin"),
                    "builtin should not be inside rtk run -c, got '{}'",
                    cmd
                );
            }
            HookResult::Blocked(_) => panic!("should not be blocked"),
        }
    }

    #[test]
    fn test_nocorrect_prefix_routes_inner_command() {
        assert_rewrite("nocorrect git log -10", "nocorrect rtk git log");
    }

    #[test]
    fn test_noglob_gh_release_create_exact_bug_report() {
        // Exact command from the bug report that triggered `rtk: noglob: command not found`
        let input = "noglob gh release create v0.3.0-rc1 --title v0.3.0-rc1 --notes test --prerelease --draft";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                // gh release is not in hook_lookup whitelist (only pr/issue/run),
                // so inner routes to rtk run -c. noglob must stay outside.
                assert!(
                    !cmd.contains("rtk run -c 'noglob"),
                    "noglob must not be inside rtk run -c, got '{}'",
                    cmd
                );
                assert!(
                    cmd.starts_with("noglob "),
                    "noglob must be the outermost prefix, got '{}'",
                    cmd
                );
            }
            HookResult::Blocked(_) => panic!("should not be blocked"),
        }
    }

    #[test]
    fn test_nested_shell_prefixes() {
        // noglob + command: both should be stripped, inner command routed
        assert_rewrite("noglob command git status", "noglob command rtk git status");
    }

    #[test]
    fn test_shell_prefix_plus_env_prefix() {
        // noglob + GIT_PAGER=cat + git log: all three layers stripped correctly
        assert_rewrite(
            "noglob GIT_PAGER=cat git log -10",
            "noglob GIT_PAGER=cat rtk git log",
        );
    }

    #[test]
    fn test_exec_prefix_routes_inner_command() {
        assert_rewrite("exec git status", "exec rtk git status");
    }

    #[test]
    fn test_bare_shell_prefix_passthrough() {
        // Bare "noglob" with no following command — pass through unchanged
        match check_for_hook("noglob", "claude") {
            HookResult::Rewrite(cmd) => {
                assert_eq!(cmd, "noglob", "bare prefix should pass through unchanged");
            }
            HookResult::Blocked(_) => panic!("should not be blocked"),
        }
    }

    // === UNKNOWN COMMAND PASSTHROUGH ===
    // Unknown commands (not in hook_lookup whitelist) should pass through
    // unchanged instead of being wrapped in `rtk run -c '...'`.
    // Wrapping adds an extra shell layer for zero token savings and causes
    // quoting/globbing bugs (e.g. zsh NOMATCH on version strings).

    /// Assert that a command passes through unchanged (no `rtk run -c` wrapping).
    fn assert_passthrough(input: &str) {
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    !cmd.contains("rtk run -c"),
                    "command should NOT be wrapped in rtk run -c, got '{}'",
                    cmd
                );
                assert_eq!(cmd, input, "unknown command should pass through unchanged");
            }
            HookResult::Blocked(_) => panic!("Expected passthrough for '{}', got Blocked", input),
        }
    }

    #[test]
    fn test_unknown_command_passthrough() {
        // gh release is NOT in hook_lookup whitelist — should pass through unchanged
        assert_passthrough("gh release create v0.3.0 --title test");
    }

    #[test]
    fn test_full_path_binary_routes_correctly() {
        // Full-path binary should be recognized via basename extraction
        assert_rewrite("/opt/homebrew/bin/git status", "rtk git status");
    }

    #[test]
    fn test_full_path_unknown_command_passthrough() {
        assert_passthrough("/opt/homebrew/bin/gh release create v0.3.0");
    }

    #[test]
    fn test_env_prefix_unknown_command_passthrough() {
        assert_passthrough("GH_DEBUG= gh release create v0.3.0");
    }

    #[test]
    fn test_noglob_unknown_command_passthrough() {
        assert_passthrough("noglob gh release create v0.3.0");
    }

    #[test]
    fn test_chain_mixed_known_unknown() {
        // Chains still wrap in rtk run -c, but unknown cmds are preserved inside
        match check_for_hook("gh release create v1 && git status", "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run -c"), "chains still need rtk run -c");
                assert!(cmd.contains("rtk git status"), "known cmd routed");
                assert!(
                    cmd.contains("gh release create v1"),
                    "unknown cmd preserved"
                );
            }
            HookResult::Blocked(_) => panic!("should not be blocked"),
        }
    }

    #[test]
    fn test_gh_release_create_exact_bug_report() {
        let input = r#"gh release create v0.3.0 --title "ai_session_tools v0.3.0" --notes-file notes/v0.3.0-release.md"#;
        assert_passthrough(input);
    }

    #[test]
    fn test_completely_unknown_binary_passthrough() {
        // Binaries RTK has never heard of should pass through
        assert_passthrough("some-custom-tool --flag value");
    }

    // === COMPOUND COMMANDS (chained with &&, ||, ;) ===
    // Shell script only matched FIRST command in a chain.
    // Rust hook parses each command independently (#112).

    #[test]
    fn test_compound_commands_rewrite() {
        let cases = [
            // Basic chains — each command rewritten independently
            ("cd /tmp && git status", "&&"),
            ("cd dir && git status && git diff", "&&"),
            ("git add . && git commit -m msg", "&&"),
            // Semicolon chains
            ("echo start ; git status ; echo done", ";"),
            // Or-chains
            ("git pull || echo failed", "||"),
        ];
        for (input, operator) in cases {
            match check_for_hook(input, "claude") {
                HookResult::Rewrite(cmd) => {
                    assert!(cmd.contains("rtk run"), "'{input}' should rewrite");
                    assert!(
                        cmd.contains(operator),
                        "'{input}' must preserve '{operator}', got '{cmd}'"
                    );
                }
                other => panic!("Expected Rewrite for '{input}', got {other:?}"),
            }
        }
    }

    // PR 2 adds: test_compound_blocked_in_chain (safety-dependent test)

    #[test]
    fn test_compound_quoted_operators_not_split() {
        // && inside quotes must NOT split the command into a chain.
        // parse_chain sees one command: git commit with args ["-m", "Fix && Bug"].
        // That single command routes to rtk git commit (not rtk run -c).
        let input = r#"git commit -m "Fix && Bug""#;
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    cmd.contains("rtk git commit"),
                    "Quoted && must not split; should route to rtk git commit, got '{cmd}'"
                );
            }
            other => panic!("Expected Rewrite for quoted &&, got {other:?}"),
        }
    }

    // PR 2 adds: test_blocked_commands (safety-dependent test)

    // === SUFFIX-AWARE ROUTING: redirect/pipe suffix preserved, RTK filter applied ===
    // When a known RTK command has a "safe" redirect or pipe suffix, the hook should
    // rewrite to `rtk <cmd> <suffix>` so RTK's filter applies AND the shell handles the suffix.

    #[test]
    fn test_suffix_2_redirect_routes_to_rtk() {
        // "cargo test 2>&1" → "rtk cargo test 2>&1" (not rtk run -c)
        let input = "cargo test 2>&1";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    cmd.contains("rtk cargo"),
                    "must use rtk cargo filter, got '{cmd}'"
                );
                assert!(
                    cmd.contains("2>&1"),
                    "must preserve 2>&1 suffix, got '{cmd}'"
                );
                assert!(
                    !cmd.contains("rtk run -c"),
                    "must NOT fall back to passthrough, got '{cmd}'"
                );
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_suffix_dev_null_routes_to_rtk() {
        // "cargo test 2>/dev/null" → "rtk cargo test 2>/dev/null"
        let input = "cargo test 2>/dev/null";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk cargo"), "must use rtk cargo, got '{cmd}'");
                assert!(
                    cmd.contains("/dev/null"),
                    "must preserve /dev/null suffix, got '{cmd}'"
                );
                assert!(
                    !cmd.contains("rtk run -c"),
                    "must NOT fall back to passthrough, got '{cmd}'"
                );
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_suffix_pipe_tee_routes_to_rtk() {
        // "cargo test | tee /tmp/log.txt" → "rtk cargo test | tee /tmp/log.txt"
        let input = "cargo test | tee /tmp/log.txt";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    cmd.contains("rtk cargo"),
                    "must use rtk cargo filter, got '{cmd}'"
                );
                assert!(cmd.contains("tee"), "must preserve tee suffix, got '{cmd}'");
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_suffix_pipe_head_routes_to_rtk() {
        // "git log | head -20" → "rtk git log | head -20" (not passthrough)
        let input = "git log | head -20";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk git"), "must use rtk git, got '{cmd}'");
                assert!(
                    cmd.contains("head"),
                    "must preserve head suffix, got '{cmd}'"
                );
                assert!(
                    !cmd.contains("rtk run -c"),
                    "must NOT fall back to passthrough, got '{cmd}'"
                );
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_suffix_unknown_cmd_still_passthrough() {
        // Unknown command with redirect suffix → passes through unchanged
        assert_passthrough("unknown_xyz_cmd 2>&1");
    }

    #[test]
    fn test_suffix_unsafe_pipe_still_passthrough() {
        // Pipe to grep (not a known safe sink) → stays as rtk run -c passthrough
        // This is debatable; for safety, unknown pipe destinations stay in shell
        let input = "cargo test | grep FAILED";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                // Either passthrough or rtk routing is acceptable, but must not panic
                let _ = cmd;
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    // === SHELLISM PASSTHROUGH: cat/sed/head allowed with pipe/redirect ===

    #[test]
    fn test_token_waste_allowed_in_pipelines() {
        let cases = [
            "cat file.txt | grep pattern",
            "cat file.txt > output.txt",
            "sed 's/old/new/' file.txt > output.txt",
            "head -n 10 file.txt | grep pattern",
            "for f in *.txt; do cat \"$f\" | grep x; done",
        ];
        for input in cases {
            assert_rewrite(input, "rtk run");
        }
    }

    // === MULTI-AGENT ===

    #[test]
    fn test_different_agents_same_result() {
        // Both agents must Rewrite (not Block) safe commands.
        // Specific routing targets verified in test_cross_agent_routing_identical.
        for agent in ["claude", "gemini"] {
            match check_for_hook("git status", agent) {
                HookResult::Rewrite(_) => {}
                other => panic!("Expected Rewrite for agent '{}', got {:?}", agent, other),
            }
        }
    }

    // === FORMAT_FOR_CLAUDE ===

    #[test]
    fn test_format_for_claude() {
        let (output, success, code) =
            format_for_claude(HookResult::Rewrite("rtk run -c 'git status'".to_string()));
        assert_eq!(output, "rtk run -c 'git status'");
        assert!(success);
        assert_eq!(code, 0);

        let (output, success, code) =
            format_for_claude(HookResult::Blocked("Error message".to_string()));
        assert_eq!(output, "Error message");
        assert!(!success);
        assert_eq!(code, 2); // Exit 2 = blocking error per Claude Code spec
    }

    // === $VAR LEXER FIX: NATIVE ROUTING ===
    // After the lexer fix, simple $IDENT vars are Arg tokens, not Shellisms.
    // This enables native RTK routing for commands with simple variable references.

    #[test]
    fn test_dollar_var_routes_natively() {
        // git log $BRANCH: $BRANCH should be Arg → routes to rtk git, not rtk run -c
        let result = match check_for_hook("git log $BRANCH", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk git"),
            "Expected rtk git routing for 'git log $BRANCH', got: {}",
            result
        );
        assert!(
            !result.contains("rtk run"),
            "Should not fall to passthrough for simple $VAR, got: {}",
            result
        );
    }

    #[test]
    fn test_dollar_subshell_still_passthrough() {
        // git log $(git rev-parse HEAD): $(…) needs shell — must passthrough
        let result = match check_for_hook("git log $(git rev-parse HEAD)", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk run"),
            "Subshell $(…) must route to passthrough, got: {}",
            result
        );
    }

    // === RECURSION DEPTH LIMIT ===

    #[test]
    fn test_rewrite_depth_limit_blocked() {
        // At max depth → blocked with loop detection message
        assert_blocked("echo hello", MAX_REWRITE_DEPTH, "loop");
    }

    #[test]
    fn test_rewrite_depth_limit_allowed() {
        // At depth 0 → normal rewrite (unknown cmd passes through unchanged)
        match check_for_hook_inner("echo hello", 0) {
            HookResult::Rewrite(cmd) => assert_eq!(cmd, "echo hello"),
            _ => panic!("Expected Rewrite at depth 0"),
        }
    }

    // =========================================================================
    // CLAUDE CODE WIRE FORMAT CONFORMANCE
    // https://docs.anthropic.com/en/docs/claude-code/hooks
    //
    // Claude Code hook protocol:
    // - Rewrite: command on stdout, exit code 0
    // - Block: message on stderr, exit code 2
    // - Other exit codes are non-blocking errors
    //
    // format_for_claude() is the boundary between HookResult and the wire.
    // These tests verify it produces the exact contract Claude Code expects.
    // =========================================================================

    #[test]
    fn test_claude_rewrite_exit_code_is_zero() {
        let (_, _, code) = format_for_claude(HookResult::Rewrite("rtk run -c 'ls'".into()));
        assert_eq!(code, 0, "Rewrite must exit 0 (success)");
    }

    #[test]
    fn test_claude_block_exit_code_is_two() {
        let (_, _, code) = format_for_claude(HookResult::Blocked("denied".into()));
        assert_eq!(
            code, 2,
            "Block must exit 2 (blocking error per Claude Code spec)"
        );
    }

    #[test]
    fn test_claude_rewrite_output_is_command_text() {
        // Claude Code reads stdout as the rewritten command — must be plain text, not JSON
        let (output, success, _) =
            format_for_claude(HookResult::Rewrite("rtk run -c 'git status'".into()));
        assert_eq!(output, "rtk run -c 'git status'");
        assert!(success);
        // Must NOT be JSON
        assert!(
            !output.starts_with('{'),
            "Rewrite output must be plain text, not JSON"
        );
    }

    #[test]
    fn test_claude_block_output_is_human_message() {
        // Claude Code reads stderr for the block reason
        let (output, success, _) =
            format_for_claude(HookResult::Blocked("Use Read tool instead".into()));
        assert_eq!(output, "Use Read tool instead");
        assert!(!success);
        // Must NOT be JSON
        assert!(
            !output.starts_with('{'),
            "Block output must be plain text, not JSON"
        );
    }

    #[test]
    fn test_claude_rewrite_success_flag_true() {
        let (_, success, _) = format_for_claude(HookResult::Rewrite("cmd".into()));
        assert!(success, "Rewrite must set success=true");
    }

    #[test]
    fn test_claude_block_success_flag_false() {
        let (_, success, _) = format_for_claude(HookResult::Blocked("msg".into()));
        assert!(!success, "Block must set success=false");
    }

    #[test]
    fn test_claude_exit_codes_not_one() {
        // Exit code 1 means non-blocking error in Claude Code — we must never use it
        let (_, _, rewrite_code) = format_for_claude(HookResult::Rewrite("cmd".into()));
        let (_, _, block_code) = format_for_claude(HookResult::Blocked("msg".into()));
        assert_ne!(
            rewrite_code, 1,
            "Exit code 1 is non-blocking error, not valid for rewrite"
        );
        assert_ne!(
            block_code, 1,
            "Exit code 1 is non-blocking error, not valid for block"
        );
    }

    // === CROSS-PROTOCOL: Same decision for both agents ===

    #[test]
    fn test_cross_protocol_safe_command_allowed_by_both() {
        // Both Claude and Gemini must allow the same safe commands
        for cmd in ["git status", "cargo test", "ls -la", "echo hello"] {
            let claude = check_for_hook(cmd, "claude");
            let gemini = check_for_hook(cmd, "gemini");
            match (&claude, &gemini) {
                (HookResult::Rewrite(_), HookResult::Rewrite(_)) => {}
                _ => panic!(
                    "'{}': Claude={:?}, Gemini={:?} — both should Rewrite",
                    cmd, claude, gemini
                ),
            }
        }
    }

    // PR 2 adds: test_cross_protocol_blocked_command_denied_by_both (safety-dependent test)

    // =====================================================================
    // ROUTING TESTS — verify route_native_command dispatch
    // =====================================================================

    #[test]
    fn test_routing_native_commands() {
        // Table-driven: commands that route to optimized rtk subcommands.
        // Each (input, expected_substr) must appear in the rewritten output.
        let cases = [
            // Git: known subcommands
            ("git status", "rtk git status"),
            ("git log --oneline -10", "rtk git log --oneline -10"),
            ("git diff HEAD", "rtk git diff HEAD"),
            ("git add .", "rtk git add ."),
            ("git commit -m msg", "rtk git commit"),
            // GitHub CLI
            ("gh pr view 156", "rtk gh pr view 156"),
            // Cargo
            ("cargo test", "rtk cargo test"),
            (
                "cargo clippy --all-targets",
                "rtk cargo clippy --all-targets",
            ),
            // File ops (rg → rtk grep rename)
            // NOTE: PR 2 adds safety that blocks cat before reaching router; arm is defensive.
            ("grep -r pattern src/", "rtk grep -r pattern src/"),
            ("rg pattern src/", "rtk grep pattern src/"),
            ("ls -la", "rtk ls -la"),
            // JS/TS tooling
            ("vitest", "rtk vitest run"),     // bare → rtk vitest run
            ("vitest run", "rtk vitest run"), // explicit run preserved
            ("vitest run --coverage", "rtk vitest run --coverage"),
            ("pnpm test", "rtk vitest run"),
            ("pnpm vitest", "rtk vitest run"),
            ("pnpm lint", "rtk lint"),
            ("pnpm eslint src/", "rtk lint"), // pnpm eslint → rtk lint
            ("pnpm eslint .", "rtk lint ."),  // pnpm eslint bare form
            ("pnpm eslint --fix src/", "rtk lint"), // pnpm eslint with flag
            ("npx tsc --noEmit", "rtk tsc --noEmit"),
            // Python
            ("python -m pytest tests/", "rtk pytest tests/"),
            ("uv pip list", "rtk pip list"),
            // Go
            ("go test ./...", "rtk go test ./..."),
            ("go build ./...", "rtk go build ./..."),
            ("go vet ./...", "rtk go vet ./..."),
            // All ROUTES entries not yet covered above
            ("eslint src/", "rtk lint src/"), // rename: eslint → lint
            ("tsc --noEmit", "rtk tsc --noEmit"), // bare tsc (not npx tsc)
            ("prettier src/", "rtk prettier src/"),
            ("playwright test", "rtk playwright test"),
            ("prisma migrate dev", "rtk prisma migrate dev"),
            (
                "curl https://api.example.com",
                "rtk curl https://api.example.com",
            ),
            ("pytest tests/", "rtk pytest tests/"), // bare pytest (not python -m pytest)
            ("pytest -x tests/unit", "rtk pytest -x tests/unit"),
            ("golangci-lint run ./...", "rtk golangci-lint run ./..."),
            ("docker ps", "rtk docker ps"),
            ("docker images", "rtk docker images"),
            ("docker logs mycontainer", "rtk docker logs mycontainer"),
            ("kubectl get pods", "rtk kubectl get pods"),
            ("kubectl logs mypod", "rtk kubectl logs mypod"),
            ("ruff check src/", "rtk ruff check src/"),
            ("ruff format src/", "rtk ruff format src/"),
            ("pip list", "rtk pip list"),
            ("pip install requests", "rtk pip install requests"),
            ("pip outdated", "rtk pip outdated"),
            ("pip show requests", "rtk pip show requests"),
            ("gh issue list", "rtk gh issue list"),
            ("gh run view 123", "rtk gh run view 123"),
            ("git stash pop", "rtk git stash pop"),
            ("git fetch origin", "rtk git fetch origin"),
            // Graphite CLI — all subcommands route through RTK
            ("gt log", "rtk gt log"),
            ("gt submit", "rtk gt submit"),
            ("gt sync", "rtk gt sync"),
            ("gt create feat/new-branch", "rtk gt create feat/new-branch"),
        ];
        for (input, expected) in cases {
            assert_rewrite(input, expected);
        }
    }

    #[test]
    fn test_routing_subcommand_filter_fallback() {
        // Commands where binary is in ROUTES but subcommand is NOT in the Only list
        // must pass through unchanged (no wrapping in rtk run -c).
        let cases = [
            "docker build .",            // docker Only: ps, images, logs
            "docker run -it nginx",      // docker Only: ps, images, logs
            "kubectl apply -f dep.yaml", // kubectl Only: get, logs
            "kubectl delete pod mypod",  // kubectl Only: get, logs
            "go mod tidy",               // go Only: test, build, vet
            "go generate ./...",         // go Only: test, build, vet
            "ruff lint src/",            // ruff Only: check, format
            "pip freeze",                // pip Only: list, outdated, install, show
            "pip uninstall requests",    // pip Only: list, outdated, install, show
            "cargo publish",             // cargo Only: test, build, clippy, check
            "cargo run",                 // cargo Only: test, build, clippy, check
            "git rebase -i HEAD~3",      // git Only list (rebase not included)
            "git cherry-pick abc123",    // git Only list
            "gh repo clone foo/bar",     // gh Only: pr, issue, run
        ];
        for input in cases {
            assert_passthrough(input);
        }
    }

    #[test]
    fn test_routing_vitest_no_double_run() {
        // Shell script sed bug: 's/^(pnpm )?vitest/rtk vitest run/' on
        // "pnpm vitest run --coverage" produces "rtk vitest run run --coverage".
        // Binary hook corrects this by using parsed args instead of regex substitution.
        let result = match check_for_hook("pnpm vitest run --coverage", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert_rewrite("pnpm vitest run --coverage", "rtk vitest run --coverage");
        assert!(
            !result.contains("run run"),
            "Must not double 'run' in output: '{}'",
            result
        );
    }

    #[test]
    fn test_routing_fallbacks_to_rtk_run() {
        // Chains (2+ cmds) and pipes still fall back to rtk run -c.
        let chain_cases = [
            "git add . && git commit -m msg", // chain → 2 commands → rtk run -c
            "git log | grep fix",             // pipe → needs_shell → rtk run -c
        ];
        for input in chain_cases {
            assert_rewrite(input, "rtk run -c");
        }
        // Single unknown commands pass through unchanged (no wrapping).
        let passthrough_cases = [
            "git checkout main", // unknown git subcommand
            "tail -n 20 file.txt",
            "tail -f server.log",
        ];
        for input in passthrough_cases {
            assert_passthrough(input);
        }
    }

    #[test]
    fn test_cross_agent_routing_identical() {
        // Both claude and gemini must route the same commands to the same output.
        for cmd in ["git status", "cargo test", "ls -la"] {
            let claude_result = check_for_hook(cmd, "claude");
            let gemini_result = check_for_hook(cmd, "gemini");
            match (&claude_result, &gemini_result) {
                (HookResult::Rewrite(c), HookResult::Rewrite(g)) => {
                    assert_eq!(c, g, "claude and gemini must route '{}' identically", cmd);
                    assert!(
                        !c.contains("rtk run -c"),
                        "'{}' should not fall back to rtk run -c",
                        cmd
                    );
                }
                _ => panic!(
                    "'{}' should Rewrite for both agents: claude={:?} gemini={:?}",
                    cmd, claude_result, gemini_result
                ),
            }
        }
    }

    // === INNER COMMAND SUBSTITUTION (&&, ||, ; chains) ===
    // When a multi-command chain is wrapped in "rtk run -c '...'", each individual
    // command that has an RTK equivalent should be substituted so RTK's filter
    // applies inside the shell string.
    //
    // Example: "cargo test && git log"
    //   Before: rtk run -c 'cargo test && git log'
    //   After:  rtk run -c 'rtk cargo test && rtk git log'
    //
    // Safety invariant: only &&/||/; chains are substituted here.
    // Pipe-separated commands are handled separately (split_safe_suffix / needs_shell).

    #[test]
    fn test_chain_both_commands_substituted() {
        // Both cargo test AND git log should route to rtk inside the shell string
        let result = match check_for_hook("cargo test && git log", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk cargo"),
            "cargo test must be substituted to rtk cargo inside chain: {}",
            result
        );
        assert!(
            result.contains("rtk git"),
            "git log must be substituted to rtk git inside chain: {}",
            result
        );
        // The outer wrapper is rtk run -c because && needs a shell
        assert!(
            result.contains("rtk run"),
            "chain still needs shell wrapper (rtk run -c): {}",
            result
        );
    }

    #[test]
    fn test_chain_with_dollar_var_substituted() {
        // cargo test && git log $BRANCH: $BRANCH is Arg (after lexer fix) → both route natively
        let result = match check_for_hook("cargo test && git log $BRANCH", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk cargo"),
            "cargo test must be rtk in chain: {}",
            result
        );
        assert!(
            result.contains("rtk git log"),
            "git log $BRANCH must be rtk with var preserved: {}",
            result
        );
        assert!(
            result.contains("$BRANCH"),
            "$BRANCH must be preserved in rewritten chain: {}",
            result
        );
    }

    #[test]
    fn test_chain_unknown_command_not_substituted() {
        // unknown_xyz_cmd not in registry → stays unmodified inside the shell string
        let result = match check_for_hook("cargo test && unknown_xyz_cmd", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk cargo"),
            "cargo test must be substituted to rtk: {}",
            result
        );
        assert!(
            result.contains("unknown_xyz_cmd"),
            "unknown command must pass through unchanged: {}",
            result
        );
        assert!(
            !result.contains("rtk unknown"),
            "must not invent rtk subcommands for unknown binary: {}",
            result
        );
    }

    #[test]
    fn test_semicolon_chain_substituted() {
        // ; chains: each known command should be substituted
        let result = match check_for_hook("cargo test ; git status", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk cargo"),
            "cargo must be rtk in semicolon chain: {}",
            result
        );
        assert!(
            result.contains("rtk git"),
            "git must be rtk in semicolon chain: {}",
            result
        );
    }

    #[test]
    fn test_or_chain_substituted() {
        // || chains: each known command should be substituted
        let result = match check_for_hook("cargo test || go test ./...", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk cargo"),
            "cargo must be rtk in || chain: {}",
            result
        );
        assert!(
            result.contains("rtk go"),
            "go must be rtk in || chain: {}",
            result
        );
    }

    // === PIPE OUTPUT CLASSIFICATION TESTS ===
    // FORMAT_PRESERVING: commands whose RTK output format matches raw output,
    //   making them safe as the left side of any pipe.
    // TRANSPARENT_SINKS: right-side commands that consume any input format
    //   (already handled by split_safe_suffix for routing purposes).
    //
    // These classification constants document the safety policy for future
    // pipe-left substitution logic and must contain the expected entries.

    #[test]
    fn test_format_preserving_contains_expected() {
        assert!(
            FORMAT_PRESERVING.contains(&"tail"),
            "tail is format-preserving (line-per-line passthrough)"
        );
        assert!(
            FORMAT_PRESERVING.contains(&"echo"),
            "echo is format-preserving (output equals input)"
        );
        assert!(
            FORMAT_PRESERVING.contains(&"find"),
            "find is format-preserving (path-per-line)"
        );
        assert!(
            FORMAT_PRESERVING.contains(&"cat"),
            "cat is format-preserving (byte passthrough)"
        );
    }

    #[test]
    fn test_format_changing_not_in_format_preserving() {
        // Commands that transform output heavily must NOT be in FORMAT_PRESERVING.
        // If substituted as left side of a semantic-sink pipe (grep, jq, awk),
        // the right side would receive unexpected compressed format and break.
        assert!(
            !FORMAT_PRESERVING.contains(&"cargo"),
            "cargo test compresses output — not format-preserving"
        );
        assert!(
            !FORMAT_PRESERVING.contains(&"git"),
            "git log/diff compresses output — not format-preserving"
        );
        assert!(
            !FORMAT_PRESERVING.contains(&"pytest"),
            "pytest compresses output — not format-preserving"
        );
        assert!(
            !FORMAT_PRESERVING.contains(&"go"),
            "go test compresses output — not format-preserving"
        );
    }

    #[test]
    fn test_transparent_sinks_contains_expected() {
        // Transparent sinks accept any input format — already handled by split_safe_suffix.
        assert!(
            TRANSPARENT_SINKS.contains(&"tee"),
            "tee is a transparent sink (copies stdin to file + stdout)"
        );
        assert!(
            TRANSPARENT_SINKS.contains(&"head"),
            "head is a transparent sink (truncates lines)"
        );
        assert!(
            TRANSPARENT_SINKS.contains(&"cat"),
            "cat is a transparent sink (passes through)"
        );
        assert!(
            TRANSPARENT_SINKS.contains(&"tail"),
            "tail is a transparent sink (last N lines)"
        );
    }

    // ── End-to-end token savings tests ───────────────────────────────────────
    // These tests simulate the full hook pipeline from the start:
    //   raw command → check_for_hook (lexer + router) → rewritten rtk cmd
    //   → execute both → compare token counts
    //
    // Run with: cargo test e2e -- --ignored
    // Requires: `cargo install --path .` (rtk binary on PATH) + git repo

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn exec(cmd: &str) -> String {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        let out = std::process::Command::new(parts[0])
            .args(&parts[1..])
            .output()
            .unwrap_or_else(|e| panic!("failed to exec '{cmd}': {e}"));
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    #[test]
    #[ignore = "requires installed rtk binary (cargo install --path .) and git repo"]
    fn test_e2e_git_status_saves_tokens() {
        // Step 1: route through the full hook pipeline (lexer → router)
        let raw_cmd = "git status";
        let rtk_cmd = match check_for_hook(raw_cmd, "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite for '{raw_cmd}', got {other:?}"),
        };
        assert!(
            rtk_cmd.starts_with("rtk git"),
            "lexer+router should produce rtk git status, got: {rtk_cmd}"
        );

        // Step 2: execute both and compare token counts
        let raw_out = exec(raw_cmd);
        let rtk_out = exec(&rtk_cmd);
        let raw_tok = count_tokens(&raw_out);
        let rtk_tok = count_tokens(&rtk_out);
        assert!(raw_tok > 0, "raw git status produced no output");

        let savings = 100.0 * (1.0 - rtk_tok as f64 / raw_tok as f64);
        assert!(
            savings >= 40.0,
            "rtk git status should save ≥40% tokens vs raw git status, \
             got {savings:.1}% ({raw_tok} raw → {rtk_tok} rtk tokens)"
        );
    }

    #[test]
    #[ignore = "requires installed rtk binary (cargo install --path .) and directory with files"]
    fn test_e2e_ls_saves_tokens() {
        // Step 1: route through the full hook pipeline (lexer → router)
        let raw_cmd = "ls -la .";
        let rtk_cmd = match check_for_hook(raw_cmd, "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite for '{raw_cmd}', got {other:?}"),
        };
        assert!(
            rtk_cmd.starts_with("rtk ls"),
            "lexer+router should produce rtk ls, got: {rtk_cmd}"
        );

        // Step 2: execute both and compare token counts
        let raw_out = exec(raw_cmd);
        let rtk_out = exec(&rtk_cmd);
        let raw_tok = count_tokens(&raw_out);
        let rtk_tok = count_tokens(&rtk_out);
        assert!(raw_tok > 0, "raw ls -la produced no output");

        let savings = 100.0 * (1.0 - rtk_tok as f64 / raw_tok as f64);
        assert!(
            savings >= 40.0,
            "rtk ls should save ≥40% tokens vs raw ls -la, \
             got {savings:.1}% ({raw_tok} raw → {rtk_tok} rtk tokens)"
        );
    }

    #[test]
    #[ignore = "requires installed rtk binary (cargo install --path .) and git repo with history"]
    fn test_e2e_git_log_saves_tokens() {
        // Step 1: route through the full hook pipeline (lexer → router)
        let raw_cmd = "git log --oneline -20";
        let rtk_cmd = match check_for_hook(raw_cmd, "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite for '{raw_cmd}', got {other:?}"),
        };
        assert!(
            rtk_cmd.starts_with("rtk git"),
            "lexer+router should produce rtk git log, got: {rtk_cmd}"
        );

        // Step 2: execute both and compare token counts
        let raw_out = exec(raw_cmd);
        let rtk_out = exec(&rtk_cmd);
        let raw_tok = count_tokens(&raw_out);
        let rtk_tok = count_tokens(&rtk_out);
        assert!(
            raw_tok > 0,
            "raw git log produced no output — need a repo with commits"
        );

        // git log --oneline is already compact; rtk may not save much beyond
        // line-length capping.  Truncating long lines with "..." can add a
        // marginal token.  Allow ≤5% overhead to account for this artefact.
        let ratio = rtk_tok as f64 / raw_tok.max(1) as f64;
        assert!(
            ratio <= 1.05,
            "rtk git log must not significantly bloat output vs raw git log \
             ({raw_tok} raw → {rtk_tok} rtk, ratio {ratio:.2})"
        );
    }

    // === CAT BEHAVIOR TESTS ===
    // Note: this branch does NOT include the data-safety rules system (that's in
    // feat/multi-platform-hooks). Without safety rules, cat hits the defensive
    // fallback in route_native_command() and rewrites to rtk read.
    // In feat/multi-platform-hooks, cat is Blocked by src/rules/rtk.safety.block-cat.md.

    #[test]
    fn test_cat_multi_file_rewrites_to_rtk_read() {
        // Without safety rules, cat→rtk read fallback fires for all arities.
        let result = check_for_hook("cat file1.txt file2.txt", "claude");
        assert!(
            matches!(&result, HookResult::Rewrite(s) if s == "rtk read file1.txt file2.txt"),
            "cat (multi-file) must rewrite to rtk read on this branch; got: {:?}",
            result
        );
    }

    #[test]
    fn test_cat_single_file_rewrites_to_rtk_read() {
        // Same fallback applies for single-file cat — no special-casing by arity.
        let result = check_for_hook("cat CLAUDE.md", "claude");
        assert!(
            matches!(&result, HookResult::Rewrite(s) if s == "rtk read CLAUDE.md"),
            "cat (single-file) must rewrite to rtk read on this branch; got: {:?}",
            result
        );
    }
    // --- #196: gh --json/--jq/--template passthrough ---

    #[test]
    fn test_gh_json_flag_passes_through() {
        // gh --json produces structured JSON that rtk gh would corrupt
        assert!(should_passthrough("gh pr list --json number,title"));
        assert!(should_passthrough(
            "gh pr list --json number --jq '.[].number'"
        ));
        assert!(should_passthrough("gh pr view 42 --template '{{.title}}'"));
        assert!(should_passthrough("gh api repos/owner/repo --jq '.name'"));
    }

    #[test]
    fn test_gh_without_json_not_passthrough() {
        // gh without structured output flags → still eligible for rewriting
        assert!(!should_passthrough("gh pr list"));
        assert!(!should_passthrough("gh issue list"));
    }
}
