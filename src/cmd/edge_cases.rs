//! Cross-module integration tests for the hybrid command engine.
//!
//! These tests exercise the full execute() pipeline: lexer → analysis → safety → exec.
//! Unit tests for individual modules live in their respective files.

#[cfg(test)]
mod tests {
    use crate::cmd::exec::execute;

    // ============================================================================
    // EXEC PIPELINE: OPERATOR SEMANTICS
    // ============================================================================

    #[test]
    fn test_chain_and_stops_on_failure() {
        let result = execute("true && false && true", 0).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_chain_or_skips_on_success() {
        let result = execute("true || echo should_not_run", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_chain_or_runs_on_failure() {
        let result = execute("false || true", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_chain_semicolon_runs_all() {
        let result = execute("false ; true", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_chain_mixed_operators() {
        // false -> || runs true -> true && runs echo
        let result = execute("false || true && echo works", 0).unwrap();
        assert!(result);
    }

    // ============================================================================
    // EXEC PIPELINE: SHELL PASSTHROUGH
    // ============================================================================

    #[test]
    fn test_passthrough_pipe() {
        let result = execute("echo hello | cat", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_passthrough_glob() {
        let result = execute("echo *", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_passthrough_redirect() {
        let result = execute("echo test > /dev/null", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_passthrough_quoted_operator() {
        let result = execute(r#"echo "hello && world""#, 0).unwrap();
        assert!(result);
    }

    // ============================================================================
    // EXEC PIPELINE: BUILTIN INTEGRATION
    // ============================================================================

    #[test]
    fn test_integration_cd_tilde() {
        let original = std::env::current_dir().unwrap();
        let result = execute("cd ~", 0).unwrap();
        assert!(result);
        let _ = std::env::set_current_dir(&original);
    }

    #[test]
    fn test_integration_echo() {
        let result = execute("echo hello world", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_integration_pwd() {
        let result = execute("pwd", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_integration_export() {
        let result = execute("export TEST_VAR=value", 0).unwrap();
        assert!(result);
        std::env::remove_var("TEST_VAR");
    }

    #[test]
    fn test_integration_env_prefix() {
        let result = execute("TEST=1 echo hello", 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_integration_dash_args() {
        let result = execute("echo --help -v --version", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_integration_quoted_empty() {
        let result = execute(r#"echo """#, 0).unwrap();
        assert!(result);
    }
}
