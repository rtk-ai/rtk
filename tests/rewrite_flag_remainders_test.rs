//! A wrapper's own options are never a command. Under a project filter that
//! matches every command, whatever is left of a wrapper's words after its
//! grammar would otherwise be handed to that filter, so these rows show that
//! `rtk` never lands inside the wrapper's grammar, whether the wrapper is
//! peeled by the built-in tables or a configured prefix claims part of it.
//! Nor is that filter handed a command `exclude_commands` covers, which is
//! read on the words bash runs.
//!
//! Unix only: the binary resolves its config directory through `$HOME`, and
//! only Unix takes that from the environment.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::Stdio;

const CATCH_ALL: &str = "schema_version = 1\n\n[filters.anything]\ndescription = \"probe\"\nmatch_command = \"^.\"\nmax_lines = 5\n";

/// A home whose config holds `prefixes`, and a project with the catch-all
/// filter.
fn project(prefixes: &str) -> tempfile::TempDir {
    project_excluding(prefixes, "[]")
}

/// [`project`], with `exclude_commands` as well.
fn project_excluding(prefixes: &str, excluded: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("work").join(".rtk")).expect("project dir");
    std::fs::write(
        dir.path().join("work").join(".rtk").join("filters.toml"),
        CATCH_ALL,
    )
    .expect("filters");
    // The config directory is `.config` on Linux and `Library/Application
    // Support` on macOS; both hold the config so either platform reads it.
    for config_dir in [
        dir.path().join(".config").join("rtk"),
        dir.path()
            .join("Library")
            .join("Application Support")
            .join("rtk"),
    ] {
        std::fs::create_dir_all(&config_dir).expect("config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!("[hooks]\ntransparent_prefixes = {prefixes}\nexclude_commands = {excluded}\n"),
        )
        .expect("config");
    }
    dir
}

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
        .stdin(Stdio::null())
        .output()
        .expect("run rtk");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_prefix_inside_a_wrappers_own_words_leaves_them_alone() {
    let dir = project("[\"nice\", \"timeout 5\", \"time\", \"env\"]");
    for cmd in [
        "nice -5 echo hi",
        "nice -n 5 npm test",
        "nice -n 5\techo hi",
        "nice --adjustment=3 echo hi",
        "timeout 5 -- -- git status",
        "timeout 5 -- git status",
        "timeout 5 -v git status",
        "time -f %e git status",
        "time -o out git status",
        "env -i git status",
    ] {
        assert_eq!(rewrite(dir.path(), cmd), "", "{cmd:?}");
    }
}

