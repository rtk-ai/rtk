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
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .args(args)
        .current_dir(dir)
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
fn git_show_dash_dash_stat_pathspec_after_double_dash_is_not_stat_flag() {
    // Regression: `rtk git show -- --stat` must not be misread as a request for the real
    // `--stat` summary flag. Before restore_double_dash + arg_tokenizer, run_show's
    // wants_stat_only check was a raw `arg == "--stat"` scan with no `--`-boundary awareness, so
    // a file literally named "--stat" after the boundary was wrongly treated as the flag and
    // sent down the raw-passthrough path instead of RTK's own compacted-diff path.
    let dir = init_git_repo();
    std::fs::write(dir.path().join("--stat"), "not a summary flag\n").expect("write --stat file");
    git_in_dir(dir.path(), &["add", "--", "--stat"]);
    git_in_dir(
        dir.path(),
        &["commit", "-q", "-m", "add dash-dash-stat file"],
    );

    let (stdout, stderr, code) = rtk_output_in_dir(dir.path(), &["git", "show", "--", "--stat"]);

    assert_eq!(code, Some(0), "rtk stderr: {stderr}");
    assert!(
        !stdout.contains("diff --git"),
        "-- --stat should stay on RTK's compacted-diff path, not raw passthrough: {stdout:?}"
    );
    assert!(
        stdout.contains("--stat") && stdout.contains("+1"),
        "expected RTK's compacted diff summary for the --stat file: {stdout:?}"
    );
}

#[test]
fn git_diff_dash_dash_stat_pathspec_after_double_dash_is_not_stat_flag() {
    // Regression: `rtk git diff -- --stat` must not be misread as a request for the real
    // `--stat` diffstat-only flag. Before this fix, run_diff's wants_stat check was a raw
    // `arg == "--stat"` scan with no `--`-boundary awareness, so a file literally named "--stat"
    // after the boundary was wrongly treated as the flag and sent down the raw-passthrough path
    // (plain diffstat output) instead of RTK's own stat+compacted-diff path.
    let dir = init_git_repo();
    std::fs::write(dir.path().join("--stat"), "line one\n").expect("write --stat file");
    git_in_dir(dir.path(), &["add", "--", "--stat"]);
    git_in_dir(
        dir.path(),
        &["commit", "-q", "-m", "add dash-dash-stat file"],
    );
    std::fs::write(dir.path().join("--stat"), "line one\nline two\n").expect("modify --stat file");

    let (stdout, stderr, code) = rtk_output_in_dir(dir.path(), &["git", "diff", "--", "--stat"]);

    assert_eq!(code, Some(0), "rtk stderr: {stderr}");
    assert!(
        !stdout.contains("diff --git"),
        "-- --stat should stay on RTK's stat+compacted-diff path, not raw passthrough: {stdout:?}"
    );
    assert!(
        stdout.contains("--stat | 1 +") && stdout.contains("Changes:"),
        "expected RTK's stat-summary-plus-compacted-diff output for the modified --stat file: {stdout:?}"
    );
}

#[test]
fn git_diff_name_only_passes_through_raw() {
    // Regression: run_diff's wants_stat check only recognized --stat/--numstat/--shortstat, a
    // narrower list than requests_raw_diff_shape (which run_log already used, covering
    // --name-only/--name-status/--raw/--dirstat/--summary/-p/-u too). `--name-only` fell through
    // to RTK's default stat+compacted-diff path instead of a raw passthrough of git's own
    // name-only output.
    let dir = init_git_repo();
    std::fs::write(dir.path().join("file.txt"), "one\n").expect("write file");
    git_in_dir(dir.path(), &["add", "file.txt"]);
    git_in_dir(dir.path(), &["commit", "-q", "-m", "add file"]);
    std::fs::write(dir.path().join("file.txt"), "one\ntwo\n").expect("modify file");

    let (stdout, stderr, code) = rtk_output_in_dir(dir.path(), &["git", "diff", "--name-only"]);

    assert_eq!(code, Some(0), "rtk stderr: {stderr}");
    assert_eq!(
        stdout.trim(),
        "file.txt",
        "--name-only should pass through as git's own bare filename list: {stdout:?}"
    );
}

