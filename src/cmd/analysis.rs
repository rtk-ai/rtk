//! Analyzes tokens to decide: Native execution or Passthrough?

use super::lexer::{strip_quotes, ParsedToken, TokenKind};

/// Represents a single command in a chain
#[derive(Debug, Clone, PartialEq)]
pub struct NativeCommand {
    pub binary: String,
    pub args: Vec<String>,
    pub operator: Option<String>, // &&, ||, ;, or None for last command
}

/// Try to strip a known "safe" redirect or pipe suffix from the end of a token list.
///
/// Safe suffixes are shell constructs that can be applied to any command's output
/// without changing the command's semantics.  By stripping them, the hook can route
/// the core command through an RTK filter and then re-attach the suffix in the shell.
///
/// # Returns
/// `(core_tokens, suffix_string)` where:
/// - `core_tokens` is the token list with the suffix removed (unchanged if no match)
/// - `suffix_string` is the raw shell suffix to append to the rewritten command, or `""`
///
/// # Recognized patterns (checked from longest to shortest)
/// - `2>&1`           — Arg("2") + Redirect(">") + Shellism("&") + Arg("1")
/// - `| tee <file>`   — Pipe + Arg("tee") + Arg(any)
/// - `| head <arg>`   — Pipe + Arg("head"/"tail") + Arg(any)
/// - `| cat`          — Pipe + Arg("cat")
/// - `2>/dev/null`    — Arg("2") + Redirect(">") + Arg("/dev/null")
/// - `> /dev/null`    — Redirect(">") + Arg("/dev/null")
/// - `>> <file>`      — Redirect(">>") + Arg(any)
/// - `&`              — Shellism("&") at end, only when no other Shellism in core
///   (`cargo build &` → core `cargo build`, suffix `&`; `cargo build 2>&1 &` → no strip,
///   because the `&` in `2>&1` is already a Shellism in the core)
pub fn split_safe_suffix(mut tokens: Vec<ParsedToken>) -> (Vec<ParsedToken>, String) {
    let mut suffixes: Vec<String> = Vec::new();

    loop {
        let n = tokens.len();
        let mut matched_len: usize = 0;
        let mut matched_suffix = String::new();

        // 4-token: 2>&1
        if n >= 5 {
            let t = &tokens[n - 4..];
            if matches!(t[0].kind, TokenKind::Arg)
                && t[0].value == "2"
                && matches!(t[1].kind, TokenKind::Redirect)
                && t[1].value == ">"
                && matches!(t[2].kind, TokenKind::Shellism)
                && t[2].value == "&"
                && matches!(t[3].kind, TokenKind::Arg)
                && t[3].value == "1"
            {
                matched_suffix = "2>&1".to_string();
                matched_len = 4;
            }
        }

        // 3-token: | tee <file>
        if matched_len == 0 && n >= 4 {
            let t = &tokens[n - 3..];
            if matches!(t[0].kind, TokenKind::Pipe)
                && matches!(t[1].kind, TokenKind::Arg)
                && t[1].value == "tee"
                && matches!(t[2].kind, TokenKind::Arg)
            {
                matched_suffix = format!("| tee {}", t[2].value);
                matched_len = 3;
            }
        }

        // 3-token: | head <arg> or | tail <arg>
        if matched_len == 0 && n >= 4 {
            let t = &tokens[n - 3..];
            if matches!(t[0].kind, TokenKind::Pipe)
                && matches!(t[1].kind, TokenKind::Arg)
                && matches!(t[1].value.as_str(), "head" | "tail")
                && matches!(t[2].kind, TokenKind::Arg)
            {
                matched_suffix = format!("| {} {}", t[1].value, t[2].value);
                matched_len = 3;
            }
        }

        // 3-token: 2>/dev/null
        if matched_len == 0 && n >= 4 {
            let t = &tokens[n - 3..];
            if matches!(t[0].kind, TokenKind::Arg)
                && t[0].value == "2"
                && matches!(t[1].kind, TokenKind::Redirect)
                && t[1].value == ">"
                && matches!(t[2].kind, TokenKind::Arg)
                && t[2].value == "/dev/null"
            {
                matched_suffix = "2>/dev/null".to_string();
                matched_len = 3;
            }
        }

        // 2-token: | cat
        if matched_len == 0 && n >= 3 {
            let t = &tokens[n - 2..];
            if matches!(t[0].kind, TokenKind::Pipe)
                && matches!(t[1].kind, TokenKind::Arg)
                && t[1].value == "cat"
            {
                matched_suffix = "| cat".to_string();
                matched_len = 2;
            }
        }

        // 2-token: > /dev/null
        if matched_len == 0 && n >= 3 {
            let t = &tokens[n - 2..];
            if matches!(t[0].kind, TokenKind::Redirect)
                && t[0].value == ">"
                && matches!(t[1].kind, TokenKind::Arg)
                && t[1].value == "/dev/null"
            {
                matched_suffix = "> /dev/null".to_string();
                matched_len = 2;
            }
        }

        // 2-token: >> <file>
        if matched_len == 0 && n >= 3 {
            let t = &tokens[n - 2..];
            if matches!(t[0].kind, TokenKind::Redirect)
                && t[0].value == ">>"
                && matches!(t[1].kind, TokenKind::Arg)
            {
                matched_suffix = format!(">> {}", t[1].value);
                matched_len = 2;
            }
        }

        // 1-token: & (trailing background job operator)
        // Guard: strip only when no other Shellism exists in the core.
        if matched_len == 0 && n >= 2 {
            let last = &tokens[n - 1];
            if matches!(last.kind, TokenKind::Shellism) && last.value == "&" {
                let core = &tokens[..n - 1];
                if !core.iter().any(|t| matches!(t.kind, TokenKind::Shellism)) {
                    matched_suffix = "&".to_string();
                    matched_len = 1;
                }
            }
        }

        if matched_len == 0 {
            break;
        }

        tokens.truncate(n - matched_len);
        suffixes.push(matched_suffix);
    }

    suffixes.reverse();
    let suffix = suffixes.join(" ");
    (tokens, suffix)
}

