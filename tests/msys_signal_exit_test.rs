#![cfg(windows)]
//! #5073: an MSYS/Cygwin program killed by a signal (Git for Windows' GNU grep 3.0
//! aborts on `grep -Fi`) reaches a native parent with exit code `signal << 8`, e.g.
//! 1536 for SIGABRT. rtk passed that through, and a POSIX shell keeps only the low
//! byte, so the agent saw exit 0 and an empty result: "no matches". It must come
//! back as `128 + signal`, the code bash itself reports, with the reason on stderr.

mod common;

fn rtk_exit(code: &str) -> std::process::Output {
    let mut cmd = common::rtk_command();
    cmd.args(["cmd", "/d", "/c", &format!("exit {code}")]);
    cmd.output().expect("run rtk")
}

#[test]
fn an_msys_signal_exit_comes_back_as_128_plus_the_signal() {
    let out = rtk_exit("1536");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(134), "stderr: {stderr}");
    assert!(stderr.contains("signal 6"), "{stderr}");
}

#[test]
fn ordinary_exit_codes_pass_through_unchanged() {
    for code in [0, 1, 2, 255, 1537] {
        let out = rtk_exit(&code.to_string());
        assert_eq!(out.status.code(), Some(code), "exit {code}");
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains("signal"),
            "exit {code}"
        );
    }
}
