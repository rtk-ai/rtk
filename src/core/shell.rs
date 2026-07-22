//! Builds direct and explicit-shell commands without guessing the caller's shell.

use crate::core::utils::{resolve_binary, resolved_command};
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Build a command that preserves the argument boundaries supplied by Clap.
pub fn direct_command(args: &[String]) -> Result<Command> {
    let Some((program, program_args)) = args.split_first() else {
        bail!("command is required");
    };

    let mut command = resolved_command(program);
    command.args(program_args);
    Ok(command)
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
pub fn command_from_args(args: &[String], shell: Option<&str>) -> Result<Command> {
    match shell {
        Some(shell) => match args {
            [script] => shell_command(script, Some(shell)),
            [] => bail!("command is required when --shell is used"),
            _ => bail!("pass the shell command as one quoted argument after --shell"),
        },
        None => direct_command(args),
    }
}

pub fn display_args(args: &[String]) -> String {
    args.join(" ")
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

    #[test]
    fn direct_command_preserves_argument_boundaries() {
        let args = vec![
            "echo".to_string(),
            "a b".to_string(),
            "*".to_string(),
            "$HOME".to_string(),
        ];
        let command = direct_command(&args).expect("build direct command");
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
        assert!(command_from_args(
            &quoted,
            Some(current_exe.to_str().expect("test executable path is UTF-8"))
        )
        .is_ok());
    }
}
