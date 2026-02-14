//! Edge case tests for the hybrid command engine.
//! These tests cover corner cases that might cause bugs in real-world usage.

#[cfg(test)]
mod tests {
    use crate::cmd::lexer::{tokenize, strip_quotes, has_shellisms, TokenKind};
    use crate::cmd::analysis::{parse_chain, needs_shell, should_run, NativeCommand};
    use crate::cmd::exec::execute;
    use crate::cmd::safety::{check, check_raw, SafetyResult};

    // ============================================================================
    // LEXER EDGE CASES
    // ============================================================================

    /// Test: Very long argument (10KB+)
    #[test]
    fn test_lexer_very_long_argument() {
        let long_arg = "a".repeat(10000);
        let input = format!("echo {}", long_arg);
        let tokens = tokenize(&input);
        assert_eq!(tokens.len(), 2);
        assert!(tokens[1].value.contains(&"a".repeat(100)));
    }

    /// Test: Newlines and tabs in commands
    #[test]
    fn test_lexer_newlines_and_tabs() {
        let tokens = tokenize("echo\t hello\n world");
        // Newlines and tabs are whitespace, should split
        assert!(tokens.len() >= 2);
        assert!(tokens.iter().any(|t| t.value == "echo"));
    }

    /// Test: Mixed quote styles in same command
    #[test]
    fn test_lexer_mixed_quotes() {
        let tokens = tokenize(r#"echo 'single' "double" 'again'"#);
        assert!(tokens.iter().any(|t| t.value.contains("single")));
        assert!(tokens.iter().any(|t| t.value.contains("double")));
    }

    /// Test: Escape at end of input (backslash as last char)
    #[test]
    fn test_lexer_escape_at_end() {
        let tokens = tokenize("echo hello\\");
        // Should not panic, backslash at end is just part of arg
        assert!(!tokens.is_empty());
    }

    /// Test: Single character commands
    #[test]
    fn test_lexer_single_char_command() {
        let tokens = tokenize("a");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, "a");
    }

