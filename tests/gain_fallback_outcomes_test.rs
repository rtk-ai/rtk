#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;

#[test]
fn gain_separates_execution_failures_from_successful_fallbacks() {
    let dir = common::tempdir();
    let root = dir.path();
    for (name, script) in [
        ("quiet-ok", "exit 0"),
        ("quiet-fail", "exit 7"),
        ("output-ok", "printf 'visible output\\n'"),
        ("make", "printf 'make[1]: routine output\\n'; exit 9"),
    ] {
        let tool = root.join(name);
        std::fs::write(&tool, format!("#!/bin/sh\n{script}\n")).expect("write fallback fixture");
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
            .expect("make fallback fixture executable");
    }

    let db_path = root.join("history.db");
    let rtk = |args: &[&str]| {
        common::rtk_command()
            .current_dir(root)
            .env("PATH", root)
            .env("RTK_DB_PATH", &db_path)
            .env("RTK_RECALL", "0")
            .args(args)
            .output()
            .expect("run rtk")
    };

    let quiet_failure = rtk(&["quiet-fail"]);
    assert_eq!(quiet_failure.status.code(), Some(7));
    assert!(quiet_failure.stdout.is_empty());
    let toml_failure = rtk(&["make"]);
    assert_eq!(toml_failure.status.code(), Some(9));
    assert_eq!(toml_failure.stdout, b"make: ok\n");
    assert_eq!(rtk(&["missing-tool"]).status.code(), Some(127));

    // More successes than the report's recent-row limit must leave failures visible.
    for _ in 0..11 {
        let quiet_success = rtk(&["quiet-ok"]);
        assert!(quiet_success.status.success());
        assert!(quiet_success.stdout.is_empty());
    }
    let output_success = rtk(&["output-ok"]);
    assert!(output_success.status.success());
    assert_eq!(output_success.stdout, b"visible output\n");

    let report = rtk(&["gain", "--failures"]);
    assert!(report.status.success(), "{report:?}");
    let stdout = String::from_utf8(report.stdout).expect("UTF-8 fallback report");
    let (_, recent) = stdout
        .split_once("Recent execution failures")
        .unwrap_or_else(|| panic!("missing execution failures section:\n{stdout}"));
    let (failures, coverage) = recent
        .split_once("Top fallback commands")
        .unwrap_or_else(|| panic!("missing top fallback commands:\n{stdout}"));
    let (top, other) = coverage
        .split_once("Recent other fallbacks")
        .unwrap_or_else(|| panic!("missing other fallbacks section:\n{stdout}"));
    assert!(failures.contains("[exit 7] quiet-fail"), "{stdout}");
    assert!(failures.contains("[exit 9] make"), "{stdout}");
    assert!(
        failures.contains("[execution error] missing-tool"),
        "{stdout}"
    );
    assert!(!failures.contains("[ok]"), "{stdout}");
    assert!(top.contains("11x  quiet-ok"), "{stdout}");
    assert!(other.contains("[ok] quiet-ok"), "{stdout}");
    assert!(other.contains("[ok] output-ok"), "{stdout}");
}
