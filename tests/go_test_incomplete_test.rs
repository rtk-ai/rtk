use std::io::Write;
use std::process::{Output, Stdio};

mod common;

const FIXTURE: &str = include_str!("fixtures/go_test_truncation_raw.jsonl");

fn pipe_go_test(input: &str) -> Output {
    let mut child = common::rtk_command()
        .args(["pipe", "-f", "go-test"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk pipe");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input.as_bytes())
        .expect("write Go test events");
    child.wait_with_output().expect("wait for rtk pipe")
}

#[test]
fn truncated_go_test_pipe_keeps_failure_diagnostics() {
    let input = FIXTURE
        .split_inclusive('\n')
        .take_while(|line| {
            !line.contains(
                "\"Action\":\"fail\",\"Package\":\"example.com/stream/zz\",\"Test\":\"TestX\"",
            )
        })
        .collect::<String>();
    assert!(input.contains("expected 1, got 2"));

    let output = pipe_go_test(&input);
    assert!(output.status.success());
    assert_eq!(output.stdout, input.as_bytes());
}

#[test]
fn complete_go_test_pipe_still_summarizes_failures() {
    let output = pipe_go_test(FIXTURE);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
    assert!(stdout.starts_with("Go test: 1 passed, 1 failed, 1 skipped in 2 packages"));
    assert!(stdout.contains("z_test.go:6: expected 1, got 2"));
}
