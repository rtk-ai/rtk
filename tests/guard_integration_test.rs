//! End-to-end proof of the never-worse guard (src/core/guard.rs).

use std::io::Write;
use std::process::{Command, Stdio};

fn rtk_stdin(args: &[&str], input: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
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
fn guard_shows_raw_when_filter_would_bloat_tiny_input() {
    let input = "{\"a\":1,\"b\":2,\"c\":3,\"d\":4}";
    let out = rtk_stdin(&["json", "-"], input);

    assert_eq!(
        out.trim(),
        input,
        "guard should emit the raw minified JSON, not a larger pretty-printed form"
    );
    assert!(
        out.trim().len() <= input.len(),
        "never-worse violated: {} chars emitted for a {}-char raw input",
        out.trim().len(),
        input.len()
    );
}

#[test]
fn guard_does_not_block_real_compression() {
    let mut input = String::from("{");
    for i in 0..60 {
        input.push_str(&format!("\"key_{i}\":\"value_{i}\","));
    }
    input.push_str("\"last\":1}");

    let out = rtk_stdin(&["json", "-"], &input);
    assert!(
        out.len() < input.len(),
        "filter should compress large input (guard must not over-trigger): {} vs {}",
        out.len(),
        input.len()
    );
}

fn rtk_output_in_dir(dir: &std::path::Path, args: &[&str]) -> (String, String, Option<i32>) {
    let tracking_dir = if dir.join(".git").is_dir() {
        dir.join(".git")
    } else {
        dir.to_path_buf()
    };
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .args(args)
        .current_dir(dir)
        .env("CLAUDE_CONFIG_DIR", dir.join(".rtk-test-no-claude"))
        .env("RTK_DB_PATH", tracking_dir.join("rtk-test-history.db"))
        .output()
        .expect("spawn rtk");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

fn rtk_in_dir(dir: &std::path::Path, args: &[&str]) -> (String, Option<i32>) {
    let (stdout, _, code) = rtk_output_in_dir(dir, args);
    (stdout, code)
}

fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn init_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@t.t"][..],
        &["config", "user.name", "t"][..],
        &["commit", "-q", "--allow-empty", "-m", "init"][..],
    ] {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git setup failed: {args:?}");
    }
    dir
}

fn init_dirty_git_repo() -> tempfile::TempDir {
    let dir = init_git_repo();
    std::fs::write(dir.path().join("tracked.txt"), "before\n").expect("write tracked");
    git_in_dir(dir.path(), &["add", "tracked.txt"]);
    git_in_dir(dir.path(), &["commit", "-q", "-m", "add tracked"]);
    std::fs::write(dir.path().join("tracked.txt"), "after\n").expect("modify tracked");
    dir
}

fn git_in_dir(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git command failed: {args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn assert_rtk_git_matches_native(
    dir: &std::path::Path,
    git_args: &[&str],
) -> (String, String, Option<i32>) {
    let raw = Command::new("git")
        .args(git_args)
        .current_dir(dir)
        .output()
        .expect("spawn native git");
    let mut rtk_args = vec!["git"];
    rtk_args.extend_from_slice(git_args);
    let actual = rtk_output_in_dir(dir, &rtk_args);

    assert_eq!(actual.2, raw.status.code(), "exit mismatch: {git_args:?}");
    assert_eq!(
        actual.0.as_bytes(),
        raw.stdout.as_slice(),
        "stdout mismatch: {git_args:?}"
    );
    assert_eq!(
        actual.1.as_bytes(),
        raw.stderr.as_slice(),
        "stderr mismatch: {git_args:?}"
    );

    actual
}

fn read_text_normalized(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
}

#[test]
fn grep_no_match_emits_empty_not_a_message() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "hello world\n").expect("write");
    // Faithful grep needs -r to descend a directory; we no longer force recursion
    // by routing through rg (the engine-faithful contract).
    let (out, code) = rtk_in_dir(dir.path(), &["grep", "-r", "zzz_no_match_xyz", "."]);
    assert!(
        out.trim().is_empty(),
        "no-match grep must emit empty, not a '0 matches' line: {out:?}"
    );
    assert_eq!(code, Some(1), "grep no-match must preserve exit 1");
}

#[test]
fn find_no_results_emits_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "x").expect("write");
    let (out, _) = rtk_in_dir(dir.path(), &["find", ".", "-name", "zzz_no_match_xyz"]);
    assert!(
        out.trim().is_empty(),
        "no-result find must emit empty, not a '0 for' line: {out:?}"
    );
}

