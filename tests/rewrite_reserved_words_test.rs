//! A bash reserved word is never a command, so the rewrite never puts `rtk` in
//! front of one: `rtk until false; do …; done` or `rtk esac` is a syntax error
//! where the line was a loop or a `case`. Nor does it put one inside a
//! `[[ … ]]` expression or before a `case` pattern, where nothing is a command.
//! Only a filter that matches such a word can offer the rewrite, which a
//! trusted project filter matching every command does here. Word text (an
//! extglob group, an array literal, a `${ }`) is never a command either, and
//! after a pipe `time` is a program, not a reserved word.
//!
//! Unix only: the binary resolves its config directory through `$HOME`, and
//! only Unix takes that from the environment.
#![cfg(unix)]

mod common;

use std::path::Path;

/// A project filter matching every command.
const CATCH_ALL: &str = "schema_version = 1\n\n[filters.anything]\ndescription = \"probe\"\nmatch_command = \"^.\"\nmax_lines = 5\n";

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("work").join(".rtk")).expect("project dir");
    std::fs::write(
        dir.path().join("work").join(".rtk").join("filters.toml"),
        CATCH_ALL,
    )
    .expect("filters");
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
fn no_rewrite_starts_with_a_reserved_word() {
    let dir = project();
    for (cmd, expected) in [
        ("until false; do git status; done", ""),
        ("select x in a; do ls; done", ""),
        ("if true; then ls x; elif true; then ls y; fi", ""),
        ("case x in x) ls;; esac", "case x in x) rtk ls;; esac"),
        ("function f { ls; }", ""),
        ("coproc ls -la", ""),
        ("[[ -f x ]] && ls", "[[ -f x ]] && rtk ls"),
        ("esac", ""),
        // `time` is also a program, and the rewrite reads it as one.
        ("time ls x", "time rtk ls x"),
        // Quoted or glued to more text, the word is an ordinary command.
        ("\"if\" x", "rtk \"if\" x"),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

/// Inside `[[ … ]]` nothing is a command: `&&`/`||` there are expression
/// operators, and so are the brackets and a regex's alternatives.
#[test]
fn no_rewrite_inside_a_test_expression() {
    let dir = project();
    for (cmd, expected) in [
        ("[[ -f a || ls ]]", ""),
        ("[[ -f a && -d b ]]", ""),
        ("[[ -f a || -d b ]] || ls", "[[ -f a || -d b ]] || rtk ls"),
        (
            "[[ ( -f a || -d b ) && -e c ]] && ls",
            "[[ ( -f a || -d b ) && -e c ]] && rtk ls",
        ),
        ("[[(-f a||-d b)]] || ls", "[[(-f a||-d b)]] || rtk ls"),
        ("if [[ -f a || -d b ]]; then ls; fi", ""),
        ("while [[ -f a && -d b ]]; do ls; done", ""),
        (
            "ls && [[ -f a || -d b ]] && ls",
            "rtk ls && [[ -f a || -d b ]] && rtk ls",
        ),
        (
            "time [[ -f a || -d b ]] && ls",
            "time [[ -f a || -d b ]] && rtk ls",
        ),
        // `time`'s options `-p` and `--` leave `[[` in command position.
        ("time -p [[ -f a || -d b ]]", ""),
        ("time -- [[ -f a || -d b ]]", ""),
        (
            "time -p -- [[ -f a && -d b ]] || ls",
            "time -p -- [[ -f a && -d b ]] || rtk ls",
        ),
        ("[[ $x =~ a||b ]] && ls", "[[ $x =~ a||b ]] && rtk ls"),
        ("[[ $x =~ a&&b ]] || ls", "[[ $x =~ a&&b ]] || rtk ls"),
        ("function f { [[ -f a || -d b ]]; }", ""),
        // An unterminated `[[` runs to the end of the line.
        ("[[ -f a || ls", ""),
        // Not in command position, `[[` is a program's name or argument.
        ("a=1 [[ -f a || ls ]]", "a=1 [[ -f a || rtk ls ]]"),
        ("echo [[ -f a || ls ]]", "echo [[ -f a || rtk ls ]]"),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

/// An arithmetic command `(( … ))` runs no command: its `||`, `&&` and
/// brackets belong to the expression.
#[test]
fn no_rewrite_inside_an_arithmetic_command() {
    let dir = project();
    for (cmd, expected) in [
        ("(( a || b ))", ""),
        ("((a||b))", ""),
        ("(( a || b )) && ls", "(( a || b )) && rtk ls"),
        (
            "ls && (( x++ )) || git status",
            "rtk ls && (( x++ )) || rtk git status",
        ),
        ("time (( a || b ))", ""),
        ("time -p (( a || b ))", ""),
        ("if (( a || b )); then ls; fi", ""),
        ("( (( a || b )) && ls )", "( (( a || b )) && rtk ls )"),
        (
            "(( ( a ) || ( b ) )) && ls",
            "(( ( a ) || ( b ) )) && rtk ls",
        ),
        // Not followed at once by `)`, the `)` that closes the second `(` makes
        // the brackets two subshells, one inside the other.
        ("((a) || (b))", "((rtk a) || (rtk b))"),
        ("((ls) )", "((rtk ls) )"),
        // An unterminated `((` runs to the end of the line.
        ("(( a || b", ""),
        ("ls; (( a || b", "rtk ls; (( a || b"),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

/// A `case` pattern is never a command, and `esac` closes the `case` only
/// where a pattern would start or in command position.
#[test]
fn no_rewrite_of_a_case_pattern() {
    let dir = project();
    for (cmd, expected) in [
        (
            "case $x in a) echo hi ;; b) ls ;; esac",
            "case $x in a) echo hi ;; b) rtk ls ;; esac",
        ),
        (
            "case $x in a) echo hi ;; ls) ls ;; esac",
            "case $x in a) echo hi ;; ls) rtk ls ;; esac",
        ),
        (
            "case $x in a) echo esac ;; (ls) ls ;; esac",
            "case $x in a) echo esac ;; (ls) rtk ls ;; esac",
        ),
        (
            "case $x in a) ls;;& b) ls;& (c) ls;; esac",
            "case $x in a) rtk ls;;& b) rtk ls;& (c) rtk ls;; esac",
        ),
        (
            "case $x in a) ls; esac; ls",
            "case $x in a) rtk ls; esac; rtk ls",
        ),
        (
            "case $x in a) case $y in b) ls;; esac;; c) ls;; esac",
            "case $x in a) case $y in b) rtk ls;; esac;; c) rtk ls;; esac",
        ),
        ("case $x in esac", ""),
        (
            "case $x in a) [[ -f a || -d b ]] || ls;; esac",
            "case $x in a) [[ -f a || -d b ]] || rtk ls;; esac",
        ),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

/// After a compound command's last word (`)`, `}`, `]]`, `))`, `fi`, `done`,
/// `esac`), and after the name of a `for`, `select` or `coproc`, bash reads a
/// reserved word and no command: the `then`, `do` or `esac` there is one, and
/// a `[[ ]]` or `(( ))` behind it runs no command.
#[test]
fn reserved_word_after_a_closer() {
    let dir = project();
    for (cmd, expected) in [
        ("for x do [[ -f a || ls ]]; done", ""),
        ("select x do [[ -f a || ls ]]; done", ""),
        ("for x\ndo [[ -f a || ls ]]; done", ""),
        ("if (true) then [[ -f a || ls ]]; fi", ""),
        ("if [[ b ]] then [[ -f a || ls ]]; fi", ""),
        ("while (( 0 )) do [[ -f a || ls ]]; done", ""),
        ("until (true) do [[ -f a || ls ]]; done", ""),
        ("coproc NAME [[ -f a || ls ]]", ""),
        ("coproc NAME (( ls || x ))", ""),
        ("for x do (( ls || x )); done", ""),
        ("if (true) then (( ls || x )); fi", ""),
        (
            "for x do [[ -f a || ls ]] && ls; done",
            "for x do [[ -f a || ls ]] && rtk ls; done",
        ),
        (
            "if (true) then [[ -f a || ls ]] && git status; fi",
            "if (true) then [[ -f a || ls ]] && rtk git status; fi",
        ),
        (
            "while (( 0 )) do [[ -f a || ls ]]; done; git status",
            "while (( 0 )) do [[ -f a || ls ]]; done; rtk git status",
        ),
        (
            "for x do (( ls || x )) || ls; done",
            "for x do (( ls || x )) || rtk ls; done",
        ),
        // `esac` after a subshell or another compound command closes the
        // `case`.
        (
            "case x in x) (ls) esac; [[ -f a || ls ]] || git status",
            "case x in x) (rtk ls) esac; [[ -f a || ls ]] || rtk git status",
        ),
        (
            "case x in x) case y in y) ls;; esac esac; [[ -f a || ls ]]",
            "case x in x) case y in y) rtk ls;; esac esac; [[ -f a || ls ]]",
        ),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

/// Only command text pipes or groups commands: a `case` pattern's `|` and
/// brackets, and a `[[ ]]` regex's, leave the line to the rewrite, while a
/// subshell next to a pipe still keeps it as written.
#[test]
fn pattern_and_regex_brackets_group_nothing() {
    let dir = project();
    for (cmd, expected) in [
        (
            "[[ $y =~ ^(a|b)$ ]] && git status",
            "[[ $y =~ ^(a|b)$ ]] && rtk git status",
        ),
        (
            "[[ $y =~ (a|b) ]] && git status | head",
            "[[ $y =~ (a|b) ]] && rtk git status | head",
        ),
        (
            "case $y in a|b) git status;; esac",
            "case $y in a|b) rtk git status;; esac",
        ),
        (
            "case $y in (a|b) git status | head;; esac",
            "case $y in (a|b) rtk git status | head;; esac",
        ),
        (
            "case $y in a|b) git status;; c) ls | head;; esac",
            "case $y in a|b) rtk git status;; c) rtk ls | head;; esac",
        ),
        ("(ls) | head", ""),
        ("ls | (head)", ""),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

/// Word text is one word's, never a command: an extglob group, an array
/// literal and a `${ }` keep their text as written, where an `rtk` inside
/// one changes the array, the pattern or the default value.
#[test]
fn no_rewrite_inside_word_text() {
    let dir = project();
    for (cmd, expected) in [
        ("arr=(a)", ""),
        ("x=(git status)", ""),
        ("arr=(a b) git status", "arr=(a b) rtk git status"),
        ("x=(git status) && ls", "x=(git status) && rtk ls"),
        ("ls a!(b) c", "rtk ls a!(b) c"),
        ("ls x@(a;b)&&git status", "rtk ls x@(a;b)&&rtk git status"),
        ("ls ?((ls))", "rtk ls ?((ls))"),
        ("ls !(a) && git status", "rtk ls !(a) && rtk git status"),
        ("echo !(git status)", ""),
        (
            "case $x in !(a)) git status;; esac",
            "case $x in !(a)) rtk git status;; esac",
        ),
        ("ls ${x-a)b} && ls", "rtk ls ${x-a)b} && rtk ls"),
        (
            "ls ${x-;ls }; git status",
            "rtk ls ${x-;ls }; rtk git status",
        ),
        (
            "x=(${y-a b} c) && git status",
            "x=(${y-a b} c) && rtk git status",
        ),
        (
            "case ${x-a} in a) ls;; esac",
            "case ${x-a} in a) rtk ls;; esac",
        ),
        // In a `case` pattern or a `[[ ]]` expression, a `)`, a `|` or a
        // `]]` inside a `${ }` ends nothing.
        (
            "case $x in ${y:-a)b}) ls;; esac",
            "case $x in ${y:-a)b}) rtk ls;; esac",
        ),
        (
            "case $x in ${y:-a|b}) ls;; esac",
            "case $x in ${y:-a|b}) rtk ls;; esac",
        ),
        (
            "case $x in ${y-a) ls;; b}) git status;; esac",
            "case $x in ${y-a) ls;; b}) rtk git status;; esac",
        ),
        (
            "case $x in ${y-a)b} | c) git status;; ${y-d)}) ls;; esac",
            "case $x in ${y-a)b} | c) rtk git status;; ${y-d)}) rtk ls;; esac",
        ),
        ("[[ ${x- ]] } == a || ls ]]", ""),
        (
            "[[ ${x-)} == a || ls ]] && ls",
            "[[ ${x-)} == a || ls ]] && rtk ls",
        ),
        (
            "[[ ${x-a ]] || ls } ]] && git status",
            "[[ ${x-a ]] || ls } ]] && rtk git status",
        ),
        (
            "[[ $x == @(a|${y- ]] }) || ls ]] && ls",
            "[[ $x == @(a|${y- ]] }) || ls ]] && rtk ls",
        ),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}

/// `time` is reserved only where a pipeline starts: after `|` or `|&` it is
/// the program `time`, so a `[[` after it is its argument and `||` ends it.
#[test]
fn time_after_a_pipe_is_a_program() {
    let dir = project();
    for (cmd, expected) in [
        (
            "ls | time [[ -f a || ls ]]",
            "ls | time [[ -f a || rtk ls ]]",
        ),
        (
            "ls |& time [[ -f a || ls ]]",
            "ls |& time [[ -f a || rtk ls ]]",
        ),
        (
            "ls | time -p [[ -f a || ls ]]",
            "ls | time -p [[ -f a || rtk ls ]]",
        ),
        (
            "ls && time [[ -f a || ls ]] && ls",
            "rtk ls && time [[ -f a || ls ]] && rtk ls",
        ),
        (
            "time [[ -f a || ls ]] || ls",
            "time [[ -f a || ls ]] || rtk ls",
        ),
        // The stage runs the program `time`, which no built-in rule takes,
        // and a filter rewrites no stage after a pipe.
        ("ls | time git status", ""),
        ("ls | time -p git status", ""),
        ("ls |& time git status", ""),
        ("git log | time head", ""),
    ] {
        assert_eq!(rewrite(dir.path(), cmd), expected, "{cmd:?}");
    }
}