/// Check if command needs real shell (has shellisms, pipes, redirects)
pub fn needs_shell(tokens: &[ParsedToken]) -> bool {
    tokens.iter().any(|t| {
        matches!(
            t.kind,
            TokenKind::Shellism | TokenKind::Pipe | TokenKind::Redirect
        )
    })
}

/// Parse tokens into native command chain
/// Returns error if syntax is invalid (e.g., operator with no preceding command)
pub fn parse_chain(tokens: Vec<ParsedToken>) -> Result<Vec<NativeCommand>, String> {
    let mut commands = Vec::new();
    let mut current_args = Vec::new();

    for token in tokens {
        match token.kind {
            TokenKind::Arg => {
                // Strip quotes from the argument
                current_args.push(strip_quotes(&token.value));
            }
            TokenKind::Operator => {
                if current_args.is_empty() {
                    return Err(format!(
                        "Syntax error: operator {} with no command",
                        token.value
                    ));
                }
                // First arg is the binary, rest are args
                let binary = current_args.remove(0);
                commands.push(NativeCommand {
                    binary,
                    args: current_args.clone(),
                    operator: Some(token.value.clone()),
                });
                current_args.clear();
            }
            TokenKind::Pipe | TokenKind::Redirect | TokenKind::Shellism => {
                // Should not reach here if needs_shell() was checked first
                // But handle gracefully
                return Err(format!(
                    "Unexpected {:?} in native mode - use passthrough",
                    token.kind
                ));
            }
        }
    }

    // Handle last command (no trailing operator)
    if !current_args.is_empty() {
        let binary = current_args.remove(0);
        commands.push(NativeCommand {
            binary,
            args: current_args,
            operator: None,
        });
    }

    Ok(commands)
}

/// Should the next command run based on operator and last result?
pub fn should_run(operator: Option<&str>, last_success: bool) -> bool {
    match operator {
        Some("&&") => last_success,
        Some("||") => !last_success,
        Some(";") | None => true,
        _ => true, // Unknown operator, just run
    }
}
