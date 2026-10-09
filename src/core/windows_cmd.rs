//! Command-line encoding for an explicitly requested cmd.exe child (#4333).
//!
//! cmd reads everything after /C or /K as shell source, not MSVCRT argv.
//! Preserve the source's quotes instead of escaping them with backslashes.
//! This is not an escaping API for untrusted data: the caller chose a shell,
//! so operators and expansions retain their meaning. Batch shims stay on
//! std's separate, batch-aware encoder.

use std::ffi::OsStr;

/// Check the resolved basename without requiring valid Unicode. Both path
/// separators are recognized so this decision can also be tested on Unix.
pub(super) fn is_cmd_program(program: &OsStr) -> bool {
    let name = program
        .as_encoded_bytes()
        .rsplit(|&byte| byte == b'/' || byte == b'\\')
        .next()
        .unwrap_or_default();
    name.eq_ignore_ascii_case(b"cmd") || name.eq_ignore_ascii_case(b"cmd.exe")
}

/// Append the command switch and its complete payload together. A single
/// payload is shell source; multiple arguments retain their quote grouping.
#[cfg(windows)]
pub(super) fn append_args<I, S>(
    command: &mut std::process::Command,
    args: I,
) -> &mut std::process::Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::process::CommandExt;

    let args: Vec<Vec<u16>> = args
        .into_iter()
        .map(|arg| arg.as_ref().encode_wide().collect())
        .collect();
    if !args.is_empty() {
        command.raw_arg(OsString::from_wide(&command_line(&args)));
    }
    command
}

const QUOTE: u16 = b'"' as u16;
const SPACE: u16 = b' ' as u16;

fn starts_switch(arg: &[u16], letter: u8) -> bool {
    arg.len() >= 2
        && arg[0] == u16::from(b'/')
        && (arg[1] == u16::from(letter) || arg[1] == u16::from(letter.to_ascii_lowercase()))
}

/// Keep existing quotes; group empty or whitespace-containing arguments with
/// plain quotes. cmd does not treat a backslash before a quote as an escape.
fn push_argument(out: &mut Vec<u16>, arg: &[u16]) {
    let whitespace = arg.iter().any(|&unit| {
        unit == SPACE
            || unit == u16::from(b'\t')
            || unit == u16::from(b'\r')
            || unit == u16::from(b'\n')
    });
    let wrap = !arg.contains(&QUOTE) && (arg.is_empty() || whitespace);
    if wrap {
        out.push(QUOTE);
    }
    out.extend_from_slice(arg);
    if wrap {
        out.push(QUOTE);
    }
}

fn push_word(out: &mut Vec<u16>, arg: &[u16]) {
    if !out.is_empty() {
        out.push(SPACE);
    }
    push_argument(out, arg);
}

