#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn npm_scripts_preserve_argv_diagnostics_and_child_exit_codes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let npm = dir.path().join("npm");
    fs::write(
        &npm,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGV_LOG\"\ncat \"$FIXTURE\" >&2\nexit \"$EXIT_CODE\"\n",
    )
    .expect("write npm");
    fs::set_permissions(&npm, fs::Permissions::from_mode(0o755)).expect("chmod");
    let historical = include_str!("fixtures/script_jest_obsolete_snapshots_raw.txt");
    let failure = format!(
        "{}FAIL broken.test.ts\n  Expected: 1\n  Received: 2\n  ...\n\n  at broken.test.ts:12:3\nTest Suites: 1 failed, 100 passed, 101 total\nTests: 1 failed, 100 passed, 101 total\n",
        (0..100).map(|i| format!("PASS component{i}.test.ts\n")).collect::<String>()
    );
    let args = ["run", "test", "--", "a b.test.ts", "literal; echo unsafe"];
    let fixture = dir.path().join("fixture.txt");
    let argv = dir.path().join("argv.txt");
    for (raw, code, compact) in [
        (historical, 1, true),
        (failure.as_str(), 7, true),
        (failure.as_str(), 0, true),
        ("PASS custom validation\n12 passing\n", 0, false),
        ("error: failed to load configuration\n", 4, false),
        ("", 0, false),
    ] {
        fs::write(&fixture, raw).expect("fixture");
        let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .arg("npm")
            .args(args)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    dir.path().display(),
                    std::env::var("PATH").expect("PATH")
                ),
            )
            .env("ARGV_LOG", &argv)
            .env("FIXTURE", &fixture)
            .env("EXIT_CODE", code.to_string())
            .env("RTK_DB_PATH", dir.path().join("history.db"))
            .output()
            .expect("run npm");
        assert_eq!(output.status.code(), Some(code));
        assert_eq!(
            fs::read_to_string(&argv).expect("argv"),
            format!("{}\n", args.join("\n"))
        );
        let shown = String::from_utf8_lossy(&output.stdout);
        if compact {
            let expected: String = raw
                .split_inclusive('\n')
                .filter(|line| !line.starts_with("PASS "))
                .collect();
            assert_eq!(shown, format!("{expected}\n"));
            assert!(
                shown.len() * 100 < raw.len() * 40,
                "expected >60% byte reduction"
            );
        } else {
            assert!(shown.contains(raw.trim_end()), "{shown}");
        }
    }
}
