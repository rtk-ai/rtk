//! `rtk read` defaults to `--level none`, which is the right default for source
//! code. These tests pin the difference between "do not filter" and "do not
//! bound": ordinary files must come back byte for byte, while a single absurd
//! line or a runaway total must be cut with a hint that recovers the rest.

use std::process::Command;

fn rtk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

fn write_temp(name: &str, content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    std::fs::write(&path, content).expect("write");
    (dir, path)
}

fn read(path: &std::path::Path) -> String {
    let out = rtk()
        .args(["read", path.to_str().unwrap()])
        .output()
        .expect("rtk read");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn ordinary_source_file_is_returned_unchanged() {
    let source = (0..400)
        .map(|i| format!("fn function_number_{i}() -> usize {{ {i} }}"))
        .collect::<Vec<_>>()
        .join("\n");
    let content = format!("{source}\n");
    let (_dir, path) = write_temp("ordinary.rs", &content);

    assert_eq!(
        read(&path),
        content,
        "a normal source file must not be touched"
    );
}

#[test]
fn absurd_line_is_cut_and_says_how_to_recover_it() {
    let content = format!("const DATA: &str = \"{}\";\nfn main() {{}}\n", "x".repeat(500_000));
    let (_dir, path) = write_temp("generated.rs", &content);

    let out = read(&path);

    assert!(
        out.len() < 10_000,
        "a 500 000-char line must be cut, got {} bytes",
        out.len()
    );
    assert!(
        out.contains("fn main() {}"),
        "the rest of the file must survive:\n{out}"
    );
    assert!(
        out.contains("rtk proxy cat"),
        "the cut must name the way back to the full text:\n{out}"
    );
}

#[test]
fn runaway_total_is_cut_and_says_how_to_read_on() {
    // Comfortably past the 400 000-char default ceiling, in ordinary short lines.
    let content = (0..40_000)
        .map(|i| format!("line {i} of a very long generated file"))
        .collect::<Vec<_>>()
        .join("\n");
    let (_dir, path) = write_temp("huge.log", &format!("{content}\n"));

    let out = read(&path);

    assert!(
        out.len() < content.len() / 2,
        "a runaway file must be cut, got {} of {} bytes",
        out.len(),
        content.len()
    );
    assert!(
        out.contains("more lines"),
        "the cut must report how many lines it dropped:\n{}",
        &out[out.len().saturating_sub(400)..]
    );
    assert!(
        out.contains("--tail-lines"),
        "the cut must name the command that reads on:\n{}",
        &out[out.len().saturating_sub(400)..]
    );
}

#[test]
fn empty_and_tiny_files_are_untouched() {
    let (_dir, empty) = write_temp("empty.txt", "");
    assert_eq!(read(&empty), "", "an empty file stays empty");

    let (_dir2, tiny) = write_temp("tiny.txt", "one line\n");
    assert_eq!(read(&tiny), "one line\n", "a tiny file stays byte for byte");
}
