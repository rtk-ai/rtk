//! The proxy setup explains its compatibility cost before asking for consent.
#![cfg(all(unix, feature = "engine"))]

use std::fs::File;
use std::io::Write;
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

mod common;

fn init(home: &Path, bin: &Path, marker: &Path) -> Command {
    let mut cmd = common::rtk_command();
    let mut paths = vec![bin.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    cmd.args(["init", "-g", "--agent", "claude"])
        .current_dir(home)
        .env("PATH", std::env::join_paths(paths).expect("test PATH"))
        .env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("RTK_ONRAMP_DISABLED", "0")
        .env("RTK_PROMPT_TEST_DAEMON_MARKER", marker)
        .stdin(Stdio::null());
    cmd
}

#[test]
fn proxy_notice_precedes_consent_and_declining_preserves_settings() {
    let home = common::tempdir();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).expect("create test bin");
    let marker = home.path().join("daemon-executed");
    let proxy = bin.join("rtk-proxy");
    std::fs::write(
        &proxy,
        "#!/bin/sh\nprintf started > \"$RTK_PROMPT_TEST_DAEMON_MARKER\"\nexit 99\n",
    )
    .expect("write daemon stub");
    std::fs::set_permissions(&proxy, std::fs::Permissions::from_mode(0o755))
        .expect("make daemon stub executable");

    let setup = init(home.path(), &bin, &marker)
        .arg("--auto-patch")
        .output()
        .expect("install the hook without adopting the proxy");
    assert!(setup.status.success(), "{setup:?}");
    let settings = home.path().join(".claude/settings.json");
    let before = std::fs::read(&settings).expect("read installed settings");

    // Only stdin needs a terminal: the setup deliberately refuses piped consent.
    let mut master_fd = -1;
    let mut slave_fd = -1;
    // SAFETY: openpty receives valid output pointers and null optional settings.
    #[allow(unsafe_code)]
    let opened = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(opened, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: a successful openpty returned two distinct, newly owned descriptors.
    #[allow(unsafe_code)]
    let (mut master, slave) =
        unsafe { (File::from_raw_fd(master_fd), File::from_raw_fd(slave_fd)) };
    // Decline proxy and delegation setup; EOF also prevents another consent read.
    master.write_all(b"n\nn\n\x04").expect("queue refusals");
    let output = init(home.path(), &bin, &marker)
        .stdin(Stdio::from(slave))
        .output()
        .expect("run interactive init");
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let prompt = stderr.find("Set up the RTK proxy").expect("proxy question");
    let notice = stderr.find("Remote Control").expect("compatibility notice");
    assert!(notice < prompt, "notice must precede consent: {stderr}");
    assert!(
        stderr.contains("rtk claude"),
        "per-session alternative: {stderr}"
    );
    assert_eq!(
        std::fs::read(&settings).expect("read final settings"),
        before
    );
    assert!(!marker.exists(), "declining must not execute the daemon");
}
