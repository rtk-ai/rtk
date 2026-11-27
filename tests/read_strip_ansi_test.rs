use std::io::Write;
use std::process::Stdio;

mod common;

#[test]
fn ansi_cleanup_is_opt_in_for_files_stdin_and_line_windows() {
    let dir = common::tempdir();
    let file = dir.path().join("terminal output.txt");
    let raw = "\x1b[31mfirst\x1b[0m\r\n\x1b[Ksecond\n\x1b[1;32mété\x1b[0m";
    std::fs::write(&file, raw).expect("write fixture");
    let plain = common::rtk_command()
        .arg("read")
        .arg(&file)
        .output()
        .expect("read");
    assert!(plain.status.success());
    assert_eq!(plain.stdout, raw.as_bytes());

    for (args, expected) in [
        (vec![], "first\r\nsecond\nété"),
        (vec!["--head-lines", "1"], "first\r\n"),
        (vec!["--tail-lines", "1"], "été"),
        (vec!["--head-lines", "0"], ""),
        (vec!["--line-numbers"], "1\tfirst\n2\tsecond\n3\tété\n"),
    ] {
        for stdin in [false, true] {
            let mut command = common::rtk_command();
            command
                .arg("read")
                .arg("--strip-ansi")
                .args(&args)
                .arg(if stdin {
                    std::path::Path::new("-")
                } else {
                    &file
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if stdin {
                command.stdin(Stdio::piped());
            }
            let mut child = command.spawn().expect("spawn read");
            if stdin {
                child
                    .stdin
                    .take()
                    .expect("stdin")
                    .write_all(raw.as_bytes())
                    .expect("write stdin");
            }
            let output = child.wait_with_output().expect("read output");
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                expected,
                "{args:?}, stdin={stdin}"
            );
        }
    }
}

#[test]
fn ansi_cleanup_precedes_language_filtering_and_handles_empty_results() {
    let dir = common::tempdir();
    let file = dir.path().join("source.js");
    std::fs::write(
        &file,
        "\x1b[32m// comment\x1b[0m\n\x1b[31mconst result = 3;\x1b[0m\n",
    )
    .expect("write source");
    let output = common::rtk_command()
        .args(["read", "--strip-ansi", "--level", "minimal"])
        .arg(&file)
        .output()
        .expect("read source");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"const result = 3;");
    std::fs::write(&file, "\x1b[0m\x1b[K").expect("write controls");
    let output = common::rtk_command()
        .args(["read", "--strip-ansi"])
        .arg(&file)
        .output()
        .expect("read controls");
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
}
