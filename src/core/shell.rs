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
        let outcome = classify_unrunnable(program);
        if program_args.is_empty()
            && outcome.code == EXIT_COMMAND_NOT_FOUND
            && (program.is_empty() || program.contains(SHELL_METACHARACTERS))
        {
            eprintln!(
                "rtk: single-string command; running through {} -c — pass arguments separately, or use --shell for scripts",
                default_shell()
            );
            return shell_command(program, None);
        }
        return Ok(Launch::Unrunnable(outcome));
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
/// Each word goes through [`quote_word`], so a label reads back as the words
/// that ran: `rtk err /bin/echo '*' 'a;b'` is recorded with its `*` and `;`
/// intact, and `grep 'a b' f` stays apart from `grep a b f`.
///
/// Every tracked label builds its user-supplied words through this function,
/// [`display_command`] when the words start with the program, or [`quote_word`]
/// for a single word. Never join argv with `" "` for
/// a label passed to `TimedExecution::track` or a runner: the `label_scan` test
/// fails on one.
pub fn display_args<S: AsRef<str>>(args: &[S]) -> String {
    args.iter()
        .map(|arg| quote_word(arg.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `prefix` followed by already rendered words, with no trailing space when
/// there are none: `with_args("git diff", "")` is `git diff`.
pub fn with_args(prefix: &str, rendered: &str) -> String {
    if rendered.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix} {rendered}")
    }
}

/// Render a whole command line, program word first: [`display_args`], with the
/// program word through [`quote_program`].
pub fn display_command<S: AsRef<str>>(argv: &[S]) -> String {
    let Some((program, args)) = argv.split_first() else {
        return String::new();
    };
    let program = quote_program(program.as_ref());
    if args.is_empty() {
        program.into_owned()
    } else {
        format!("{program} {}", display_args(args))
    }
}

/// Everything a POSIX shell gives meaning to, plus the quote characters.
const SHELL_METACHARACTERS: &[char] = &[
    ' ', '\t', '\n', '\r', '\'', '"', '\\', '*', '?', '[', ']', '{', '}', '(', ')', '$', '&', ';',
    '|', '<', '>', '`', '!', '#', '~', '=', '^',
];

/// Quote one word the way Python's `shlex.quote` does.
///
/// A word made only of ASCII letters, digits and `-_./=:,@+%` is returned as
/// written, unless it starts with `=`, which zsh expands to a command's path.
/// Anything else, the empty word included, is wrapped in single quotes, with an
/// inner `'` written as `'\''`.
pub fn quote_word(word: &str) -> Cow<'_, str> {
    let plain = |b: u8| b.is_ascii_alphanumeric() || b"-_./=:,@+%".contains(&b);
    if !word.is_empty() && !word.starts_with('=') && word.bytes().all(plain) {
        return Cow::Borrowed(word);
    }
    always_quoted(word)
}

/// The words bash reads as part of its own grammar in command position. The
/// symbol ones (`!`, `[[`, `]]`, `{`, `}`) are left out: [`quote_word`] quotes
/// them already.
const BASH_RESERVED_WORDS: &[&str] = &[
    "case", "coproc", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in",
    "select", "then", "time", "until", "while",
];

/// Quote the program word of a command line: as [`quote_word`], and also when a
/// shell would not run it as a program. That is a bash reserved word (`time`
/// runs bash's keyword, not `/usr/bin/time`) or the shape of an assignment
/// (`NAME=...` or `NAME+=...`), which sets a variable. An argument such as `time`, `KEY=value`
/// or `--format=x` stays bare.
pub fn quote_program(word: &str) -> Cow<'_, str> {
    let is_name = |name: &str| {
        name.bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    };
    // `NAME+=value` appends, and is an assignment too.
    let assignment = word
        .split_once('=')
        .is_some_and(|(name, _)| is_name(name.strip_suffix('+').unwrap_or(name)));
    if assignment || BASH_RESERVED_WORDS.contains(&word) {
        always_quoted(word)
    } else {
        quote_word(word)
    }
}

