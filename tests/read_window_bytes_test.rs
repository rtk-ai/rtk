use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn read_stdin(input: &[u8], args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["read", "-"])
        .args(args)
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
fn read_windows_preserve_non_utf8_files() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("binary.log");
    fs::write(&file, b"\xff\xfe bad\nline2\nline3\n").expect("write binary file");

    for (flag, expected) in [
        ("--head-lines", b"\xff\xfe bad\nline2\n".as_slice()),
        ("--tail-lines", b"line2\nline3\n".as_slice()),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .arg("read")
            .arg(&file)
            .args([flag, "2"])
            .output()
            .expect("run rtk read");

        assert!(output.status.success(), "{flag}: {:?}", output.stderr);
        assert_eq!(output.stdout, expected, "{flag}");
    }
}

#[test]
fn read_windows_preserve_binary_boundaries_for_files_and_stdin() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("binary.log");
    for input in [
        b"\xff\r\nvalid\n\0\xfe".as_slice(),
        b"\xff\r\nvalid\n\0\xfe\n".as_slice(),
    ] {
        fs::write(&file, input).expect("write binary file");
        let last = if input.ends_with(b"\n") {
            b"\0\xfe\n".as_slice()
        } else {
            b"\0\xfe".as_slice()
        };
        for (flag, count, expected) in [
            ("--head-lines", "0", b"".as_slice()),
            ("--tail-lines", "0", b"".as_slice()),
            ("--head-lines", "1", b"\xff\r\n".as_slice()),
            ("--tail-lines", "1", last),
            ("--head-lines", "99", input),
            ("--tail-lines", "99", input),
        ] {
            let from_file = Command::new(env!("CARGO_BIN_EXE_rtk"))
                .arg("read")
                .arg(&file)
                .args([flag, count])
                .output()
                .expect("run rtk read");
            let from_stdin = read_stdin(input, &[flag, count]);
            for output in [from_file, from_stdin] {
                assert!(
                    output.status.success(),
                    "{flag} {count}: {:?}",
                    output.stderr
                );
                assert_eq!(output.stdout, expected, "{flag} {count}");
            }
        }
    }
}

#[test]
fn read_windows_accept_invalid_bytes_outside_the_selected_window() {
    for (input, flag) in [
        (b"valid\n\xff\n".as_slice(), "--head-lines"),
        (b"\xff\nvalid\n".as_slice(), "--tail-lines"),
    ] {
        let output = read_stdin(input, &[flag, "1"]);
        assert!(output.status.success(), "{flag}: {:?}", output.stderr);
        assert_eq!(output.stdout, b"valid\n");
    }
}

#[test]
fn read_windows_still_apply_text_filtering_and_line_numbers() {
    let input = b"// comment\nfn alpha() {}\nfn beta() {}\nfn gamma() {}\n";
    for (args, expected) in [
        (
            ["--head-lines", "1", "--level", "minimal"].as_slice(),
            "fn alpha() {}\n",
        ),
        (
            ["--tail-lines", "1", "--level", "minimal"].as_slice(),
            "fn gamma() {}",
        ),
        (
            ["--head-lines", "1", "--line-numbers"].as_slice(),
            "1 │ // comment\n",
        ),
        (
            ["--tail-lines", "1", "--line-numbers"].as_slice(),
            "1 │ fn gamma() {}\n",
        ),
    ] {
        let output = read_stdin(input, args);
        assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        assert_eq!(output.stdout, expected.as_bytes(), "{args:?}");
    }
}

#[test]
fn read_stdin_python_minimal_strips_comments() {
    let input = b"import glob\nfiles = glob.glob(\"src/**/*.py\")\n# c\nx = 1\n";
    let output = read_stdin(input, &["--level", "minimal"]);

    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        output.stdout,
        b"import glob\nfiles = glob.glob(\"src/**/*.py\")\nx = 1"
    );
}

#[test]
fn read_stdin_python_shebang_uses_python_filter() {
    let input = b"#!/usr/bin/env python3\n# c\nx = 1\n";
    let output = read_stdin(input, &["--level", "minimal"]);

    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"x = 1");
}

#[cfg(unix)]
#[test]
fn rewritten_head_spellings_match_native_on_non_utf8_files() {
    let dir = tempfile::tempdir().expect("create test directory");
    let claude_dir = dir.path().join(".claude");
    fs::create_dir(&claude_dir).expect("create isolated Claude config directory");
    let config_dir = dir.path().join("config");
    fs::create_dir(&config_dir).expect("create isolated rtk config directory");
    let file = dir.path().join("binary.log");
    fs::write(&file, b"\xff\xfe bad\nline2\nline3\n").expect("write binary file");
    for flags in ["-2", "-n 2", "--lines 2", "--lines=2", ""] {
        let command = format!("head {flags} {}", file.display());
        let rewrite = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .current_dir(dir.path())
            .env("CLAUDE_CONFIG_DIR", &claude_dir)
            .env("XDG_CONFIG_HOME", &config_dir)
            .args(["rewrite", &command])
            .output()
            .expect("rewrite head command");
        assert_eq!(
            rewrite.status.code(),
            Some(3),
            "{command}: {:?}",
            rewrite.stderr
        );
        let rewritten = String::from_utf8(rewrite.stdout).expect("UTF-8 command");
        let count = if flags.is_empty() { "10" } else { "2" };
        assert_eq!(
            rewritten.trim(),
            format!("rtk read {} --head-lines {count}", file.display())
        );
        let actual = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(
                rewritten
                    .trim()
                    .strip_prefix("rtk ")
                    .expect("rtk rewrite prefix")
                    .split_whitespace(),
            )
            .output()
            .expect("execute rewritten head");
        let native = Command::new("head")
            .args(flags.split_whitespace())
            .arg(&file)
            .output()
            .expect("run native head");
        assert!(native.status.success(), "{command}: {:?}", native.stderr);
        assert_eq!(actual.status.code(), native.status.code(), "{command}");
        assert_eq!(actual.stdout, native.stdout, "{command}");
    }
}
