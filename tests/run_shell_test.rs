mod common;

/// Run `script` through `run -c` and through `run`, `err`, `test` and
/// `summary --shell` naming the same default shell, each in a fresh directory,
/// and check that it exits 7 having appended exactly the line `expected` to
/// `out.txt`. The file is the witness because `err`, `test` and `summary`
/// filter what a script prints. Without `--shell` those three would still
/// reach the same shell through the single-string fallback, so its notice
/// must stay absent.
#[cfg(any(unix, windows))]
fn assert_script_surfaces(script: &str, expected: &str) {
    const SHELL: &str = if cfg!(windows) { "cmd" } else { "sh" };
    let surfaces: [&[&str]; 5] = [
        &["run", "-c"],
        &["run", "--shell", SHELL, "-c"],
        &["err", "--shell", SHELL],
        &["test", "--shell", SHELL],
        &["summary", "--shell", SHELL],
    ];
    for surface in surfaces {
        let dir = tempfile::tempdir().expect("create temp dir");
        let output = common::rtk_command()
            .args(surface)
            .arg(script)
            .current_dir(dir.path())
            .output()
            .unwrap_or_else(|e| panic!("run rtk {surface:?}: {e}"));

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let context = format!("{surface:?} {stdout:?} {stderr:?}");
        assert_eq!(output.status.code(), Some(7), "{context}");
        assert!(!stderr.contains("pass arguments separately"), "{context}");
        let written = std::fs::read_to_string(dir.path().join("out.txt"))
            .unwrap_or_else(|e| panic!("{context} wrote no out.txt: {e}"));
        let lines: Vec<&str> = written.lines().map(str::trim_end).collect();
        assert_eq!(lines, [expected], "{context}");
    }
}

#[cfg(unix)]
mod unix {
    use std::process::Command;

    fn rtk() -> Command {
        crate::common::rtk_command()
    }

