//! `--` handling for `rtk ruff`. clap's `trailing_var_arg` strips a `--` that leads `args`, so
//! without `restore_double_dash` `rtk ruff -- check` reached `run()` as `["check"]`, was routed
//! as the `check` subcommand with no path, and linted the whole working directory. ruff itself
//! answers `ruff -- check` with its usage error, and `ruff check -- check` lints only the file
//! named `check`. Stubs `ruff` on PATH with a script that records its argv.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;

mod common;

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// Runs `rtk ruff <args>` against a `ruff` stub and returns the argv the stub received.
fn ruff_argv(args: &[&str]) -> Vec<String> {
    let dir = common::tempdir();
    let argv_file = dir.path().join("argv.txt");
    let stub_path = dir.path().join("ruff");

    // `[]` is a clean `check --output-format=json` run, so the filtered path exits 0 too.
    fs::write(
        &stub_path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\necho '[]'\n",
            shell_quote(&argv_file)
        ),
    )
    .expect("write stub");
    fs::set_permissions(&stub_path, fs::Permissions::from_mode(0o755)).expect("chmod stub");

    let path_with_stub = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = common::rtk_command()
        .arg("ruff")
        .args(args)
        .env("PATH", path_with_stub)
        .env("LC_ALL", "C")
        .current_dir(dir.path())
        .output()
        .expect("spawn rtk");

    fs::read_to_string(&argv_file)
        .unwrap_or_else(|e| {
            panic!(
                "read captured argv: {e}; rtk stdout={} stderr={}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        })
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn ruff_leading_double_dash_reaches_ruff() {
    let argv = ruff_argv(&["--", "check"]);
    assert_eq!(argv, vec!["--", "check"]);
}

#[test]
fn ruff_check_double_dash_is_forwarded_once() {
    let argv = ruff_argv(&["check", "--", "check"]);
    assert_eq!(argv, vec!["check", "--output-format=json", "--", "check"]);
}

#[test]
fn ruff_double_dash_after_a_flag_is_forwarded_once() {
    let argv = ruff_argv(&["--isolated", "--", "check"]);
    assert_eq!(argv, vec!["--isolated", "--", "check"]);
}
