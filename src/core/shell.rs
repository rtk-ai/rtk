//! One place that decides which shell runs a user-supplied command string.
//!
//! `rtk err`, `rtk test` and `rtk summary` all take a command as a single
//! string and hand it to a shell. Hardcoding `cmd /C` on Windows breaks every
//! PowerShell cmdlet and pipeline, so the shell is resolved here instead:
//!
//! 1. `RTK_SHELL`, if set, wins (`cmd`, `powershell`, `pwsh`, `sh`, `bash`, or
//!    any executable name/path).
//! 2. On Windows, if the command's leading word is neither a `cmd` builtin nor
//!    a program on PATH, it is PowerShell syntax — route it to PowerShell.
//! 3. Otherwise the platform default: `cmd` on Windows, `sh` elsewhere.
//!
//! Step 2 replaces the obvious approach of detecting the *parent* shell, which
//! is not available here: walking parent processes needs Win32 calls this crate
//! forbids (`unsafe_code = "deny"`), and the environment carries no usable
//! signal — `PSModulePath` is inherited by `cmd` children, so it proves
//! nothing. Deciding from the command text needs no parent at all, costs one
//! PATH lookup, and is not locale-dependent the way parsing cmd.exe's
//! "is not recognized as an internal or external command" would be.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Name of the environment variable that pins the shell.
pub const SHELL_ENV: &str = "RTK_SHELL";

/// `cmd.exe` builtins: no file on PATH backs these, so a PATH lookup alone
/// would misread them as PowerShell. Sorted for `binary_search`.
const CMD_BUILTINS: &[&str] = &[
    "assoc", "break", "call", "cd", "chdir", "cls", "color", "copy", "date", "del", "dir", "echo",
    "endlocal", "erase", "exit", "for", "ftype", "goto", "if", "md", "mkdir", "mklink", "move",
    "path", "pause", "popd", "prompt", "pushd", "rd", "rem", "ren", "rename", "rmdir", "set",
    "setlocal", "shift", "start", "time", "title", "type", "ver", "verify", "vol",
];

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Cmd,
    PowerShell,
    Posix,
}

/// Build the shell invocation for `command`.
pub fn build_shell_command(command: &str) -> Command {
    match std::env::var(SHELL_ENV) {
        Ok(shell) if !shell.trim().is_empty() => spawn_with(shell.trim(), command),
        _ if cfg!(target_os = "windows") => spawn_with(windows_shell_for(command), command),
        _ => spawn_with("sh", command),
    }
}

/// Pick between `cmd` and PowerShell for `command` on Windows.
fn windows_shell_for(command: &str) -> &'static str {
    if runs_under_cmd(command) {
        return "cmd";
    }
    powershell_exe().unwrap_or("cmd")
}

/// True when `cmd.exe` can resolve the command's leading word — a builtin, or a
/// program on PATH. An empty command is left to `cmd` so it reports its own
/// error rather than PowerShell reporting a different one.
fn runs_under_cmd(command: &str) -> bool {
    let Some(word) = leading_word(command) else {
        return true;
    };
    let lower = word.to_ascii_lowercase();
    if CMD_BUILTINS.binary_search(&lower.as_str()).is_ok() {
        return true;
    }
    which::which(&word).is_ok()
}

/// The command's first whitespace-separated word, unquoted.
///
/// Only the leading word matters, so this deliberately does not implement full
/// shell tokenization — a quoted program path is the one case worth handling.
fn leading_word(command: &str) -> Option<String> {
    let trimmed = command.trim_start();
    if let Some(rest) = trimmed.strip_prefix('"') {
        let (word, _) = rest.split_once('"')?;
        return (!word.is_empty()).then(|| word.to_string());
    }
    trimmed
        .split_whitespace()
        .next()
        .map(|w| w.trim_matches('"').to_string())
        .filter(|w| !w.is_empty())
}

/// The PowerShell binary to use, preferring PowerShell 7+ when installed.
fn powershell_exe() -> Option<&'static str> {
    ["pwsh", "powershell"]
        .into_iter()
        .find(|exe| which::which(exe).is_ok())
}

