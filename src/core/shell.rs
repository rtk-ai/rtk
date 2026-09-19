//! Builds direct and explicit-shell commands without guessing the caller's shell.

use crate::core::utils::{resolve_binary, resolved_command};
use anyhow::{Context, Result, bail};
use std::borrow::Cow;
use std::path::Path;
use std::process::Command;

/// POSIX "command not found".
///
/// Direct execution resolves the program itself, so nothing is left to report
/// the code the interposed `sh -c` used to return. Callers keep returning it:
/// CI steps branch on 127, and RTK's own `[FAIL]` line carries it.
pub const EXIT_COMMAND_NOT_FOUND: i32 = 127;

/// A command ready to spawn, or the program name that could not be resolved.
///
/// A missing program is an execution outcome, not an RTK error: the runners
/// render it through their own failure path and exit [`EXIT_COMMAND_NOT_FOUND`],
/// the way the shell they replaced did.
pub enum Launch {
    Ready(Command),
    NotFound(String),
}

/// The output a shell prints for a program it cannot resolve, in RTK's voice.
///
/// Fed to the runners' filters so a missing program renders like any other
/// failed run instead of surfacing as an `anyhow` chain on stderr.
pub fn not_found_output(program: &str) -> String {
    format!("rtk: {program}: command not found\n")
}

/// Build a command that preserves the argument boundaries supplied by Clap.
pub fn direct_command(args: &[String]) -> Result<Launch> {
    let Some((program, program_args)) = args.split_first() else {
        bail!("command is required");
    };

    if resolve_binary(program).is_err() {
        return Ok(Launch::NotFound(program.clone()));
    }

    let mut command = resolved_command(program);
    command.args(program_args);
    Ok(Launch::Ready(command))
}

/// Build a command string invocation using an explicit shell or the platform default.
pub fn shell_command(script: &str, shell: Option<&str>) -> Result<Command> {
    let program = shell.unwrap_or(default_shell());
    if program.trim().is_empty() {
        bail!("shell must not be empty");
    }

    let mut command = match shell {
        Some(_) => Command::new(
            resolve_binary(program).with_context(|| format!("Shell '{program}' not found"))?,
        ),
        None => resolved_command(program),
    };
    command.arg(command_flag(program)).arg(script);
    Ok(command)
}

/// Build a direct command by default, or an explicit shell command when requested.
///
/// Shell mode requires one argument containing the complete script. This avoids
/// reconstructing quoting and argument boundaries by joining already-parsed argv.
pub fn command_from_args(args: &[String], shell: Option<&str>) -> Result<Launch> {
    match shell {
        Some(shell) => match args {
            [script] => shell_command(script, Some(shell)).map(Launch::Ready),
            [] => bail!("command is required when --shell is used"),
            _ => bail!("pass the shell command as one quoted argument after --shell"),
        },
        None => direct_command(args),
    }
}

/// Render argv for logging, tracking labels and ecosystem detection.
///
/// Arguments that carry whitespace or quotes are re-quoted, so a tracked label
/// reads back as the command that ran instead of collapsing
/// `--filter "a b"` into two words.
pub fn display_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_for_display(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_for_display(arg: &str) -> Cow<'_, str> {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\'', '"', '\\']) {
        return Cow::Borrowed(arg);
    }
    Cow::Owned(format!("'{}'", arg.replace('\'', r"'\''")))
}

#[cfg(windows)]
fn default_shell() -> &'static str {
    "cmd"
}

#[cfg(not(windows))]
fn default_shell() -> &'static str {
    "sh"
}

fn command_flag(shell: &str) -> &'static str {
    let basename = Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(shell)
        .to_ascii_lowercase();

    match basename.as_str() {
        "cmd" | "cmd.exe" => "/C",
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe" => "-Command",
        _ => "-c",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn ready(launch: Launch) -> Command {
        match launch {
            Launch::Ready(command) => command,
            Launch::NotFound(program) => panic!("expected a spawnable command, got {program}"),
        }
    }

    #[test]
    fn direct_command_preserves_argument_boundaries() {
        let args = vec![
            "echo".to_string(),
            "a b".to_string(),
            "*".to_string(),
            "$HOME".to_string(),
        ];
        let command = ready(direct_command(&args).expect("build direct command"));
        let actual: Vec<_> = command.get_args().collect();

        assert_eq!(
            actual,
            [OsStr::new("a b"), OsStr::new("*"), OsStr::new("$HOME")]
        );
    }

    #[test]
    fn direct_command_requires_program() {
        assert!(direct_command(&[]).is_err());
    }

    #[test]
    fn direct_command_reports_an_unresolvable_program() {
        let args = vec!["rtk-no-such-binary-4c1f".to_string(), "arg".to_string()];
        match direct_command(&args).expect("missing program is an outcome, not an error") {
            Launch::NotFound(program) => assert_eq!(program, "rtk-no-such-binary-4c1f"),
            Launch::Ready(_) => panic!("unresolvable program must not be spawnable"),
        }
    }

    #[test]
    fn shell_command_uses_shell_specific_flag() {
        assert_eq!(command_flag("fish"), "-c");
        assert_eq!(command_flag("/bin/zsh"), "-c");
        assert_eq!(command_flag("cmd.exe"), "/C");
        assert_eq!(command_flag("pwsh"), "-Command");
    }

    #[test]
    fn shell_command_rejects_empty_shell() {
        assert!(shell_command("echo ok", Some(" ")).is_err());
    }

    #[test]
    fn shell_mode_requires_one_script_argument() {
        let split = vec!["echo".to_string(), "ok".to_string()];
        assert!(command_from_args(&split, Some("unused-shell")).is_err());

        let quoted = vec!["echo ok".to_string()];
        let current_exe = std::env::current_exe().expect("resolve current test executable");
        assert!(
            command_from_args(
                &quoted,
                Some(current_exe.to_str().expect("test executable path is UTF-8"))
            )
            .is_ok()
        );
    }

    #[test]
    fn display_args_requotes_arguments_that_carry_spaces() {
        let args = vec![
            "cargo".to_string(),
            "test".to_string(),
            "--filter".to_string(),
            "a b".to_string(),
        ];

        assert_eq!(display_args(&args), "cargo test --filter 'a b'");
    }
}
