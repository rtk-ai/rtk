//! Builds direct and explicit-shell commands without guessing the caller's shell.

use crate::core::utils::{ChildArgExt, resolve_binary, resolved_command};
use anyhow::{Result, bail};
use std::borrow::Cow;
use std::path::Path;
use std::process::Command;

/// POSIX "command not found".
///
/// Direct execution resolves the program itself, so nothing is left to report
/// the code the interposed `sh -c` used to return. Callers keep returning it:
/// CI steps branch on 127, and RTK's own `[FAIL]` line carries it.
pub const EXIT_COMMAND_NOT_FOUND: i32 = 127;

/// POSIX "found, but could not be executed" — a directory, a file without `+x`,
/// or something the kernel refuses to exec. `sh`, `dash` and `bash` all answer
/// 126 here and 127 only for a genuinely missing program; the same distinction
/// is what a CI step reads.
pub const EXIT_COMMAND_NOT_EXECUTABLE: i32 = 126;

/// `--shell` runs one complete script, so it takes exactly one argument.
///
/// One rule, one wording: the CLI reports it as a clap usage error before
/// execution, and [`command_from_args`] refuses the same shape.
pub const SHELL_ARITY_MESSAGE: &str = "--shell takes the complete command as one quoted argument";

/// A command ready to spawn, or the reason it can never run.
///
/// A program that cannot be run is an execution outcome, not an RTK error: the
/// runners render it through their own failure path and exit with the code the
/// shell they replaced would have returned.
pub enum Launch {
    Ready(Command),
    Unrunnable(Unrunnable),
}

/// What a shell prints, and exits with, for a program it cannot run.
pub struct Unrunnable {
    /// The line a shell writes to stderr, in RTK's voice.
    pub message: String,
    /// [`EXIT_COMMAND_NOT_FOUND`] or [`EXIT_COMMAND_NOT_EXECUTABLE`].
    pub code: i32,
}

impl Unrunnable {
    fn not_found(program: &str) -> Self {
        Self {
            message: format!("rtk: {program}: command not found\n"),
            code: EXIT_COMMAND_NOT_FOUND,
        }
    }

    fn not_executable(program: &str, reason: &str) -> Self {
        Self {
            message: format!("rtk: {program}: {reason}\n"),
            code: EXIT_COMMAND_NOT_EXECUTABLE,
        }
    }
}

/// Classify a program that `resolve_binary` could not resolve.
///
/// `which` answers one thing — "is this runnable from here" — for two different
/// situations, so a spelling that addresses the filesystem is stat'd to tell
/// them apart. A bare `PATH` name stays 127 even when a non-executable file of
/// that name exists somewhere, which is what `dash` and `bash` do.
fn classify_unrunnable(program: &str) -> Unrunnable {
    if !names_a_path(program) {
        return Unrunnable::not_found(program);
    }

    match std::fs::metadata(program) {
        Ok(meta) if meta.is_dir() => Unrunnable::not_executable(program, "Is a directory"),
        Ok(_) => Unrunnable::not_executable(program, "Permission denied"),
        // A directory on the way is unsearchable, so whether the program exists
        // was never established — 127 would assert what the call could not
        // answer. `dash` reports 126 here.
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Unrunnable::not_executable(program, "Permission denied")
        }
        Err(_) => Unrunnable::not_found(program),
    }
}

/// True for a spelling that addresses the filesystem rather than `PATH`.
fn names_a_path(program: &str) -> bool {
    program.contains('/') || (cfg!(windows) && program.contains('\\'))
}

