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
    fn explicit_missing_shell_reports_actionable_error() {
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

        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("Shell 'rtk-missing-shell-for-test' not found"));
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
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("pass the shell command as one quoted argument after --shell"));
    }
}
