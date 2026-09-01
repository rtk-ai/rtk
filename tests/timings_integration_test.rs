//! End-to-end contract for the `RTK_TIMINGS=1` stderr breakdown:
//! exactly one line, on stderr only, absent when the variable is unset,
//! with child time attributed on both the central and the passthrough
//! spawn paths.

use std::process::Command;

fn rtk(dir: &std::path::Path, timings: bool, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    cmd.args(args)
        .current_dir(dir)
        // Keep analytics writes out of the developer's real history DB.
        .env("RTK_DB_PATH", dir.join("history.db"))
        .env_remove("RTK_TIMINGS");
    if timings {
        cmd.env("RTK_TIMINGS", "1");
    }
    cmd.output().expect("failed to run rtk binary")
}

fn timings_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter(|l| l.starts_with("rtk timings: "))
        .map(str::to_string)
        .collect()
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("rtk_timings_test_{}_{}", name, std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn timings_line_present_once_on_stderr_and_absent_when_unset() {
    let dir = temp_dir("wc");
    std::fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();

    let on = rtk(&dir, true, &["wc", "-l", "f.txt"]);
    let lines = timings_lines(&on);
    assert_eq!(
        lines.len(),
        1,
        "stderr: {}",
        String::from_utf8_lossy(&on.stderr)
    );
    let line = &lines[0];
    for key in [
        "total=", "startup=", "handler=", "child=", "spawns=", "filter=", "track=",
    ] {
        assert!(line.contains(key), "missing {key} in: {line}");
    }
    assert!(
        !String::from_utf8_lossy(&on.stdout).contains("rtk timings:"),
        "timings line leaked to stdout"
    );

    let off = rtk(&dir, false, &["wc", "-l", "f.txt"]);
    assert!(timings_lines(&off).is_empty());
    assert_eq!(
        on.stdout, off.stdout,
        "stdout must be identical with timings on"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn timings_attributes_child_time_on_passthrough_paths() {
    // `git version` is an unsupported subcommand, so it takes the
    // run_passthrough spawn that bypasses stream.rs.
    let dir = temp_dir("passthrough");
    let out = rtk(&dir, true, &["git", "version"]);
    let lines = timings_lines(&out);
    assert_eq!(
        lines.len(),
        1,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !lines[0].contains("spawns=0"),
        "passthrough child not attributed: {}",
        lines[0]
    );

    std::fs::remove_dir_all(&dir).ok();
}
