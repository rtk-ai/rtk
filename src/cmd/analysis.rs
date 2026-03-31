use super::lexer::{strip_quotes, ParsedToken, TokenKind};

#[derive(Debug, Clone, PartialEq)]
pub struct NativeCommand {
    pub binary: String,
    pub args: Vec<String>,
    pub operator: Option<String>,
}

pub fn split_safe_suffix(mut tokens: Vec<ParsedToken>) -> (Vec<ParsedToken>, String) {
    let mut suffixes: Vec<String> = Vec::new();

    loop {
        let n = tokens.len();
        let mut matched_len: usize = 0;
        let mut matched_suffix = String::new();

        if n >= 4 {
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

        if matched_len == 0 && n >= 3 {
            let t = &tokens[n - 2..];
            if matches!(t[0].kind, TokenKind::Redirect)
                && t[0].value.starts_with('2')
                && t[0].value.contains('>')
                && !t[0].value.contains('&')
                && matches!(t[1].kind, TokenKind::Arg)
                && t[1].value == "/dev/null"
            {
                matched_suffix = format!("{}{}", t[0].value, t[1].value);
                matched_len = 2;
            }
        }

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

        if matched_len == 0 && n >= 2 {
            let last = &tokens[n - 1];
            if matches!(last.kind, TokenKind::Redirect) && last.value.contains(">&") {
                matched_suffix = last.value.clone();
                matched_len = 1;
            }
        }

        if matched_len == 0 && n >= 2 {
            let last = &tokens[n - 1];
            if matches!(last.kind, TokenKind::Shellism) && last.value == "&" {
                matched_suffix = "&".to_string();
                matched_len = 1;
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

pub fn needs_shell(tokens: &[ParsedToken]) -> bool {
    tokens.iter().any(|t| {
        matches!(
            t.kind,
            TokenKind::Shellism | TokenKind::Pipe | TokenKind::Redirect
        )
    })
}

pub fn parse_chain(tokens: Vec<ParsedToken>) -> Result<Vec<NativeCommand>, String> {
    let mut commands = Vec::new();
    let mut current_args = Vec::new();

    for token in tokens {
        match token.kind {
            TokenKind::Arg => {
                current_args.push(strip_quotes(&token.value));
            }
            TokenKind::Operator => {
                if current_args.is_empty() {
                    return Err(format!(
                        "Syntax error: operator {} with no command",
                        token.value
                    ));
                }
                let binary = current_args.remove(0);
                commands.push(NativeCommand {
                    binary,
                    args: current_args.clone(),
                    operator: Some(token.value.clone()),
                });
                current_args.clear();
            }
            TokenKind::Pipe | TokenKind::Redirect | TokenKind::Shellism => {
                return Err(format!(
                    "Unexpected {:?} in native mode - use passthrough",
                    token.kind
                ));
            }
        }
    }

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

pub fn should_run(operator: Option<&str>, last_success: bool) -> bool {
    match operator {
        Some("&&") => last_success,
        Some("||") => !last_success,
        Some(";") | None => true,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::lexer::tokenize;

    #[test]
    fn test_split_suffix_2_redirect() {
        let tokens = tokenize("cargo test 2>&1");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "2>&1");
        assert!(!needs_shell(&core));
        let cmds = parse_chain(core).unwrap();
        assert_eq!(cmds[0].binary, "cargo");
        assert_eq!(cmds[0].args, vec!["test"]);
    }

    #[test]
    fn test_split_suffix_dev_null() {
        let tokens = tokenize("cargo test 2>/dev/null");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "2>/dev/null");
        let cmds = parse_chain(core).unwrap();
        assert_eq!(cmds[0].binary, "cargo");
    }

    #[test]
    fn test_split_suffix_stdout_dev_null() {
        let tokens = tokenize("cargo test > /dev/null");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "> /dev/null");
        let cmds = parse_chain(core).unwrap();
        assert_eq!(cmds[0].binary, "cargo");
    }

    #[test]
    fn test_split_suffix_pipe_tee() {
        let tokens = tokenize("cargo test | tee /tmp/log.txt");
        let (core, suffix) = split_safe_suffix(tokens);
        assert!(suffix.starts_with("| tee"), "suffix: {suffix}");
        assert!(suffix.contains("/tmp/log.txt"), "suffix: {suffix}");
        let cmds = parse_chain(core).unwrap();
        assert_eq!(cmds[0].binary, "cargo");
    }

    #[test]
    fn test_split_suffix_pipe_head() {
        let tokens = tokenize("git log | head -20");
        let (core, suffix) = split_safe_suffix(tokens);
        assert!(suffix.starts_with("| head"), "suffix: {suffix}");
        let cmds = parse_chain(core).unwrap();
        assert_eq!(cmds[0].binary, "git");
    }

    #[test]
    fn test_split_suffix_pipe_tail() {
        let tokens = tokenize("git log | tail -10");
        let (_core, suffix) = split_safe_suffix(tokens);
        assert!(suffix.starts_with("| tail"), "suffix: {suffix}");
    }

    #[test]
    fn test_split_suffix_pipe_cat() {
        let tokens = tokenize("ls --color | cat");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "| cat");
        let cmds = parse_chain(core).unwrap();
        assert_eq!(cmds[0].binary, "ls");
    }

    #[test]
    fn test_split_suffix_append_redirect() {
        let tokens = tokenize("cargo build >> /tmp/build.log");
        let (core, suffix) = split_safe_suffix(tokens);
        assert!(suffix.starts_with(">>"), "suffix: {suffix}");
        let cmds = parse_chain(core).unwrap();
        assert_eq!(cmds[0].binary, "cargo");
    }

    #[test]
    fn test_split_suffix_none() {
        let tokens = tokenize("cargo test");
        let n = tokens.len();
        let (core, suffix) = split_safe_suffix(tokens);
        assert!(suffix.is_empty(), "no suffix expected, got: {suffix}");
        assert_eq!(core.len(), n);
    }

    #[test]
    fn test_split_suffix_glob_core_stays_shellism() {
        let tokens = tokenize("ls *.rs 2>&1");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "2>&1");
        assert!(needs_shell(&core));
    }

    #[test]
    fn test_split_suffix_requires_core_token() {
        let tokens = tokenize("2>&1");
        let (core, suffix) = split_safe_suffix(tokens);
        assert!(
            suffix.is_empty() || core.is_empty(),
            "bare suffix with no core should not produce a valid split"
        );
    }

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
        assert!(!needs_shell(&tokens));
    }

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
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].operator, Some("&&".to_string()));
    }

    #[test]
    fn test_parse_quoted_arg() {
        let tokens = tokenize("git commit -m \"Fix && Bug\"");
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].args.len(), 3);
        assert_eq!(cmds[0].args[2], "Fix && Bug");
    }

    #[test]
    fn test_parse_empty() {
        let tokens = tokenize("");
        let cmds = parse_chain(tokens).unwrap();
        assert!(cmds.is_empty());
    }

    #[test]
    fn test_needs_shell_find_piped_to_grep() {
        let tokens = tokenize("find . -name \"*.rs\" | grep pattern");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_rg_piped_to_head() {
        let tokens = tokenize("rg pattern src/ | head -20");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_grep_with_redirect() {
        let tokens = tokenize("grep -r pattern . > results.txt");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_find_with_glob_arg() {
        let tokens = tokenize("find . -name *.rs");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_quoted_pipe_in_grep_arg_no_shell() {
        let tokens = tokenize("grep \"a|b\" src/");
        assert!(!needs_shell(&tokens));
    }

    #[test]
    fn test_parse_chain_find_with_quoted_name() {
        let tokens = tokenize("find . -name \"*.rs\"");
        assert!(!needs_shell(&tokens));
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds[0].binary, "find");
        assert!(cmds[0].args.contains(&"-name".to_string()));
        assert!(
            cmds[0].args.iter().any(|a| a == "*.rs"),
            "quoted glob stripped to bare glob in args: {:?}",
            cmds[0].args
        );
    }

    #[test]
    fn test_parse_chain_grep_native_no_pipe() {
        let tokens = tokenize("grep pattern file.rs");
        assert!(!needs_shell(&tokens));
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds[0].binary, "grep");
        assert_eq!(cmds[0].args, vec!["pattern", "file.rs"]);
    }

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

    #[test]
    fn test_needs_shell_redirect_to_dev_null() {
        let tokens = tokenize("cmd > /dev/null");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_stderr_to_dev_null() {
        let tokens = tokenize("cmd 2>/dev/null");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_stderr_to_dev_null_spaced() {
        let tokens = tokenize("cmd 2> /dev/null");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_stderr_to_stdout() {
        let tokens = tokenize("cmd 2>&1");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_stdout_to_stderr() {
        let tokens = tokenize("cmd 1>&2");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_combined_redirect_chain() {
        let tokens = tokenize("cmd > /dev/null 2>&1");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_redirect_append() {
        let tokens = tokenize("cmd >> /tmp/output.txt");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_stderr_redirect_to_file() {
        let tokens = tokenize("cmd 2> /tmp/err.log");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_pipe_to_tail() {
        let tokens = tokenize("git log | tail -20");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_pipe_to_cat() {
        let tokens = tokenize("ls --color | cat");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_pipe_to_tee() {
        let tokens = tokenize("cargo build 2>&1 | tee /tmp/build.log");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_needs_shell_pipe_to_wc() {
        let tokens = tokenize("find . -name '*.rs' | wc -l");
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_operator_and_does_not_trigger_shell() {
        let tokens = tokenize("cargo fmt && cargo clippy");
        assert!(!needs_shell(&tokens));
    }

    #[test]
    fn test_operator_or_does_not_trigger_shell() {
        let tokens = tokenize("cargo test || true");
        assert!(!needs_shell(&tokens));
    }

    #[test]
    fn test_operator_semicolon_does_not_trigger_shell() {
        let tokens = tokenize("true ; false");
        assert!(!needs_shell(&tokens));
    }

    #[test]
    fn test_redirect_suffix_is_passed_through_verbatim() {
        let raw = "cargo test 2>&1 | tee /tmp/test.log";
        let tokens = tokenize(raw);
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_background_job_suffix_simple() {
        let tokens = tokenize("cargo build &");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "&");
        assert_eq!(core.len(), 2);
        assert!(!needs_shell(&core));
    }

    #[test]
    fn test_background_job_suffix_git_status() {
        let tokens = tokenize("git status &");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "&");
        assert_eq!(core.len(), 2);
        assert!(!needs_shell(&core));
    }

    #[test]
    fn test_background_job_suffix_with_fd_redirect() {
        // With the current lexer, 2>&1 is a single Redirect token (no Shellism),
        // so both 2>&1 and & are safely stripped as independent suffixes
        let tokens = tokenize("cargo build 2>&1 &");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "2>&1 &");
        assert!(!needs_shell(&core));
    }

    #[test]
    fn test_background_job_suffix_single_token_not_stripped() {
        let tokens = tokenize("&");
        let (core, suffix) = split_safe_suffix(tokens);
        assert!(suffix.is_empty());
        assert_eq!(core.len(), 1);
    }

    #[test]
    fn test_cargo_test_pipe_grep_is_not_safe_suffix() {
        let tokens = tokenize("cargo test | grep FAILED");
        let (_core, suffix) = split_safe_suffix(tokens.clone());
        assert!(suffix.is_empty());
        assert!(needs_shell(&tokens));
    }

    #[test]
    fn test_nohup_background_strips_ampersand() {
        let tokens = tokenize("nohup cargo build &");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "&");
        assert_eq!(core[0].value, "nohup");
        assert_eq!(core.len(), 3);
        assert!(!needs_shell(&core));
    }

    #[test]
    fn test_split_suffix_compound_redirect_pipe_tail() {
        let tokens = tokenize("cargo test 2>&1 | tail -50");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "2>&1 | tail -50");
        assert!(!needs_shell(&core));
        let cmds = parse_chain(core).expect("core must parse");
        assert_eq!(cmds[0].binary, "cargo");
        assert_eq!(cmds[0].args, vec!["test"]);
    }

    #[test]
    fn test_split_suffix_compound_devnull_redirect() {
        let tokens = tokenize("cmd > /dev/null 2>&1");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "> /dev/null 2>&1");
        assert!(!needs_shell(&core));
        assert_eq!(core.len(), 1);
    }

    #[test]
    fn test_split_suffix_compound_redirect_pipe_tee() {
        let tokens = tokenize("cargo build 2>&1 | tee /tmp/log");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "2>&1 | tee /tmp/log");
        assert!(!needs_shell(&core));
    }

    #[test]
    fn test_split_suffix_triple_compound() {
        let tokens = tokenize("cmd >> /tmp/log 2>&1 | tail -5");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, ">> /tmp/log 2>&1 | tail -5");
        assert!(!needs_shell(&core));
        assert_eq!(core.len(), 1);
    }

    #[test]
    fn test_split_suffix_unsafe_pipe_with_redirect_not_stripped() {
        let tokens = tokenize("cargo test | grep FAILED 2>&1");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "2>&1");
        assert!(needs_shell(&core));
    }

    #[test]
    fn test_split_suffix_devnull_background() {
        let tokens = tokenize("cargo build > /dev/null &");
        let (core, suffix) = split_safe_suffix(tokens);
        assert_eq!(suffix, "> /dev/null &");
        assert!(!needs_shell(&core));
    }
}
