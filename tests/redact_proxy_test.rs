//! End-to-end proof of PII redaction (src/core/redact.rs) through the real
//! binary: proxy passthrough, pipe mode, and the never-worse interaction.
//!
//! These tests assume the default config (`[redaction]` absent or enabled) —
//! the same guarantee older configs get after upgrade.

use std::io::Write;
use std::process::{Command, Stdio};

fn rtk(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run rtk");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn rtk_stdin(args: &[&str], input: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rtk");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait rtk");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn proxy_redacts_pii_in_child_stdout() {
    let out = rtk(&[
        "proxy",
        "sh",
        "-c",
        "echo contact ravi.chopra@slicebank.com",
    ]);
    assert!(out.contains("[REDACTED:email]"), "out={}", out);
    assert!(!out.contains("ravi.chopra@slicebank.com"), "out={}", out);
}

#[test]
fn proxy_redacts_pii_in_child_stderr() {
    let out = rtk(&["proxy", "sh", "-c", "echo card 4111111111111111 >&2"]);
    assert!(out.contains("[REDACTED:card]"), "out={}", out);
    assert!(!out.contains("4111111111111111"), "out={}", out);
}

#[test]
fn proxy_no_redact_flag_disables_redaction() {
    let out = rtk(&[
        "--no-redact",
        "proxy",
        "sh",
        "-c",
        "echo contact ravi.chopra@slicebank.com",
    ]);
    assert!(out.contains("ravi.chopra@slicebank.com"), "out={}", out);
    assert!(!out.contains("[REDACTED:email]"), "out={}", out);
}

#[test]
fn proxy_redacts_pii_split_across_chunk_boundary() {
    // A >8 KiB line forces the email across multiple 8 KiB pipe reads; the
    // line-buffered redactor must still catch it.
    let out = rtk(&[
        "proxy",
        "sh",
        "-c",
        r#"awk 'BEGIN{for(i=0;i<9000;i++)printf "a"; print " ravi.chopra@slicebank.com"}'"#,
    ]);
    assert!(
        out.contains("[REDACTED:email]"),
        "boundary-split email not redacted"
    );
    assert!(!out.contains("ravi.chopra@slicebank.com"));
}

#[test]
fn proxy_redacts_partial_final_line_no_trailing_newline() {
    let out = rtk(&[
        "proxy",
        "sh",
        "-c",
        "printf 'email ravi.chopra@slicebank.com'",
    ]);
    assert!(
        out.contains("[REDACTED:email]"),
        "EOF partial line not redacted"
    );
    assert!(!out.contains("ravi.chopra@slicebank.com"));
}

#[test]
fn pipe_mode_redacts_stdin() {
    let out = rtk_stdin(&["pipe"], "user ravi.chopra@slicebank.com logged in\n");
    assert!(out.contains("[REDACTED:email]"), "out={}", out);
    assert!(!out.contains("ravi.chopra@slicebank.com"), "out={}", out);
}

#[test]
fn redaction_survives_never_worse_guard() {
    // "a@b.co" (6 chars) redacts to "[REDACTED:email]" (16 chars) — the
    // redacted form is LARGER than raw, so if redaction ran after the
    // never-worse token guard, the guard would pick the raw side and leak.
    let out = rtk_stdin(&["pipe"], "a@b.co\n");
    assert!(
        out.contains("[REDACTED:email]"),
        "never_worse resurrected raw PII: out={}",
        out
    );
    assert!(!out.contains("a@b.co"), "out={}", out);
}
