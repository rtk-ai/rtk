#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

// The child waits for input after printing. Observing the line before sending
// input proves passthrough is live, rather than merely unfiltered at exit.
#[test]
fn npm_and_npx_watch_output_is_live_and_preserves_execution() {
    let dir = tempfile::tempdir().expect("tempdir");
    for tool in ["npm", "npx"] {
        let executable = dir.path().join(tool);
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGV_LOG\"\nprintf 'npm notice watch ready\\n'\nread -r reply\n[ \"$reply\" = continue ] || exit 9\n[ \"$SKIP_ENV_VALIDATION\" = 1 ] || exit 8\nexit 7\n",
        )
        .expect("write executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("chmod");
        for flag in [
            "--watch",
            "--watch=src",
            "--watchAll",
            "--watchAll=true",
            "--watch=false",
            "--watchAll=false",
        ] {
            let args = if tool == "npm" {
                vec!["run", "custom", "--", flag, "a b", "literal; echo unsafe"]
            } else {
                vec!["custom", flag, "a b", "literal; echo unsafe"]
            };
            let argv_log = dir.path().join("argv.txt");
            let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
                .args(["--skip-env", tool])
                .args(&args)
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        dir.path().display(),
                        std::env::var("PATH").expect("PATH")
                    ),
                )
                .env("ARGV_LOG", &argv_log)
                .env("RTK_DB_PATH", dir.path().join("history.db"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn rtk");
            let stdout = child.stdout.take().expect("stdout");
            let (tx, rx) = mpsc::channel();
            let reader = std::thread::spawn(move || {
                let mut line = String::new();
                BufReader::new(stdout)
                    .read_line(&mut line)
                    .expect("read line");
                let _ = tx.send(line);
            });
            let observed = rx.recv_timeout(Duration::from_secs(5));
            let live = child.try_wait().expect("try_wait").is_none();
            // Always release the stub before asserting, including on failure.
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(b"continue\n");
            }
            let output = child.wait_with_output().expect("wait");
            reader.join().expect("reader");
            assert_eq!(
                observed.expect("output before child exit"),
                "npm notice watch ready\n",
                "{tool} {flag}"
            );
            assert!(live, "child exited before input: {tool} {flag}");
            assert_eq!(output.status.code(), Some(7));
            assert_eq!(
                fs::read_to_string(&argv_log).expect("argv"),
                format!("{}\n", args.join("\n"))
            );
        }
    }
}

#[test]
fn unrelated_flags_still_filter_npm_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let npm = dir.path().join("npm");
    fs::write(
        &npm,
        "#!/bin/sh\nprintf 'npm notice boilerplate\\nbuild complete\\n'\n",
    )
    .expect("write npm");
    fs::set_permissions(&npm, fs::Permissions::from_mode(0o755)).expect("chmod");
    for flag in ["--no-watch", "--watchAllOfThese", "--watchdog"] {
        let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["npm", "run", "custom", "--", flag])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    dir.path().display(),
                    std::env::var("PATH").expect("PATH")
                ),
            )
            .env("RTK_DB_PATH", dir.path().join("history.db"))
            .output()
            .expect("run rtk");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "build complete\n");
    }
}
