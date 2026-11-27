//! End-to-end regression test for `rtk playwright test` with flaky/retried tests (#5142).
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;

mod common;

#[test]
fn playwright_flaky_run_surfaces_count_error_and_recall_hint() {
    let dir = common::tempdir();
    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).expect("create bin dir");

    let fixture = dir.path().join("playwright.json");
    let json = r#"{
  "config": {
    "rootDir": "/tmp/project/tests/e2e",
    "workers": 1,
    "projects": [{"name": "chromium", "retries": 1}]
  },
  "suites": [
    {
      "title": "zz-flaky.spec.ts",
      "file": "tests/e2e/zz-flaky.spec.ts",
      "specs": [
        {
          "title": "flaky on first attempt",
          "ok": true,
          "tests": [
            {
              "projectName": "chromium",
              "status": "flaky",
              "results": [
                {
                  "workerIndex": 0,
                  "status": "failed",
                  "duration": 420,
                  "retry": 0,
                  "errors": [
                    {
                      "message": "Error: intentional first-attempt failure",
                      "stack": "Error: intentional first-attempt failure\n    at tests/e2e/zz-flaky.spec.ts:8:11"
                    }
                  ],
                  "attachments": [
                    {
                      "name": "error-context",
                      "contentType": "text/markdown",
                      "path": "test-results/zz-flaky/error-context.md"
                    }
                  ]
                },
                {
                  "workerIndex": 1,
                  "status": "passed",
                  "duration": 550,
                  "retry": 1,
                  "errors": [],
                  "attachments": []
                }
              ]
            }
          ]
        }
      ],
      "suites": []
    }
  ],
  "errors": [],
  "stats": {
    "startTime": "2026-11-17T03:00:00.000Z",
    "duration": 970.0,
    "expected": 0,
    "skipped": 0,
    "unexpected": 0,
    "flaky": 1
  }
}"#;
    fs::write(&fixture, json).expect("write fixture");

    let npx = bin.join("npx");
    fs::write(&npx, "#!/bin/sh\ncat \"$PW_JSON\"\nexit 0\n").expect("write npx shim");
    fs::set_permissions(&npx, fs::Permissions::from_mode(0o755)).expect("chmod npx shim");

    let path = format!("{}:/usr/bin:/bin", bin.display());
    let output = common::rtk_command()
        .args([
            "playwright",
            "test",
            "tests/e2e/zz-flaky.spec.ts",
            "--workers=1",
            "--retries=1",
        ])
        .current_dir(dir.path())
        .env("PATH", path)
        .env("PW_JSON", &fixture)
        .output()
        .expect("run rtk playwright test");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("PASS (0) FAIL (0) FLAKY (1)"),
        "must report flaky count, got:\n{stdout}"
    );
    assert!(
        stdout.contains("flaky on first attempt"),
        "must surface flaky test name, got:\n{stdout}"
    );
    assert!(
        stdout.contains("intentional first-attempt failure"),
        "must surface first-attempt error, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[full output: rtk recall "),
        "must persist raw JSON with recall hint on flaky exit-0 run, got:\n{stdout}"
    );
}
