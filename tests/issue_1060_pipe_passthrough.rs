//! Integration test for issue #1060: `rtk <cmd> | <consumer>` must preserve
//! the raw command output. The TOML filter on `ps` (and similar commands)
//! truncates to 30 lines, which silently breaks pipelines such as
//! `rtk ps aux | grep opencode` or `rtk ps aux | wc -l`.
//!
//! This test spawns the rtk binary with stdout captured (i.e. piped, not a
//! terminal) so it reproduces the user-visible failure mode end-to-end. It
//! gracefully skips on hosts that either lack `ps aux` or run too few
//! processes to make the truncation observable (small Docker images, etc.).

use std::process::{Command, Stdio};

const SKIP_THRESHOLD: usize = 35;
const TOLERANCE_LINES: usize = 5;

fn line_count(buf: &[u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    let mut n = buf.iter().filter(|&&b| b == b'\n').count();
    if *buf.last().unwrap() != b'\n' {
        n += 1;
    }
    n
}

fn run_piped(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(output.stdout)
}

/// Spawns `rtk ps aux` with a piped stdout (the exact shape of
/// `rtk ps aux | grep ...`) and compares its line count to the raw
/// `ps aux` output. With issue #1060 in place the TOML filter caps
/// `rtk ps aux` at ~30 lines regardless of how many processes the host
/// actually runs, breaking downstream consumers.
#[test]
fn rtk_ps_aux_piped_preserves_raw_line_count() {
    let raw = match run_piped("ps", &["aux"]) {
        Some(r) => r,
        None => {
            eprintln!("Skipping: `ps aux` is unavailable on this host");
            return;
        }
    };
    let raw_lines = line_count(&raw);
    if raw_lines < SKIP_THRESHOLD {
        eprintln!(
            "Skipping: host runs only {raw_lines} processes — too few to \
             observe `max_lines=30` truncation"
        );
        return;
    }

    let rtk_bin = env!("CARGO_BIN_EXE_rtk");
    let rtk_out = run_piped(rtk_bin, &["ps", "aux"]).expect("`rtk ps aux` failed");
    let rtk_lines = line_count(&rtk_out);

    assert!(
        rtk_lines + TOLERANCE_LINES >= raw_lines,
        "issue #1060: `rtk ps aux` piped should preserve raw output \
         (got {rtk_lines} lines, expected close to {raw_lines})"
    );
}
