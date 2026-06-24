use std::process::{Command, Output};

fn run_rtk(args: &[&str], cwd: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .current_dir(cwd)
        .env("RTK_TELEMETRY_DISABLED", "1")
        .output()
        .expect("run rtk")
}

fn git(args: &[&str], cwd: &std::path::Path) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repo_with_file() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("file.txt"), "alpha one\nbeta\nalpha two\n").unwrap();
    git(&["init"], dir.path());
    git(&["add", "file.txt"], dir.path());
    dir
}

#[test]
fn git_grep_is_compacted_with_line_numbers() {
    let dir = repo_with_file();
    let output = run_rtk(&["git", "grep", "alpha"], dir.path());
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("2 matches in 1 files:"), "{stdout}");
    assert!(stdout.contains("file.txt:1:alpha one"), "{stdout}");
    assert!(stdout.contains("file.txt:3:alpha two"), "{stdout}");
}

#[test]
fn git_grep_machine_output_flags_stay_raw() {
    let dir = repo_with_file();
    let output = run_rtk(&["git", "grep", "-l", "alpha"], dir.path());
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "file.txt\n");
    assert!(!stdout.contains("matches in"));
}
