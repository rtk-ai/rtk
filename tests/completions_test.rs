//! End-to-end tests for `rtk completions <shell>`.

use std::process::Command;

fn rtk_stdout(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .expect("spawn rtk");
    assert!(
        out.status.success(),
        "rtk {:?} failed with stderr: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn completions_bash_generates_script() {
    let out = rtk_stdout(&["completions", "bash"]);
    assert!(!out.is_empty(), "bash completion script must not be empty");
    assert!(
        out.contains("_rtk"),
        "bash script should define the _rtk function"
    );
    assert!(
        out.contains("completions"),
        "bash script should complete the completions subcommand"
    );
}

#[test]
fn completions_zsh_generates_script() {
    let out = rtk_stdout(&["completions", "zsh"]);
    assert!(!out.is_empty(), "zsh completion script must not be empty");
    assert!(
        out.contains("#compdef rtk"),
        "zsh script should declare the compdef"
    );
    assert!(
        out.contains("_rtk"),
        "zsh script should define the _rtk function"
    );
}

#[test]
fn completions_fish_generates_script() {
    let out = rtk_stdout(&["completions", "fish"]);
    assert!(!out.is_empty(), "fish completion script must not be empty");
    assert!(
        out.contains("rtk"),
        "fish script should reference the rtk command"
    );
}

#[test]
fn completions_powershell_generates_script() {
    let out = rtk_stdout(&["completions", "powershell"]);
    assert!(
        !out.is_empty(),
        "powershell completion script must not be empty"
    );
    assert!(
        out.contains("rtk"),
        "powershell script should reference the rtk command"
    );
}

#[test]
fn completions_elvish_generates_script() {
    let out = rtk_stdout(&["completions", "elvish"]);
    assert!(
        !out.is_empty(),
        "elvish completion script must not be empty"
    );
    assert!(
        out.contains("rtk"),
        "elvish script should reference the rtk command"
    );
}

#[test]
fn completions_invalid_shell_fails_with_usage() {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .args(["completions", "tcsh"])
        .output()
        .expect("spawn rtk");
    assert!(
        !out.status.success(),
        "an unsupported shell must produce a parse error"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid value"),
        "stderr should explain the invalid value: {}",
        stderr
    );
    assert!(
        stderr.contains("bash"),
        "stderr should list possible values incl. bash: {}",
        stderr
    );
}

#[test]
fn completions_missing_shell_fails() {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .arg("completions")
        .output()
        .expect("spawn rtk");
    assert!(!out.status.success(), "missing shell argument must fail");
}
