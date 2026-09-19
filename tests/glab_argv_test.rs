//! Child-argv tests for `rtk glab`: which argument becomes the MR/issue identifier, and where
//! the user's `--` and rtk's own `-R`/`-g` end up. Stubs `glab` on PATH with a script that
//! records its argv, so every assertion is on what glab actually received.

#![cfg(unix)]

use std::process::Command;

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// Runs `rtk <args>` against a `glab` stub and returns the argv the stub received.
fn glab_argv(args: &[&str]) -> Vec<String> {
    run_with_stub(args, "").0
}

/// Runs `rtk <args>` against a `glab` stub that prints `stub_stdout`, and returns rtk's stdout.
fn glab_stdout(args: &[&str], stub_stdout: &str) -> String {
    run_with_stub(args, stub_stdout).1
}

fn run_with_stub(args: &[&str], stub_stdout: &str) -> (Vec<String>, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let argv_file = dir.path().join("argv.txt");
    let stub_path = dir.path().join("glab");

    std::fs::write(
        &stub_path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\ncat <<'STUB_EOF'\n{}\nSTUB_EOF\nexit 0\n",
            shell_quote(&argv_file),
            stub_stdout
        ),
    )
    .expect("write stub");
    let mut perms = std::fs::metadata(&stub_path)
        .expect("stat stub")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&stub_path, perms).expect("chmod stub");

    let path_with_stub = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("PATH", path_with_stub)
        .env("LC_ALL", "C")
        // Without these the run reads the developer's real config and writes to their real
        // tracking DB, so the assertions depend on local machine state.
        .env("HOME", dir.path())
        .env("RTK_DB_PATH", dir.path().join("rtk.db"))
        .current_dir(dir.path())
        .args(args)
        .output()
        .expect("spawn rtk");

    let argv = std::fs::read_to_string(&argv_file)
        .unwrap_or_else(|e| {
            panic!(
                "read captured argv: {e}; rtk stdout={} stderr={}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        })
        .lines()
        .map(str::to_string)
        .collect();
    (argv, String::from_utf8_lossy(&out.stdout).into_owned())
}

// ── identifier extraction: a flag's value is never the MR/issue id ──────

#[test]
fn mr_view_page_value_is_not_the_mr_id() {
    // `-p/--page` takes a value on `glab mr view`; the walker's list omitted it, so "2" was
    // hoisted out of the flag and re-emitted as the MR identifier.
    let argv = glab_argv(&["glab", "mr", "view", "--page", "2"]);
    assert_eq!(argv, vec!["mr", "view", "-F", "json", "--page", "2"]);
}

#[test]
fn mr_view_jq_expression_is_not_the_mr_id() {
    // `--jq` is a user-supplied projection, so rtk forwards the command untouched rather than
    // injecting `-F json` and reformatting the result as an MR summary.
    let argv = glab_argv(&["glab", "mr", "view", "--jq", ".iid"]);
    assert_eq!(argv, vec!["mr", "view", "--jq", ".iid"]);
}

#[test]
fn mr_view_jq_output_reaches_the_user_verbatim() {
    // Long enough that rtk's MR-shaped rendering of it would be the shorter output, so the
    // never-worse guard does not rescue the projection by accident.
    let payload = format!(
        "[{}]",
        (0..40)
            .map(|i| format!("\"label-number-{i:02}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    let out = glab_stdout(&["glab", "mr", "view", "--jq", ".labels"], &payload);
    assert_eq!(out.trim(), payload);
}

#[test]
fn issue_view_per_page_value_is_not_the_issue_id() {
    let argv = glab_argv(&["glab", "issue", "view", "-P", "50"]);
    assert_eq!(argv, vec!["issue", "view", "-F", "json", "-P", "50"]);
}

#[test]
fn mr_view_keeps_an_explicit_id_ahead_of_a_valued_flag() {
    let argv = glab_argv(&["glab", "mr", "view", "--page", "2", "42"]);
    assert_eq!(argv, vec!["mr", "view", "42", "-F", "json", "--page", "2"]);
}

// ── `--` handling ──────────────────────────────────────────────────────

#[test]
fn glab_double_dash_before_subcommand_is_dropped() {
    // That `--` ended rtk's own option parsing. Forwarding it makes glab stop looking for a
    // subcommand and print its root help instead of the MR.
    let argv = glab_argv(&["glab", "--", "mr", "view", "42"]);
    assert_eq!(argv, vec!["mr", "view", "42", "-F", "json"]);
}

#[test]
fn glab_double_dash_after_subcommand_is_dropped() {
    // clap strips a `--` that immediately follows the subcommand too, and glab answers
    // `glab mr -- view 42` with the `mr` help instead of dispatching to `view`.
    let argv = glab_argv(&["glab", "mr", "--", "view", "42"]);
    assert_eq!(argv, vec!["mr", "view", "42", "-F", "json"]);
}

#[test]
fn glab_double_dash_after_subcommand_is_dropped_before_a_flag() {
    // glab reads everything past `--` as a positional, so this spelling is `Accepts 1 arg(s),
    // received 2` on glab 1.117 while `glab api projects/1 --paginate` works.
    let argv = glab_argv(&["glab", "api", "--", "projects/1", "--paginate"]);
    assert_eq!(argv, vec!["api", "projects/1", "--paginate"]);
}

#[test]
fn glab_double_dash_inside_trailing_region_is_preserved() {
    // An interior `--` is the user's own: rtk forwards it and lets glab answer for it.
    let argv = glab_argv(&["glab", "api", "projects/1", "--", "--paginate"]);
    assert_eq!(argv, vec!["api", "projects/1", "--", "--paginate"]);
}

#[test]
fn glab_escaped_flag_is_not_hoisted_into_flag_position() {
    // `--web` sits behind the boundary the user typed; reading it as the MR number and
    // re-emitting it ahead of the `--` would make rtk open a browser the user escaped.
    let argv = glab_argv(&["glab", "mr", "view", "--", "--web", "42"]);
    let web = argv.iter().position(|a| a == "--web").expect("--web sent");
    let boundary = argv.iter().position(|a| a == "--").expect("-- forwarded");
    assert!(
        boundary < web,
        "the escaped flag must stay behind the boundary: {argv:?}"
    );
}

#[test]
fn glab_lone_escaped_flag_is_not_hoisted_into_flag_position() {
    // Same hazard with nothing else behind the boundary: `--web` is the only escaped token, so
    // reading it as the MR number leaves it in flag position with the `--` stranded behind it.
    let argv = glab_argv(&["glab", "mr", "view", "--", "--web"]);
    assert_eq!(argv, vec!["mr", "view", "--", "--web"]);
}

#[test]
fn glab_lone_escaped_identifier_is_still_hoisted() {
    // `glab mr view -- 42` views MR 42, so unescaping a token glab reads as a positional anyway
    // is what lets rtk inject `-F json` instead of passing the command through.
    let argv = glab_argv(&["glab", "mr", "view", "--", "42"]);
    assert_eq!(argv, vec!["mr", "view", "42", "-F", "json", "--"]);
}

#[test]
fn glab_repo_flag_is_injected_before_the_boundary() {
    // glab reads everything past `--` as a positional, so an appended `-R` would arrive as two
    // extra arguments rather than as the repo flag.
    let argv = glab_argv(&["glab", "-R", "o/r", "mr", "diff", "--", "42"]);
    assert_eq!(argv, vec!["mr", "diff", "-R", "o/r", "--", "42"]);
}
