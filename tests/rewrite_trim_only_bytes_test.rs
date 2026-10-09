//! A vertical tab, a form feed, a carriage return and a non-breaking space are
//! word bytes in bash: only space, tab and newline separate words. Next to a
//! digit they make it part of a word rather than a redirect's fd number, so
//! `\x0b2>&1` is the command `\x0b2` with a `>&1` redirect. The difference
//! only shows through a rewrite of that word, which a trusted project filter
//! matching every command provides here. `exclude_commands` names command words
//! as bash ends them too, while a user filter's `match_command` is applied as
//! written.
//!
//! Unix only: the binary resolves its config directory through `$HOME`, and
//! only Unix takes that from the environment.
#![cfg(unix)]

mod common;

use std::path::Path;

/// A project filter matching every command, so that one which is rewritten at
/// all is rewritten, and only where a word starts and ends decides where.
const CATCH_ALL: &str = "schema_version = 1\n\n[filters.anything]\ndescription = \"probe\"\nmatch_command = \"^.\"\nmax_lines = 5\n";

fn project() -> tempfile::TempDir {
    project_with(CATCH_ALL, None)
}

/// A project whose trusted filters are `filters`, with `config` as the user's
/// `config.toml` when given.
///
/// The config goes into every location the loader may pick for this home:
/// `dirs::config_dir` is `$XDG_CONFIG_HOME` on Linux and
/// `~/Library/Application Support` on macOS.
fn project_with(filters: &str, config: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("work").join(".rtk")).expect("project dir");
    std::fs::write(
        dir.path().join("work").join(".rtk").join("filters.toml"),
        filters,
    )
    .expect("filters");
    if let Some(config) = config {
        for rtk_config in [
            dir.path().join(".config").join("rtk"),
            dir.path()
                .join("Library")
                .join("Application Support")
                .join("rtk"),
        ] {
            std::fs::create_dir_all(&rtk_config).expect("config dir");
            std::fs::write(rtk_config.join("config.toml"), config).expect("config");
        }
    }
    dir
}

/// `rtk rewrite <cmd>` as one argv, run in the project, returning stdout.
///
/// The exit status is checked against the output: 3 with a rewrite, 1 (no
/// rewrite) with nothing on stdout. Stderr must stay empty, so a panic cannot
/// pass for a refusal.
fn rewrite(dir: &Path, cmd: &str) -> String {
    let out = common::rtk_command()
        .args(["rewrite", cmd])
        .current_dir(dir.join("work"))
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join(".config"))
        .env("XDG_DATA_HOME", dir.join(".local").join("share"))
        .env("RTK_DB_PATH", dir.join("rtk.db"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env("RTK_TRUST_PROJECT_FILTERS", "1")
        .env("CI", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run rtk");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let expected_code = if stdout.is_empty() { 1 } else { 3 };
    assert_eq!(
        out.status.code(),
        Some(expected_code),
        "exit status for {cmd:?} (stdout {stdout:?})"
    );
    assert!(
        out.stderr.is_empty(),
        "stderr for {cmd:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

#[test]
fn a_word_byte_glued_to_an_fd_number_makes_it_a_word() {
    let dir = project();
    for (cmd, expected) in [
        // The glued byte and the digit are a command, rewritten like any other
        // behind the wrapper or assignment in front of it; the redirect stays.
        ("noglob \x0b2>&1", "noglob rtk \x0b2>&1"),
        ("FOO=1 \x0b12>&-", "FOO=1 rtk \x0b12>&-"),
        ("nice -n 5 \x0b2>&1", "nice -n 5 rtk \x0b2>&1"),
        ("exec \x0c2>&1", "exec rtk \x0c2>&1"),
        ("uv run \u{a0}2>&1", "uv run rtk \u{a0}2>&1"),
        ("git status; \x0b2>&1", "rtk git status; rtk \x0b2>&1"),
        ("ls\n\u{a0}2>/dev/null", "rtk ls\nrtk \u{a0}2>/dev/null"),
        // The `\r` after `&&` is a command of its own, so the first line does
        // not continue onto the second.
        (
            "git status &&\r\n\x0b2>&1",
            "rtk git status &&rtk \r\nrtk \x0b2>&1",
        ),
        ("( git status )\x0b2>&1", "( rtk git status )rtk \x0b2>&1"),
        // Only space and tab around it: a plain fd redirect, left alone.
        ("a &&  2<&0", "rtk a &&  2<&0"),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

#[test]
fn a_word_byte_at_either_end_stays_in_its_word() {
    let dir = project();
    for (cmd, expected) in [
        ("\x0bgit status", "rtk \x0bgit status"),
        ("ls -la\x0c", "rtk ls -la\x0c"),
        ("git\u{a0}status", "rtk git\u{a0}status"),
        // `status\r` and `status\u{a0}` are not git subcommands, and git is
        // never handed to the catch-all filter, so these stay as written.
        ("git status\r", ""),
        ("git status\r\n", ""),
        ("git status\u{a0}", ""),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

/// `head`, `tail` and `cat` are recognised by their first word, after a tab as
/// after a space: `head` and `tail` with arguments map their line range onto
/// `rtk read`, and `cat` with an option `rtk read` lacks is left alone. A bare
/// one has nothing to map or check and reaches the filters like any other
/// command.
#[test]
fn head_tail_and_cat_are_recognised_by_their_first_word() {
    let dir = project();
    for (cmd, expected) in [
        ("head\t-3 f", "rtk read f --head-lines 3"),
        ("tail\t-n 2 f", "rtk read f --tail-lines 2"),
        ("cat\t-A f", ""),
        ("cat -n", ""),
        ("head", "rtk head"),
        ("git status || cat", "rtk git status || rtk cat"),
        ("git log |\n  tail", "rtk git log |\n  tail"),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

/// `exclude_commands` is decided on the command word as the lexer ends it, at
/// a space, a tab or a newline, while a trusted user filter is matched with its
/// regex as written. `^mytool\b` also matches `mytool\x0bx`, `mytool-x` and
/// `mytool.x`, programs other than `mytool`, which excluding `mytool` leaves
/// alone.
#[test]
fn exclusion_reads_the_command_word_and_a_user_filter_matches_as_written() {
    let dir = project_with(
        "schema_version = 1\n\n[filters.mytool]\ndescription = \"probe\"\nmatch_command = \"^mytool\\\\b\"\nmax_lines = 5\n",
        Some("[hooks]\nexclude_commands = [\"mytool\"]\n"),
    );
    for (cmd, expected) in [
        ("mytool", ""),
        ("mytool x", ""),
        ("mytool\tx", ""),
        ("FOO=1 mytool x", ""),
        ("mytool\x0bx", "rtk mytool\x0bx"),
        ("mytool-x", "rtk mytool-x"),
        ("mytool.x y", "rtk mytool.x y"),
        ("mytoolx", ""),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}
