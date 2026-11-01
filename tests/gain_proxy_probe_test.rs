#![cfg(unix)]
//! `rtk gain` must not knock on an unrelated service's port (#4821).
//!
//! The gain report probes the LLM proxy bind with `GET /health` when it
//! prints the no-data hint. With no `rtk-proxy` binary installed and no
//! user-moved bind, nothing of ours can answer, so the probe would land on
//! whatever unrelated local service listens on the default port. A loopback
//! listener stands in for such a service: the kernel buffers any connection a
//! probe makes, so after the report exits, one `accept` tells whether the
//! probe happened.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Run `rtk gain` against an empty tracking database with stdout piped — the
/// "No tracking data yet" branch and its proxy probe.
///
/// PATH is always scrubbed to `path_dir`, so `which::which("rtk-proxy")`
/// finds only what this test planted, never a developer's own installation.
fn run_gain(path_dir: &Path, bind: Option<&str>) {
    let dir = common::tempdir();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("mkdir home");

    let mut cmd = common::rtk_command();
    cmd.arg("gain")
        .env("HOME", &home)
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("PATH", path_dir)
        .env("LC_ALL", "C")
        .env("RTK_PROXY_BIND", bind.unwrap_or(""));
    let out = cmd.output().expect("spawn rtk gain");
    assert!(
        out.status.success(),
        "rtk gain failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// After the report has exited, reap the connection a probe left in the
/// backlog, or `None` when nothing ever connected.
fn drain(listener: &TcpListener, window: Duration) -> Option<String> {
    listener.set_nonblocking(true).expect("nonblocking");
    let deadline = Instant::now() + window;
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0u8; 512];
                let read = stream.read(&mut buf).unwrap_or(0);
                let text = String::from_utf8_lossy(&buf[..read]).into_owned();
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n{\"service\":\"rtk-proxy\"}");
                let _ = stream.flush();
                return text.lines().next().map(String::from);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("accept: {e}"),
        }
    }
    None
}

/// An empty directory as the child's whole PATH.
fn empty_path_dir() -> PathBuf {
    common::tempdir().path().to_path_buf()
}

/// A directory whose only content is a stand-in `rtk-proxy`, so
/// `binary_available()` is true and the probe is allowed through.
fn proxy_stand_in_dir() -> PathBuf {
    let dir = common::tempdir();
    let stand_in = dir.path().join("rtk-proxy");
    std::fs::write(&stand_in, "#!/bin/sh\nexit 0\n").expect("write stand-in");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stand_in, std::fs::Permissions::from_mode(0o755))
        .expect("chmod stand-in");
    dir.path().to_path_buf()
}

/// No binary installed, no user bind: the report must leave the default port
/// unvisited. The listener takes the default bind to catch a probe that
/// should not happen; when a real service already owns the port, the test
/// steps aside rather than knocking on it itself.
#[test]
fn no_installed_proxy_means_no_probe_of_the_default_bind() {
    let exe_dir = std::env::current_exe()
        .expect("current exe")
        .parent()
        .expect("exe dir")
        .to_path_buf();
    assert!(
        !exe_dir.join("rtk-proxy").exists(),
        "precondition: a rtk-proxy sits next to the test binary at {}",
        exe_dir.display()
    );

    let listener = match TcpListener::bind("127.0.0.1:8080") {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("skipping: the default bind 8080 is taken ({e})");
            return;
        }
    };
    let path_dir = empty_path_dir();
    run_gain(&path_dir, None);
    let heard = drain(&listener, Duration::from_millis(400));
    assert_eq!(
        heard, None,
        "a probe reached the default bind with no proxy installed"
    );
}

/// The probe still happens when the user named their own bind, even with no
/// binary: `RTK_PROXY_BIND` is the variable the daemon itself reads, so
/// something of the user's answers there by definition.
#[test]
fn an_explicit_bind_is_still_probed_without_a_binary() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    let path_dir = empty_path_dir();
    run_gain(&path_dir, Some(&addr));
    let heard = drain(&listener, Duration::from_secs(1));
    assert_eq!(
        heard.as_deref(),
        Some("GET /health HTTP/1.1"),
        "the user's own bind must still be probed"
    );
}

/// The probe happens when a daemon binary is installed, even though the gate
/// now stands in front of it.
#[test]
fn an_installed_proxy_is_still_probed() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    let path_dir = proxy_stand_in_dir();
    run_gain(&path_dir, Some(&addr));
    let heard = drain(&listener, Duration::from_secs(1));
    assert_eq!(
        heard.as_deref(),
        Some("GET /health HTTP/1.1"),
        "with a proxy installed, gain still probes the bind"
    );
}