    #[test]
    fn positional_arguments_are_not_interpreted_by_a_shell() {
        let output = rtk()
            .args([
                "run",
                "/usr/bin/printf",
                "[%s]\\n",
                "a b",
                "*",
                "$HOME",
                ";",
                "&&",
                "|",
                "$(printf injected)",
                "`id`",
                "line1\nline2",
            ])
            .output()
            .expect("run rtk");

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "[a b]\n[*]\n[$HOME]\n[;]\n[&&]\n[|]\n[$(printf injected)]\n[`id`]\n[line1\nline2]\n"
        );
    }

    #[test]
    fn direct_execution_preserves_child_exit_code() {
        let status = rtk()
            .args(["run", "/bin/sh", "-c", "exit 42"])
            .status()
            .expect("run rtk");

        assert_eq!(status.code(), Some(42));
    }

    #[test]
    fn command_string_keeps_posix_shell_default() {
        let output = rtk()
            .args([
                "run",
                "-c",
                "value=$(printf posix_ok); printf '%s\\n' \"$value\"",
            ])
            .output()
            .expect("run rtk");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "posix_ok\n");
    }

    #[test]
    fn explicit_fish_shell_runs_fish_syntax_when_available() {
        let Ok(fish) = which::which("fish") else {
            return;
        };
        let output = rtk()
            .args([
                "run",
                "--shell",
                fish.to_str().expect("fish path is UTF-8"),
                "-c",
                "set value (printf fish_ok); printf '%s\\n' $value",
            ])
            .output()
            .expect("run rtk");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "fish_ok\n");
    }

    #[test]
    fn explicit_missing_shell_reports_the_shell_contract() {
        let output = rtk()
            .args([
                "run",
                "--shell",
                "rtk-missing-shell-for-test",
                "-c",
                "echo ok",
            ])
            .output()
            .expect("run rtk");

        // A shell that is not there answers like a program that is not there:
        // the shell's own line and exit 127, not an RTK error chain.
        assert_eq!(output.status.code(), Some(127));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("rtk-missing-shell-for-test: command not found"),
            "{stderr}"
        );
    }

    #[test]
    fn summary_arguments_are_not_interpreted_by_a_shell() {
        let output = rtk()
            .args(["summary", "/bin/echo", "*"])
            .output()
            .expect("run rtk summary");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "*\n\n\n");
    }

    #[test]
    fn filtered_wrapper_accepts_one_explicit_shell_script() {
        let Ok(fish) = which::which("fish") else {
            return;
        };
        let output = rtk()
            .args([
                "err",
                "--shell",
                fish.to_str().expect("fish path is UTF-8"),
                "printf 'error: fish_ok\\n'",
            ])
            .output()
            .expect("run rtk err");

        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("error: fish_ok"));
    }

    #[test]
    fn filtered_wrapper_rejects_reconstructed_shell_arguments() {
        let output = rtk()
            .args(["err", "--shell", "sh", "printf", "error: split"])
            .output()
            .expect("run rtk err");

        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("--shell takes the complete command as one quoted argument")
        );
    }

    #[test]
    fn err_reports_an_unresolvable_program_as_exit_127() {
        // The `sh -c` RTK no longer interposes returned 127 here; CI steps and
        // the `[FAIL]` line both key on it.
        let output = rtk()
            .args(["err", "rtk-no-such-binary-4c1f"])
            .output()
            .expect("run rtk err");

        assert_eq!(output.status.code(), Some(127));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("[FAIL] Command failed (exit code: 127)"),
            "{stdout}"
        );
        assert!(
            stdout.contains("rtk-no-such-binary-4c1f: command not found"),
            "{stdout}"
        );
    }

    #[test]
    fn test_and_summary_report_an_unresolvable_program_as_exit_127() {
        for subcommand in ["test", "summary"] {
            let output = rtk()
                .args([subcommand, "rtk-no-such-binary-4c1f"])
                .output()
                .unwrap_or_else(|e| panic!("run rtk {subcommand}: {e}"));

            assert_eq!(output.status.code(), Some(127), "{subcommand}");
            assert!(
                String::from_utf8_lossy(&output.stdout)
                    .contains("rtk-no-such-binary-4c1f: command not found"),
                "{subcommand}"
            );
        }
    }

    #[test]
    fn run_reports_an_unresolvable_program_as_exit_127() {
        let output = rtk()
            .args(["run", "rtk-no-such-binary-4c1f"])
            .output()
            .expect("run rtk run");

        assert_eq!(output.status.code(), Some(127));
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("rtk-no-such-binary-4c1f: command not found")
        );
    }

    /// 126 is the other half of the contract: `sh`, `dash` and `bash` all
    /// answer 126 for something that exists and cannot be executed, and 127
    /// only for a program that is not there at all.
    #[test]
    fn an_unexecutable_path_reports_exit_126() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let file = dir.path().join("noexec");
        std::fs::write(&file, b"not executable\n").expect("write file");
        let path = file.to_string_lossy().into_owned();

        for subcommand in ["err", "test", "summary"] {
            let output = rtk()
                .args([subcommand, &path])
                .output()
                .unwrap_or_else(|e| panic!("run rtk {subcommand}: {e}"));

            assert_eq!(output.status.code(), Some(126), "{subcommand}");
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("Permission denied"),
                "{subcommand}"
            );
        }

        let output = rtk().args(["run", &path]).output().expect("run rtk run");
        assert_eq!(output.status.code(), Some(126));
        assert!(String::from_utf8_lossy(&output.stderr).contains("Permission denied"));
    }

    #[test]
    fn a_directory_reports_exit_126() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().to_string_lossy().into_owned();

        let output = rtk().args(["run", &path]).output().expect("run rtk run");
        assert_eq!(output.status.code(), Some(126));
    }

    /// Resolution proves the name resolves; `execve` still refuses a CRLF
    /// shebang (its interpreter is `/bin/sh\r`) and a file that is not a valid
    /// executable. Both used to surface as an anyhow chain and exit 1.
    #[test]
    fn spawn_failures_keep_the_shell_contract() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("create tempdir");

        let crlf = dir.path().join("crlf.sh");
        std::fs::write(&crlf, b"#!/bin/sh\r\necho hi\r\n").expect("write script");
        std::fs::set_permissions(&crlf, std::fs::Permissions::from_mode(0o755))
            .expect("chmod script");

        let output = rtk()
            .args(["err", &crlf.to_string_lossy()])
            .output()
            .expect("run rtk err");
        assert_eq!(output.status.code(), Some(127));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("[FAIL] Command failed (exit code: 127)"),
            "{stdout}"
        );

        // An `+x` file the kernel cannot exec: on Linux `execvp` reports
        // ENOEXEC, while on macOS it falls back to `sh`, which answers 127
        // itself. Either way the outcome is a shell's, never an RTK error.
        let binary = dir.path().join("not-an-executable");
        std::fs::write(&binary, b"\x7fELF-but-not-really").expect("write file");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .expect("chmod file");

        let output = rtk()
            .args(["run", &binary.to_string_lossy()])
            .output()
            .expect("run rtk run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            matches!(output.status.code(), Some(126) | Some(127)),
            "{:?} / {stderr}",
            output.status.code()
        );
        assert!(!stderr.contains("Failed to spawn process"), "{stderr}");
    }

    /// `--shell` is the first flag these surfaces have, and a missing value is
    /// the likeliest way to get it wrong. It must report the flag error, not
    /// exec a program named after the subcommand.
    #[test]
    fn a_missing_shell_value_reports_a_flag_error() {
        for subcommand in ["err", "test", "summary", "run"] {
            let output = rtk()
                .args([subcommand, "--shell"])
                .output()
                .unwrap_or_else(|e| panic!("run rtk {subcommand}: {e}"));

            assert_eq!(output.status.code(), Some(2), "{subcommand}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("--shell <SHELL>"),
                "{subcommand}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// The two named-shell tests above skip wherever `fish` is absent, which is
    /// every CI runner — this one names a shell that always exists, so the
    /// explicit-shell branch is actually exercised somewhere.
    #[test]
    fn explicit_shell_by_name_runs_the_named_shell() {
        let output = rtk()
            .args(["run", "--shell", "sh", "-c", "printf 'shell_ok'"])
            .output()
            .expect("run rtk run");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "shell_ok");
    }

    /// A `&` inside a script's quotes stays there on every surface that hands
    /// sh a script; out of its quotes it would put `printf` in the background.
    #[test]
    fn shell_scripts_keep_a_quoted_ampersand_and_their_exit_code() {
        crate::assert_script_surfaces(r#"printf '%s\n' "a & b" >> out.txt; exit 7"#, "a & b");
    }

    #[test]
    fn an_unresolvable_shell_reports_the_shell_contract() {
        let output = rtk()
            .args(["run", "--shell", "rtk-no-such-shell-4c1f", "-c", "echo hi"])
            .output()
            .expect("run rtk run");

        assert_eq!(output.status.code(), Some(127));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("rtk-no-such-shell-4c1f"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A `( … )` pair only wraps the whole command when it balances across it.
    /// `( a ) b ( c )` closes its first group before the end, so peeling the
    /// outer tokens would hand `b` an argument list it never had.
    #[test]
    fn a_group_that_closes_early_is_not_peeled() {
        let output = rtk()
            .args(["test", "(", "/bin/true", ")", "x", "(", "/bin/false", ")"])
            .output()
            .expect("run rtk test");

        assert_eq!(output.status.code(), Some(127));
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("command not found"),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    /// `--shell` is RTK's flag on `rtk test`, and a flag other tools carry too.
    /// The guard that answers a misused `rtk test --shell` must not read argv
    /// belonging to a command RTK is only passing through.
    #[test]
    fn a_passthrough_keeps_its_own_shell_flag() {
        let output = rtk()
            .args(["/bin/echo", "--shell", "bash", "hi"])
            .output()
            .expect("run rtk passthrough");

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end(),
            "--shell bash hi"
        );
    }

    /// `metadata` answers EACCES rather than yes-or-no when a directory on the
    /// way to the program is not searchable, so its existence is unknown from
    /// there; 127 would assert that it is not there.
    #[test]
    fn an_unsearchable_parent_reports_exit_126() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("create tempdir");
        let closed = dir.path().join("closed");
        std::fs::create_dir(&closed).expect("create dir");
        let program = closed.join("prog");
        std::fs::write(&program, b"#!/bin/sh\nexit 0\n").expect("write program");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("chmod program");
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000))
            .expect("close dir");

        // Root ignores the search bit, so `metadata` answers there instead of
        // failing and the premise does not hold. Leave rather than assert an
        // outcome the environment cannot produce.
        if std::fs::read_dir(&closed).is_ok() {
            std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).ok();
            return;
        }

        let output = rtk()
            .arg("run")
            .arg(&program)
            .output()
            .expect("run rtk run");

        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).ok();

        assert_eq!(output.status.code(), Some(126));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Permission denied"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn grouped_and_negated_commands_keep_their_exit_codes() {
        // `!` and `( … )` are `test`'s syntax as much as the shell's, and they
        // nest: the joined `sh -c` string used to apply both. Stripping one of
        // each in a single pass left `(` as the program, so a negation turned
        // that 127 into a reported *pass* for a command that never ran.
        for (args, expected) in [
            (vec!["test", "!", "false"], 0),
            (vec!["test", "!", "true"], 1),
            (vec!["test", "(", "false", ")"], 1),
            (vec!["test", "(", "!", "false", ")"], 0),
            (vec!["test", "!", "(", "true", ")"], 1),
            (vec!["test", "!", "(", "false", ")"], 0),
            (vec!["test", "(", "(", "false", ")", ")"], 1),
            (vec!["test", "!", "!", "(", "true", ")"], 0),
        ] {
            let output = rtk()
                .args(&args)
                .output()
                .unwrap_or_else(|e| panic!("run rtk {args:?}: {e}"));
            assert_eq!(output.status.code(), Some(expected), "{args:?}");
            assert!(
                !String::from_utf8_lossy(&output.stdout).contains("command not found"),
                "{args:?} must run the command, not report it missing"
            );
        }
    }

    #[test]
    fn a_single_shell_phrase_falls_back_to_the_platform_shell() {
        let output = rtk()
            .args(["run", "echo one two"])
            .output()
            .expect("run rtk run");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "one two\n");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("pass arguments separately"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn the_fallback_propagates_the_script_exit_code() {
        let output = rtk()
            .args(["err", "true && false"])
            .output()
            .expect("run rtk err");

        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn single_string_shell_syntax_reaches_the_shell() {
        for (args, expected_code) in [
            (["run", "echo x | grep x"], 0),
            (["summary", "echo a && echo b"], 0),
            (["test", "cd /tmp && pwd"], 0),
            (["err", "cargo --version 2>&1"], 0),
        ] {
            let output = rtk()
                .args(args)
                .output()
                .unwrap_or_else(|e| panic!("run rtk {args:?}: {e}"));
            assert_eq!(output.status.code(), Some(expected_code), "{args:?}");
            assert!(
                !String::from_utf8_lossy(&output.stdout).contains("command not found"),
                "{args:?} must run through the shell, not report it missing"
            );
        }
    }

    #[test]
    fn a_single_bare_word_still_answers_127_without_the_shell() {
        let output = rtk()
            .args(["run", "rtk-no-such-binary-4c1f"])
            .output()
            .expect("run rtk run");

        assert_eq!(output.status.code(), Some(127));
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("pass arguments separately"),
            "a bare word must not trigger the compatibility fallback"
        );
    }

    #[test]
    fn the_fallback_requires_a_single_argument() {
        let output = rtk()
            .args(["run", "echo one two", "x"])
            .output()
            .expect("run rtk run");

        assert_eq!(output.status.code(), Some(127));
    }
}

