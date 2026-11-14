#![cfg(unix)]
//! Observe the process boundary, not just the rewrite string.
mod common;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;

fn executable(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create bin");
    std::fs::write(path, body).expect("write executable");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[test]
fn rewritten_module_uses_named_interpreter_and_preserves_arguments() {
    let dir = common::tempdir();
    let python = dir.path().join("venv 'quoted'/bin/python3.11");
    let record = dir.path().join("record");
    executable(
        &python,
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$RECORD\"\nprintf '%s' \"$PWD|$CUSTOM\" > \"$CONTEXT\"\nprintf x >> \"$COUNT\"\nprintf '================ test session starts ================\\ncollected 1 item\\ntest_sample.py F [100%%]\\n================ 1 failed in 0.01s ================\\n'\nexit 7\n",
    );
    let wrong = dir.path().join("other/pytest");
    executable(&wrong, "#!/bin/sh\nprintf wrong > \"$RECORD\"\nexit 0\n");
    let script = format!("{} -m pytest -- '-dash.py' 'a b' ''", quote(&python));
    let rewrite = common::rtk_command()
        .args(["rewrite", &script])
        .output()
        .expect("rewrite");
    assert_eq!(rewrite.status.code(), Some(3), "permission remains ask");
    let rewritten = String::from_utf8(rewrite.stdout).expect("UTF8");
    assert!(rewritten.starts_with("rtk run --filter pytest -- "));
    let isolated_rtk = common::rtk_command();
    let path = std::env::join_paths([
        Path::new(isolated_rtk.get_program())
            .parent()
            .expect("rtk bin"),
        wrong.parent().expect("wrong bin"),
        Path::new("/usr/bin"),
        Path::new("/bin"),
    ])
    .expect("PATH");
    let mut shell = common::native_command("sh");
    common::isolate_rtk(&mut shell);
    let out = shell
        .args(["-c", rewritten.trim()])
        .current_dir(dir.path())
        .env("PATH", path)
        .env("RECORD", &record)
        .env("CUSTOM", "kept value")
        .env("CONTEXT", dir.path().join("context"))
        .env("COUNT", dir.path().join("count"))
        .output()
        .expect("run rewrite");
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(
        std::fs::read(record).expect("args"),
        b"-m\0pytest\0--\0-dash.py\0a b\0\0"
    );
    assert_eq!(
        std::fs::read(dir.path().join("count")).expect("count"),
        b"x"
    );
    // macOS may expose the physical /private path through PWD.
    let context = std::fs::read_to_string(dir.path().join("context")).expect("context");
    let (cwd, custom) = context.split_once('|').expect("two fields");
    assert_eq!(
        Path::new(cwd).canonicalize().expect("cwd"),
        dir.path().canonicalize().expect("dir")
    );
    assert_eq!(custom, "kept value");
    assert!(String::from_utf8_lossy(&out.stdout).contains("1 failed"));
}

#[test]
fn filtered_run_preserves_stdin_and_unknown_output() {
    let dir = common::tempdir();
    let python = dir.path().join("python");
    executable(
        &python,
        "#!/bin/sh\nIFS= read -r line\nprintf 'help: %s\\n' \"$line\"\nprintf 'module diagnostic\\n' >&2\nexit 4\n",
    );
    let mut child = common::rtk_command()
        .args(["run", "--filter", "pytest", "--"])
        .arg(&python)
        .args(["-m", "pytest", "--help"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"read exactly once\n")
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&out.stdout).contains("help: read exactly once"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("module diagnostic"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("No tests collected"));
}

#[test]
fn missing_interpreter_is_not_replaced_by_another_python() {
    let dir = common::tempdir();
    let out = common::rtk_command()
        .args(["run", "--filter", "pytest", "--"])
        .arg(dir.path().join("absent/bin/python"))
        .args(["-m", "pytest"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(127));
    assert!(!out.stderr.is_empty());
}

#[test]
fn filtered_run_keeps_the_relative_executable_spelling() {
    let dir = common::tempdir();
    executable(
        &dir.path().join("venv/python"),
        "#!/bin/sh\nprintf '%s' \"$0\" > \"$RECORD\"\nprintf 'help text\\n'\n",
    );
    let record = dir.path().join("record");
    let original = "./venv/../venv/python";
    let out = common::rtk_command()
        .current_dir(dir.path())
        .env("RECORD", &record)
        .args(["run", "--filter", "pytest", "--", original, "-m", "pytest"])
        .output()
        .expect("run");
    assert!(out.status.success());
    assert_eq!(std::fs::read_to_string(record).expect("program"), original);
}

#[test]
fn module_rewrite_checks_permissions_on_the_original_command() {
    for (decision, expected_code) in [("deny", 2), ("ask", 3), ("allow", 0)] {
        let dir = common::tempdir();
        let settings = dir.path().join(".claude");
        std::fs::create_dir(&settings).expect("settings directory");
        std::fs::write(
            settings.join("settings.json"),
            format!(r#"{{"permissions":{{"{decision}":["Bash(python -m pytest *)"]}}}}"#),
        )
        .expect("permission settings");
        let out = common::rtk_command()
            .current_dir(dir.path())
            .args(["rewrite", "python -m pytest test_sample.py"])
            .output()
            .expect("rewrite");
        assert_eq!(out.status.code(), Some(expected_code), "{decision}");
        if decision == "deny" {
            assert!(out.stdout.is_empty());
        } else {
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim(),
                "rtk run --filter pytest -- python -m pytest test_sample.py"
            );
        }
    }
}