/// Map a spawn failure to the answer a shell would have given, or `None` when it
/// is not about the program itself.
///
/// Resolution proves a name resolves; it cannot prove `execve` will accept the
/// file. A shebang with CRLF line endings, an interpreter that is missing, and a
/// file that is not a valid executable all fail here instead, and a shell
/// reports them as 127 or 126 rather than as an error of its own.
pub fn spawn_failure(program: &str, error: &anyhow::Error) -> Option<Unrunnable> {
    let io_error = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())?;

    // "Not a recognized executable format" has no stable `ErrorKind`, so it is
    // matched on the raw code — which is per-platform: 8 is ENOEXEC on Unix,
    // while on Windows `raw_os_error` is a Win32 code, 8 there is
    // ERROR_NOT_ENOUGH_MEMORY, and the bad-format code is 193.
    #[cfg(unix)]
    const BAD_FORMAT: i32 = 8;
    #[cfg(windows)]
    const BAD_FORMAT: i32 = 193;
    #[cfg(any(unix, windows))]
    if io_error.raw_os_error() == Some(BAD_FORMAT) {
        return Some(Unrunnable::not_executable(
            program,
            "cannot execute binary file",
        ));
    }

    match io_error.kind() {
        std::io::ErrorKind::NotFound => Some(Unrunnable::not_found(program)),
        std::io::ErrorKind::PermissionDenied => {
            Some(Unrunnable::not_executable(program, "Permission denied"))
        }
        std::io::ErrorKind::IsADirectory => {
            Some(Unrunnable::not_executable(program, "Is a directory"))
        }
        _ => None,
    }
}

/// Build a command that preserves the argument boundaries supplied by Clap.
pub fn direct_command(args: &[String]) -> Result<Launch> {
    let Some((program, program_args)) = args.split_first() else {
        bail!("command is required");
    };

    if resolve_binary(program).is_err() {
        return Ok(Launch::Unrunnable(classify_unrunnable(program)));
    }

    let mut command = resolved_command(program);
    // These arguments came off rtk's own command line, so they take the
    // encoding MSYS/Cygwin children expect on Windows (#3728).
    command.child_args(program_args);
    Ok(Launch::Ready(command))
}

/// Build a command string invocation using an explicit shell or the platform default.
pub fn shell_command(script: &str, shell: Option<&str>) -> Result<Launch> {
    let program = shell.unwrap_or(default_shell());
    if program.trim().is_empty() {
        bail!("shell must not be empty");
    }

    // A named shell is resolved up front so an unusable one reports the shell's
    // own answer, the same way an unusable program does. The platform default
    // keeps `resolved_command`'s fallback: it is RTK's choice, not the caller's.
    if shell.is_some() && resolve_binary(program).is_err() {
        return Ok(Launch::Unrunnable(classify_unrunnable(program)));
    }

    let mut command = resolved_command(program);
    command.arg(command_flag(program)).child_arg(script);
    Ok(Launch::Ready(command))
}

/// Build a direct command by default, or an explicit shell command when requested.
///
/// Shell mode requires one argument containing the complete script. This avoids
/// reconstructing quoting and argument boundaries by joining already-parsed argv.
pub fn command_from_args(args: &[String], shell: Option<&str>) -> Result<Launch> {
    match shell {
        Some(shell) => match args {
            [script] => shell_command(script, Some(shell)),
            [] => bail!("command is required when --shell is used"),
            _ => bail!(SHELL_ARITY_MESSAGE),
        },
        None => direct_command(args),
    }
}

/// The program a launch would have executed, for the message a failure carries.
///
/// Without `--shell`, a command string runs through the platform default, so
/// that is the program a failure is about — naming it beats the placeholder a
/// caller has no way to see otherwise.
pub fn program_name<'a>(args: &'a [String], shell: Option<&'a str>) -> &'a str {
    shell
        .or_else(|| args.first().map(String::as_str))
        .unwrap_or(default_shell())
}