#[cfg(windows)]
mod windows {
    use std::process::Command;

    fn rtk() -> Command {
        crate::common::rtk_command()
    }

    /// Direct execution resolves through `%PATH%` (and `PATHEXT`), where the
    /// `cmd /C` string it replaced also searched the working directory and
    /// carried builtins. Anything `cmd`-specific now needs `rtk run -c`.
    #[test]
    fn direct_execution_resolves_through_path() {
        let output = rtk()
            .args(["run", "cmd", "/C", "echo windows_ok"])
            .output()
            .expect("run rtk run");

        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("windows_ok"));
    }

    /// A command string runs through cmd, the Windows default, as
    /// `cmd /S /C "<script>"`: cmd strips exactly that wrapping pair of quotes,
    /// so the script's own quotes and its `&` reach cmd as written. `echo "a b"`
    /// prints the quotes, and `&` runs the second command.
    #[test]
    fn command_string_reaches_cmd_as_written() {
        let output = rtk()
            .args(["run", "-c", r#"echo "a b" & echo c"#])
            .output()
            .expect("run rtk run -c");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout:?}");
        // cmd's `echo` keeps the space before `&`, hence the trim.
        let lines: Vec<&str> = stdout.lines().map(str::trim_end).collect();
        assert_eq!(lines, [r#""a b""#, "c"], "{stdout:?}");
    }

    /// A script that starts with a quoted program path holding a space runs
    /// that program (#4288): cmd receives `""<dir with space>\rtk copy.exe"
    /// --version"`, strips the outer pair, and runs the quoted path.
    #[test]
    fn command_string_runs_a_quoted_program_path_with_a_space() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let spaced = dir.path().join("dir with space");
        std::fs::create_dir(&spaced).expect("create spaced dir");
        let program = spaced.join("rtk copy.exe");
        // The copy only ever runs under the rtk spawned below, as a child of its
        // cmd, so it inherits the data redirection `rtk()` gives that process.
        std::fs::copy(rtk().get_program(), &program).expect("copy rtk");

        let script = format!("\"{}\" --version", program.display());
        let output = rtk()
            .args(["run", "-c", &script])
            .output()
            .expect("run rtk run -c");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout:?} {stderr:?}");
        assert!(stdout.starts_with("rtk "), "{stdout:?} {stderr:?}");
    }

    /// A `&` inside a cmd script's quotes stays there on every surface that
    /// hands cmd a script. Escaped quotes would write `\"a & b\"`, and a `&`
    /// out of its quotes would split the echo.
    #[test]
    fn cmd_scripts_keep_a_quoted_ampersand_and_their_exit_code() {
        crate::assert_script_surfaces(r#"echo "a & b">> out.txt & exit 7"#, r#""a & b""#);
    }

    /// An argument carrying a `"` reaches a non-batch child with the encoding
    /// MSYS/Cygwin and libuv children expect (`ChildCommand::args`, #3728).
    ///
    /// `cmd.exe` is the wrong witness for this — it parses its own way and
    /// echoes the encoding back verbatim — so the child here is `rtk` itself:
    /// `rtk rewrite` prints the command string it received, which is only the
    /// one that was sent if the quote survived re-encoding on both sides.
    #[test]
    fn quoted_arguments_reach_the_child_intact() {
        let home = tempfile::tempdir().expect("create isolated home");
        let child = rtk();
        let output = rtk()
            .arg("run")
            .arg(child.get_program())
            .args(["rewrite", "git status \"a b\""])
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .output()
            .expect("run rtk run");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim_end(), "rtk git status \"a b\"", "{stdout}");
    }

    #[test]
    fn missing_program_reports_exit_127() {
        let output = rtk()
            .args(["err", "rtk-no-such-binary-4c1f"])
            .output()
            .expect("run rtk err");

        assert_eq!(output.status.code(), Some(127));
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("rtk-no-such-binary-4c1f: command not found")
        );
    }
}
