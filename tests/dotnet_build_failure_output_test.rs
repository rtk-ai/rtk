#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn parsed_build_errors_are_not_repeated_but_unknown_failures_survive() {
    let dir = common::tempdir();
    let executable = dir.path().join("dotnet");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' \"$BUILD_STDOUT\"\nprintf '%s' \"$BUILD_STDERR\" >&2\nexit 1\n",
    )
    .expect("write SDK fixture");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
        .expect("make SDK fixture executable");
    let run = |stdout: &str, stderr: &str| {
        let output = common::rtk_command()
            .args(["dotnet", "build"])
            .current_dir(dir.path())
            .env("PATH", dir.path())
            .env("BUILD_STDOUT", stdout)
            .env("BUILD_STDERR", stderr)
            .output()
            .expect("run failed build");
        assert_eq!(output.status.code(), Some(1));
        String::from_utf8(output.stdout).expect("UTF-8 build output")
    };

    let raw = include_str!("fixtures/dotnet/build_failed.txt");
    let shown = run(raw, "");
    assert_eq!(shown.matches("CS1525").count(), 1, "{shown}");
    assert!(shown.contains("Broken.cs(7,17)"), "{shown}");
    assert!(shown.contains("Invalid expression term"), "{shown}");
    assert!(shown.contains("fail dotnet build:"), "{shown}");
    assert!(shown.len() * 100 <= raw.len() * 80);
    let with_stderr = run(raw, "custom task diagnostic on stderr\n");
    assert!(with_stderr.contains("custom task diagnostic on stderr"));
    assert_eq!(with_stderr.matches("CS1525").count(), 1);

    for (stdout, stderr, expected) in [
        (
            "SDK resolver stopped unexpectedly\n",
            "",
            "SDK resolver stopped unexpectedly",
        ),
        (
            "",
            "No installed SDK supports this project\n",
            "No installed SDK supports this project",
        ),
        (
            "Compiler terminated before reporting details\nBuild FAILED.\n    1 Error(s)\n",
            "",
            "Compiler terminated before reporting details",
        ),
    ] {
        assert!(run(stdout, stderr).contains(expected), "{expected}");
    }
}
