//! Built-in commands that RTK handles natively within a single `rtk run -c` invocation.
//! Note: state does NOT persist across separate Claude Code hook calls (each is a new process).

use super::predicates::{expand_tilde, get_home};
use anyhow::{Context, Result};

/// Change directory within the current `rtk run -c` invocation.
/// Does NOT persist across separate hook invocations.
pub fn builtin_cd(args: &[String]) -> Result<bool> {
    let target = args
        .first()
        .map(|s| expand_tilde(s))
        .unwrap_or_else(get_home);

    std::env::set_current_dir(&target)
        .with_context(|| format!("cd: {}: No such file or directory", target))?;

    Ok(true)
}

/// Returns true if the name is a valid POSIX shell identifier: [A-Za-z_][A-Za-z0-9_]*
fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Export environment variable
pub fn builtin_export(args: &[String]) -> Result<bool> {
    for arg in args {
        if let Some((key, value)) = arg.split_once('=') {
            // Reject invalid identifiers (e.g. "123=x") — fail-open: skip without error.
            // bash rejects these with "not a valid identifier"; we silently skip to preserve
            // RTK's fail-open principle (user workflow is never broken).
            if !is_valid_env_name(key) {
                continue;
            }
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
    matches!(
        binary,
        "cd" | "export" | "pwd" | "echo" | "true" | "false" | ":"
    )
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
