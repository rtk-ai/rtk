use std::process::{Command, Output};

fn rewrite(command: &str) -> Output {
    let home = tempfile::tempdir().expect("create isolated home");
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["rewrite", command])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("RTK_TELEMETRY_DISABLED", "1")
        .output()
        .expect("run rtk rewrite")
}

#[test]
fn bash_command_string_rewrites_inner_commands() {
    let output = rewrite(r#"bash -c "head foo && grep -R bar .""#);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        r#"bash -c "rtk read foo --head-lines 10 && rtk grep -R bar .""#
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn zsh_command_string_rewrites_inner_commands() {
    let output = rewrite("zsh -c 'git status; cargo test'");

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "zsh -c 'rtk git status; rtk cargo test'"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn fish_command_string_passes_through() {
    let output = rewrite("fish -c 'git status; cargo test'");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn unsafe_inner_script_passes_through_without_output() {
    let output = rewrite("bash -c 'git status $(whoami)'");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

/// The wrapper is reached behind a transparent prefix, and the permission gate
/// reads back the same position — so this is rewritten and asks (3), never
/// auto-allowed (#3828 review).
#[test]
fn prefixed_command_string_rewrites_inner_commands() {
    let output = rewrite("nice bash -c 'git status; cargo test'");

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "nice bash -c 'rtk git status; rtk cargo test'"
    );
    assert!(output.stderr.is_empty());
}

/// A word that merely looks like a wrapper is not one: the rewriter never
/// offers that position, so nothing here is treated as a second boundary.
#[test]
fn wrapper_shaped_argument_is_not_a_wrapper() {
    let output = rewrite("echo bash -c 'git status'");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
