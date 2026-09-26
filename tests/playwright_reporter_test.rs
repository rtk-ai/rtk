//! `rtk playwright test` forces `--reporter=json` and drops the user's own reporter
//! flag. A reporter given as `--reporter list` has to take its value with it: left
//! behind, `list` reaches Playwright as a test filter and the run silently executes
//! the wrong (usually empty) test set. These pin what Playwright is actually invoked
//! with.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

/// Runs `rtk playwright test <user_args>` against an `npx` that only records its
/// argv, and returns that argv.
fn argv_playwright_is_run_with(user_args: &[&str]) -> Vec<String> {
    let dir = tempfile::tempdir().expect("tempdir");
    let argv_file = dir.path().join("argv.txt");
    let npx = dir.path().join("npx");
    fs::write(
        &npx,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            argv_file.display()
        ),
    )
    .expect("write fake npx");
    fs::set_permissions(&npx, fs::Permissions::from_mode(0o755)).expect("chmod fake npx");

    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // No lockfile in the working directory, so rtk picks npx.
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["playwright", "test"])
        .args(user_args)
        .current_dir(dir.path())
        .env("PATH", path)
        .output()
        .expect("run rtk playwright");
    assert!(
        out.status.success(),
        "rtk playwright failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    fs::read_to_string(&argv_file)
        .expect("fake npx was never run")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn space_separated_reporter_value_is_not_left_behind_as_a_test_filter() {
    let argv = argv_playwright_is_run_with(&["--reporter", "list", "e2e/login.spec.ts"]);

    assert!(
        argv.ends_with(&[
            "test".to_string(),
            "--reporter=json".to_string(),
            "e2e/login.spec.ts".to_string()
        ]),
        "the reporter's value must go with its flag, got: {argv:?}"
    );
}

#[test]
fn equals_form_reporter_is_replaced_by_the_forced_json_reporter() {
    let argv = argv_playwright_is_run_with(&["--reporter=dot", "e2e/login.spec.ts"]);

    assert!(
        argv.ends_with(&[
            "test".to_string(),
            "--reporter=json".to_string(),
            "e2e/login.spec.ts".to_string()
        ]),
        "got: {argv:?}"
    );
}
