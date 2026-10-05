//! Exercise the public CLI, package-manager selection, spawn and output wiring.
//! A native Rust fake runs on Unix and Windows; no JS tools or downloads are used.
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::LazyLock;

mod common;

const PASS_JSON: &str =
    r#"{"numTotalTests":2,"numPassedTests":2,"numFailedTests":0,"testResults":[]}"#;
const FAIL_JSON: &str = r#"{"numTotalTests":1,"numPassedTests":0,"numFailedTests":1,"testResults":[{"name":"src/fail.test.ts","assertionResults":[{"fullName":"fails deliberately","status":"failed","failureMessages":["expected true, got false"]}]}]}"#;

// Compile once using the same Rust toolchain required to run the test suite.
// Keep bytes rather than a TempDir in a static so the compiler's scratch files
// are cleaned up, and every fixture owns its own executables.
static FAKE_RUNNER: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let dir = tempfile::tempdir().expect("compiler scratch directory");
    let output = dir
        .path()
        .join(format!("fake{}", std::env::consts::EXE_SUFFIX));
    let result = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_test_runner.rs"))
        .args(["--edition=2024", "--deny", "warnings", "-o"])
        .arg(&output)
        .output()
        .expect("compile native fake runner");
    assert!(
        result.status.success(),
        "compile fake runner: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    fs::read(output).expect("read native fake runner")
});

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(direct_tools: bool, lockfile: Option<&str>) -> Self {
        let dir = tempfile::tempdir().expect("fixture directory");
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).expect("fixture bin");
        // Install all runner fakes so choosing the wrong one is observable too.
        let mut names = vec!["pnpm", "yarn", "npx", "bunx"];
        if direct_tools {
            names.extend(["vitest", "jest"]);
        }
        for name in names {
            let path = bin.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            fs::write(&path, &*FAKE_RUNNER).expect("write fake executable");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                    .expect("make fake executable");
            }
        }
        if let Some(lockfile) = lockfile {
            fs::write(dir.path().join(lockfile), "").expect("write lockfile");
        }
        Self { dir }
    }

    fn run(&self, args: &[&str], stdout: &str, stderr: &str, exit: i32) -> Output {
        common::rtk_command()
            .args(args)
            .current_dir(self.dir.path())
            // No inherited PATH: an installed runner must never defeat the fake
            // or cause these tests to execute/download real JavaScript packages.
            .env("PATH", self.dir.path().join("bin"))
            .env("PATHEXT", ".EXE")
            .env("NO_COLOR", "1")
            .env("RTK_TEST_TRACE", self.dir.path().join("trace"))
            .env("RTK_TEST_STDOUT", stdout)
            .env("RTK_TEST_STDERR", stderr)
            .env("RTK_TEST_EXIT", exit.to_string())
            .output()
            .expect("run rtk")
    }

    fn assert_invocation(&self, expected: &[&str]) {
        let trace = fs::read_to_string(self.dir.path().join("trace")).expect("read spawn trace");
        let expected: Vec<_> = expected.iter().copied().chain(["END"]).collect();
        assert_eq!(trace.lines().collect::<Vec<_>>(), expected);
    }
}

fn framework_args(framework: &str) -> Vec<&str> {
    match framework {
        "vitest" => vec!["run", "--reporter=json"],
        "jest" => vec!["--no-watch", "--json"],
        _ => panic!("unknown test framework"),
    }
}

#[test]
fn cli_dispatches_each_framework_to_its_own_executable_and_arguments() {
    for framework in ["vitest", "jest"] {
        // A lockfile must not displace a runner already available on PATH.
        let fixture = Fixture::new(true, Some("pnpm-lock.yaml"));
        let user_args = ["src/has space.test.ts", "-t", "works with spaces"];
        let args: Vec<_> = [framework].into_iter().chain(user_args).collect();
        let out = fixture.run(&args, PASS_JSON, "", 0);
        let mut expected = vec![framework];
        expected.extend(framework_args(framework));
        expected.extend(user_args);
        fixture.assert_invocation(&expected);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(out.stdout, b"PASS (2) FAIL (0)\n");
        assert!(out.stderr.is_empty());
    }
}

#[test]
fn missing_framework_uses_the_projects_package_manager_without_fetching() {
    for (lockfile, prefix) in [
        (Some("pnpm-lock.yaml"), vec!["pnpm", "exec", "--"]),
        (Some("yarn.lock"), vec!["yarn", "exec", "--"]),
        (Some("bun.lock"), vec!["npx", "--no-install", "--"]),
        (None, vec!["npx", "--no-install", "--"]),
    ] {
        for framework in ["vitest", "jest"] {
            let fixture = Fixture::new(false, lockfile);
            let out = fixture.run(&[framework, "src/a.test.ts"], PASS_JSON, "", 0);
            let mut expected = prefix.clone();
            expected.push(framework);
            expected.extend(framework_args(framework));
            expected.push("src/a.test.ts");
            fixture.assert_invocation(&expected);
            assert_eq!(out.status.code(), Some(0));
            assert_eq!(out.stdout, b"PASS (2) FAIL (0)\n");
            assert!(out.stderr.is_empty());
        }
    }
}

#[test]
fn explicit_vitest_reporter_reaches_the_spawn_and_bypasses_parsing() {
    let verbose =
        " ✓ src/a.test.ts > keeps this test name\n\n Tests  2 passed (2)\n Duration  10ms\n";
    for (reporter, raw) in [
        (vec!["--reporter=verbose"], verbose),
        (vec!["--reporter", "verbose"], verbose),
        // An explicitly requested JSON report must also remain JSON.
        (vec!["--reporter=json"], PASS_JSON),
    ] {
        let fixture = Fixture::new(true, None);
        let mut args = vec!["vitest", "src/a.test.ts"];
        args.extend(&reporter);
        let out = fixture.run(&args, raw, "", 0);
        let mut expected = vec!["vitest", "run", "src/a.test.ts"];
        expected.extend(&reporter);
        fixture.assert_invocation(&expected);
        assert_eq!(out.status.code(), Some(0));
        // The existing emitter appends a newline to the unparsed report.
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{raw}\n"));
        assert!(out.stderr.is_empty());
    }
}

#[test]
fn framework_failures_keep_the_child_exit_status() {
    for framework in ["vitest", "jest"] {
        let fixture = Fixture::new(true, None);
        let out = fixture.run(&[framework], FAIL_JSON, "", 7);
        let mut expected = vec![framework];
        expected.extend(framework_args(framework));
        fixture.assert_invocation(&expected);
        assert_eq!(out.status.code(), Some(7));
        let shown = String::from_utf8_lossy(&out.stdout);
        assert!(shown.contains("FAIL (1)"), "{shown}");
        assert!(shown.contains("fails deliberately"), "{shown}");
        assert!(shown.contains("expected true, got false"), "{shown}");
        assert!(out.stderr.is_empty());
    }
}

#[test]
fn explicit_reporter_keeps_stderr_diagnostics_and_failure_status() {
    let fixture = Fixture::new(true, None);
    let stdout = " Tests  2 passed (2)\n";
    let stderr = "reporter teardown failed\n";
    let out = fixture.run(&["vitest", "--reporter=verbose"], stdout, stderr, 3);
    fixture.assert_invocation(&["vitest", "run", "--reporter=verbose"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{stdout}{stderr}\n")
    );
    assert!(out.stderr.is_empty());
}
