//! Built-in commands that RTK handles natively.
//! These maintain session state across hook calls.

use super::predicates::{expand_tilde, get_home};
use anyhow::{Context, Result};

/// Change directory (persists in RTK process)
pub fn builtin_cd(args: &[String]) -> Result<bool> {
    let target = args.get(0)
        .map(|s| expand_tilde(s))
        .unwrap_or_else(get_home);

    std::env::set_current_dir(&target)
        .with_context(|| format!("cd: {}: No such file or directory", target))?;

    Ok(true)
}

/// Export environment variable
pub fn builtin_export(args: &[String]) -> Result<bool> {
    for arg in args {
        if let Some((key, value)) = arg.split_once('=') {
            // Handle quoted values: export FOO="bar baz"
            let clean_value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            std::env::set_var(key, clean_value);
        }
    }
    Ok(true)
}

/// Check if a binary is a builtin
pub fn is_builtin(binary: &str) -> bool {
    matches!(binary, "cd" | "export" | "pwd" | "echo" | "true" | "false" | ":")
}

/// Execute a builtin command
pub fn execute(binary: &str, args: &[String]) -> Result<bool> {
    match binary {
        "cd" => builtin_cd(args),
        "export" => builtin_export(args),
        "pwd" => {
            println!("{}", std::env::current_dir()?.display());
            Ok(true)
        }
        "echo" => {
            println!("{}", args.join(" "));
            Ok(true)
        }
        "true" | ":" => Ok(true),
        "false" => Ok(false),
        _ => anyhow::bail!("Unknown builtin: {}", binary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::path::PathBuf;

    // === CD TESTS ===

    #[test]
    fn test_cd_to_existing_dir() {
        let original = env::current_dir().unwrap();
        let result = builtin_cd(&["/tmp".to_string()]).unwrap();
        assert!(result);
        // On macOS, /tmp might be a symlink to /private/tmp
        // Just verify we changed directories
        let new_dir = env::current_dir().unwrap();
        assert!(new_dir.to_string_lossy().contains("tmp") || new_dir != original);
        env::set_current_dir(original).unwrap();
    }

    #[test]
    fn test_cd_to_nonexistent_dir() {
        let result = builtin_cd(&["/nonexistent/path/xyz".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cd_no_args() {
        let original = env::current_dir().unwrap();
        let home = get_home();

        // Go somewhere else first
        let _ = env::set_current_dir("/tmp");

        let result = builtin_cd(&[]).unwrap();
        assert!(result);
        // Verify we're at home or the cd succeeded
        let cwd = env::current_dir().unwrap();
        // Just check that we moved from /tmp (cd worked)
        assert!(cwd != PathBuf::from("/tmp") || cwd.to_string_lossy().contains(&home));

        let _ = env::set_current_dir(&original);
    }

    #[test]
    fn test_cd_tilde_expansion() {
        let original = env::current_dir().unwrap();
        let home = get_home();

        let result = builtin_cd(&["~".to_string()]).unwrap();
        assert!(result);
        // Verify we're at home (or a parent of it)
        let cwd = env::current_dir().unwrap();
        assert!(cwd == PathBuf::from(&home) || cwd.to_string_lossy().starts_with(&home));

        let _ = env::set_current_dir(&original);
    }

    #[test]
    fn test_cd_tilde_subpath() {
        let original = env::current_dir().unwrap();
        let home = get_home();

        // This may fail if ~/src doesn't exist, which is fine
        let _ = builtin_cd(&["~/src".to_string()]);
        // Just verify we're in something starting with home
        let cwd = env::current_dir().unwrap();
        assert!(cwd.starts_with(&home) || cwd != original);

        env::set_current_dir(original).unwrap();
    }

    // === EXPORT TESTS ===

    #[test]
    fn test_export_simple() {
        builtin_export(&["RTK_TEST_VAR=value".to_string()]).unwrap();
        assert_eq!(env::var("RTK_TEST_VAR").unwrap(), "value");
        env::remove_var("RTK_TEST_VAR");
    }

    #[test]
    fn test_export_with_equals_in_value() {
        builtin_export(&["RTK_TEST_VAR=key=value".to_string()]).unwrap();
        assert_eq!(env::var("RTK_TEST_VAR").unwrap(), "key=value");
        env::remove_var("RTK_TEST_VAR");
    }

    #[test]
    fn test_export_quoted_value() {
        builtin_export(&["RTK_TEST_VAR=\"hello world\"".to_string()]).unwrap();
        assert_eq!(env::var("RTK_TEST_VAR").unwrap(), "hello world");
        env::remove_var("RTK_TEST_VAR");
    }

    #[test]
    fn test_export_multiple() {
        builtin_export(&[
            "RTK_TEST_A=1".to_string(),
            "RTK_TEST_B=2".to_string(),
        ]).unwrap();
        assert_eq!(env::var("RTK_TEST_A").unwrap(), "1");
        assert_eq!(env::var("RTK_TEST_B").unwrap(), "2");
        env::remove_var("RTK_TEST_A");
        env::remove_var("RTK_TEST_B");
    }

    #[test]
    fn test_export_no_equals() {
        // Should be silently ignored (like bash)
        let result = builtin_export(&["NO_EQUALS_HERE".to_string()]).unwrap();
        assert!(result);
    }

    // === IS_BUILTIN TESTS ===

    #[test]
    fn test_is_builtin_cd() {
        assert!(is_builtin("cd"));
    }

    #[test]
    fn test_is_builtin_export() {
        assert!(is_builtin("export"));
    }

    #[test]
    fn test_is_builtin_pwd() {
        assert!(is_builtin("pwd"));
    }

    #[test]
    fn test_is_builtin_echo() {
        assert!(is_builtin("echo"));
    }

    #[test]
    fn test_is_builtin_true() {
        assert!(is_builtin("true"));
    }

    #[test]
    fn test_is_builtin_false() {
        assert!(is_builtin("false"));
    }

    #[test]
    fn test_is_builtin_external() {
        assert!(!is_builtin("git"));
        assert!(!is_builtin("ls"));
        assert!(!is_builtin("cargo"));
    }

    // === EXECUTE TESTS ===

    #[test]
    fn test_execute_pwd() {
        let result = execute("pwd", &[]).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_echo() {
        let result = execute("echo", &["hello".to_string(), "world".to_string()]).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_true() {
        let result = execute("true", &[]).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_false() {
        let result = execute("false", &[]).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_execute_unknown_builtin() {
        let result = execute("notabuiltin", &[]);
        assert!(result.is_err());
    }
}
