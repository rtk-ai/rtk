//! Analyzes tokens to decide: Native execution or Passthrough?

use super::lexer::{strip_quotes, ParsedToken, TokenKind};

/// Represents a single command in a chain
#[derive(Debug, Clone, PartialEq)]
pub struct NativeCommand {
    pub binary: String,
    pub args: Vec<String>,
    pub operator: Option<String>, // &&, ||, ;, or None for last command
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::lexer::tokenize;

    // === NEEDS_SHELL TESTS ===

    #[test]
    fn test_needs_shell_simple() {
        let tokens = tokenize("git status");
        assert!(!needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_with_glob() {
        let tokens = tokenize("ls *.rs");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_with_pipe() {
        let tokens = tokenize("cat file | grep x");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_with_redirect() {
        let tokens = tokenize("cmd > file");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_with_chain() {
        let tokens = tokenize("cd dir && git status");
        // && is an Operator, not a Shellism - should NOT need shell
        assert!(!needs_shell(&tokens));
    }

    // === PARSE_CHAIN TESTS ===

    #[test]
    fn test_parse_simple_command() {
        let tokens = tokenize("git status");
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].binary, "git");
        assert_eq!(cmds[0].args, vec!["status"]);
        assert_eq!(cmds[0].operator, None);
    }

    #[test]
    fn test_parse_command_with_multiple_args() {
        let tokens = tokenize("git commit -m message");
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].binary, "git");
        assert_eq!(cmds[0].args, vec!["commit", "-m", "message"]);
    }

    #[test]
    fn test_parse_chained_and() {
        let tokens = tokenize("cd dir && git status");
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].binary, "cd");
        assert_eq!(cmds[0].args, vec!["dir"]);
        assert_eq!(cmds[0].operator, Some("&&".to_string()));
        assert_eq!(cmds[1].binary, "git");
        assert_eq!(cmds[1].args, vec!["status"]);
        assert_eq!(cmds[1].operator, None);
    }

    #[test]
    fn test_parse_chained_or() {
        let tokens = tokenize("cmd1 || cmd2");
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].operator, Some("||".to_string()));
    }

    #[test]
    fn test_parse_chained_semicolon() {
        let tokens = tokenize("cmd1 ; cmd2 ; cmd3");
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0].operator, Some(";".to_string()));
        assert_eq!(cmds[1].operator, Some(";".to_string()));
        assert_eq!(cmds[2].operator, None);
    }

    #[test]
    fn test_parse_triple_chain() {
        let tokens = tokenize("a && b && c");
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds.len(), 3);
    }

    #[test]
    fn test_parse_operator_at_start() {
        let tokens = tokenize("&& cmd");
        let result = parse_chain(tokens);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_operator_at_end() {
        let tokens = tokenize("cmd &&");
        let cmds = parse_chain(tokens).unwrap();
        // cmd is parsed, && triggers flush but no second command
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].operator, Some("&&".to_string()));
    }

    #[test]
    fn test_parse_quoted_arg() {
        let tokens = tokenize("git commit -m \"Fix && Bug\"");
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds.len(), 1);
        // The && inside quotes should be in the arg, not an operator
        // args are: commit, -m, "Fix && Bug"
        assert_eq!(cmds[0].args.len(), 3);
        assert_eq!(cmds[0].args[2], "Fix && Bug");
    }

    #[test]
    fn test_parse_empty() {
        let tokens = tokenize("");
        let cmds = parse_chain(tokens).unwrap();
        assert!(cmds.is_empty());
    }

    // === PIPE/FIND/GREP ROUTING TESTS ===
    // These verify that piped invocations of find/grep/rg correctly trigger
    // needs_shell(), so the hook routes them to /bin/sh instead of native mode.

    #[test]
    fn test_needs_shell_find_piped_to_grep() {
        // `find . -name "*.rs" | grep pattern` must go through shell (has Pipe)
        let tokens = tokenize("find . -name \"*.rs\" | grep pattern");
        assert!(
            needs_shell(&tokens),
            "find | grep must trigger shell (Pipe token present)"
        );
    }

    #[test]
    fn test_needs_shell_rg_piped_to_head() {
        let tokens = tokenize("rg pattern src/ | head -20");
        assert!(needs_shell(&tokens), "rg | head must trigger shell");
    }

    #[test]
    fn test_needs_shell_grep_with_redirect() {
        let tokens = tokenize("grep -r pattern . > results.txt");
        assert!(needs_shell(&tokens), "grep > file must trigger shell");
    }

    #[test]
    fn test_needs_shell_find_with_glob_arg() {
        // find . -name *.rs — glob in arg triggers Shellism
        let tokens = tokenize("find . -name *.rs");
        assert!(needs_shell(&tokens), "unquoted glob arg must trigger shell");
    }

    #[test]
    fn test_needs_shell_quoted_pipe_in_grep_arg_no_shell() {
        // grep "a|b" file — pipe inside quotes is an Arg, not a Pipe token
        let tokens = tokenize("grep \"a|b\" src/");
        assert!(
            !needs_shell(&tokens),
            "pipe inside quoted arg must NOT trigger shell"
        );
    }

    #[test]
    fn test_parse_chain_find_with_quoted_name() {
        // `find . -name "*.rs"` with quoted glob → no shellism; parses natively
        let tokens = tokenize("find . -name \"*.rs\"");
        assert!(!needs_shell(&tokens), "quoted glob should not need shell");
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds[0].binary, "find");
        assert!(cmds[0].args.contains(&"-name".to_string()));
        // strip_quotes removes the outer quotes
        assert!(
            cmds[0].args.iter().any(|a| a == "*.rs"),
            "quoted glob stripped to bare glob in args: {:?}",
            cmds[0].args
        );
    }

    #[test]
    fn test_parse_chain_grep_native_no_pipe() {
        // `grep pattern file` — no pipe/redirect, parses natively
        let tokens = tokenize("grep pattern file.rs");
        assert!(!needs_shell(&tokens));
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds[0].binary, "grep");
        assert_eq!(cmds[0].args, vec!["pattern", "file.rs"]);
    }

    // === SHOULD_RUN TESTS ===

    #[test]
    fn test_should_run_and_success() {
        assert!(should_run(Some("&&"), true));
    }

    #[test]
    fn test_should_run_and_failure() {
        assert!(!should_run(Some("&&"), false));
    }

    #[test]
    fn test_should_run_or_success() {
        assert!(!should_run(Some("||"), true));
    }

    #[test]
    fn test_should_run_or_failure() {
        assert!(should_run(Some("||"), false));
    }

    #[test]
    fn test_should_run_semicolon() {
        assert!(should_run(Some(";"), true));
        assert!(should_run(Some(";"), false));
    }

    #[test]
    fn test_should_run_none() {
        assert!(should_run(None, true));
        assert!(should_run(None, false));
    }

    // === SUFFIX REDIRECT TESTS — common patterns Claude Code appends ===
    // These lock in behaviour that must never silently regress.

    // --- stdout / stderr discard ---
    #[test]
    fn test_needs_shell_redirect_to_dev_null() {
        let tokens = tokenize("cmd > /dev/null");
        assert!(needs_shell(&tokens), "> /dev/null must trigger shell");
    }

    #[test]
    fn test_needs_shell_stderr_to_dev_null() {
        // "2>/dev/null" produces a "2>" Redirect token → needs_shell
        let tokens = tokenize("cmd 2>/dev/null");
        assert!(needs_shell(&tokens), "2>/dev/null must trigger shell");
    }

    #[test]
    fn test_needs_shell_stderr_to_dev_null_spaced() {
        let tokens = tokenize("cmd 2> /dev/null");
        assert!(needs_shell(&tokens), "2> /dev/null must trigger shell");
    }

    // --- compound FD redirects (2>&1, 1>&2) ---
    // The `&` in "2>&1" tokenises as Shellism; Shellism triggers needs_shell.
    #[test]
    fn test_needs_shell_stderr_to_stdout() {
        let tokens = tokenize("cmd 2>&1");
        assert!(
            needs_shell(&tokens),
            "2>&1 must trigger shell (& is Shellism)"
        );
    }

    #[test]
    fn test_needs_shell_stdout_to_stderr() {
        let tokens = tokenize("cmd 1>&2");
        assert!(
            needs_shell(&tokens),
            "1>&2 must trigger shell (& is Shellism)"
        );
    }

    #[test]
    fn test_needs_shell_combined_redirect_chain() {
        // Classic "silence everything": >/dev/null 2>&1
        let tokens = tokenize("cmd > /dev/null 2>&1");
        assert!(needs_shell(&tokens), ">/dev/null 2>&1 must trigger shell");
    }

    #[test]
    fn test_needs_shell_redirect_append() {
        let tokens = tokenize("cmd >> /tmp/output.txt");
        assert!(needs_shell(&tokens), ">> must trigger shell");
    }

    #[test]
    fn test_needs_shell_stderr_redirect_to_file() {
        let tokens = tokenize("cmd 2> /tmp/err.log");
        assert!(needs_shell(&tokens), "2> file must trigger shell");
    }

    // --- pipe suffixes ---
    #[test]
    fn test_needs_shell_pipe_to_tail() {
        let tokens = tokenize("git log | tail -20");
        assert!(needs_shell(&tokens), "| tail must trigger shell");
    }

    #[test]
    fn test_needs_shell_pipe_to_cat() {
        // Piping to `cat` forces non-TTY output; must go through shell
        let tokens = tokenize("ls --color | cat");
        assert!(needs_shell(&tokens), "| cat must trigger shell");
    }

    #[test]
    fn test_needs_shell_pipe_to_tee() {
        let tokens = tokenize("cargo build 2>&1 | tee /tmp/build.log");
        assert!(needs_shell(&tokens), "| tee must trigger shell");
    }

    #[test]
    fn test_needs_shell_pipe_to_wc() {
        let tokens = tokenize("find . -name '*.rs' | wc -l");
        assert!(needs_shell(&tokens), "| wc must trigger shell");
    }

    // --- compound chains that must NOT trigger shell alone ---
    #[test]
    fn test_operator_and_does_not_trigger_shell() {
        // && is an Operator, not Shellism/Pipe/Redirect; handled by parse_chain
        let tokens = tokenize("cargo fmt && cargo clippy");
        assert!(
            !needs_shell(&tokens),
            "&& alone must NOT trigger needs_shell"
        );
    }

    #[test]
    fn test_operator_or_does_not_trigger_shell() {
        let tokens = tokenize("cargo test || true");
        assert!(
            !needs_shell(&tokens),
            "|| alone must NOT trigger needs_shell"
        );
    }

    #[test]
    fn test_operator_semicolon_does_not_trigger_shell() {
        let tokens = tokenize("true ; false");
        assert!(
            !needs_shell(&tokens),
            "; alone must NOT trigger needs_shell"
        );
    }

    // --- RTK-rewrite end-to-end: suffix redirect preserves original command ---
    #[test]
    fn test_redirect_suffix_is_passed_through_verbatim() {
        // When needs_shell, the hook wraps in "rtk run -c '<raw>'"
        // Verify routing logic sends the command to passthrough, not native routing
        use crate::cmd::analysis::needs_shell;
        use crate::cmd::lexer::tokenize;
        let raw = "cargo test 2>&1 | tee /tmp/test.log";
        let tokens = tokenize(raw);
        assert!(
            needs_shell(&tokens),
            "complex redirect+pipe must trigger shell passthrough"
        );
    }
}
