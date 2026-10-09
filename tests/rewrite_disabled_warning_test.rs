//! #508's `RTK_DISABLED` warning and the config's own warnings are stderr, so
//! no rewrite test that compares stdout can see them move. These tests pin
//! which lines draw the `RTK_DISABLED` warning (exactly once), what the line
//! rewrites to alongside it, and where the warning prints relative to the
//! config's and the filter registry's own warnings.
//!
//! Unix only: the binary resolves its config directory through `$HOME`, and
//! only Unix takes that from the environment.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::Stdio;

const WARNING: &str = "[rtk] warning: RTK_DISABLED=1 detected — skipping filter for this command.";

/// Runs `rtk rewrite <cmd>` (one argv) against a home holding `config`, and
/// returns stdout and stderr.
fn run(home: &Path, cmd: &str) -> (String, String) {
    let out = common::rtk_command()
        .args(["rewrite", cmd])
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .stdin(Stdio::null())
        .output()
        .expect("run rtk");
    (
        String::from_utf8_lossy(&out.stdout).trim_end().to_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn stderr_of(home: &Path, cmd: &str) -> String {
    run(home, cmd).1
}

/// A `[hooks]` table with `exclude_commands` and `transparent_prefixes` set to
/// the given entries.
fn hooks_config(ex: &[&str], tp: &[&str]) -> String {
    let list = |items: &[&str]| {
        let quoted: Vec<String> = items.iter().map(|i| format!("\"{i}\"")).collect();
        format!("[{}]", quoted.join(", "))
    };
    format!(
        "[hooks]\nexclude_commands = {}\ntransparent_prefixes = {}\n",
        list(ex),
        list(tp)
    )
}

/// A home holding `config` as `config.toml`, in both places the config
/// directory resolves to: `.config` on Linux, `Library/Application Support` on
/// macOS.
fn home_with(config: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    for dir in [
        home.path().join(".config").join("rtk"),
        home.path()
            .join("Library")
            .join("Application Support")
            .join("rtk"),
    ] {
        std::fs::create_dir_all(&dir).expect("config dir");
        std::fs::write(dir.join("config.toml"), config).expect("config");
    }
    home
}

#[test]
fn the_warning_prints_before_the_exclude_patterns_own_warnings() {
    let home = home_with("[hooks]\nexclude_commands = [\"^\", \"^(\", \"  \"]\n");
    let stderr = stderr_of(home.path(), "RTK_DISABLED=1 git status");
    let at_warning = stderr.find(WARNING).expect("the RTK_DISABLED warning");
    let at_pattern = stderr
        .find("exclude_commands pattern")
        .expect("a pattern warning");
    assert!(
        at_warning < at_pattern,
        "the RTK_DISABLED warning comes first: {stderr}"
    );
}

#[test]
fn the_patterns_warnings_print_for_a_line_with_nothing_to_rewrite() {
    let home = home_with("[hooks]\nexclude_commands = [\"^\"]\n");
    for cmd in ["", "   ", "echo hi", "echo a\necho b"] {
        let stderr = stderr_of(home.path(), cmd);
        assert!(
            stderr.contains("[rtk] warning: ignoring trivial exclude_commands pattern '^'"),
            "{cmd:?} compiles the config first: {stderr}"
        );
    }
}

/// The warning is the refusal: it prints once where the rewrite's walk stops at
/// `RTK_DISABLED=`, however the prefix got there, and the line is left as
/// written.
#[test]
fn the_warning_prints_where_the_rewrite_refuses() {
    let rows: &[(&str, &[&str], &[&str])] = &[
        ("RTK_DISABLED=1 git status", &[], &[]),
        ("RTK_DISABLED+=1 git status", &[], &[]),
        ("timeout 5 env RTK_DISABLED=1 git status", &[], &[]),
        ("nice -n 5 env RTK_DISABLED=1 git status", &[], &[]),
        (
            "uv run timeout 5 env RTK_DISABLED=1 pytest",
            &["pytest"],
            &[],
        ),
        ("uv run env RTK_DISABLED=1 git status", &[], &[]),
        ("cargo test | RTK_DISABLED=1 grep FAILED", &[], &[]),
        ("nohup RTK_DISABLED=1 git status", &[], &["nohup"]),
        (
            "docker exec c RTK_DISABLED=1 git status",
            &[],
            &["docker exec c"],
        ),
    ];
    for (cmd, ex, tp) in rows {
        let home = home_with(&hooks_config(ex, tp));
        let (stdout, stderr) = run(home.path(), cmd);
        assert_eq!(stdout, "", "{cmd:?} is left as written");
        assert_eq!(stderr.matches(WARNING).count(), 1, "{cmd:?}: {stderr}");
    }
}

/// A line whose first command is rewritten draws the warning for a later
/// command that carries the marker.
#[test]
fn a_later_command_with_the_marker_draws_the_warning_beside_a_rewrite() {
    let home = home_with("");
    let (stdout, stderr) = run(home.path(), "git status && RTK_DISABLED=1 cargo build");
    assert_eq!(stdout, "rtk git status && RTK_DISABLED=1 cargo build");
    assert_eq!(stderr.matches(WARNING).count(), 1, "{stderr}");
}

/// Where the marker is an option operand or the walk never reaches it, the
/// line is left as written and no warning is printed.
#[test]
fn a_line_left_as_written_without_reaching_the_marker_draws_no_warning() {
    for cmd in [
        "timeout 5 RTK_DISABLED=1 git status",
        "noglob RTK_DISABLED=1 git status",
        "RTK_DISABLED=1 git status | wc -l",
        "uv run RTK_DISABLED=1 git status",
        "uv run A=1 pytest",
    ] {
        let home = home_with(&hooks_config(&[], &[]));
        let (stdout, stderr) = run(home.path(), cmd);
        assert_eq!(stdout, "", "{cmd:?} is left as written");
        assert_eq!(stderr.matches(WARNING).count(), 0, "{cmd:?}: {stderr}");
    }
}

/// The name has to match exactly and stand in an assignment position; a longer
/// name, text inside a value, an `env` operand with `+=`, or an assignment
/// statement with no command behind it is no bypass, so the next command is
/// rewritten and nothing is printed.
#[test]
fn a_line_that_only_mentions_the_marker_is_rewritten_without_a_warning() {
    let rows: &[(&str, &str, &[&str])] = &[
        (
            "XRTK_DISABLED=1 git status",
            "XRTK_DISABLED=1 rtk git status",
            &[],
        ),
        (
            "NOTE='set RTK_DISABLED=1 x' git status",
            "NOTE='set RTK_DISABLED=1 x' rtk git status",
            &[],
        ),
        (
            "MSG='RTK_DISABLED=1' git status",
            "MSG='RTK_DISABLED=1' rtk git status",
            &[],
        ),
        (
            "env MSG=RTK_DISABLED=1 git status",
            "env MSG=RTK_DISABLED=1 rtk git status",
            &[],
        ),
        (
            "env RTK_DISABLED+=1 git status",
            "env RTK_DISABLED+=1 rtk git status",
            &[],
        ),
        (
            "RTK_DISABLED=1; git status",
            "RTK_DISABLED=1; rtk git status",
            &[],
        ),
        (
            "timeout 5 nice -n 19 RTK_DISABLED=1 git status",
            "timeout 5 nice -n 19 RTK_DISABLED=1 rtk git status",
            &["timeout 5 nice -n 19 RTK_DISABLED=1", "nice -n 19"],
        ),
        ("uv run env python x.py", "rtk uv run env python x.py", &[]),
    ];
    for (cmd, expected, tp) in rows {
        let home = home_with(&hooks_config(&[], tp));
        let (stdout, stderr) = run(home.path(), cmd);
        assert_eq!(stdout, *expected, "{cmd:?}");
        assert_eq!(stderr.matches(WARNING).count(), 0, "{cmd:?}: {stderr}");
    }
}

#[test]
fn the_warning_comes_before_the_prefix_warnings() {
    let home = home_with("[hooks]\ntransparent_prefixes = [\"x 'a\"]\n");
    let stderr = stderr_of(home.path(), "RTK_DISABLED=1 git status");
    let at_warning = stderr.find(WARNING).expect("the RTK_DISABLED warning");
    let at_prefix = stderr
        .find("[rtk] warning: ignoring transparent_prefixes entry 'x 'a': it has an unclosed quote or a trailing backslash")
        .expect("a prefix warning");
    assert!(at_warning < at_prefix, "{stderr}");
    assert_eq!(stderr.matches(WARNING).count(), 1, "{stderr}");
    assert_eq!(stderr.matches("transparent_prefixes entry").count(), 1);
}

/// Everything the rewrite says besides its answer comes after the
/// `RTK_DISABLED` warning, in the order it was made: the config's own
/// warnings, then what the filter registry says about trust.
#[test]
fn the_warning_comes_before_the_config_and_trust_warnings() {
    let home = home_with("[hooks]\ntransparent_prefixes = [\"x 'a\"]\n");
    let work = home.path().join("work");
    std::fs::create_dir_all(work.join(".rtk")).expect("project dir");
    std::fs::write(
        work.join(".rtk").join("filters.toml"),
        "schema_version = 1\n\n[filters.f]\ndescription = \"d\"\nmatch_command = \"^zzz\"\n",
    )
    .expect("filters");
    let out = common::rtk_command()
        .args(["rewrite", "RTK_DISABLED=1 git status; zzz go"])
        .current_dir(&work)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("XDG_DATA_HOME", home.path().join(".local").join("share"))
        .env("RTK_DB_PATH", home.path().join("rtk.db"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env("RTK_TRUST_PROJECT_FILTERS", "1")
        .env_remove("CI")
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITLAB_CI")
        .env_remove("JENKINS_URL")
        .env_remove("BUILDKITE")
        .stdin(Stdio::null())
        .output()
        .expect("run rtk");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let at = |needle: &str| {
        stderr
            .find(needle)
            .unwrap_or_else(|| panic!("{needle}: {stderr}"))
    };
    let disabled = at(WARNING);
    let config = at("transparent_prefixes entry");
    let trust = at("RTK_TRUST_PROJECT_FILTERS=1 ignored");
    assert!(disabled < config && config < trust, "{stderr}");
    assert_eq!(stderr.matches(WARNING).count(), 1, "{stderr}");
}
