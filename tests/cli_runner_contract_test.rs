use std::process::Command;

fn rtk() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rtk"));
    command
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env("RTK_SUPPRESS_HOOK_WARNING", "1");
    command
}

#[test]
fn empty_shell_value_is_rejected_before_execution() {
    for subcommand in ["err", "test", "summary"] {
        let output = rtk()
            .args([subcommand, "--shell=", "echo"])
            .output()
            .unwrap_or_else(|e| panic!("run rtk {subcommand}: {e}"));

        assert_eq!(output.status.code(), Some(2), "{subcommand}");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("--shell"), "{subcommand}: {stderr}");
        assert!(
            !stderr.contains("Failed to run"),
            "{subcommand} reached execution instead of clap: {stderr}"
        );
    }
}

#[test]
fn bare_test_is_silent_and_returns_one() {
    let output = rtk().arg("test").output().expect("run bare rtk test");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
