#![cfg(unix)]
//! For un-capped searches `rtk grep X` must be byte-identical to `grep -n X`:
//! line number always, filename only when grep itself prints one. Covers content
//! and regex edge cases (colons, digits, shell metachars, unicode, colon-in-path)
//! that must never be misparsed.

use std::io::Write;
use std::process::{Command, Stdio};

fn rtk_grep(args: &[&str]) -> (String, Option<i32>) {
    let mut a = vec!["grep"];
    a.extend_from_slice(args);
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(&a)
        .output()
        .expect("rtk");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code(),
    )
}

fn grep_n(args: &[&str]) -> (String, Option<i32>) {
    let mut a = vec!["-n"];
    a.extend_from_slice(args);
    let out = Command::new("grep").args(&a).output().expect("grep");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code(),
    )
}

fn assert_eq_grep_n(args: &[&str]) {
    let (rtk, rc) = rtk_grep(args);
    let (grep, gc) = grep_n(args);
    assert_eq!(rtk, grep, "stdout mismatch for {args:?}");
    assert_eq!(rc, gc, "exit code mismatch for {args:?}");
}

fn write(dir: &std::path::Path, name: &str, body: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, body).expect("write");
    p.to_str().unwrap().to_string()
}

#[test]
fn single_multi_recursive_and_h_match_grep_n() {
    let d = tempfile::tempdir().unwrap();
    let f1 = write(d.path(), "f1.txt", "apple\nzebra apple\nbanana\n");
    let f2 = write(d.path(), "f2.txt", "apricot\n");
    assert_eq_grep_n(&["apple", &f1]); // single: position only
    assert_eq_grep_n(&["a", &f1, &f2]); // multi: file + position
    assert_eq_grep_n(&["-H", "apple", &f1]); // -H forces filename on a single file
    assert_eq_grep_n(&["-r", "a", d.path().to_str().unwrap()]); // recursive
}

#[test]
fn no_match_matches_grep_n() {
    let d = tempfile::tempdir().unwrap();
    let f = write(d.path(), "f.txt", "hello\n");
    assert_eq_grep_n(&["zzz_no_match_xyz", &f]); // empty stdout, exit 1
}

#[test]
fn nasty_content_is_not_misparsed() {
    let d = tempfile::tempdir().unwrap();
    let f = write(
        d.path(),
        "n.txt",
        "12:34 looks like a line number\na::b::c ClassRegistry::init('x')\nport :8080: here\n$(rm -rf /) `whoami` && echo\n中文 テスト مرحبا\n",
    );
    assert_eq_grep_n(&[":", &f]);
    assert_eq_grep_n(&["ClassRegistry", &f]);
    assert_eq_grep_n(&["中文", &f]);
    assert_eq_grep_n(&["whoami", &f]);
}

#[test]
fn regex_metacharacters_match_grep_n() {
    let d = tempfile::tempdir().unwrap();
    let f = write(d.path(), "r.txt", "a.b\naxb\nfoo.bar\n[x]\n");
    assert_eq_grep_n(&["a.b", &f]); // . is any-char
    assert_eq_grep_n(&["-F", "a.b", &f]); // fixed-string
    assert_eq_grep_n(&["^foo", &f]); // anchor
    assert_eq_grep_n(&["-E", "ax?b", &f]); // ERE
}

#[test]
fn context_flags_match_grep_n() {
    let d = tempfile::tempdir().unwrap();
    let f = write(d.path(), "c.txt", "x\nMATCH\ny\nz\n");
    assert_eq_grep_n(&["-A1", "MATCH", &f]);
    assert_eq_grep_n(&["-B1", "MATCH", &f]);
    assert_eq_grep_n(&["-C1", "MATCH", &f]);
}

#[test]
fn context_group_separator_matches_grep_n() {
    let d = tempfile::tempdir().unwrap();
    let f = write(
        d.path(),
        "sep.txt",
        "match A\nfill1\nfill2\nfill3\nmatch B\n",
    );
    assert_eq_grep_n(&["-A1", "match", &f]);
}

// #1436: a single-file `-n` search with `::` content and a BRE pattern with
// literal `()` must equal grep exactly — no broadened matches, and no synthetic
// `[file]` bucket from splitting on `::`.
#[test]
fn issue_1436_colons_and_literal_parens() {
    let d = tempfile::tempdir().unwrap();
    let f = write(
        d.path(),
        "shell.php",
        "use Util\\ClassRegistry;\nclass Foo {\n    public function run() {\n        $this->m = ClassRegistry::init('Collections.QueueProcess');\n        try {\n            deleteRow($id);\n            $this->delete();\n        } catch (\\Exception $e) {\n        } finally {}\n    }\n    private function delete() {}\n}\n",
    );
    assert_eq_grep_n(&[
        "private function delete\\|try\\|catch\\|finally\\|delete()",
        &f,
    ]);
    assert_eq_grep_n(&["init", &f]); // ClassRegistry::init must stay one intact line
}

// #1436 (comments): a leading `^` anchor must stay scoped to the given file
// (tenequm), and a line ending in `:` (Python `if x:`) must keep full content
// (BTCAlchemist).
#[test]
fn issue_1436_anchor_scope_and_trailing_colon() {
    let d = tempfile::tempdir().unwrap();
    let f = write(d.path(), "code.rs", "pub fn a\nprivate b\npub fn c\n");
    let py = write(
        d.path(),
        "t.py",
        "x = 1\nif crypto >= MAX_CRYPTO:\nclass Foo:\n",
    );
    assert_eq_grep_n(&["^pub", &f]); // anchored pattern must not escape the path
    assert_eq_grep_n(&["MAX_CRYPTO", &py]); // trailing-colon line kept intact
    assert_eq_grep_n(&["class", &py]);
}

#[test]
fn colon_in_filename_matches_grep_n() {
    let d = tempfile::tempdir().unwrap();
    let f1 = write(d.path(), "weird:name.txt", "hit\n");
    let f2 = write(d.path(), "other.txt", "hit\n");
    assert_eq_grep_n(&["hit", &f1, &f2]); // colon in a path must not fool the parser
}

#[test]
fn case_insensitive_and_invert_match_grep_n() {
    let d = tempfile::tempdir().unwrap();
    let f = write(d.path(), "i.txt", "Apple\nbanana\nAPPLE\n");
    assert_eq_grep_n(&["-i", "apple", &f]);
    assert_eq_grep_n(&["-v", "apple", &f]);
}

#[test]
fn piped_stdin_matches_grep_n() {
    let input = "apple\nzebra\napple pie\n";
    let feed = |cmd: &mut Command| {
        let mut c = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");
        c.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        String::from_utf8_lossy(&c.wait_with_output().unwrap().stdout).into_owned()
    };
    let rtk = feed(Command::new(env!("CARGO_BIN_EXE_rtk")).args(["grep", "apple"]));
    let grep = feed(Command::new("grep").args(["-n", "apple"]));
    assert_eq!(rtk, grep, "piped stdin must equal grep -n");
}
