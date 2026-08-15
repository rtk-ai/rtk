//! `rtk-shell` — thin, minimal entry point for the rtk-managed shell.
//!
//! Deliberately distinct from the main `rtk` Clap-based CLI
//! (`src/main.rs`): this binary inspects `argv` itself instead of going
//! through Clap, because it needs to behave like a drop-in backing shell
//! (`sh`/`bash`/`zsh`), not like a subcommand-based CLI.
//!
//! Supported invocations:
//! - `rtk-shell -c "<command line>"` — one-shot mode: run a single command
//!   line and exit (mirrors `sh -c`). Everything after `-c` is treated as a
//!   single command string, exactly like `sh -c` does.
//! - `rtk-shell` (no arguments) — persistent-session mode: start an
//!   interactive, rtk-managed shell session.
//!
//! Anything else (missing operand after `-c`, unrecognized flags, etc.) is
//! reported as a usage error on stderr with a non-zero exit code; it does
//! not fall back to Clap parsing.

use rtk::shell::{oneshot, session};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let code = match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rtk-shell: {:#}", e);
            1
        }
    };

    std::process::exit(code);
}

/// Dispatch parsed `argv` (program name already stripped) to one-shot or
/// session mode. Kept separate from `main` so it can be unit-tested without
/// calling `std::process::exit`.
fn run(args: &[String]) -> anyhow::Result<i32> {
    match args {
        [] => session::run(),
        [flag, line] if flag == "-c" => oneshot::run(line),
        [flag] if flag == "-c" => {
            anyhow::bail!("rtk-shell: -c requires a command string argument")
        }
        _ => {
            anyhow::bail!(
                "rtk-shell: unrecognized arguments {:?}\nUsage: rtk-shell [-c <command>]",
                args
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_args_reaches_session_implementation() {
        // With no args pending on the test process's stdin, the session
        // loop hits EOF immediately and exits cleanly (not the arg-parsing
        // usage error), now that session mode is implemented.
        let result = run(&[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_dash_c_reaches_oneshot_implementation() {
        // Reaches oneshot::run (not the arg-parsing error) and actually
        // executes the command now that oneshot mode is implemented.
        let result = run(&["-c".to_string(), "echo hi".to_string()]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_dash_c_without_command_is_usage_error() {
        let err = run(&["-c".to_string()]).unwrap_err();
        assert!(err.to_string().contains("requires a command string"));
    }

    #[test]
    fn test_unrecognized_args_is_usage_error() {
        let err = run(&["--bogus".to_string()]).unwrap_err();
        assert!(err.to_string().contains("unrecognized arguments"));
    }
}
