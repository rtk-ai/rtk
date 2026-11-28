//! The telemetry question closes every interactive `rtk init`, project or global, and an answer stands for 6 months.
#![cfg(all(unix, feature = "engine"))]

use std::fs::File;
use std::io::Write;
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

mod common;

const QUESTION: &str = "Enable anonymous telemetry?";

fn init(home: &Path, cwd: &Path, args: &[&str]) -> Command {
    let bin = home.join("bin");
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut cmd = common::rtk_command();
    cmd.arg("init")
        .args(args)
        .current_dir(cwd)
        .env("PATH", std::env::join_paths(paths).expect("test PATH"))
        .env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("RTK_ONRAMP_DISABLED", "0")
        .env("RTK_TELEMETRY_DISABLED", "0")
        .env(
            "RTK_PROMPT_TEST_DAEMON_MARKER",
            home.join("daemon-executed"),
        )
        .stdin(Stdio::null());
    cmd
}

fn answered(mut cmd: Command, input: &[u8]) -> Output {
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
    master.write_all(input).expect("queue answers");
    let output = cmd
        .stdin(Stdio::from(slave))
        .output()
        .expect("run interactive init");
    drop(master);
    output
}

#[test]
fn a_global_setup_asks_telemetry_after_the_daemon_and_only_once() {
    let home = common::tempdir();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).expect("create test bin");
    let proxy = bin.join("rtk-proxy");
    std::fs::write(&proxy, "#!/bin/sh\nexit 99\n").expect("write daemon stub");
    std::fs::set_permissions(&proxy, std::fs::Permissions::from_mode(0o755))
        .expect("make daemon stub executable");
    let args = ["-g", "--agent", "claude"];

    let setup = init(home.path(), home.path(), &args)
        .arg("--auto-patch")
        .output()
        .expect("install the hook");
    assert!(setup.status.success(), "{setup:?}");
    let setup_err = String::from_utf8_lossy(&setup.stderr);
    assert!(!setup_err.contains(QUESTION), "{setup_err}");

    let first = answered(init(home.path(), home.path(), &args), b"n\nn\nn\n\x04");
    assert!(first.status.success(), "{first:?}");
    let stderr = String::from_utf8_lossy(&first.stderr);
    let daemon = stderr
        .find("Set up the RTK daemon")
        .expect("daemon question");
    let telemetry = stderr.find(QUESTION).expect("telemetry question");
    assert!(daemon < telemetry, "telemetry comes last: {stderr}");
    assert!(stderr.contains("Telemetry disabled."), "{stderr}");

    let second = answered(init(home.path(), home.path(), &args), b"n\nn\nn\n\x04");
    assert!(second.status.success(), "{second:?}");
    let again = String::from_utf8_lossy(&second.stderr);
    assert!(!again.contains(QUESTION), "asked once: {again}");
}

#[test]
fn a_project_setup_for_any_agent_ends_with_the_telemetry_question() {
    let home = common::tempdir();
    std::fs::create_dir(home.path().join("bin")).expect("create test bin");
    let project = home.path().join("project");
    std::fs::create_dir(&project).expect("create project");

    let output = answered(
        init(home.path(), &project, &["--agent", "kilocode"]),
        b"n\n\x04",
    );
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(QUESTION), "{stderr}");
    assert!(stderr.contains("Telemetry disabled."), "{stderr}");
    assert!(project.join(".kilocode/rules/rtk-rules.md").exists());
}

fn saved_config(dir: &Path) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        if path.is_dir() {
            saved_config(&path)
        } else if path.file_name()? == "config.toml"
            && std::fs::read_to_string(&path)
                .ok()?
                .contains("consent_date")
        {
            Some(path)
        } else {
            None
        }
    })
}

#[test]
fn an_answer_older_than_six_months_is_asked_again_once() {
    let home = common::tempdir();
    std::fs::create_dir(home.path().join("bin")).expect("create test bin");
    let project = home.path().join("project");
    std::fs::create_dir(&project).expect("create project");
    let args = ["--agent", "kilocode"];

    let first = answered(init(home.path(), &project, &args), b"n\n\x04");
    assert!(first.status.success(), "{first:?}");
    let config = saved_config(home.path()).expect("the answer is saved");
    let saved = std::fs::read_to_string(&config).expect("read config");
    let line = saved
        .lines()
        .find(|l| l.starts_with("consent_date"))
        .expect("consent_date line");
    let old = (chrono::Utc::now() - chrono::Duration::days(213)).to_rfc3339();
    std::fs::write(
        &config,
        saved.replace(line, &format!("consent_date = \"{old}\"")),
    )
    .expect("backdate the answer");

    let renewed = answered(init(home.path(), &project, &args), b"n\n\x04");
    assert!(renewed.status.success(), "{renewed:?}");
    let stderr = String::from_utf8_lossy(&renewed.stderr);
    assert!(stderr.contains(QUESTION), "asked again: {stderr}");
    assert!(
        stderr.contains(&format!("You answered No on {}", &old[..10])),
        "{stderr}"
    );

    let after = answered(init(home.path(), &project, &args), b"n\n\x04");
    let again = String::from_utf8_lossy(&after.stderr);
    assert!(!again.contains(QUESTION), "kept 6 months again: {again}");
}
