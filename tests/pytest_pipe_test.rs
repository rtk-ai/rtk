use std::io::Write;
use std::process::Stdio;

mod common;

fn pipe(input: &str, explicit: bool) -> String {
    let mut cmd = common::rtk_command();
    cmd.arg("pipe");
    if explicit {
        cmd.args(["--filter", "pytest"]);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start rtk pipe");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn summaryless_quiet_progress_is_verbatim() {
    // A sufficiently long progress line exposes the bug even through never_worse.
    for input in [
        format!("{} [100%]\n", ".".repeat(120)),
        format!("\x1b[32m{} [100%]\x1b[0m\n", ".".repeat(120)),
    ] {
        assert_eq!(pipe(&input, true), input);
    }
}

#[test]
fn summaryless_failure_is_verbatim_in_explicit_and_auto_modes() {
    let input = "================ test session starts ================\n\
                 collected 1 item\n\
                 tests/test_sample.py F [100%]\n\
                 ==================== FAILURES ====================\n\
                 ____________________ test_fail ____________________\n\
                 E   AssertionError: PIPE_FAILURE_MUST_SURVIVE\n";
    for explicit in [true, false] {
        assert_eq!(pipe(input, explicit), input);
    }
}

#[test]
fn captured_summary_before_failure_tail_is_not_a_result() {
    let input = "================ test session starts ================\n\
                 collected 1 item\n\
                 ==================== FAILURES ====================\n\
                 ____________________ test_fail ____________________\n\
                 ---------------- Captured stdout call ----------------\n\
                 99 passed in 0.01s\n\
                 ================ short test summary info ================\n\
                 FAILED tests/test_sample.py::test_fail - AssertionError\n";
    for explicit in [true, false] {
        assert_eq!(pipe(input, explicit), input);
    }
}

#[test]
fn empty_collection_and_unrecognized_text_are_verbatim() {
    for input in [
        "",
        "   \n\n",
        "================ test session starts ================\nno tests ran in 0.01s\n",
        "pytest: error: unrecognized arguments: --unknown-option-with-details\n",
        "================ test session starts ================\n1 warning in 0.01s\n",
    ] {
        assert_eq!(pipe(input, true), input);
    }
}

#[test]
fn terminal_summary_still_compacts_in_explicit_and_auto_modes() {
    let input = "================ test session starts ================\n\
                 platform linux -- Python 3.13, pytest-9.1.1\n\
                 collected 120 items\n\
                 tests/test_sample.py .................................. [100%]\n\
                 \x1b[32m================ 120 passed in 0.12s ================\x1b[0m\n\n";
    for explicit in [true, false] {
        assert_eq!(pipe(input, explicit), "Pytest: 120 passed in 0.12s");
    }
}