#[test]
fn git_stash_list_no_stashes_emits_empty() {
    let dir = init_git_repo();
    let (out, code) = rtk_in_dir(dir.path(), &["git", "stash", "list"]);
    assert!(
        out.trim().is_empty(),
        "no-stashes must emit empty, not 'No stashes': {out:?}"
    );
    assert_eq!(code, Some(0));
}

#[test]
fn git_log_patch_output_matches_raw_git() {
    let dir = init_git_repo();
    std::fs::write(
        dir.path().join("history.txt"),
        "STRIPE_KEY=sk_live_FAKE1234567890\n",
    )
    .expect("write history fixture");
    git_in_dir(dir.path(), &["add", "history.txt"]);
    git_in_dir(dir.path(), &["commit", "-q", "-m", "add history fixture"]);

    let raw = Command::new("git")
        .args(["log", "-p", "--all"])
        .current_dir(dir.path())
        .output()
        .expect("spawn raw git log");
    assert!(raw.status.success());

    let (rtk_stdout, rtk_stderr, rtk_code) =
        rtk_output_in_dir(dir.path(), &["git", "log", "-p", "--all"]);

    assert_eq!(rtk_code, Some(0), "rtk stderr: {rtk_stderr}");
    assert_eq!(rtk_stdout.as_bytes(), raw.stdout.as_slice());
    assert!(rtk_stdout.contains("STRIPE_KEY=sk_live_FAKE1234567890"));
}

#[test]
fn git_log_dash_p_pathspec_after_double_dash_is_not_patch_flag() {
    // Regression: `rtk git log -- -p` must not be misread as the real `-p`
    // patch flag. Clap's `trailing_var_arg` strips the literal "--" before
    // `run_log` sees `args`, so the pathspec-separator check must restore it
    // (via restore_double_dash) before deciding whether to pass through raw
    // patch output; otherwise a file literally named "-p" after "--" is
    // wrongly treated as a request for `git log -p`.
    let dir = init_git_repo();
    std::fs::write(dir.path().join("-p"), "not a diff flag\n").expect("write -p file");
    git_in_dir(dir.path(), &["add", "--", "-p"]);
    git_in_dir(dir.path(), &["commit", "-q", "-m", "add dash-p file"]);

    let (stdout, stderr, code) = rtk_output_in_dir(dir.path(), &["git", "log", "--", "-p"]);

    assert_eq!(code, Some(0), "rtk stderr: {stderr}");
    assert!(
        !stdout.contains("diff --git") && !stdout.contains("@@"),
        "-- -p should stay on RTK's filtered path, not raw patch output: {stdout:?}"
    );
    assert!(
        stdout.contains("add dash-p file"),
        "expected the commit touching the -p pathspec: {stdout:?}"
    );
}

#[test]
fn git_stash_show_no_stash_emits_empty_and_propagates_failure() {
    // Regression: previously printed "Empty stash" and returned Ok(0), masking
    // the underlying `git stash show` failure.
    let dir = init_git_repo();
    let (out, code) = rtk_in_dir(dir.path(), &["git", "stash", "show"]);
    assert!(
        out.trim().is_empty(),
        "must emit empty, not 'Empty stash': {out:?}"
    );
    assert_ne!(
        code,
        Some(0),
        "a real git stash show failure must not be masked as exit 0"
    );
}

#[test]
fn git_status_short_clean_emits_explicit_state() {
    let dir = init_git_repo();

    for args in [
        &["git", "status", "--short"][..],
        &["git", "status", "-s"][..],
    ] {
        let (out, code) = rtk_in_dir(dir.path(), args);

        assert_eq!(code, Some(0));
        assert_eq!(
            out.trim(),
            "* main\nclean — nothing to commit",
            "a clean human-readable status must not look like missing output: {args:?}"
        );
    }
}

#[test]
fn git_status_short_preserves_additional_filter_flags() {
    let dir = init_git_repo();
    std::fs::write(dir.path().join("only-untracked.txt"), "untracked\n").expect("write untracked");

    assert_rtk_git_matches_native(dir.path(), &["status", "--short", "--untracked-files=no"]);
}

#[test]
fn git_status_short_dirty_preserves_native_output() {
    let dir = init_git_repo();
    std::fs::write(dir.path().join("untracked.txt"), "untracked\n").expect("write untracked");

    assert_rtk_git_matches_native(dir.path(), &["status", "--short"]);
}

#[test]
fn git_status_short_preserves_pathspec() {
    let dir = init_git_repo();
    std::fs::write(dir.path().join("a.txt"), "a\n").expect("write a");
    std::fs::write(dir.path().join("b.txt"), "b\n").expect("write b");

    assert_rtk_git_matches_native(dir.path(), &["status", "--short", "--", "a.txt"]);
}

