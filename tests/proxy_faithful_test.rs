//! Proxy mode must preserve the child command's arguments, output streams,
//! and exit status. This protects the documented raw escape hatch (#3549).

use std::process::Command;

#[cfg(unix)]
#[test]
fn proxy_preserves_args_streams_and_exit_code() {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([
            "proxy",
            "sh",
            "-c",
            "printf 'out:%s' \"$1\"; printf 'err:%s' \"$2\" >&2; exit 23",
            "proxy-fixture",
            "value with spaces",
            "--query=cluster.status",
        ])
        .output()
        .expect("run rtk proxy fixture");

    assert_eq!(
        output.status.code(),
        Some(23),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "out:value with spaces"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "err:--query=cluster.status"
    );
}

#[cfg(windows)]
#[test]
fn proxy_preserves_args_streams_and_exit_code() {
    let fixture_dir = tempfile::tempdir().expect("create proxy fixture directory");
    let script = fixture_dir.path().join("proxy-fixture.ps1");
    std::fs::write(
        &script,
        "[Console]::Out.Write('out:' + $args[0]); [Console]::Error.Write('err:' + $args[1]); exit 23",
    )
    .expect("write proxy fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("proxy")
        .arg("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .args(["value with spaces", "--query=cluster.status"])
        .env("CLAUDE_CONFIG_DIR", fixture_dir.path().join("no-claude"))
        .output()
        .expect("run rtk proxy fixture");

    assert_eq!(
        output.status.code(),
        Some(23),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "out:value with spaces"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "err:--query=cluster.status"
    );
}
