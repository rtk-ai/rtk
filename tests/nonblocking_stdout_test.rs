#![cfg(unix)]

use std::io::Read;
use std::os::fd::AsRawFd;
use std::thread::sleep;
use std::time::Duration;

mod common;

fn nonblocking(fd: &impl AsRawFd) -> bool {
    #[allow(unsafe_code)]
    // nosemgrep: unsafe-block
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0, "F_GETFL failed");
    flags & libc::O_NONBLOCK != 0
}

fn set_nonblocking(fd: &impl AsRawFd) {
    #[allow(unsafe_code)]
    // nosemgrep: unsafe-block
    let rc = unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK)
    };
    assert_eq!(rc, 0, "F_SETFL failed");
}

/// Agent runners can hand rtk a stdout pipe with O_NONBLOCK set. Output larger than
/// the pipe buffer then fails with EAGAIN, which `print!` turns into a panic (SIGABRT
/// in release). See #4177.
#[test]
fn large_output_to_nonblocking_stdout_is_written_in_full() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("big.txt");
    let content: String = (0..40_000)
        .map(|i| format!("line {i} with enough payload to outgrow any pipe buffer\n"))
        .collect();
    std::fs::write(&file, &content).expect("write big file");

    let (mut reader, writer) = std::io::pipe().expect("pipe");
    set_nonblocking(&writer);

    let mut child = common::rtk_command()
        .arg("read")
        .arg(&file)
        .stdout(writer.try_clone().expect("dup pipe writer"))
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn rtk");

    // Let rtk fill the pipe and hit EAGAIN before anyone drains it.
    sleep(Duration::from_millis(500));
    let drain = std::thread::spawn(move || {
        let mut out = Vec::new();
        reader.read_to_end(&mut out).expect("read rtk stdout");
        out
    });

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr pipe")
        .read_to_string(&mut stderr)
        .expect("read rtk stderr");
    let status = child.wait().expect("wait for rtk");

    // O_NONBLOCK lives on the shared open file description; the caller's flag must be back.
    let restored = nonblocking(&writer);
    drop(writer);
    let out = drain.join().expect("drain thread");

    assert!(status.success(), "rtk failed: {status:?}\nstderr: {stderr}");
    assert_eq!(out.len(), content.len(), "stdout was truncated");
    assert!(restored, "rtk left the caller's stdout blocking");
}
