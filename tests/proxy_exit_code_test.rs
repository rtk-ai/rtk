use std::process::Command;

#[test]
#[cfg(windows)]
fn proxy_preserves_child_nonzero_exit_code() {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["proxy", "cmd", "/d", "/c", "exit", "7"])
        .output()
        .expect("run rtk proxy");

    assert_eq!(
        output.status.code(),
        Some(7),
        "proxy must preserve the child process exit code"
    );
}