    /// Test: Command with only operators
    #[test]
    fn test_lexer_only_operators() {
        let tokens = tokenize("&& || ;");
        let ops: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.kind, TokenKind::Operator))
            .collect();
        assert_eq!(ops.len(), 3);
    }

    /// Test: Backslash followed by quote
    #[test]
    fn test_lexer_escaped_quote_outside_quotes() {
        let tokens = tokenize(r#"echo \""#);
        // Backslash-quote outside quotes
        assert!(!tokens.is_empty());
    }

    /// Test: Multiple consecutive operators
    #[test]
    fn test_lexer_consecutive_operators() {
        let tokens = tokenize("a && b || c ; d");
        let ops: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.kind, TokenKind::Operator))
            .collect();
        assert_eq!(ops.len(), 3);
    }

    /// Test: Heredoc detection (<<)
    #[test]
    fn test_lexer_heredoc_detection() {
        let tokens = tokenize("cat << EOF");
        // < followed by < should be detected as redirects
        let redirects: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.kind, TokenKind::Redirect))
            .collect();
        assert!(!redirects.is_empty());
    }

    /// Test: Empty args between operators
    #[test]
    fn test_lexer_empty_args_between_operators() {
        let tokens = tokenize("a &&   && c");
        // Middle part should not produce an arg
        let ops: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.kind, TokenKind::Operator))
            .collect();
        assert_eq!(ops.len(), 2);
    }

    /// Test: Unicode in command arguments
    #[test]
    fn test_lexer_unicode_args() {
        let tokens = tokenize("echo 日本語 🎉");
        assert!(tokens.iter().any(|t| t.value.contains("日本語")));
    }

    /// Test: Quote inside different quote type
    #[test]
    fn test_lexer_quote_in_other_quote() {
        let tokens = tokenize(r#"echo 'He said "hello"' "#);
        // Double quote inside single quotes should be preserved
        assert!(tokens.iter().any(|t| t.value.contains("\"hello\"")));
    }

    /// Test: Dollar sign at various positions
    #[test]
    fn test_lexer_dollar_positions() {
        // $ at start
        assert!(has_shellisms("$VAR"));

        // $ in middle
        assert!(has_shellisms("echo $VAR"));

        // $ at end (should be shellism)
        assert!(has_shellisms("echo test$"));
    }

    /// Test: Single ampersand (background operator)
    #[test]
    fn test_lexer_single_ampersand() {
        let tokens = tokenize("cmd &");
        // Single & is not &&, should be treated differently
        assert!(tokens.iter().any(|t| t.value == "&"));
    }

    /// Test: Single pipe character
    #[test]
    fn test_lexer_single_pipe() {
        let tokens = tokenize("a | b");
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Pipe)));
    }

    /// Test: Complex redirect patterns
    #[test]
    fn test_lexer_complex_redirects() {
        let tokens = tokenize("cmd 2>&1");
        let redirects: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.kind, TokenKind::Redirect))
            .collect();
        assert!(!redirects.is_empty());
    }

    /// Test: strip_quotes with only opening quote
    #[test]
    fn test_strip_quotes_unclosed() {
        assert_eq!(strip_quotes("\"unclosed"), "\"unclosed");
        assert_eq!(strip_quotes("'unclosed"), "'unclosed");
    }

    /// Test: strip_quotes with single char
    #[test]
    fn test_strip_quotes_single_char() {
        assert_eq!(strip_quotes("a"), "a");
        assert_eq!(strip_quotes("\""), "\"");
    }

    /// Test: Empty quoted string
    #[test]
    fn test_lexer_empty_quoted() {
        let tokens = tokenize("echo ''");
        assert!(tokens.iter().any(|t| t.value == "''"));
    }

    /// Test: Multiple backslashes
    #[test]
    fn test_lexer_multiple_backslashes() {
        let tokens = tokenize(r#"echo \\\\ test"#);
        assert!(!tokens.is_empty());
    }

    /// Test: Backslash-n inside double quotes (not a newline, literal)
    #[test]
    fn test_lexer_backslash_n_in_double_quotes() {
        let tokens = tokenize(r#"echo "\n""#);
        // \n in double quotes should be preserved as literal
        assert!(tokens.iter().any(|t| t.value.contains("\\n")));
    }

    // ============================================================================
    // ANALYSIS EDGE CASES
    // ============================================================================

    /// Test: Empty command after operator should error
    #[test]
    fn test_analysis_empty_after_operator() {
        let tokens = tokenize("&& cmd");
        let result = parse_chain(tokens);
        assert!(result.is_err());
    }

    /// Test: Very long chain
    #[test]
    fn test_analysis_long_chain() {
        let mut input = String::new();
        for i in 0..50 {
            if i > 0 {
                input.push_str(" && ");
            }
            input.push_str(&format!("cmd{}", i));
        }
        let tokens = tokenize(&input);
        let result = parse_chain(tokens);
        assert!(result.is_ok());
        let cmds = result.unwrap();
        assert_eq!(cmds.len(), 50);
    }

    /// Test: Mixed operators in chain
    #[test]
    fn test_analysis_mixed_operators() {
        let tokens = tokenize("a && b || c ; d && e");
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds.len(), 5);
        assert_eq!(cmds[0].operator, Some("&&".to_string()));
        assert_eq!(cmds[1].operator, Some("||".to_string()));
        assert_eq!(cmds[2].operator, Some(";".to_string()));
        assert_eq!(cmds[3].operator, Some("&&".to_string()));
        assert_eq!(cmds[4].operator, None);
    }

    /// Test: Command with many args
    #[test]
    fn test_analysis_many_args() {
        let mut args = String::new();
        for i in 0..100 {
            if i > 0 {
                args.push(' ');
            }
            args.push_str(&format!("arg{}", i));
        }
        let input = format!("cmd {}", args);
        let tokens = tokenize(&input);
        let cmds = parse_chain(tokens).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].args.len(), 100);
    }

    /// Test: should_run with various operators
    #[test]
    fn test_should_run_edge_cases() {
        // && with success -> run
        assert!(should_run(Some("&&"), true));
        // && with failure -> don't run
        assert!(!should_run(Some("&&"), false));
        // || with success -> don't run
        assert!(!should_run(Some("||"), true));
        // || with failure -> run
        assert!(should_run(Some("||"), false));
        // ; always runs
        assert!(should_run(Some(";"), true));
        assert!(should_run(Some(";"), false));
        // None always runs
        assert!(should_run(None, true));
        assert!(should_run(None, false));
        // Unknown operator -> run (safe default)
        assert!(should_run(Some("unknown"), true));
    }

    /// Test: needs_shell with various patterns
    #[test]
    fn test_needs_shell_patterns() {
        // Simple commands don't need shell
        assert!(!needs_shell(&tokenize("git status")));

        // Glob needs shell
        assert!(needs_shell(&tokenize("ls *.rs")));

        // Pipe needs shell
        assert!(needs_shell(&tokenize("cat file | grep x")));

        // Redirect needs shell
        assert!(needs_shell(&tokenize("cmd > file")));

        // Variable needs shell
        assert!(needs_shell(&tokenize("echo $HOME")));

        // Backtick needs shell
        assert!(needs_shell(&tokenize("echo `date`")));

        // Subshell needs shell
        assert!(needs_shell(&tokenize("echo $(date)")));

        // Brace expansion needs shell
        assert!(needs_shell(&tokenize("echo {a,b}.txt")));

        // Operators DON'T need shell
        assert!(!needs_shell(&tokenize("a && b")));
        assert!(!needs_shell(&tokenize("a || b")));
        assert!(!needs_shell(&tokenize("a ; b")));
    }

    // ============================================================================
    // EXEC EDGE CASES
    // ============================================================================

    /// Test: Empty command returns success
    #[test]
    fn test_exec_empty() {
        let result = execute("", 0).unwrap();
        assert!(result);
    }

    /// Test: Whitespace-only command returns success
    #[test]
    fn test_exec_whitespace() {
        let result = execute("   \t\n  ", 0).unwrap();
        assert!(result);
    }

    /// Test: Nonexistent binary
    #[test]
    fn test_exec_nonexistent_binary() {
        let result = execute("nonexistent_command_xyz_12345", 0).unwrap();
        assert!(!result);
    }

    /// Test: True command
    #[test]
    fn test_exec_true() {
        let result = execute("true", 0).unwrap();
        assert!(result);
    }

    /// Test: False command
    #[test]
    fn test_exec_false() {
        let result = execute("false", 0).unwrap();
        assert!(!result);
    }

    /// Test: Chain with all true
    #[test]
    fn test_exec_chain_all_true() {
        let result = execute("true && true && true", 0).unwrap();
        assert!(result);
    }

    /// Test: Chain with one false
    #[test]
    fn test_exec_chain_one_false() {
        let result = execute("true && false && true", 0).unwrap();
        // Stops at false, returns false
        assert!(!result);
    }

    /// Test: || chain with first true
    #[test]
    fn test_exec_or_first_true() {
        let result = execute("true || echo should_not_run", 0).unwrap();
        assert!(result);
    }

    /// Test: || chain with first false
    #[test]
    fn test_exec_or_first_false() {
        let result = execute("false || true", 0).unwrap();
        assert!(result);
    }

    /// Test: Semicolon runs all
    #[test]
    fn test_exec_semicolon_all() {
        let result = execute("false ; true", 0).unwrap();
        // Both run, last result is true
        assert!(result);
    }

    /// Test: Complex chain
    #[test]
    fn test_exec_complex_chain() {
        // false || true && echo works
        // false -> || runs true -> true && runs echo
        let result = execute("false || true && echo works", 0).unwrap();
        assert!(result);
    }

    /// Test: Passthrough for pipe
    #[test]
    fn test_exec_passthrough_pipe() {
        let result = execute("echo hello | cat", 0).unwrap();
        assert!(result);
    }

    /// Test: Passthrough for glob
    #[test]
    fn test_exec_passthrough_glob() {
        let result = execute("echo *", 0).unwrap();
        assert!(result);
    }

    /// Test: Passthrough for redirect
    #[test]
    fn test_exec_passthrough_redirect() {
        // This won't actually create a file in test context
        // but should execute via passthrough
        let result = execute("echo test > /dev/null", 0).unwrap();
        assert!(result);
    }

    /// Test: Quoted operator
    #[test]
    fn test_exec_quoted_operator() {
        let result = execute(r#"echo "hello && world""#, 0).unwrap();
        assert!(result);
    }

    /// Test: Recursion prevention (rtk run inside rtk run)
    #[test]
    fn test_exec_recursion_prevention() {
        let result = execute(r#"rtk run "echo hello""#, 0);
        assert!(result.is_ok());
    }

    // ============================================================================
    // SAFETY EDGE CASES
    // ============================================================================

    use std::sync::{Mutex, MutexGuard};

    // Mutex to serialize tests that modify environment variables
    static ENV_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

    fn env_lock() -> MutexGuard<'static, ()> {
        // Recover from poisoned mutex if a previous test panicked
        ENV_LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn cleanup_safety_env() {
        std::env::remove_var("RTK_SAFE_COMMANDS");
        std::env::remove_var("RTK_BLOCK_TOKEN_WASTE");
    }

    /// Test: rm with various flags
    #[test]
    fn test_safety_rm_flags() {
        let _lock = env_lock();
        cleanup_safety_env();
        std::env::set_var("RTK_SAFE_COMMANDS", "1");

        // -rf
        let result = check("rm", &["-rf".to_string(), "dir".to_string()]);
        if let SafetyResult::TrashRequested(paths) = result {
            assert_eq!(paths, vec!["dir"]);
        } else {
            panic!("Expected TrashRequested");
        }

        // -f
        let result = check("rm", &["-f".to_string(), "file".to_string()]);
        if let SafetyResult::TrashRequested(paths) = result {
            assert_eq!(paths, vec!["file"]);
        } else {
            panic!("Expected TrashRequested");
        }

        // -i (interactive)
        let result = check("rm", &["-i".to_string(), "file".to_string()]);
        if let SafetyResult::TrashRequested(paths) = result {
            assert_eq!(paths, vec!["file"]);
        } else {
            panic!("Expected TrashRequested");
        }

        std::env::remove_var("RTK_SAFE_COMMANDS");
    }

    /// Test: rm with multiple paths
    #[test]
    fn test_safety_rm_multiple() {
        let _lock = env_lock();
        cleanup_safety_env();
        std::env::set_var("RTK_SAFE_COMMANDS", "1");

        let result = check("rm", &["a".to_string(), "b".to_string(), "c".to_string()]);
        if let SafetyResult::TrashRequested(paths) = result {
            assert_eq!(paths, vec!["a", "b", "c"]);
        } else {
            panic!("Expected TrashRequested");
        }

        std::env::remove_var("RTK_SAFE_COMMANDS");
    }

    /// Test: Safety enabled by default (rm->trash, git clean blocked)
    #[test]
    fn test_safety_enabled_by_default() {
        let _lock = env_lock();
        cleanup_safety_env();

        // rm should be redirected to trash by default
        let result = check("rm", &["file".to_string()]);
        assert!(matches!(result, SafetyResult::TrashRequested(_)));

        // git clean should be blocked by default
        let result = check("git", &["clean".to_string(), "-fd".to_string()]);
        assert!(matches!(result, SafetyResult::Blocked(_)));
    }

    /// Test: Safety can be disabled with RTK_SAFE_COMMANDS=0
    #[test]
    fn test_safety_can_be_disabled() {
        let _lock = env_lock();
        cleanup_safety_env();
        std::env::set_var("RTK_SAFE_COMMANDS", "0");

        // rm should pass through when disabled
        let result = check("rm", &["file".to_string()]);
        assert_eq!(result, SafetyResult::Safe);

        // git clean should pass through when disabled
        let result = check("git", &["clean".to_string(), "-fd".to_string()]);
        assert_eq!(result, SafetyResult::Safe);

        std::env::remove_var("RTK_SAFE_COMMANDS");
    }

    /// Test: Token waste prevention enabled by default
    #[test]
    fn test_safety_token_waste_default() {
        let _lock = env_lock();
        cleanup_safety_env();

        // cat should be blocked by default
        let result = check("cat", &["file".to_string()]);
        assert!(matches!(result, SafetyResult::Blocked(_)));

        // sed should be blocked by default
        let result = check("sed", &["-i".to_string(), "s/x/y/".to_string()]);
        assert!(matches!(result, SafetyResult::Blocked(_)));

        // head should be blocked by default
        let result = check("head", &["-n".to_string(), "10".to_string()]);
        assert!(matches!(result, SafetyResult::Blocked(_)));
    }

    /// Test: Token waste prevention can be disabled
    #[test]
    fn test_safety_token_waste_disabled() {
        let _lock = env_lock();
        cleanup_safety_env();
        std::env::set_var("RTK_BLOCK_TOKEN_WASTE", "0");

        // cat should pass through
        let result = check("cat", &["file".to_string()]);
        assert_eq!(result, SafetyResult::Safe);

        // sed should pass through
        let result = check("sed", &["-i".to_string(), "s/x/y/".to_string()]);
        assert_eq!(result, SafetyResult::Safe);

        std::env::remove_var("RTK_BLOCK_TOKEN_WASTE");
    }

    /// Test: check_raw with various patterns
    #[test]
    fn test_safety_check_raw_patterns() {
        let _lock = env_lock();
        cleanup_safety_env();
        std::env::set_var("RTK_SAFE_COMMANDS", "1");

        // rm at start
        let result = check_raw("rm file");
        assert!(matches!(result, SafetyResult::Blocked(_)));

        // rm with sudo
        let result = check_raw("sudo rm file");
        assert!(matches!(result, SafetyResult::Blocked(_)));

        // rm with absolute path
        let result = check_raw("/bin/rm file");
        assert!(matches!(result, SafetyResult::Blocked(_)));

        // Safe command
        let result = check_raw("ls -la");
        assert_eq!(result, SafetyResult::Safe);

        std::env::remove_var("RTK_SAFE_COMMANDS");
    }

    /// Test: Empty args
    #[test]
    fn test_safety_empty_args() {
        let _lock = env_lock();
        cleanup_safety_env();

        let result = check("pwd", &[]);
        assert_eq!(result, SafetyResult::Safe);
    }

    /// Test: Unknown command
    #[test]
    fn test_safety_unknown_command() {
        let _lock = env_lock();
        cleanup_safety_env();

        let result = check("unknowncmd", &["arg".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
    }

    // ============================================================================
    // INTEGRATION EDGE CASES
    // ============================================================================

    /// Test: Builtin cd with tilde
    #[test]
    fn test_integration_cd_tilde() {
        let original = std::env::current_dir().unwrap();
        let result = execute("cd ~", 0).unwrap();
        assert!(result);
        let _ = std::env::set_current_dir(&original);
    }

    /// Test: Builtin echo
    #[test]
    fn test_integration_echo() {
        let result = execute("echo hello world", 0).unwrap();
        assert!(result);
    }

    /// Test: Builtin pwd
    #[test]
    fn test_integration_pwd() {
        let result = execute("pwd", 0).unwrap();
        assert!(result);
    }

    /// Test: Export builtin
    #[test]
    fn test_integration_export() {
        let result = execute("export TEST_VAR=value", 0).unwrap();
        assert!(result);
        std::env::remove_var("TEST_VAR");
    }

    /// Test: Command with env prefix
    #[test]
    fn test_integration_env_prefix() {
        let result = execute("TEST=1 echo hello", 0);
        // Should either work via passthrough or handle gracefully
        assert!(result.is_ok());
    }

    /// Test: Very short command
    #[test]
    fn test_integration_short_command() {
        let result = execute("ls", 0).unwrap();
        assert!(result);
    }

    /// Test: Command with dashes in args
    #[test]
    fn test_integration_dash_args() {
        let result = execute("echo --help -v --version", 0).unwrap();
        assert!(result);
    }

    /// Test: Quoted empty string
    #[test]
    fn test_integration_quoted_empty() {
        let result = execute(r#"echo """#, 0).unwrap();
        assert!(result);
    }
}