#[test]
fn a_prefix_that_takes_the_whole_wrapper_applies() {
    let dir = project("[\"nice\"]");
    for (cmd, expected) in [
        ("nice git status", "nice rtk git status"),
        ("nice -5 git status", "nice -5 rtk git status"),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

/// A prefix that ends inside the words of a later built-in layer, not only the
/// one it starts at, would leave that layer's flags as the command.
#[test]
fn a_prefix_ending_inside_any_later_layers_words_is_skipped() {
    let dir = project_excluding("[\"nice timeout\"]", "[\"foo\"]");
    assert_eq!(rewrite(dir.path(), "nice timeout 5 foo"), "");
    assert_eq!(
        rewrite(dir.path(), "nice timeout 5 bar"),
        "nice timeout 5 rtk bar"
    );
}

/// Where a pipeline starts, `time` is bash's reserved word, whose only option
/// is `-p`: when what follows is not accepted, `time` is not the GNU program
/// either, and nothing — a filter, a prefix — may take it for that.
#[test]
fn a_reserved_word_that_rejects_what_follows_is_refused() {
    for prefixes in ["[]", "[\"time\"]", "[\"time -v\"]"] {
        let dir = project(prefixes);
        for cmd in [
            "time -v git status",
            "time -o x git status",
            "time -f %e git status",
            "time -p -p git status",
            "time -pv git status",
            "( time -v git status )",
            "ls && time -v git status",
            "time -o x rtk git status",
        ] {
            let out = rewrite(dir.path(), cmd);
            assert!(
                !out.contains("rtk -") && !out.contains("rtk time"),
                "{prefixes} {cmd:?}: {out}"
            );
            assert!(
                !out.contains("time rtk") && !out.contains("x rtk"),
                "{prefixes} {cmd:?}: {out}"
            );
        }
        // The GNU program, spelled so that bash does not claim it, is the program.
        assert_eq!(
            rewrite(dir.path(), "/usr/bin/time -v git status"),
            "/usr/bin/time -v rtk git status"
        );
        assert_eq!(
            rewrite(dir.path(), "time -p git status"),
            "time -p rtk git status"
        );
    }
}

/// A bare `cat`, `head` or `tail` names no file for `rtk read`, so the guards
/// on them leave it to whatever else claims it — a project filter, here.
#[test]
fn a_bare_cat_head_or_tail_reaches_a_project_filter() {
    let dir = project("[]");
    for (cmd, expected) in [
        ("cat", "rtk cat"),
        ("head", "rtk head"),
        ("tail", "rtk tail"),
        ("echo x || head", "echo x || rtk head"),
        ("git status && cat", "rtk git status && rtk cat"),
        // With arguments they are guarded.
        ("cat -A file", ""),
        ("head -5", ""),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

/// A head that is an option to the shell's eyes is no command, however it is
/// quoted.
#[test]
fn a_quoted_option_is_not_a_command_either() {
    let dir = project("[]");
    for cmd in [
        "timeout 300 \"-s\" KILL git status",
        "timeout 300 '-s' KILL git status",
        "timeout 300 \\-s KILL git status",
        "timeout 300 '--' git status",
        "timeout 300 \"--\" git status",
        "timeout 300 \"-\"s KILL git status",
    ] {
        assert_eq!(rewrite(dir.path(), cmd), "", "{cmd:?}");
    }
}

/// `exclude_commands` reads a command on the words bash runs, their quotes
/// and escapes removed, up to the redirections behind it, the program's name
/// with or without its path, whether the command is behind `uv run` or not and
/// whether a filter would take it or not. A word that holds a blank is one
/// word, the program `pytest x` here.
#[test]
fn an_excluded_command_is_read_on_the_words_bash_runs() {
    for (excluded, cmd, filtered, unfiltered) in [
        ("[\"pytest\"]", "'pytest' -x", "", ""),
        ("[\"pytest\"]", "\\pytest -x", "", ""),
        ("[\"pytest\"]", "timeout 5 'pytest' -x", "", ""),
        ("[\"pytest\"]", "'/opt/my tools/pytest' -x", "", ""),
        ("[\"pytest\"]", "uv run '/opt/my tools/pytest' -x", "", ""),
        ("[\"pytest\"]", "uv run timeout 5 'pytest' 2>&1", "", ""),
        ("[\"^pytest$\"]", "'pytest' 2>&1", "", ""),
        ("[\"^pytest$\"]", "uv run 'pytest' 2>&1", "", ""),
        ("[\"^pytest$\"]", "uv run timeout 5 'pytest' 2>&1", "", ""),
        ("[\"^pytest$\"]", "'pytest' -x", "rtk 'pytest' -x", ""),
        (
            "[\"^pytest$\"]",
            "uv run '/opt/my tools/pytest' -x",
            "uv run rtk '/opt/my tools/pytest' -x",
            "rtk uv run '/opt/my tools/pytest' -x",
        ),
        ("[\"pytest\"]", "'pytest x'", "rtk 'pytest x'", ""),
        (
            "[\"pytest\"]",
            "uv run 'pytest x'",
            "uv run rtk 'pytest x'",
            "rtk uv run 'pytest x'",
        ),
    ] {
        let dir = project_excluding("[]", excluded);
        assert_eq!(
            rewrite(dir.path(), cmd),
            filtered,
            "{cmd:?} under {excluded}"
        );
        std::fs::remove_file(dir.path().join("work").join(".rtk").join("filters.toml"))
            .expect("remove the filter");
        assert_eq!(
            rewrite(dir.path(), cmd),
            unfiltered,
            "{cmd:?} under {excluded}, no filter"
        );
    }
}

/// A wrapper head spelled by a path is the program at that path, which no
/// layer takes and which hides what it runs, so the line is left as written,
/// whether or not a command is excluded and whether or not a project filter
/// takes every command. A head written with quotes or escapes only is the
/// wrapper itself, and the command behind it is read.
#[test]
fn a_path_spelled_wrapper_head_leaves_the_line_as_written() {
    for excluded in ["[]", "[\"pytest\"]"] {
        let dir = project_excluding("[]", excluded);
        for head in [
            "/usr/bin/env",
            "/usr/bin/command",
            "/usr/bin/exec",
            "/usr/bin/noglob",
            "/usr/bin/uv run",
        ] {
            for cmd in [
                format!("{head} pytest"),
                format!("{head} git status"),
                format!("timeout 5 {head} pytest -x"),
            ] {
                assert_eq!(rewrite(dir.path(), &cmd), "", "{excluded} {cmd}");
            }
            // `uv run` has its own rule, so it is refused only for what it
            // hides from an exclusion.
            if excluded != "[]" {
                let cmd = format!("uv run {head} pytest");
                assert_eq!(rewrite(dir.path(), &cmd), "", "{cmd}");
            }
        }
    }
}

#[test]
fn a_quoted_wrapper_head_behind_uv_run_is_read_for_exclusion() {
    let dir = project_excluding("[]", "[\"pytest\"]");
    for cmd in [
        "uv run env pytest",
        "uv run 'env' pytest",
        "uv run \\env pytest",
        "uv run 'command' pytest",
        "uv run \"noglob\" pytest",
        "uv run 'exec' pytest",
        "uv run 'uv' run pytest",
        "timeout 5 'env' pytest -x",
    ] {
        assert_eq!(rewrite(dir.path(), cmd), "", "{cmd}");
    }
}