#[test]
fn git_status_short_invalid_flag_preserves_failure() {
    let dir = init_git_repo();

    let (_, stderr, code) = assert_rtk_git_matches_native(
        dir.path(),
        &["status", "--short", "--definitely-not-a-status-option"],
    );

    assert_ne!(code, Some(0));
    assert!(stderr.contains("definitely-not-a-status-option"));
}

#[test]
fn git_diff_clean_emits_explicit_state() {
    let dir = init_git_repo();

    let (out, code) = rtk_in_dir(dir.path(), &["git", "diff"]);

    assert_eq!(code, Some(0));
    assert_eq!(
        out.trim(),
        "No changes",
        "a clean human-readable diff must not look like missing output"
    );
}

#[test]
fn git_diff_summaries_clean_emit_explicit_state() {
    let dir = init_git_repo();

    for flag in ["--stat", "--stat=80", "--shortstat"] {
        let (out, code) = rtk_in_dir(dir.path(), &["git", "diff", flag, "HEAD"]);

        assert_eq!(code, Some(0));
        assert_eq!(
            out.trim(),
            "No changes",
            "a clean diff summary must not look like missing output: {flag}"
        );
    }
}

#[test]
fn git_diff_check_clean_stays_silent() {
    let dir = init_git_repo();

    let (out, code) = rtk_in_dir(dir.path(), &["git", "diff", "--check"]);

    assert_eq!(code, Some(0));
    assert!(
        out.trim().is_empty(),
        "git diff --check is a validation command and must preserve native silence: {out:?}"
    );
}

#[test]
fn git_diff_machine_and_listing_modes_clean_stay_silent() {
    let dir = init_git_repo();

    for args in [
        &["git", "diff", "--quiet", "HEAD"][..],
        &["git", "diff", "--exit-code", "HEAD"][..],
        &["git", "diff", "--no-patch", "HEAD"][..],
        &["git", "diff", "--name-only", "HEAD"][..],
        &["git", "diff", "--name-status", "HEAD"][..],
        &["git", "diff", "--raw", "HEAD"][..],
        &["git", "diff", "--numstat", "HEAD"][..],
        &["git", "diff", "--numstat", "-z", "HEAD"][..],
        &["git", "diff", "--no-compact", "HEAD"][..],
        &["git", "diff", "--stat", "--quiet", "HEAD"][..],
        &["git", "diff", "--stat", "--exit-code", "HEAD"][..],
        &["git", "diff", "--stat", "--check", "HEAD"][..],
        &["git", "diff", "--stat", "--output=stat.txt", "HEAD"][..],
    ] {
        let (out, code) = rtk_in_dir(dir.path(), args);

        assert_eq!(code, Some(0), "unexpected exit for {args:?}");
        assert!(
            out.is_empty(),
            "machine/listing diff mode must preserve native silence: {args:?} => {out:?}"
        );
    }
}

#[test]
fn git_diff_stat_dirty_preserves_native_summary() {
    let dir = init_dirty_git_repo();

    let (out, _, _) = assert_rtk_git_matches_native(dir.path(), &["diff", "--stat", "HEAD"]);

    assert!(
        out.contains("tracked.txt"),
        "dirty stat lost its path: {out:?}"
    );
}

#[test]
fn git_diff_exit_code_dirty_preserves_native_output() {
    let dir = init_dirty_git_repo();

    let (stdout, _, code) =
        assert_rtk_git_matches_native(dir.path(), &["diff", "--exit-code", "HEAD"]);

    assert_eq!(code, Some(1));
    assert!(stdout.contains("tracked.txt"));
    assert!(stdout.contains("-before"));
    assert!(stdout.contains("+after"));
}

#[test]
fn git_diff_no_index_different_files_preserves_native_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("before.txt"), "before\n").expect("write before");
    std::fs::write(dir.path().join("after.txt"), "after\n").expect("write after");

    let (stdout, _, code) = assert_rtk_git_matches_native(
        dir.path(),
        &["diff", "--no-index", "before.txt", "after.txt"],
    );

    assert_eq!(code, Some(1));
    assert!(stdout.contains("before.txt"));
    assert!(stdout.contains("after.txt"));
}

#[test]
fn git_diff_output_modes_dirty_match_native() {
    let dir = init_dirty_git_repo();

    for args in [
        &["diff", "--quiet", "HEAD"][..],
        &["diff", "--no-patch", "HEAD"][..],
        &["diff", "--name-only", "HEAD"][..],
        &["diff", "--name-status", "HEAD"][..],
        &["diff", "--raw", "HEAD"][..],
        &["diff", "--numstat", "-z", "HEAD"][..],
    ] {
        assert_rtk_git_matches_native(dir.path(), args);
    }
}

