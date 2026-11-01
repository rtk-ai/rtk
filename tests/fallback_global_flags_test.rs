//! Exercise the real fallback dispatcher, not just the argv classifier.
//! A PATH stub makes it observable whether the child ran and which args it received.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::process::Output;

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn run_stub(args: &[&str], child_exit: i32) -> (Output, bool) {
    let dir = common::tempdir();
    let stub = dir.path().join("rtk-fallback-probe");
    let marker = dir.path().join("stub-was-run");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\nprintf 'called\\n' > {}\nprintf '%s\\n' \"$@\"\nexit {}\n",
            shell_quote(&marker),
            child_exit
        ),
    )
    .expect("write fallback stub");
    let mut permissions = std::fs::metadata(&stub).expect("stat stub").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&stub, permissions).expect("chmod stub");

    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = common::rtk_command()
        .current_dir(dir.path())
        .env("PATH", path)
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .expect("spawn rtk");
    (output, marker.exists())
}

#[test]
fn fallback_dispatch_strips_only_leading_rtk_flags() {
    let (output, ran) = run_stub(
        &["-v", "--skip-env", "rtk-fallback-probe", "-v", "x"],
        0,
    );
    assert!(ran, "stub not invoked: {output:?}");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "-v\nx\n");
}

#[test]
fn fallback_dispatch_propagates_child_exit_code() {
    let (output, ran) = run_stub(&["--verbose", "rtk-fallback-probe"], 7);
    assert!(ran, "stub not invoked: {output:?}");
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn fallback_dispatch_rejects_removed_flag_without_running_child() {
    let (output, ran) = run_stub(&["--ultra-compact", "rtk-fallback-probe", "x"], 0);
    assert!(!ran, "removed flag must not dispatch a child");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument '--ultra-compact'"), "{stderr}");
    assert!(stderr.contains("Usage: rtk"), "{stderr}");
}