#[test]
fn git_branch_dash_prefixed_name_after_double_dash_attempts_creation_not_a_silent_list() {
    // Regression: `rtk git branch -- -weird` must be classified as a branch-creation attempt
    // (and let real git's own ref-name validation reject it), not silently fall through to list
    // mode as if no branch name were given. Before restore_double_dash + arg_tokenizer,
    // run_branch's has_positional_arg check was a raw `!a.starts_with('-')` scan with no
    // `--`-boundary awareness, so a branch name starting with '-' after the separator was
    // misclassified as a flag: has_positional_arg came back false, and (with no list flag
    // either) rtk silently ran `git branch -a --no-color -- -weird-branch` -- a harmless, empty,
    // exit-0 *list* filtered on a pattern that matches nothing, giving no indication the
    // requested branch was never created. A real branch named "-weird-branch" is impossible
    // (git's own check-ref-format forbids a leading '-'), so the observable signal here is that
    // rtk actually attempts the creation and surfaces git's real rejection, instead of quietly
    // doing nothing and exiting 0.
    let dir = init_git_repo();

    let (stdout, stderr, code) =
        rtk_output_in_dir(dir.path(), &["git", "branch", "--", "-weird-branch"]);

    assert_ne!(
        code,
        Some(0),
        "a creation attempt for an invalid ref name must fail, not silently succeed as an empty list: stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stderr.contains("-weird-branch"),
        "expected git's own rejection to mention the attempted branch name: {stderr:?}"
    );
}

#[test]
fn git_log_malformed_digit_run_propagates_real_git_error() {
    // "-5x" isn't a valid git log limit; real git rejects it outright ("fatal: '5x': not an
    // integer", verified against git 2.51). run_log's internal limit-parsing for this
    // malformed input differs before/after arg_tokenizer (5 vs the old fallback of 10), but
    // that's never observable here: run_log bails out on the real git failure before ever
    // reaching the formatting code that would use it.
    let dir = init_git_repo();

    let raw = Command::new("git")
        .args(["log", "-5x"])
        .current_dir(dir.path())
        .output()
        .expect("spawn raw git log");
    assert!(!raw.status.success(), "expected real git to reject -5x");

    let (_, rtk_stderr, rtk_code) = rtk_output_in_dir(dir.path(), &["git", "log", "-5x"]);

    assert_eq!(rtk_code, raw.status.code());
    assert!(
        rtk_stderr.contains("not an integer"),
        "rtk should surface git's own error verbatim: {rtk_stderr:?}"
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
fn git_checkout_dash_b_capital_reports_what_git_actually_did() {
    let dir = init_git_repo();

    // `-B` creates *or* resets, and only git knows which. Reading the branch name out of the
    // args and returning early claimed neither, so a created branch lost its `(new)` marker.
    //
    // rtk_output_in_dir pins LC_ALL=C, which is what makes the English scan land. Under
    // another locale this degrades to the args fallback ("ok feature/test") rather than
    // claiming a marker it cannot verify -- weaker, never wrong -- so `(new)` is asserted
    // here only because the locale is pinned.
    let (out, code) = rtk_in_dir(dir.path(), &["git", "checkout", "-B", "feature/test"]);
    assert_eq!(code, Some(0));
    assert_eq!(
        out.trim(),
        "ok feature/test (new)",
        "git: Switched to a new branch"
    );

    // Reset of a branch that already exists: not new, and the args fallback names it.
    git_in_dir(dir.path(), &["checkout", "-q", "main"]);
    let (out, code) = rtk_in_dir(dir.path(), &["git", "checkout", "-B", "feature/test"]);
    assert_eq!(code, Some(0));
    assert_eq!(
        out.trim(),
        "ok feature/test",
        "git: Switched to and reset branch -- matches no scan prefix, so the args name it"
    );

    // Glued spelling routes identically; the string scans it replaced could not read it.
    git_in_dir(dir.path(), &["checkout", "-q", "main"]);
    let (out, code) = rtk_in_dir(dir.path(), &["git", "checkout", "-Bfeature/glued"]);
    assert_eq!(code, Some(0));
    assert_eq!(out.trim(), "ok feature/glued (new)");
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
