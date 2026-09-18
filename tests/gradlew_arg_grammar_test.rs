//! End-to-end regression tests for `rtk gradlew` argument handling.
//!
//! Asserts on the resolved routing (visible as whether the output was filtered) and on the
//! child's argv, not on the private predicate: a predicate-level test passes even when the
//! filter still runs on the wrong branch.
//!
//! Stubs `./gradlew` with a script that records its argv, since gradle isn't available here.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;

/// Gradle-shaped output: a daemon banner and a PASSED line that every filter strips, so their
/// presence means rtk fell through to unfiltered passthrough.
const STUB_OUTPUT: &str = "Starting a Gradle Daemon (subsequent builds will be faster)\n\
                           > Task :test\n\
                           com.example.FooTest > shouldWork PASSED\n\
                           BUILD SUCCESSFUL in 1s\n\
                           2 actionable tasks: 2 executed\n";

struct Stub {
    dir: tempfile::TempDir,
}

impl Stub {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > ./argv.txt\ncat <<'RTKEOF'\n{STUB_OUTPUT}RTKEOF\nexit 0\n"
        );
        let path = dir.path().join("gradlew");
        std::fs::write(&path, script).expect("write stub");
        let mut perms = std::fs::metadata(&path).expect("stat stub").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).expect("chmod stub");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn run(&self, args: &[&str]) -> (String, Vec<String>) {
        let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .current_dir(self.path())
            .arg("gradlew")
            .args(args)
            .output()
            .expect("spawn rtk");
        assert!(
            out.status.success(),
            "rtk gradlew {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let argv = std::fs::read_to_string(self.path().join("argv.txt"))
            .expect("read captured argv")
            .lines()
            .map(str::to_string)
            .collect();
        (String::from_utf8_lossy(&out.stdout).into_owned(), argv)
    }
}

fn assert_filtered(stdout: &str, args: &[&str]) {
    assert!(
        !stdout.contains("Starting a Gradle Daemon") && !stdout.contains("PASSED"),
        "rtk gradlew {args:?} should route to a filter, got raw passthrough:\n{stdout}"
    );
}

#[test]
fn tests_filter_value_does_not_misroute_the_task() {
    let stub = Stub::new();
    let args = ["test", "--tests", "com.example.Foo"];
    let (stdout, argv) = stub.run(&args);
    assert_filtered(&stdout, &args);
    assert_eq!(argv, ["test", "--tests", "com.example.Foo"]);
}

#[test]
fn project_dir_value_does_not_misroute_the_task() {
    let stub = Stub::new();
    let args = ["assembleDebug", "-p", "../other"];
    let (stdout, argv) = stub.run(&args);
    assert_filtered(&stdout, &args);
    assert_eq!(argv, ["assembleDebug", "-p", "../other"]);
}

#[test]
fn group_value_does_not_misroute_the_task_listing() {
    let stub = Stub::new();
    let (stdout, argv) = stub.run(&["tasks", "--group", "build"]);
    assert!(
        stdout.contains("Starting a Gradle Daemon"),
        "`tasks` has no filter; its listing must not be swallowed by the build filter:\n{stdout}"
    );
    assert_eq!(argv, ["tasks", "--group", "build"]);
}

#[test]
fn flags_without_a_task_do_not_route_to_the_build_filter() {
    let stub = Stub::new();
    let (stdout, argv) = stub.run(&["-p", "../other"]);
    assert!(
        stdout.contains("Starting a Gradle Daemon"),
        "gradle runs its default task here; that output is not build output:\n{stdout}"
    );
    assert_eq!(argv, ["-p", "../other"]);
}

#[test]
fn no_args_does_not_route_to_the_build_filter() {
    let stub = Stub::new();
    let (stdout, _) = stub.run(&[]);
    assert!(
        stdout.contains("Starting a Gradle Daemon"),
        "bare gradlew runs the default task; that output is not build output:\n{stdout}"
    );
}

#[test]
fn short_info_flag_bypasses_filtering_like_its_long_spelling() {
    let stub = Stub::new();
    let (stdout, argv) = stub.run(&["build", "-i"]);
    assert!(
        stdout.contains("Starting a Gradle Daemon"),
        "-i means --info, which must pass output through unfiltered:\n{stdout}"
    );
    assert_eq!(argv, ["build", "-i"]);
}

#[test]
fn users_double_dash_reaches_gradle_and_tasks_past_it_still_route() {
    let stub = Stub::new();
    let args = ["--", "test", "--tests", "com.example.Foo"];
    let (stdout, argv) = stub.run(&args);
    assert_filtered(&stdout, &args);
    assert_eq!(argv, ["--", "test", "--tests", "com.example.Foo"]);
}