fn always_quoted(word: &str) -> Cow<'_, str> {
    Cow::Owned(format!("'{}'", word.replace('\'', r"'\''")))
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
mod label_scan;

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
    fn a_single_shell_phrase_falls_back_to_the_platform_shell() {
        for phrase in ["echo one two", "true && false", "echo x | grep x", ""] {
            let command = ready(
                direct_command(&[phrase.to_string()]).expect("fallback builds a shell command"),
            );

            let program = command.get_program().to_string_lossy().to_string();
            assert!(
                Path::new(&program)
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(default_shell())),
                "expected the platform shell, got {program}"
            );
            let actual: Vec<_> = command.get_args().collect();
            assert_eq!(
                actual,
                [
                    OsStr::new(command_flag(default_shell())),
                    OsStr::new(phrase)
                ],
                "{phrase:?}"
            );
        }
    }

    #[test]
    fn the_fallback_requires_a_single_unresolvable_phrase() {
        let args = vec!["echo one two".to_string(), "x".to_string()];
        let outcome = unrunnable(direct_command(&args).expect("extra args stay direct"));
        assert_eq!(outcome.code, EXIT_COMMAND_NOT_FOUND);

        let args = vec!["rtk-no-such-binary-4c1f".to_string()];
        let outcome = unrunnable(direct_command(&args).expect("bare word stays direct"));
        assert_eq!(outcome.code, EXIT_COMMAND_NOT_FOUND);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_unexecutable_path_keeps_126_over_the_shell() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let spaced = dir.path().join("a b");
        std::fs::create_dir(&spaced).expect("create spaced dir");

        let args = vec![spaced.to_string_lossy().into_owned()];
        let outcome = unrunnable(direct_command(&args).expect("existing path stays direct"));
        assert_eq!(outcome.code, EXIT_COMMAND_NOT_EXECUTABLE);
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

    #[test]
    fn quote_word_keeps_plain_words_as_written() {
        for word in [
            "git",
            "--oneline",
            "-n",
            "src/main.rs",
            "--format=%H",
            "user@host:path",
            "a,b+c",
            "_x.y",
        ] {
            assert_eq!(quote_word(word), word);
        }
    }

    #[test]
    fn quote_word_quotes_a_blank() {
        assert_eq!(quote_word("a b"), "'a b'");
        assert_eq!(quote_word("a\tb"), "'a\tb'");
        assert_eq!(quote_word("a\nb"), "'a\nb'");
    }

    #[test]
    fn quote_word_quotes_every_special_byte_class() {
        for word in [
            "foo()", "$HOME", "a;b", "x&&y", "p|q", "<in", "out>", "`id`", "\"q\"", "a\\b", "*.rs",
            "a?", "[ab]", "{a,b}", "!x", "#c", "~", "^x", "\u{7}",
        ] {
            assert_eq!(quote_word(word), format!("'{word}'"), "{word:?}");
        }
    }

    #[test]
    fn quote_word_escapes_an_inner_single_quote() {
        assert_eq!(quote_word("it's"), r"'it'\''s'");
        assert_eq!(quote_word("'"), r"''\'''");
    }

    /// zsh expands a word starting with `=` to a command's path, so that word is quoted;
    /// an `=` later in a word stays bare.
    #[test]
    fn quote_word_quotes_a_leading_equals_sign() {
        assert_eq!(quote_word("=foo"), "'=foo'");
        assert_eq!(quote_word("="), "'='");
        assert_eq!(quote_word("--format=x"), "--format=x");
        assert_eq!(quote_word("KEY=value"), "KEY=value");
    }

    /// A bash reserved word in command position is part of bash's grammar, so it is quoted
    /// there and nowhere else.
    #[test]
    fn a_reserved_word_as_program_is_quoted() {
        for word in ["time", "if", "while", "function"] {
            assert_eq!(quote_program(word), format!("'{word}'"), "{word:?}");
        }
        for word in ["!", "[[", "]]", "{", "}"] {
            assert_eq!(quote_word(word), format!("'{word}'"), "{word:?}");
        }
        assert_eq!(quote_word("time"), "time");
        assert_eq!(quote_program("timeout"), "timeout");
    }

    /// A program word shaped like `NAME=...` reads as an assignment in a shell, so it is
    /// quoted; the same word as an argument is not.
    #[test]
    fn an_assignment_shaped_program_word_is_quoted() {
        assert_eq!(quote_program("FOO=1"), "'FOO=1'");
        assert_eq!(quote_program("FOO+=1"), "'FOO+=1'");
        assert_eq!(quote_program("+=1"), "+=1");
        assert_eq!(quote_word("FOO+=1"), "FOO+=1");
        assert_eq!(quote_program("_x2=a b"), "'_x2=a b'");
        assert_eq!(quote_program("./a=b"), "./a=b");
        assert_eq!(quote_program("2x=y"), "2x=y");
        assert_eq!(quote_program("make"), "make");
        assert_eq!(quote_program("/p/My Tools/run"), "'/p/My Tools/run'");
        assert_eq!(
            display_command(&["FOO=1", "KEY=value", "--format=x"]),
            "'FOO=1' KEY=value --format=x"
        );
        assert_eq!(display_command(&["make"]), "make");
        assert_eq!(display_command(&["time", "ls"]), "'time' ls");
        assert_eq!(display_command(&["env", "time"]), "env time");
        assert_eq!(display_command::<&str>(&[]), "");
    }

    #[test]
    fn with_args_leaves_no_trailing_space() {
        assert_eq!(with_args("git diff", ""), "git diff");
        assert_eq!(
            with_args("git diff", "--stat 'a b'"),
            "git diff --stat 'a b'"
        );
    }

    #[test]
    fn quote_word_writes_the_empty_word_as_two_quotes() {
        assert_eq!(quote_word(""), "''");
        assert_eq!(display_args(&["grep", "", "f"]), "grep '' f");
    }

    #[test]
    fn quote_word_quotes_non_ascii_text() {
        assert_eq!(quote_word("héllo"), "'héllo'");
        assert_eq!(quote_word("日本語"), "'日本語'");
    }

    #[test]
    fn display_args_keeps_word_boundaries() {
        assert_eq!(display_args(&["-n", "a b", "f"]), "-n 'a b' f");
        assert_eq!(display_args(&["-n", "a", "b", "f"]), "-n a b f");
    }
}
