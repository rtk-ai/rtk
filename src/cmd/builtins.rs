use super::predicates::{expand_tilde, get_home};
use anyhow::{Context, Result};

pub fn builtin_cd(args: &[String]) -> Result<bool> {
    let target = args
        .first()
        .map(|s| expand_tilde(s))
        .unwrap_or_else(get_home);

    std::env::set_current_dir(&target)
        .with_context(|| format!("cd: {}: No such file or directory", target))?;

    Ok(true)
}

fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn builtin_export(args: &[String]) -> Result<bool> {
    for arg in args {
        if let Some((key, value)) = arg.split_once('=') {
            if !is_valid_env_name(key) {
                continue;
            }
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

pub fn is_builtin(binary: &str) -> bool {
    matches!(
        binary,
        "cd" | "export" | "pwd" | "echo" | "true" | "false" | ":"
    )
}

pub fn execute(binary: &str, args: &[String]) -> Result<bool> {
    match binary {
        "cd" => builtin_cd(args),
        "export" => builtin_export(args),
        "pwd" => {
            println!("{}", std::env::current_dir()?.display());
            Ok(true)
        }
        "echo" => {
            let (print_args, no_newline) = if args.first().map(|s| s.as_str()) == Some("-n") {
                (&args[1..], true)
            } else {
                (args, false)
            };
            print!("{}", print_args.join(" "));
            if !no_newline {
                println!();
            }
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

    #[test]
    fn test_cd_all_cases() {
        let original = env::current_dir().unwrap();
        let home = get_home();

        let result = builtin_cd(&["/tmp".to_string()]).unwrap();
        assert!(result);
        let new_dir = env::current_dir().unwrap();
        let canon_tmp = std::fs::canonicalize("/tmp").unwrap();
        let canon_new = std::fs::canonicalize(&new_dir).unwrap();
        assert_eq!(canon_new, canon_tmp, "cd /tmp should land in /tmp");

        let result = builtin_cd(&["/nonexistent/path/xyz".to_string()]);
        assert!(result.is_err());
        assert_eq!(
            std::fs::canonicalize(env::current_dir().unwrap()).unwrap(),
            canon_tmp
        );

        let result = builtin_cd(&[]).unwrap();
        assert!(result);
        let cwd = env::current_dir().unwrap();
        let canon_home = std::fs::canonicalize(&home).unwrap();
        let canon_cwd = std::fs::canonicalize(&cwd).unwrap();
        assert_eq!(canon_cwd, canon_home, "cd with no args should go home");

        let _ = env::set_current_dir("/tmp");
        let result = builtin_cd(&["~".to_string()]).unwrap();
        assert!(result);
        let cwd = std::fs::canonicalize(env::current_dir().unwrap()).unwrap();
        assert_eq!(cwd, canon_home, "cd ~ should go home");

        let _ = builtin_cd(&["~/nonexistent_rtk_test_subpath_xyz".to_string()]);

        let _ = env::set_current_dir(&original);
    }

    #[test]
    fn test_export_simple() {
        builtin_export(&["RTK_TEST_SIMPLE=value".to_string()]).unwrap();
        assert_eq!(env::var("RTK_TEST_SIMPLE").unwrap(), "value");
        env::remove_var("RTK_TEST_SIMPLE");
    }

    #[test]
    fn test_export_with_equals_in_value() {
        builtin_export(&["RTK_TEST_EQUALS=key=value".to_string()]).unwrap();
        assert_eq!(env::var("RTK_TEST_EQUALS").unwrap(), "key=value");
        env::remove_var("RTK_TEST_EQUALS");
    }

    #[test]
    fn test_export_quoted_value() {
        builtin_export(&["RTK_TEST_QUOTED=\"hello world\"".to_string()]).unwrap();
        assert_eq!(env::var("RTK_TEST_QUOTED").unwrap(), "hello world");
        env::remove_var("RTK_TEST_QUOTED");
    }

    #[test]
    fn test_export_multiple() {
        builtin_export(&["RTK_TEST_A=1".to_string(), "RTK_TEST_B=2".to_string()]).unwrap();
        assert_eq!(env::var("RTK_TEST_A").unwrap(), "1");
        assert_eq!(env::var("RTK_TEST_B").unwrap(), "2");
        env::remove_var("RTK_TEST_A");
        env::remove_var("RTK_TEST_B");
    }

    #[test]
    fn test_export_no_equals() {
        let result = builtin_export(&["NO_EQUALS_HERE".to_string()]).unwrap();
        assert!(result);
    }

    #[test]
    fn test_export_invalid_identifier_ignored() {
        let result = builtin_export(&["123=x".to_string()]).unwrap();
        assert!(
            result,
            "builtin_export must succeed even with invalid identifier"
        );
        assert!(
            env::var("123").is_err(),
            "var with numeric-start name must not be set"
        );
    }

    #[test]
    fn test_export_empty_name_ignored() {
        let result = builtin_export(&["=x".to_string()]).unwrap();
        assert!(result);
    }

    #[test]
    fn test_is_valid_env_name() {
        assert!(is_valid_env_name("FOO"));
        assert!(is_valid_env_name("_FOO"));
        assert!(is_valid_env_name("foo_bar_123"));
        assert!(!is_valid_env_name("123foo"));
        assert!(!is_valid_env_name(""));
        assert!(!is_valid_env_name("foo-bar"));
        assert!(!is_valid_env_name("foo bar"));
    }

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

    #[test]
    fn test_execute_echo_n_flag() {
        let result = execute("echo", &["-n".to_string(), "hello".to_string()]).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_echo_empty_args() {
        let result = execute("echo", &[]).unwrap();
        assert!(result);
    }
}
