//! `pipe` cannot recover an upstream exit status, but it must retain the report.
use std::io::Write;
use std::process::Stdio;

mod common;

fn pipe(input: &str) -> String {
    pipe_mode(input, false)
}

fn pipe_mode(input: &str, auto: bool) -> String {
    let mut child = common::rtk_command()
        .args(if auto {
            vec!["pipe"]
        } else {
            vec!["pipe", "-f", "go-test"]
        })
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
fn mixed_json_and_text_summarizes_events_and_keeps_diagnostics() {
    let events = "{\"Action\":\"pass\",\"Package\":\"example.com/proj\",\"Test\":\"TestOK\"}\n";
    for input in [
        format!("go: downloading example.com/dep v1.0.0\n{events}"),
        format!("{events}setup failed: missing dependency\n"),
        format!("{events}{{\"error\":\"not a Go test event\"}}\n"),
        format!("{events}{{malformed json\n"),
    ] {
        let residual = input.replace(events, "");
        assert_eq!(
            pipe(&input),
            format!("Go test: 1 passed in 1 packages\n{residual}")
        );
    }
}

#[test]
fn valid_json_events_still_use_the_existing_filter() {
    let pass = "{\"Action\":\"run\",\"Package\":\"example.com/proj\",\"Test\":\"TestOK\"}\n{\"Action\":\"pass\",\"Package\":\"example.com/proj\",\"Test\":\"TestOK\"}\n{\"Action\":\"pass\",\"Package\":\"example.com/proj\"}\n";
    assert_eq!(pipe(pass), "Go test: 1 passed in 1 packages");
    let fail = "{\"Action\":\"output\",\"Package\":\"example.com/proj\",\"Test\":\"TestX\",\"Output\":\"    x_test.go:12: expected 1, got 2\\n\"}\n{\"Action\":\"fail\",\"Package\":\"example.com/proj\",\"Test\":\"TestX\"}\n{\"Action\":\"fail\",\"Package\":\"example.com/proj\"}\n";
    let shown = pipe(fail);
    assert_ne!(shown, fail);
    assert_eq!(
        shown.lines().next(),
        Some("Go test: 0 passed, 1 failed in 1 packages")
    );
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

const CORE: &str = include_str!("fixtures/go_test_pipe/core.jsonl");
const CORE_SUMMARY: &str = "Go test: 3 passed in 2 packages";

#[test]
fn captured_core_events_are_consumed_not_returned_raw() {
    for action in ["start", "output", "skip", "pause", "cont"] {
        assert!(CORE.contains(&format!(r#""Action":"{action}""#)));
    }
    assert!(CORE.contains("[no test files]"));
    assert!(CORE.contains("captured benchmark log"));
    for auto in [false, true] {
        assert_eq!(pipe_mode(CORE, auto), CORE_SUMMARY);
    }
}

#[test]
fn captured_auxiliary_actions_do_not_disable_summaries() {
    let input = include_str!("fixtures/go_test_pipe/auxiliary.jsonl");
    assert!(input.contains(r#""Action":"attr""#));
    assert!(input.contains(r#""Action":"artifacts""#));
    // Auxiliary records are retained, not guessed away or used to veto the stream.
    let auxiliary: String = input
        .split_inclusive('\n')
        .filter(|line| {
            line.contains(r#""Action":"attr""#) || line.contains(r#""Action":"artifacts""#)
        })
        .collect();
    for auto in [false, true] {
        assert_eq!(
            pipe_mode(input, auto),
            format!("Go test: 2 passed in 2 packages\n{auxiliary}")
        );
    }
}

#[test]
fn captured_package_less_test2json_with_and_without_timestamps_is_summarized() {
    for input in [
        include_str!("fixtures/go_test_pipe/package_less.jsonl"),
        include_str!("fixtures/go_test_pipe/package_less_no_time.jsonl"),
    ] {
        assert!(!input.contains(r#""Package""#));
        for auto in [false, true] {
            assert_eq!(
                pipe_mode(input, auto),
                concat!(
                    "Go test: 3 passed, 1 failed, 1 skipped in 1 packages\n",
                    "\nunknown (3 passed, 1 failed)\n",
                    "  [FAIL] TestFail\n",
                    "     capture_test.go:26: expected 1, got 2",
                )
            );
        }
    }
}

#[test]
fn captured_failure_is_reduced_and_keeps_the_exact_diagnostic() {
    let input = include_str!("fixtures/go_test_pipe/failure.jsonl");
    let shown = pipe(input);
    assert_ne!(shown, input);
    assert_eq!(
        shown.lines().next(),
        Some("Go test: 1 passed, 1 failed in 1 packages")
    );
    assert!(
        shown.contains("capture_test.go:26: expected 1, got 2"),
        "{shown}"
    );
}

#[test]
fn leading_bom_does_not_disable_a_captured_stream() {
    for auto in [false, true] {
        assert_eq!(pipe_mode(&format!("\u{feff}{CORE}"), auto), CORE_SUMMARY);
    }
    assert_eq!(
        pipe("\u{feff}not a Go event\r\n"),
        "\u{feff}not a Go event\r\n"
    );
}

#[test]
fn captured_cold_cache_stderr_survives_next_to_the_summary() {
    let input = include_str!("fixtures/go_test_pipe/mixed_downloads.jsonl");
    let downloads: String = input
        .split_inclusive('\n')
        .filter(|line| line.starts_with("go: downloading "))
        .collect();
    assert_eq!(downloads.lines().count(), 2);
    assert!(downloads.contains("go: downloading golang.org/x/text v0.21.0\n"));
    assert!(downloads.contains("go: downloading golang.org/x/sync v0.10.0\n"));
    for auto in [false, true] {
        assert_eq!(
            pipe_mode(input, auto),
            format!("{CORE_SUMMARY}\n{downloads}")
        );
    }
}

#[test]
fn captured_stream_cut_mid_object_keeps_the_exact_fragment() {
    let complete = CORE.trim_end_matches('\n');
    let last_start = complete.rfind('\n').unwrap() + 1;
    let cut = complete.floor_char_boundary(last_start + (complete.len() - last_start) / 2);
    let input = &complete[..cut];
    let fragment = &input[last_start..];
    // Pin the existing reducer's counts, not new incomplete-run semantics (#4693).
    for auto in [false, true] {
        assert_eq!(
            pipe_mode(input, auto),
            format!("{CORE_SUMMARY}\n{fragment}")
        );
    }
}

#[test]
fn captured_build_only_and_unmatched_build_errors_are_not_lost() {
    let build = include_str!("fixtures/go_test_pipe/build_only.jsonl");
    assert!(build.contains("broken/broken.go:3:9: undefined: missing"));
    for auto in [false, true] {
        assert_eq!(pipe_mode(build, auto), build);
        assert_eq!(
            pipe_mode(&format!("{CORE}{build}"), auto),
            format!("{CORE_SUMMARY}\n{build}")
        );
    }
}

#[test]
fn unknown_actions_and_non_event_bytes_survive_without_disabling_events() {
    let future = "{\"Action\":\"future-action\",\"Package\":\"example.com/rtk-pipe-fixture/cases\",\"Detail\":\"keep me\"}\r\n";
    let before = "  setup diagnostic: 日本語  \r\n";
    let tail = "  {\"truncated\": ";
    let input = format!("{before}{CORE}{future}{tail}");
    // Explicit filtering can reduce events in arbitrary mixed input. Auto-detect
    // cannot distinguish such a preamble from prose quoting a test stream.
    assert_eq!(
        pipe(&input),
        format!("{CORE_SUMMARY}\n{before}{future}{tail}")
    );
    assert_eq!(pipe_mode(&input, true), input);
    // Once a stream is identified, unknown records still survive in both modes.
    for auto in [false, true] {
        assert_eq!(
            pipe_mode(&format!("{CORE}{future}{tail}"), auto),
            format!("{CORE_SUMMARY}\n{future}{tail}")
        );
    }
}

#[test]
fn captured_legacy_benchmark_pins_all_six_auxiliary_lifecycle_actions() {
    let core = include_str!("fixtures/go_test_pipe/legacy_core.jsonl");
    let bench = include_str!("fixtures/go_test_pipe/legacy_bench.jsonl");
    for action in ["start", "output", "skip", "pause", "cont", "bench"] {
        assert!(
            core.contains(&format!(r#""Action":"{action}""#))
                || bench.contains(&format!(r#""Action":"{action}""#)),
            "missing {action}"
        );
    }
    assert!(core.contains("[no test files]"));
    assert!(bench.contains("captured benchmark log"));
    for auto in [false, true] {
        assert_eq!(pipe_mode(core, auto), CORE_SUMMARY);
        // Benchmark counts are unchanged: the existing reducer counts tests only.
        assert_eq!(pipe_mode(bench, auto), "Go test: No tests found");
    }
}

#[test]
fn malformed_output_records_remain_raw_inside_a_test_stream() {
    for record in [
        r#"{"Action":"output","Package":"example.com/proj","message":"setup failed"}"#,
        r#"{"Action":"output","Package":"example.com/proj","Output":42}"#,
        r#"{"Action":"build-output","ImportPath":"example.com/proj","message":"compile failed"}"#,
    ] {
        assert_eq!(
            pipe(&format!("{CORE}{record}")),
            format!("{CORE_SUMMARY}\n{record}")
        );
    }
}

#[test]
fn build_records_require_a_later_matching_package_failure_to_be_consumed() {
    let build = include_str!("fixtures/go_test_pipe/build_only.jsonl");
    for before in [
        r#"{"Action":"fail","Package":"example.com/rtk-pipe-fixture/broken","FailedBuild":"example.com/rtk-pipe-fixture/broken"}"#,
        r#"{"Action":"fail","Package":"example.com/rtk-pipe-fixture/broken","Test":"TestX","FailedBuild":"example.com/rtk-pipe-fixture/broken"}"#,
    ] {
        let input = format!("{CORE}{before}\n{build}");
        let shown = pipe(&input);
        assert_ne!(shown, input);
        assert!(
            shown.starts_with("Go test: 3 passed, 1 failed, 1 skipped in 3 packages"),
            "{shown}"
        );
        assert!(shown.ends_with(build), "{shown}");
    }
}

#[test]
fn repeated_build_failures_keep_both_diagnostic_batches() {
    let build = include_str!("fixtures/go_test_pipe/build_only.jsonl");
    let second = build.replace("undefined: missing", "undefined: second_missing");
    let fail = "{\"Action\":\"fail\",\"Package\":\"example.com/rtk-pipe-fixture/broken\",\"FailedBuild\":\"example.com/rtk-pipe-fixture/broken\"}\n";
    let input = format!("{CORE}{build}{fail}{second}{fail}");
    let shown = pipe(&input);
    assert_ne!(shown, input);
    assert_eq!(
        shown.lines().next(),
        Some("Go test: 3 passed, 1 failed, 1 skipped in 3 packages")
    );
    assert!(shown.ends_with(&format!("{build}{second}")), "{shown}");
}

#[test]
fn a_test_level_failure_does_not_claim_build_diagnostics() {
    let build = include_str!("fixtures/go_test_pipe/build_only.jsonl");
    let fail = "{\"Action\":\"fail\",\"Test\":\"TestX\",\"Package\":\"example.com/rtk-pipe-fixture/broken\",\"FailedBuild\":\"example.com/rtk-pipe-fixture/broken\"}\n";
    let input = format!("{CORE}{build}{fail}");
    let shown = pipe(&input);
    assert_ne!(shown, input);
    assert!(shown.ends_with(build), "{shown}");
}

#[test]
fn metadata_only_logs_with_unrelated_fields_stay_verbatim() {
    for record in [
        r#"{"Action":"fail","message":"important setup diagnostic"}"#,
        r#"{"Action":"pass","message":"keep me"}"#,
        r#"{"Action":"fail","Time":"2026-10-21T00:00:00Z","message":"important setup diagnostic"}"#,
        r#"{"Action":"pass","Elapsed":0,"message":"keep elapsed-shaped log"}"#,
        r#"{"Action":"start","Time":"2026-10-21T00:00:00Z","Elapsed":0,"diagnostic":"keep metadata-shaped log"}"#,
        r#"{"Action":"start","Time":null,"diagnostic":"keep null-shaped log"}"#,
    ] {
        for auto in [false, true] {
            assert_eq!(
                pipe_mode(&format!("{CORE}{record}"), auto),
                format!("{CORE_SUMMARY}\n{record}")
            );
        }
    }
}

#[test]
fn auto_detection_preserves_a_diff_quoting_go_events() {
    let input = include_str!("fixtures/go_test_pipe/quoted.diff");
    assert_eq!(pipe_mode(input, true), input);
}

#[test]
fn auto_detection_preserves_markdown_quoting_go_events() {
    let input = concat!(
        "# Investigating a flaky test\n\n",
        "Keep this example and its diagnostic intact:\n\n```json\n",
        "{\"Action\":\"run\",\"Package\":\"example.com/proj\",\"Test\":\"TestFlaky\"}\n",
        "{\"Action\":\"output\",\"Package\":\"example.com/proj\",\"Test\":\"TestFlaky\",\"Output\":\"    flaky_test.go:12: got 3, want 4\\n\"}\n",
        "{\"Action\":\"pass\",\"Package\":\"example.com/proj\",\"Test\":\"TestFlaky\"}\n",
        "```\n\nThis is documentation, not a test run.\n",
    );
    assert_eq!(pipe_mode(input, true), input);
}

#[test]
fn captured_build_failure_has_an_exact_summary_without_raw_json() {
    let input = include_str!("fixtures/go_test_pipe/build_failure.jsonl");
    for auto in [false, true] {
        assert_eq!(
            pipe_mode(input, auto),
            concat!(
                "Go test: 0 passed, 1 failed in 1 packages\n",
                "\nbroken [build failed]\n",
                "  broken/b.go:3:23: undefined: missing",
            )
        );
    }
}