/// Render argv for logging, tracking labels and ecosystem detection.
///
/// Anything a shell would have interpreted is quoted, so a label reads back as
/// the command that ran — `rtk err /bin/echo '*' 'a;b'` is recorded with its
/// `*` and `;` intact rather than as something a shell would expand.
pub fn display_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_for_display(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Everything a POSIX shell gives meaning to, plus the quote characters. An
/// argument containing none of these reads back the way it was typed.
const SHELL_METACHARACTERS: &[char] = &[
    ' ', '\t', '\n', '\r', '\'', '"', '\\', '*', '?', '[', ']', '{', '}', '(', ')', '$', '&', ';',
    '|', '<', '>', '`', '!', '#', '~', '=', '^',
];

fn quote_for_display(arg: &str) -> Cow<'_, str> {
    if !arg.is_empty() && !arg.contains(SHELL_METACHARACTERS) {
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
            Launch::Unrunnable(unrunnable) => {
                panic!("expected a spawnable command, got {}", unrunnable.message)
            }
        }
    }

    fn unrunnable(launch: Launch) -> Unrunnable {
        match launch {
            Launch::Unrunnable(unrunnable) => unrunnable,
            Launch::Ready(_) => panic!("expected an unrunnable program"),
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
    fn direct_command_reports_a_missing_program_as_127() {
        let args = vec!["rtk-no-such-binary-4c1f".to_string(), "arg".to_string()];
        let outcome = unrunnable(direct_command(&args).expect("missing program is an outcome"));

        assert_eq!(outcome.code, EXIT_COMMAND_NOT_FOUND);
        assert!(
            outcome.message.contains("command not found"),
            "{}",
            outcome.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn direct_command_reports_an_unexecutable_path_as_126() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let file = dir.path().join("noexec");
        std::fs::write(&file, b"not executable").expect("write file");

        let args = vec![file.to_string_lossy().into_owned()];
        let outcome = unrunnable(direct_command(&args).expect("unusable program is an outcome"));
        assert_eq!(outcome.code, EXIT_COMMAND_NOT_EXECUTABLE);
        assert!(
            outcome.message.contains("Permission denied"),
            "{}",
            outcome.message
        );

        let args = vec![dir.path().to_string_lossy().into_owned()];
        let outcome = unrunnable(direct_command(&args).expect("a directory is an outcome"));
        assert_eq!(outcome.code, EXIT_COMMAND_NOT_EXECUTABLE);
        assert!(
            outcome.message.contains("Is a directory"),
            "{}",
            outcome.message
        );
    }

    #[test]
    fn bare_path_names_without_a_separator_stay_127() {
        // `dash` answers 127 for a bare name it cannot resolve even when a
        // non-executable file of that name exists in the working directory.
        let outcome = classify_unrunnable("Cargo.toml");
        assert_eq!(outcome.code, EXIT_COMMAND_NOT_FOUND);
    }

    #[test]
    fn spawn_failure_maps_the_program_errors_and_nothing_else() {
        let not_found = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::NotFound))
            .context("Failed to spawn process");
        assert_eq!(
            spawn_failure("prog", &not_found).expect("mapped").code,
            EXIT_COMMAND_NOT_FOUND
        );

        let denied = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(
            spawn_failure("prog", &denied).expect("mapped").code,
            EXIT_COMMAND_NOT_EXECUTABLE
        );

        let unrelated = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        assert!(spawn_failure("prog", &unrelated).is_none());
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
    fn shell_command_reports_a_missing_shell_as_an_outcome() {
        let outcome = unrunnable(
            shell_command("echo ok", Some("rtk-no-such-shell-4c1f")).expect("outcome, not error"),
        );
        assert_eq!(outcome.code, EXIT_COMMAND_NOT_FOUND);
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
    fn display_args_quotes_every_shell_metacharacter() {
        let args = vec![
            "/bin/echo".to_string(),
            "a b".to_string(),
            "*".to_string(),
            "$HOME".to_string(),
            "a;b".to_string(),
            "x&&y".to_string(),
            "p|q".to_string(),
        ];

        assert_eq!(
            display_args(&args),
            "/bin/echo 'a b' '*' '$HOME' 'a;b' 'x&&y' 'p|q'"
        );
    }
}