fn spawn_with(shell: &str, command: &str) -> Command {
    let mut c = Command::new(shell);
    match kind_of(shell) {
        Kind::Cmd => {
            c.args(["/C", command]);
        }
        // -NoProfile keeps a user's $PROFILE out of captured output;
        // -NonInteractive turns a prompt into an error instead of a hung capture.
        Kind::PowerShell => {
            c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        }
        Kind::Posix => {
            c.args(["-c", command]);
        }
    }
    c
}

fn kind_of(shell: &str) -> Kind {
    let stem = Path::new(shell)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(shell)
        .to_ascii_lowercase();

    if stem == "cmd" {
        Kind::Cmd
    } else if stem == "pwsh" || stem.contains("powershell") {
        Kind::PowerShell
    } else {
        Kind::Posix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn builtin_table_is_sorted_for_binary_search() {
        let mut sorted = CMD_BUILTINS.to_vec();
        sorted.sort_unstable();
        assert_eq!(CMD_BUILTINS, sorted.as_slice());
    }

    #[test]
    fn cmd_uses_slash_c() {
        let c = spawn_with("cmd", "dir /b");
        assert_eq!(c.get_program(), "cmd");
        assert_eq!(args_of(&c), vec!["/C", "dir /b"]);
    }

    #[test]
    fn powershell_uses_dash_command_without_profile() {
        for shell in ["pwsh", "powershell", "powershell.exe", r"C:\ps\pwsh.exe"] {
            let c = spawn_with(shell, "Get-ChildItem | Measure-Object");
            assert_eq!(
                args_of(&c),
                vec![
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Get-ChildItem | Measure-Object"
                ],
                "wrong args for {shell}"
            );
        }
    }

    #[test]
    fn posix_shells_use_dash_c() {
        for shell in ["sh", "bash", "/usr/bin/zsh"] {
            let c = spawn_with(shell, "ls -la");
            assert_eq!(args_of(&c), vec!["-c", "ls -la"], "wrong args for {shell}");
        }
    }

    #[test]
    fn cmd_exe_path_is_still_cmd() {
        assert_eq!(kind_of(r"C:\Windows\System32\cmd.exe"), Kind::Cmd);
    }

    #[test]
    fn leading_word_handles_quotes_and_blanks() {
        assert_eq!(leading_word("dir /b").as_deref(), Some("dir"));
        assert_eq!(
            leading_word(r#""C:\Program Files\git\git.exe" status"#).as_deref(),
            Some(r"C:\Program Files\git\git.exe")
        );
        assert_eq!(
            leading_word("   Get-ChildItem | Measure-Object").as_deref(),
            Some("Get-ChildItem")
        );
        assert_eq!(leading_word("   ").as_deref(), None);
        assert_eq!(leading_word("").as_deref(), None);
    }

    #[test]
    fn cmd_builtins_stay_on_cmd() {
        // Not on PATH as programs, so only the builtin table keeps them here.
        for c in ["dir /b", "ECHO hi", "set FOO=1", "type file.txt", "cd .."] {
            assert!(runs_under_cmd(c), "{c} should route to cmd");
        }
    }

    #[test]
    fn cmdlets_do_not_run_under_cmd() {
        for c in [
            "Get-ChildItem | Measure-Object",
            "Select-String -Pattern foo *.txt",
            "$env:PATH -split ';'",
        ] {
            assert!(!runs_under_cmd(c), "{c} should not route to cmd");
        }
    }

    #[test]
    fn programs_on_path_stay_on_cmd() {
        // cargo is running this test, so it is on PATH on every platform.
        assert!(runs_under_cmd("cargo --version"));
    }

    #[test]
    fn empty_command_stays_on_cmd() {
        assert!(runs_under_cmd(""));
    }

    #[test]
    fn explicit_rtk_shell_is_respected() {
        // Scoped to this test only; no other test in this module reads the var.
        let prev = std::env::var_os(SHELL_ENV);
        std::env::set_var(SHELL_ENV, "pwsh");
        let c = build_shell_command("Get-ChildItem");
        let program = c.get_program().to_owned();
        match prev {
            Some(v) => std::env::set_var(SHELL_ENV, v),
            None => std::env::remove_var(SHELL_ENV),
        }
        assert_eq!(program, "pwsh");
    }
}
