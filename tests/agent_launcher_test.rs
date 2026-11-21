#![cfg(all(unix, feature = "engine"))]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn the_launcher_only_starts_the_proxy_for_its_own_route() {
    let dir = common::tempdir();
    let rtk = dir.path().join("rtk");
    // Keep an installed daemon beside the build binary out of this test.
    fs::copy(common::rtk_command().get_program(), &rtk).expect("copy rtk");
    let proxy_stub = format!("#!{}\n", dir.path().join("missing-interpreter").display());
    for (name, contents) in [
        (
            "claude",
            "#!/bin/sh\nprintf '%s\\n' \"$ANTHROPIC_BASE_URL\" \"$@\"\nexit 7\n",
        ),
        // A startup attempt fails immediately, without spawning a daemon.
        ("rtk-proxy", proxy_stub.as_str()),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, contents).expect("write executable");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("make executable");
    }

    for (existing, needs_proxy, agent_url) in [
        (None, true, ""),
        (Some(""), true, ""),
        (Some("http://127.0.0.1:0"), true, ""),
        (Some("http://127.0.0.1:0/"), true, "http://127.0.0.1:0/"),
        (
            Some("https://gateway.example/v1"),
            false,
            "https://gateway.example/v1",
        ),
    ] {
        let mut cmd = Command::new(&rtk);
        common::isolate_rtk(&mut cmd);
        cmd.args(["claude", "--help", "two words"])
            .env("PATH", dir.path())
            .env("RTK_PROXY_BIND", "127.0.0.1:0");
        if let Some(url) = existing {
            cmd.env("ANTHROPIC_BASE_URL", url);
        }
        let output = cmd.output().expect("launch claude");

        assert_eq!(output.status.code(), Some(7), "{existing:?}");
        assert_eq!(
            output.stdout,
            format!("{agent_url}\n--help\ntwo words\n").as_bytes(),
            "{existing:?}"
        );
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
        assert_eq!(
            stderr.contains("proxy unavailable"),
            needs_proxy,
            "route {existing:?}: {stderr}"
        );
    }
}
