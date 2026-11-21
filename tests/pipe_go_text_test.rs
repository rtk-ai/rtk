//! Auto mode must not discard Go package status as unrelated grep/find output.

use std::io::Write;
use std::process::Stdio;

mod common;

fn pipe(input: &str) -> String {
    let mut child = common::rtk_command()
        .arg("pipe")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk pipe");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write input");
    let output = child.wait_with_output().expect("wait for rtk pipe");
    // A pipe cannot recover the upstream go command's exit status.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 output")
}

fn successful_packages(suffix: &str) -> String {
    (0..25)
        .map(|index| format!("ok  \texample.org/project/pass/pkg{index:02}\t{suffix}\n"))
        .collect()
}

#[test]
fn auto_keeps_plain_go_failures_and_original_line_endings() {
    let report = format!(
        "# example.org/project/broken\nbroken/b.go:3:23: undefined: missing\n{}\
         --- FAIL: TestX (0.00s)\n    z_test.go:5: expected 1, got 2; café 雪\n\
         FAIL\nFAIL\texample.org/project/failing\t0.003s\n\
         FAIL\texample.org/project/broken [build failed]\nFAIL\n",
        successful_packages("0.001s")
    );
    for input in [
        report.clone(),
        report.replace('\n', "\r\n"),
        report.trim_end_matches('\n').to_string(),
    ] {
        assert_eq!(pipe(&input), input);
    }
}

#[test]
fn auto_keeps_plain_go_package_results() {
    for suffix in [
        "0.003s",
        "(cached)",
        "0.003s [no tests to run]",
        "0.003s coverage: 100.0% of statements",
        "0.003s coverage: 0.0% of statements [no tests to run]",
    ] {
        let report = successful_packages(suffix);
        assert_eq!(pipe(&report), report);
    }
    let no_tests: String = (0..25)
        .map(|index| format!("?   \texample.org/project/pkg{index:02}\t[no test files]\n"))
        .collect();
    assert_eq!(pipe(&no_tests), no_tests);
}

#[test]
fn auto_finds_go_status_after_the_initial_detection_window() {
    let diagnostics: String = (0..40)
        .map(|line| format!("broken/b.go:{line}:23: undefined: missing_identifier\n"))
        .collect();
    assert!(diagnostics.len() > 1024);
    // Local modules and command-line-arguments need not contain a slash.
    for status in [
        "FAIL\tcommand-line-arguments [build failed]",
        "FAIL\tlocalmodule [setup failed]",
        "FAIL\tlocalmodule\t0.003s",
    ] {
        let report = format!("{diagnostics}{status}\nFAIL\n");
        assert_eq!(pipe(&report), report);
    }
}

#[test]
fn auto_still_compresses_search_output_with_go_like_content() {
    let grep: String = (1..=50)
        .map(|line| format!("src/report_test.go:{line}:ok  example.org/project/pkg 0.003s\n"))
        .collect();
    assert!(pipe(&grep).starts_with("50 matches in 1F:"));

    let paths: String = ["ok", "FAIL", "PASS"]
        .into_iter()
        .flat_map(|prefix| (0..40).map(move |index| format!("./{prefix}/src/file{index:02}.go\n")))
        .collect();
    assert!(pipe(&paths).starts_with("120 files in 3 dirs:"));
}

#[test]
fn auto_still_compresses_go_test_json() {
    let report = concat!(
        "{\"Action\":\"run\",\"Package\":\"example.org/project\",\"Test\":\"TestX\"}\n",
        "{\"Action\":\"pass\",\"Package\":\"example.org/project\",\"Test\":\"TestX\",\"Elapsed\":0.001}\n",
        "{\"Action\":\"pass\",\"Package\":\"example.org/project\",\"Elapsed\":0.003}\n",
    );
    assert!(pipe(report).starts_with("Go test: 1 passed"));
}
