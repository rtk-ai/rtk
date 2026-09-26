#[cfg(unix)]
mod unix {
    use std::process::Command;

    fn rtk() -> Command {
        Command::new(env!("CARGO_BIN_EXE_rtk"))
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
}

#[cfg(windows)]
mod windows {
    use std::process::Command;

    fn rtk() -> Command {
        Command::new(env!("CARGO_BIN_EXE_rtk"))
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

    /// An argument carrying a `"` reaches a non-batch child with the encoding
    /// MSYS/Cygwin and libuv children expect (`child_args`, #3728).
    ///
    /// `cmd.exe` is the wrong witness for this — it parses its own way and
    /// echoes the encoding back verbatim — so the child here is `rtk` itself:
    /// `rtk rewrite` prints the command string it received, which is only the
    /// one that was sent if the quote survived re-encoding on both sides.
    #[test]
    fn quoted_arguments_reach_the_child_intact() {
        let home = tempfile::tempdir().expect("create isolated home");
        let output = rtk()
            .args([
                "run",
                env!("CARGO_BIN_EXE_rtk"),
                "rewrite",
                "git status \"a b\"",
            ])
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("RTK_TELEMETRY_DISABLED", "1")
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
