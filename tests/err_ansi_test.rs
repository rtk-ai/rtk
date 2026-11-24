#![cfg(unix)]

mod common;

#[test]
fn colored_compiler_diagnostic_keeps_message_location_and_exit_status() {
    // Captured with rustc 1.95.0 --color=always --emit=metadata ansi_error.rs.
    let raw = include_str!("fixtures/rustc_ansi_error_raw.txt");
    let dir = common::tempdir();
    let fixture = dir.path().join("compiler output.txt");
    std::fs::write(&fixture, raw).expect("write compiler fixture");
    let output = common::rtk_command()
        .args(["err", "sh", "-c", "cat \"$1\" >&2; exit 7", "fixture"])
        .arg(&fixture)
        .output()
        .expect("run err");
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(shown.contains("error[E0308]: mismatched types"), "{shown}");
    assert!(shown.contains("--> ansi_error.rs:2:24"), "{shown}");
    assert!(!shown.contains('\x1b'), "{shown:?}");
    assert!(shown.len() * 5 <= raw.len() * 4, "{shown}");
}
