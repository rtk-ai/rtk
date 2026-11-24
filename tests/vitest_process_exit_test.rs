mod common;

#[cfg(unix)]
#[test]
fn a_passing_report_does_not_hide_a_failed_test_process() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = common::tempdir();
    let bin = dir.path().join("node_modules/.bin");
    fs::create_dir_all(&bin).expect("create local tool directory");
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("join tool search path");
    let report = serde_json::json!({
        "numTotalTests": 2,
        "numPassedTests": 2,
        "numFailedTests": 0,
        "numPendingTests": 0,
        "success": true,
        "testResults": [{"name": "example.test.ts", "assertionResults": []}]
    })
    .to_string();
    for framework in ["vitest", "jest"] {
        let executable = bin.join(framework);
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$REPORT\"\nexit \"$TEST_EXIT\"\n",
        )
        .expect("write local test runner");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
            .expect("make test runner executable");
        for code in [0, 1, 2] {
            let output = common::rtk_command()
                .arg(framework)
                .current_dir(dir.path())
                .env("PATH", &path)
                .env("REPORT", &report)
                .env("TEST_EXIT", code.to_string())
                .output()
                .expect("run test framework");
            assert_eq!(output.status.code(), Some(code));
            let shown = String::from_utf8(output.stdout).expect("UTF-8 output");
            assert!(shown.contains("PASS (2) FAIL (0)"), "{shown}");
            if code != 0 {
                assert!(
                    shown.contains(&format!("{framework} exited with code {code}")),
                    "{shown}"
                );
            } else {
                assert!(!shown.contains("exited with code"), "{shown}");
            }
        }
    }
}
