//! A line over `[limits] max_line_chars` is cut on every `rtk read` path: the
//! raw-byte ones (a plain read, a head or tail window, stdin) as well as the
//! decoded one a `--max-lines` read takes.

use std::fs;
use std::io::Write;
use std::process::{Output, Stdio};

mod common;

fn read_stdin(input: &[u8]) -> Output {
    let mut child = common::rtk_command()
        .args(["read", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rtk read from stdin");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input)
        .expect("write stdin");
    child.wait_with_output().expect("wait for rtk read")
}

#[test]
fn read_cuts_a_long_line_on_every_path() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("records.jsonl");
    let input = format!("{}\nshort\n", "x".repeat(3000));
    fs::write(&file, &input).expect("write test file");

    let mut outputs = Vec::new();
    for args in [
        &[][..],
        &["--head-lines", "2"],
        &["--tail-lines", "2"],
        &["--max-lines", "5"],
    ] {
        let output = common::rtk_command()
            .arg("read")
            .arg(&file)
            .args(args)
            .output()
            .expect("run rtk read");
        outputs.push((format!("{args:?}"), output));
    }
    outputs.push(("stdin".to_string(), read_stdin(input.as_bytes())));

    for (what, output) in outputs {
        assert!(output.status.success(), "{what}: {:?}", output.stderr);
        let stdout = String::from_utf8(output.stdout).expect("utf-8 output");
        let (first, rest) = stdout.split_once('\n').expect("two lines");
        assert!(first.starts_with(&"x".repeat(2000)), "{what}");
        assert!(
            first[2000..].starts_with("… [+1000 chars] "),
            "{what}: {}",
            &first[2000..]
        );
        assert_eq!(rest, "short\n", "{what}");
    }
}

#[test]
fn read_leaves_a_long_line_that_is_not_utf8_alone() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("binary.log");
    let mut input = b"\xff".to_vec();
    input.extend(vec![b'y'; 3000]);
    input.push(b'\n');
    fs::write(&file, &input).expect("write test file");

    let output = common::rtk_command()
        .arg("read")
        .arg(&file)
        .output()
        .expect("run rtk read");

    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, input);
}