#[test]
fn git_diff_no_compact_dirty_matches_native_diff() {
    let dir = init_dirty_git_repo();
    let raw = Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(dir.path())
        .output()
        .expect("spawn native git");
    let actual = rtk_output_in_dir(dir.path(), &["git", "diff", "--no-compact", "HEAD"]);

    assert_eq!(actual.2, raw.status.code());
    assert_eq!(actual.0.as_bytes(), raw.stdout.as_slice());
    assert_eq!(actual.1.as_bytes(), raw.stderr.as_slice());
}

#[test]
fn git_diff_stat_invalid_revision_preserves_failure() {
    let dir = init_git_repo();

    let (stdout, stderr, code) = rtk_output_in_dir(
        dir.path(),
        &["git", "diff", "--stat", "definitely-not-a-revision"],
    );
    let combined = format!("{stdout}{stderr}");

    assert_ne!(code, Some(0));
    assert!(!stdout.contains("No changes"));
    assert!(
        combined.contains("definitely-not-a-revision"),
        "git failure detail was lost: {combined:?}"
    );
}

#[test]
fn git_checkout_branch_switch_emits_compact_ok() {
    let dir = init_git_repo();
    git_in_dir(dir.path(), &["checkout", "-q", "-b", "feature/test"]);

    let (out, code) = rtk_in_dir(dir.path(), &["git", "checkout", "main"]);

    assert_eq!(code, Some(0));
    assert_eq!(out.trim(), "ok main");
}

#[test]
fn git_checkout_new_branch_emits_compact_ok() {
    let dir = init_git_repo();

    let (out, code) = rtk_in_dir(dir.path(), &["git", "checkout", "-b", "feature/test"]);

    assert_eq!(code, Some(0));
    assert_eq!(out.trim(), "ok feature/test (new)");
}

#[test]
fn git_checkout_reset_branch_does_not_claim_new_branch() {
    let dir = init_git_repo();

    let (out, code) = rtk_in_dir(dir.path(), &["git", "checkout", "-B", "feature/test"]);

    assert_eq!(code, Some(0));
    assert_eq!(out.trim(), "ok feature/test");
}

#[test]
fn git_checkout_file_restore_emits_restored_count() {
    let dir = init_git_repo();
    std::fs::write(dir.path().join("a.txt"), "original\n").expect("write a");
    std::fs::write(dir.path().join("b.txt"), "original\n").expect("write b");
    git_in_dir(dir.path(), &["add", "a.txt", "b.txt"]);
    git_in_dir(dir.path(), &["commit", "-q", "-m", "add files"]);

    std::fs::write(dir.path().join("a.txt"), "changed\n").expect("write a");
    std::fs::write(dir.path().join("b.txt"), "changed\n").expect("write b");

    let (out, code) = rtk_in_dir(
        dir.path(),
        &["git", "checkout", "HEAD", "--", "a.txt", "b.txt"],
    );

    assert_eq!(code, Some(0));
    assert!(
        out.trim().is_empty() || out.trim() == "ok 2 files restored",
        "guarded output may stay empty when native git emits no success text: {out:?}"
    );
    assert_eq!(
        read_text_normalized(&dir.path().join("a.txt")),
        "original\n"
    );
    assert_eq!(
        read_text_normalized(&dir.path().join("b.txt")),
        "original\n"
    );
}

#[test]
fn git_checkout_dirty_tree_error_keeps_file_list() {
    let dir = init_git_repo();
    std::fs::write(dir.path().join("a.txt"), "main\n").expect("write a");
    git_in_dir(dir.path(), &["add", "a.txt"]);
    git_in_dir(dir.path(), &["commit", "-q", "-m", "add a"]);
    git_in_dir(dir.path(), &["checkout", "-q", "-b", "feature/test"]);
    std::fs::write(dir.path().join("a.txt"), "feature\n").expect("write feature");
    git_in_dir(dir.path(), &["commit", "-am", "feature change"]);
    git_in_dir(dir.path(), &["checkout", "-q", "main"]);
    std::fs::write(dir.path().join("a.txt"), "dirty\n").expect("write dirty");

    let (stdout, stderr, code) =
        rtk_output_in_dir(dir.path(), &["git", "checkout", "feature/test"]);
    let combined = format!("{stdout}{stderr}");

    assert_ne!(code, Some(0));
    assert!(
        combined.contains("error:"),
        "dirty checkout failure should keep error header: {combined:?}"
    );
    assert!(
        combined.contains("a.txt"),
        "dirty checkout failure should keep conflicting filename: {combined:?}"
    );
    assert!(
        combined.contains("Aborting"),
        "dirty checkout failure should keep abort line: {combined:?}"
    );
}
