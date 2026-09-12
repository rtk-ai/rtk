//! The proxy path must return when the child exits, even if a descendant
//! of the child keeps the inherited stdout/stderr pipes open (#2320).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Command whose direct child exits immediately while leaving behind a
/// quiet descendant that holds the inherited stdout/stderr pipes for ~30s.
#[cfg(unix)]
fn orphan_pipe_args() -> Vec<&'static str> {
    vec!["proxy", "sh", "-c", "echo ready; sleep 30 & exit 0"]
}

#[cfg(windows)]
fn orphan_pipe_args() -> Vec<&'static str> {
    vec![
        "proxy",
        "cmd",
        "/c",
        "echo ready& start /b waitfor RtkOrphanPipeTest /t 30 & exit /b 0",
    ]
}

#[test]
fn proxy_exits_when_child_dies_despite_orphaned_pipe() {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    cmd.args(orphan_pipe_args())
        .stdin(Stdio::null())
        .stderr(Stdio::null());

    // On Windows the orphan inherits rtk's own stdout handle too (handle
    // inheritance is table-wide), so reading a pipe here would block on the
    // very orphan this test creates. The streamed-output check is unix-only.
    #[cfg(unix)]
    cmd.stdout(Stdio::piped());
    #[cfg(windows)]
    cmd.stdout(Stdio::null());

    let mut child = cmd.spawn().expect("spawn rtk");

    // Well over the 2s drain grace, well under the 30s the orphan lives.
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        match child.try_wait().expect("try_wait rtk") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("rtk proxy hung on an orphaned pipe after the child exited");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    assert_eq!(
        status.code(),
        Some(0),
        "child exited 0, rtk must forward its exit code"
    );

    #[cfg(unix)]
    {
        use std::io::Read;
        let mut stdout = String::new();
        child
            .stdout
            .take()
            .expect("stdout")
            .read_to_string(&mut stdout)
            .expect("read rtk stdout");
        assert!(
            stdout.contains("ready"),
            "child output should be streamed through, got: {stdout:?}"
        );
    }
}
