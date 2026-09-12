#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

const STDOUT: &str = "=== RUN   TestPass\n    pass_test.go:4: PASS_MARKER\n--- PASS: TestPass (0.00s)\nPASS\nok\texample.com/probe\t0.001s\n";
const STDERR: &str = "test stderr marker\n";

struct GoShim {
    dir: tempfile::TempDir,
}

impl GoShim {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temporary directory");
        let shim = dir.path().join("go");
        fs::write(
            &shim,
            r#"#!/bin/sh
printf '%s\n' "$@" > "$RTK_TEST_ARGV"
for arg do
    if [ "$arg" = '-json' ]; then
        printf '%s\n' '{"Action":"output","Package":"example.com/probe","Test":"TestPass","Output":"PASS_MARKER\n"}'
        printf '%s\n' '{"Action":"pass","Package":"example.com/probe","Test":"TestPass","Elapsed":0.001}'
        printf '%s\n' '{"Action":"pass","Package":"example.com/probe","Elapsed":0.001}'
        exit "${RTK_TEST_EXIT:-0}"
    fi
done
printf '=== RUN   TestPass\n    pass_test.go:4: PASS_MARKER\n--- PASS: TestPass (0.00s)\nPASS\nok\texample.com/probe\t0.001s\n'
printf 'test stderr marker\n' >&2
exit "${RTK_TEST_EXIT:-0}"
"#,
        )
        .expect("write Go shim");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("executable shim");
        Self { dir }
    }

    fn run(&self, args: &[&str], exit_code: i32) -> Output {
        let mut paths = vec![self.dir.path().to_path_buf()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["go", "test"])
            .args(args)
            .current_dir(self.dir.path())
            .env("PATH", std::env::join_paths(paths).expect("shim PATH"))
            .env("RTK_DB_PATH", self.dir.path().join("tracking.db"))
            .env("RTK_RECALL", "0")
            .env("RTK_TEST_ARGV", self.dir.path().join("argv"))
            .env("RTK_TEST_EXIT", exit_code.to_string())
            .output()
            .expect("run rtk go test")
    }

    fn argv(&self) -> Vec<String> {
        fs::read_to_string(self.dir.path().join("argv"))
            .expect("recorded argv")
            .lines()
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn explicit_go_test_verbosity_preserves_output_and_exit_status() {
    let shim = GoShim::new();
    for args in [
        vec!["-v", "./..."],
        vec!["./...", "-test.v"],
        vec!["-v=true"],
        vec!["--test.v=1"],
        vec!["-v=false", "-v"],
        vec!["-v", "-o", "-v=false"],
        vec!["-v", "-run", "TestPass", "-args", "custom value"],
    ] {
        for exit_code in [0, 1] {
            let output = shim.run(&args, exit_code);
            assert_eq!(output.status.code(), Some(exit_code), "args: {args:?}");
            assert_eq!(output.stdout, STDOUT.as_bytes(), "args: {args:?}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(STDERR), "args: {args:?}, stderr: {stderr}");
            let expected: Vec<_> = std::iter::once("test")
                .chain(args.iter().copied())
                .map(str::to_string)
                .collect();
            assert_eq!(shim.argv(), expected);
        }
    }
}

#[test]
fn default_and_disabled_verbosity_keep_go_test_compression() {
    let shim = GoShim::new();
    for args in [
        vec!["./..."],
        vec!["-v=false"],
        vec!["-v", "-test.v=0"],
        vec!["-run", "-v"],
        vec!["-args", "-v"],
        vec!["./...", "--", "-v"],
    ] {
        let output = shim.run(&args, 0);
        assert!(output.status.success(), "args: {args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("1 passed"), "args: {args:?}: {stdout}");
        assert!(!stdout.contains("PASS_MARKER"), "args: {args:?}");
        assert_eq!(&shim.argv()[..2], ["test", "-json"]);
    }
}
