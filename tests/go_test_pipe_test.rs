//! `pipe` cannot recover an upstream exit status, but it must retain the report.
use std::io::Write;
use std::process::Stdio;

mod common;

fn pipe(input: &str) -> String {
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
        .expect("write input");
    let out = child.wait_with_output().expect("read output");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stderr.is_empty(), "unexpected stderr: {:?}", out.stderr);
    String::from_utf8(out.stdout).expect("UTF-8 output")
}

#[test]
fn plain_failure_after_many_successes_is_preserved_in_full() {
    let mut input = (0..25)
        .map(|i| format!("ok  \texample.com/proj/pkg{i}\t0.001s\n"))
        .collect::<String>();
    input.push_str("--- FAIL: TestX (0.00s)\n    x_test.go:12: expected 1, got 2\nFAIL\texample.com/proj/zz\t0.01s\nFAIL\n");
    assert_eq!(pipe(&input), input);
}

#[test]
fn plain_success_empty_and_non_event_json_are_not_misclassified() {
    for input in [
        "ok  \texample.com/proj\t0.003s\n?   \texample.com/tools\t[no test files]\n",
        "",
        " \n\t\n",
        "{\"message\":\"build failed\"}\n",
        "{\"Action\":\"deploy\",\"Package\":\"example.com/proj\"}\n",
        "{\"Action\":\"pass\"}\n",
        "{\"Action\":\"output\",\"Package\":\"example.com/proj\",\"Output\":42}\n",
    ] {
        assert_eq!(pipe(input), input);
    }
}

#[test]
fn mixed_json_and_text_keeps_every_line() {
    let events = "{\"Action\":\"pass\",\"Package\":\"example.com/proj\",\"Test\":\"TestOK\"}\n";
    for input in [
        format!("go: downloading example.com/dep v1.0.0\n{events}"),
        format!("{events}setup failed: missing dependency\n"),
        format!("{events}{{\"error\":\"not a Go test event\"}}\n"),
        format!("{events}{{malformed json\n"),
    ] {
        assert_eq!(pipe(&input), input);
    }
}

#[test]
fn valid_json_events_still_use_the_existing_filter() {
    let pass = "{\"Action\":\"run\",\"Package\":\"example.com/proj\",\"Test\":\"TestOK\"}\n{\"Action\":\"pass\",\"Package\":\"example.com/proj\",\"Test\":\"TestOK\"}\n{\"Action\":\"pass\",\"Package\":\"example.com/proj\"}\n";
    assert_eq!(pipe(pass), "Go test: 1 passed in 1 packages");
    let fail = "{\"Action\":\"output\",\"Package\":\"example.com/proj\",\"Test\":\"TestX\",\"Output\":\"    x_test.go:12: expected 1, got 2\\n\"}\n{\"Action\":\"fail\",\"Package\":\"example.com/proj\",\"Test\":\"TestX\"}\n{\"Action\":\"fail\",\"Package\":\"example.com/proj\"}\n";
    let shown = pipe(fail);
    assert!(shown.contains("TestX"), "{shown}");
    assert!(shown.contains("x_test.go:12"), "{shown}");
    assert!(!shown.contains("No tests found"), "{shown}");
}

#[test]
fn raw_fallback_preserves_line_endings_and_unterminated_text() {
    for input in [
        " \t\r\nFAIL\texample.com/proj\t0.01s\r\n  ",
        "--- FAIL: TestUnicode\n    expected 日本語, got 中文",
        "{\"message\":\"failure without newline\"}",
    ] {
        assert_eq!(pipe(input), input);
    }
}

#[test]
fn output_shaped_json_without_output_is_not_a_test_report() {
    for input in [
        r#"{"Action":"output","Package":"example.com/proj","message":"setup failed"}"#,
        r#"{"Action":"build-output","ImportPath":"example.com/proj","message":"compile failed"}"#,
    ] {
        assert_eq!(pipe(input), input);
    }
}

#[test]
fn padded_json_and_build_failures_keep_existing_summaries() {
    let input =
        " \r\n \t{\"Action\":\"pass\",\"Package\":\"example.com/proj\",\"Test\":\"TestOK\"} \r\n\t";
    assert_eq!(pipe(input), "Go test: 1 passed in 1 packages");
    let build = concat!(
        "{\"Action\":\"build-output\",\"ImportPath\":\"example.com/proj\",\"Output\":\"main.go:4: undefined: missing\\n\"}\n",
        "{\"Action\":\"build-fail\",\"ImportPath\":\"example.com/proj\"}\n",
        "{\"Action\":\"fail\",\"Package\":\"example.com/proj\",\"FailedBuild\":\"example.com/proj\"}\n",
    );
    let shown = pipe(build);
    assert!(shown.contains("main.go:4: undefined: missing"), "{shown}");
    assert!(shown.contains("[build failed]"), "{shown}");
    assert_ne!(shown, build);
}
