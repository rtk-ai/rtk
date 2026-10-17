//! Regression coverage for cmd.exe argument forwarding (#4333).
//! These launch the built RTK binary, not a model of cmd's parser.
#![cfg(windows)]

mod common;

use std::process::{Command, Output, Stdio};

fn invocation(route: &[&str], program: &str) -> Command {
    let mut command = common::rtk_command();
    command.args(route).arg(program).stdin(Stdio::null());
    command
}

fn assert_lines(output: &Output, expected: &[&str]) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?}, stdout={stdout:?}, stderr={stderr:?}",
        output.status
    );
    let lines: Vec<_> = stdout.lines().map(str::trim_end).collect();
    assert_eq!(lines, expected, "stderr={stderr:?}");
}

const ROUTES: &[&[&str]] = &[&[], &["proxy"], &["run"]];

#[test]
fn quoted_echo_survives_passthrough_proxy_and_direct_run() {
    for route in ROUTES {
        for program in ["cmd", "CMD.EXE"] {
            let output = invocation(route, program)
                .args(["/D", "/C", r#"echo "a b""#])
                .output()
                .expect("run RTK with cmd");
            assert_lines(&output, &[r#""a b""#]);
        }
    }
}

#[test]
fn if_exist_handles_a_quoted_path_without_relying_on_git_installation() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("dir with spaces & (parentheses)");
    std::fs::create_dir(&path).expect("create fixture directory");
    let path = path.join("probe.txt");
    std::fs::write(&path, "fixture").expect("write fixture");
    let script = format!(
        r#"if exist "{}" (echo FOUND) else (echo MISSING)"#,
        path.display()
    );

    for route in ROUTES {
        let output = invocation(route, "cmd.exe")
            .args(["/D", "/C", &script])
            .output()
            .expect("run if exist");
        assert_lines(&output, &["FOUND"]);
    }
}

#[test]
fn quoted_executable_path_works_with_absolute_cmd_path() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let program = dir.path().join("rtk copy.exe");
    std::fs::copy(common::rtk_command().get_program(), &program).expect("copy RTK fixture");
    let cmd = which::which("cmd.exe").expect("resolve cmd.exe");
    let script = format!(r#""{}" --version"#, program.display());

    for route in ROUTES {
        let output = invocation(route, cmd.to_str().expect("cmd path is UTF-8"))
            .args(["/D", "/C", &script])
            .output()
            .expect("run quoted executable");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "stdout={stdout:?}, stderr={stderr:?}"
        );
        assert!(
            stdout.starts_with("rtk "),
            "stdout={stdout:?}, stderr={stderr:?}"
        );
    }
}

#[test]
fn split_command_fragments_are_joined_as_one_script() {
    for route in ROUTES {
        let output = invocation(route, "cmd.exe")
            .args(["/D", "/C", "echo", r#""a b""#, "&", "echo", "done"])
            .output()
            .expect("run split script");
        assert_lines(&output, &[r#""a b""#, "done"]);
    }
}

#[test]
fn k_mode_preserves_options_expansion_and_exit_status() {
    for route in ROUTES {
        let output = invocation(route, "cmd.exe")
            .args([
                "/D",
                "/S",
                "/V:ON",
                "/K",
                r#"set RTK_4333=ok& echo "!RTK_4333!"& exit 7"#,
            ])
            .output()
            .expect("run terminating /K script");
        assert_eq!(output.status.code(), Some(7), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), r#""ok""#);
    }
}

#[test]
fn quoted_redirection_remains_text_instead_of_creating_a_file() {
    let dir = tempfile::tempdir().expect("temporary directory");
    for route in ROUTES {
        let output = invocation(route, "cmd.exe")
            .current_dir(dir.path())
            .args(["/D", "/C", r#"echo "x > stray""#])
            .output()
            .expect("run quoted redirection");
        assert_lines(&output, &[r#""x > stray""#]);
        assert!(!dir.path().join("stray").exists());
    }
}

#[test]
fn split_executable_path_retains_its_argument_boundaries() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let program = dir.path().join("rtk copy.exe");
    std::fs::copy(common::rtk_command().get_program(), &program).expect("copy RTK fixture");
    for route in ROUTES {
        let output = invocation(route, "cmd.exe")
            .args(["/D", "/C"])
            .arg(&program)
            .arg("--version")
            .output()
            .expect("run split executable path");
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).starts_with("rtk "));
    }
}

#[test]
fn attached_c_switch_preserves_embedded_quotes() {
    for route in ROUTES {
        let output = invocation(route, "cmd.exe")
            .args(["/D", r#"/cecho "a b""#])
            .output()
            .expect("run attached /C payload");
        assert_lines(&output, &[r#""a b""#]);
    }
}
