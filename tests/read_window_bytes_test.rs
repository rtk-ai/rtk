use std::fs;
use std::io::Write;
use std::process::{Output, Stdio};

mod common;

fn read_stdin(input: &[u8], args: &[&str]) -> Output {
    let mut child = common::rtk_command()
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
        let output = common::rtk_command()
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
            let from_file = common::rtk_command()
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
fn plain_reads_pass_non_utf8_bytes_through_for_files_and_stdin() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("latin1.log");
    for input in [
        b"line one\nlatin-1 byte: \xe9\nline three\n".as_slice(),
        b"\xff\r\nvalid\n\0\xfe".as_slice(),
    ] {
        fs::write(&file, input).expect("write non-UTF-8 file");
        let from_file = common::rtk_command()
            .arg("read")
            .arg(&file)
            .output()
            .expect("run rtk read");
        let from_stdin = read_stdin(input, &[]);
        for output in [from_file, from_stdin] {
            assert!(output.status.success(), "{:?}", output.stderr);
            assert_eq!(output.stdout, input);
        }
    }
}

#[test]
fn text_options_still_report_non_utf8_input() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("latin1.log");
    let input = b"line one\nlatin-1 byte: \xe9\nline three\n";
    fs::write(&file, input).expect("write non-UTF-8 file");
    let valid_file = dir.path().join("valid.log");
    let valid = b"line one\nascii\nline three\n";
    fs::write(&valid_file, valid).expect("write valid file");
    for args in [
        ["-l", "minimal"].as_slice(),
        ["--level", "aggressive"].as_slice(),
        ["--line-numbers"].as_slice(),
        ["--max-lines", "2"].as_slice(),
    ] {
        let from_file = common::rtk_command()
            .arg("read")
            .arg(&file)
            .args(args)
            .output()
            .expect("run rtk read");
        let from_stdin = read_stdin(input, args);
        for (source, output) in [
            (file.display().to_string(), from_file),
            ("-".into(), from_stdin),
        ] {
            assert_eq!(output.status.code(), Some(1), "{args:?} on {source}");
            assert!(output.stdout.is_empty(), "{args:?}: {:?}", output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains(&format!("cat: {source}: ")),
                "{args:?}: {stderr}"
            );
        }
        // The same options accept valid input, so the failure above is the decode.
        let from_valid_file = common::rtk_command()
            .arg("read")
            .arg(&valid_file)
            .args(args)
            .output()
            .expect("run rtk read");
        for output in [from_valid_file, read_stdin(valid, args)] {
            assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
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
        let rewrite = common::rtk_command()
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
        let actual = common::rtk_command()
            .args(
                rewritten
                    .trim()
                    .strip_prefix("rtk ")
                    .expect("rtk rewrite prefix")
                    .split_whitespace(),
            )
            .output()
            .expect("execute rewritten head");
        let native = common::native_command("head")
            .args(flags.split_whitespace())
            .arg(&file)
            .output()
            .expect("run native head");
        assert!(native.status.success(), "{command}: {:?}", native.stderr);
        assert_eq!(actual.status.code(), native.status.code(), "{command}");
        assert_eq!(actual.stdout, native.stdout, "{command}");
    }
}