/// Pure UTF-16 formatting. This mirrors cmd rather than getopt: /C and /K
/// (also /Cscript) consume the rest, even tokens resembling more switches.
fn command_line(args: &[Vec<u16>]) -> Vec<u16> {
    let command = args
        .iter()
        .position(|arg| starts_switch(arg, b'C') || starts_switch(arg, b'K'));
    let mut out = Vec::new();
    let Some(index) = command else {
        for arg in args {
            push_word(&mut out, arg);
        }
        return out;
    };

    for arg in &args[..index] {
        push_word(&mut out, arg);
    }
    let attached = &args[index][2..];
    let tail = &args[index + 1..];

    // Preserve cmd's legacy executable-name heuristic when a single payload
    // has no embedded quotes. For example, /C "C:\Program Files\tool.exe"
    // must not become /S /C "C:\Program Files\tool.exe": /S would strip the
    // only quotes protecting the executable. No CRT escaping is needed here.
    if attached.is_empty() && tail.len() == 1 && !tail[0].contains(&QUOTE) {
        push_word(&mut out, &args[index][..2]);
        push_word(&mut out, &tail[0]);
        return out;
    }

    if !args[..index]
        .iter()
        .any(|arg| arg.len() == 2 && starts_switch(arg, b'S'))
    {
        push_word(&mut out, &[u16::from(b'/'), u16::from(b'S')]);
    }
    push_word(&mut out, &args[index][..2]);
    out.extend_from_slice(&[SPACE, QUOTE]);

    // /S strips exactly this outer pair. A whole script is verbatim; when
    // provided as words, preserve spaces in each word without CRT escaping.
    if attached.is_empty() && tail.len() == 1 {
        out.extend_from_slice(&tail[0]);
    } else {
        out.extend_from_slice(attached);
        let mut needs_space = !attached.is_empty();
        for fragment in tail {
            if needs_space {
                out.push(SPACE);
            }
            push_argument(&mut out, fragment);
            needs_space = true;
        }
    }
    out.push(QUOTE);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(args: &[&str]) -> String {
        let args: Vec<Vec<u16>> = args
            .iter()
            .map(|arg| arg.encode_utf16().collect())
            .collect();
        String::from_utf16(&command_line(&args)).expect("valid UTF-16 fixture")
    }

    #[test]
    fn recognizes_only_cmd_itself() {
        for name in [
            "cmd",
            "CMD.EXE",
            r"C:\Windows\System32\cmd.exe",
            "/tools/cmd",
        ] {
            assert!(is_cmd_program(OsStr::new(name)), "{name}");
        }
        for name in ["cmdx", "cmd.bat", "cmd.cmd", "npm.CMD", "git.exe", "sh", ""] {
            assert!(!is_cmd_program(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn quoted_echo_is_shell_source_not_a_crt_argument() {
        assert_eq!(encoded(&["/c", r#"echo "a b""#]), r#"/S /c "echo "a b"""#);
    }

    #[test]
    fn preserves_if_exist_and_a_quoted_executable_path() {
        let script = concat!(
            r#"if exist "C:\Program Files\Git\bin\git.exe" "#,
            "(echo FOUND) else (echo MISSING)"
        );
        assert_eq!(encoded(&["/C", script]), format!("/S /C \"{script}\""));
        let program = r#""C:\path with spaces\rtk.exe" --version"#;
        assert_eq!(encoded(&["/C", program]), format!("/S /C \"{program}\""));
    }

    #[test]
    fn joins_words_once_without_reparsing_payload_switches() {
        assert_eq!(
            encoded(&["/D", "/C", "echo", r#""a b""#, "&", "echo", "/K", "/S"]),
            r#"/D /S /C "echo "a b" & echo /K /S""#
        );
    }

    #[test]
    fn split_arguments_keep_spaces_empty_values_and_trailing_backslashes() {
        assert_eq!(
            encoded(&[
                "/C",
                r"C:\Program Files\tool.exe",
                "a b",
                "",
                r"C:\dir space\"
            ]),
            r#"/S /C ""C:\Program Files\tool.exe" "a b" "" "C:\dir space\"""#
        );
    }

    #[test]
    fn unquoted_single_payload_keeps_cmds_executable_heuristic() {
        assert_eq!(encoded(&["/C", "echo a b"]), r#"/C "echo a b""#);
        assert_eq!(
            encoded(&["/C", r"C:\Program Files\tool.exe"]),
            r#"/C "C:\Program Files\tool.exe""#
        );
    }

    #[test]
    fn keeps_options_existing_s_and_k_mode() {
        assert_eq!(
            encoded(&["/d", "/s", "/v:on", "/k", r#"echo "!value!" & exit 7"#]),
            r#"/d /s /v:on /k "echo "!value!" & exit 7""#
        );
    }

    #[test]
    fn accepts_an_attached_command_string() {
        assert_eq!(encoded(&[r#"/cecho "a b""#]), r#"/S /c "echo "a b"""#);
        assert_eq!(
            encoded(&["/Kecho", "one", "&", "exit", "7"]),
            r#"/S /K "echo one & exit 7""#
        );
    }

    #[test]
    fn keeps_shell_metacharacters_and_backslashes_verbatim() {
        let script = r#"echo "x > stray" & echo C:\temp\ & echo %PATH% ^& !value!"#;
        assert_eq!(encoded(&["/C", script]), format!("/S /C \"{script}\""));
    }

    #[test]
    fn non_script_arguments_use_plain_quotes() {
        assert_eq!(encoded(&[]), "");
        assert_eq!(encoded(&["/?"]), "/?");
        assert_eq!(
            encoded(&[r"C:\a b\", "", r#"already "quoted""#]),
            r#""C:\a b\" "" already "quoted""#
        );
    }

    #[test]
    fn empty_scripts_are_preserved() {
        assert_eq!(encoded(&["/C"]), r#"/S /C """#);
        assert_eq!(encoded(&["/C", ""]), r#"/C """#);
    }

    #[test]
    fn preserves_unpaired_utf16_surrogates() {
        let args = vec![
            vec![u16::from(b'/'), u16::from(b'C')],
            vec![QUOTE, 0xD800, QUOTE],
        ];
        let mut expected: Vec<u16> = "/S /C \"".encode_utf16().collect();
        expected.extend_from_slice(&[QUOTE, 0xD800, QUOTE, QUOTE]);
        assert_eq!(command_line(&args), expected);
    }

    #[cfg(windows)]
    #[test]
    fn child_args_selects_cmd_without_changing_batch_or_native_arguments() {
        use crate::core::utils::ChildArgExt;
        use std::process::Command;

        let mut cmd = Command::new("cmd.exe");
        cmd.child_args(["/C", r#"echo "a b""#]);
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            [OsStr::new(r#"/S /C "echo "a b"""#)]
        );

        for program in ["npm.cmd", "gradlew.bat"] {
            // nosemgrep: dynamic-command-execution -- fixed fixtures; nothing is spawned
            let mut batch = Command::new(program);
            batch.child_args([r#"a "b""#, "a b", "x&y"]);
            assert_eq!(
                batch.get_args().collect::<Vec<_>>(),
                [r#"a "b""#, "a b", "x&y"]
            );
        }
        let mut native = Command::new("git.exe");
        native.child_args(["-c", r#"a "b""#]);
        assert_eq!(
            native.get_args().collect::<Vec<_>>(),
            ["-c", r#""a \"b\"""#]
        );
    }
}
