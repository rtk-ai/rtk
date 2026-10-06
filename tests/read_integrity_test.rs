use std::io::Write;
use std::process::{Command, Output, Stdio};

const GUARD_SOURCE: &str = r#"fn can_admin(user: &User) -> bool {
    if !user.is_admin {
        return false;
    }
    if user.is_banned {
        return false;
    }
    if user.mfa_ok {
        return true;
    }
    false
}
fn wipe_database() {
    drop_all();
}
"#;

fn read_stdin(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("read")
        .arg("-")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk read");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait for rtk read")
}

#[test]
fn truncation_marks_removed_guard_body_at_the_gap() {
    let output = read_stdin(&["--max-lines", "10"], GUARD_SOURCE);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("    if user.is_banned {\n[1 line omitted]\n    }"));
    assert!(!stdout.contains("    if user.is_banned {\n    }"));
    assert!(stdout.lines().count() <= 10);
}

#[test]
fn numbered_truncation_uses_source_line_numbers() {
    let output = read_stdin(&["--max-lines", "10", "--line-numbers"], GUARD_SOURCE);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(" 5 │     if user.is_banned {"));
    assert!(stdout.contains("   │ [1 line omitted]"));
    assert!(stdout.contains(" 7 │     }"));
    assert!(!stdout.contains(" 6 │     }"));
}

#[test]
fn stdin_falls_back_when_filter_removes_nonempty_input() {
    let input = "// BUILD FAILED\n// error: cannot find symbol at Main.java:42\n// 3 errors\n";
    let output = read_stdin(&["--level", "minimal"], input);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), input);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("filter produced empty output for stdin")
    );
}

#[test]
fn zero_line_window_is_empty_without_panicking() {
    let output = read_stdin(&["--max-lines", "0"], "nonempty\n");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}
