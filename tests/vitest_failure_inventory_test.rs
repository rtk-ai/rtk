#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn every_failure_can_be_located_and_omitted_details_can_be_recalled() {
    let dir = common::tempdir();
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).expect("create tool directory");
    let executable = bin.join("vitest");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s\\n' \"$REPORT\"\nexit 1\n",
    )
    .expect("write test runner");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
        .expect("make runner executable");
    let report = serde_json::json!({
        "numTotalTests": 49,
        "numPassedTests": 0,
        "numFailedTests": 49,
        "numPendingTests": 0,
        "testResults": (1..=49).map(|index| serde_json::json!({
            "name": format!("tests/case_{index:02}.test.ts"),
            "assertionResults": [{
                "fullName": format!("failure_{index:02}"),
                "status": "failed",
                "failureMessages": [format!("assertion detail {index:02}")]
            }]
        })).collect::<Vec<_>>()
    })
    .to_string();
    let database = dir.path().join("recall.db");
    let output = common::rtk_command()
        .args(["vitest", "run"])
        .current_dir(dir.path())
        .env("PATH", &bin)
        .env("REPORT", &report)
        .env_remove("RTK_TEE")
        .env("RTK_RECALL_DB", &database)
        .output()
        .expect("run failing suite");
    assert_eq!(output.status.code(), Some(1));
    let shown = String::from_utf8(output.stdout).expect("UTF-8 summary");
    for index in 1..=49 {
        assert!(shown.contains(&format!("failure_{index:02}")), "{shown}");
        assert!(
            shown.contains(&format!("tests/case_{index:02}.test.ts")),
            "{shown}"
        );
    }
    assert!(!shown.contains("assertion detail 49"), "{shown}");
    let hash = shown
        .split_once("[full output: rtk recall ")
        .expect("omitted details have a recovery hint")
        .1
        .split_once(']')
        .expect("hint terminator")
        .0;
    fs::remove_file(executable).expect("prevent rerunning the original command");
    let recalled = common::rtk_command()
        .args(["recall", hash])
        .env("RTK_RECALL_DB", &database)
        .output()
        .expect("recall failure details");
    assert!(recalled.status.success());
    assert_eq!(recalled.stdout, format!("{report}\n").as_bytes());
}
