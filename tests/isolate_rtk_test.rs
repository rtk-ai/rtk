//! A child of an isolated command, and so every tool an isolated rtk runs, inherits nothing
//! from the developer's shell that isolation drops. A shell exports options, files to source
//! and functions that change what a shell script does, and which of them it has must not
//! decide a test's result.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

mod common;

/// A command that prints its environment as `NAME=value` lines. `/D` keeps cmd.exe from
/// running the registry's AutoRun command first.
#[cfg(windows)]
const PRINT_ENVIRONMENT: [&str; 4] = ["cmd", "/D", "/C", "set"];
#[cfg(not(windows))]
const PRINT_ENVIRONMENT: [&str; 1] = ["env"];

/// A variable name as the platform compares it: Windows ignores case.
fn key(name: &str) -> String {
    if cfg!(windows) {
        name.to_ascii_uppercase()
    } else {
        name.to_owned()
    }
}

/// Defined by the child itself when its environment has none: cmd.exe sets `PROMPT`, and on
/// macOS the CoreFoundation rtk links sets `__CF_USER_TEXT_ENCODING`, which the printer rtk
/// runs then inherits.
fn defined_by_the_child(name: &str) -> bool {
    (cfg!(windows) && name == "PROMPT")
        || (cfg!(target_os = "macos") && name == "__CF_USER_TEXT_ENCODING")
}

/// Runs `cmd`, which ends in printing the environment its last child gets, and checks that
/// none of what this process inherited and isolation dropped is in it.
fn assert_isolated(mut cmd: Command) {
    let set: HashSet<String> = cmd
        .get_envs()
        .filter(|(_, value)| value.is_some())
        .filter_map(|(name, _)| name.to_str().map(key))
        .collect();
    let dropped: HashSet<String> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .map(|name| key(&name))
        .filter(|name| !set.contains(name))
        .collect();
    // Clearing shows only if this process inherited something isolation leaves out.
    assert!(
        !dropped.is_empty(),
        "this process inherited nothing that isolation leaves out"
    );

    let out = cmd.output().expect("run the environment printer");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let child: HashMap<String, &str> = stdout
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (key(name), value))
        .filter(|(name, _)| !name.is_empty())
        .collect();

    // What isolation sets is there, so the printer's output is being read, and rtk's data
    // goes to the scratch directory.
    let db = child.get(&key("RTK_DB_PATH")).copied();
    assert!(
        db.is_some_and(|db| Path::new(db).starts_with(common::scratch_dir())),
        "RTK_DB_PATH={db:?} outside {}",
        common::scratch_dir().display()
    );
    assert_eq!(
        child.get(&key("RTK_TELEMETRY_DISABLED")).copied(),
        Some("1"),
        "{child:?}"
    );
    let mut leaked: Vec<_> = child
        .keys()
        .filter(|name| dropped.contains(*name))
        .filter(|name| !defined_by_the_child(name))
        .collect();
    leaked.sort();
    assert!(
        leaked.is_empty(),
        "the child inherited these from the developer's shell: {leaked:?}"
    );
}

#[test]
fn an_isolated_child_inherits_nothing_isolation_drops() {
    let mut cmd = Command::new(PRINT_ENVIRONMENT[0]);
    cmd.args(&PRINT_ENVIRONMENT[1..]);
    common::isolate_rtk(&mut cmd);
    assert_isolated(cmd);
}

#[test]
fn a_native_tool_inherits_nothing_isolation_drops() {
    let mut cmd = common::native_command(PRINT_ENVIRONMENT[0]);
    cmd.args(&PRINT_ENVIRONMENT[1..]);
    assert_isolated(cmd);
}

#[test]
fn a_compared_git_inherits_nothing_isolation_drops() {
    let mut cmd = Command::new(PRINT_ENVIRONMENT[0]);
    cmd.args(&PRINT_ENVIRONMENT[1..]);
    common::isolate_git(&mut cmd);
    assert_isolated(cmd);
}

/// A test that isolates the git rtk runs as it does its own still gets rtk's data pinned.
#[test]
fn an_rtk_command_isolated_for_git_keeps_its_data_pinned() {
    let mut cmd = common::rtk_command();
    cmd.arg("proxy").args(PRINT_ENVIRONMENT);
    common::isolate_git(&mut cmd);
    assert_isolated(cmd);
}

#[test]
fn a_tool_rtk_runs_inherits_nothing_isolation_drops() {
    let mut cmd = common::rtk_command();
    cmd.arg("proxy").args(PRINT_ENVIRONMENT);
    assert_isolated(cmd);
}
