//! Where `rtk config` reads and writes `config.toml` (#3193): `RTK_CONFIG_DIR`
//! first, then `$XDG_CONFIG_HOME/rtk` (Unix), then the platform config dir.

use std::path::Path;
use std::process::Command;

fn rtk_config(envs: &[(&str, &Path)], args: &[&str]) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    cmd.arg("config").args(args);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let out = cmd.output().expect("run rtk config");
    assert!(
        out.status.success(),
        "rtk config failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn rtk_config_dir_redirects_create_and_load() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().join("custom-rtk");
    let expected = dir.join("config.toml");
    let envs = [("RTK_CONFIG_DIR", dir.as_path())];

    let created = rtk_config(&envs, &["--create"]);
    assert!(
        created.contains(&expected.display().to_string()),
        "config must be created under RTK_CONFIG_DIR: {created}"
    );
    assert!(expected.exists(), "{} was not written", expected.display());

    let shown = rtk_config(&envs, &[]);
    assert!(
        shown.contains(&format!("Config: {}", expected.display())),
        "rtk config must report the RTK_CONFIG_DIR path: {shown}"
    );
    assert!(
        !shown.contains("file not created"),
        "rtk config must load the file it just created: {shown}"
    );
}

#[cfg(unix)]
#[test]
fn xdg_config_home_is_used_for_new_installs() {
    let home = tempfile::tempdir().expect("tempdir");
    let xdg = home.path().join("xdg");

    let shown = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("config")
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", &xdg)
        .env_remove("RTK_CONFIG_DIR")
        .output()
        .expect("run rtk config");
    let stdout = String::from_utf8_lossy(&shown.stdout);

    let expected = xdg.join("rtk").join("config.toml");
    assert!(
        stdout.contains(&format!("Config: {}", expected.display())),
        "with no existing config, XDG_CONFIG_HOME must decide the path: {stdout}"
    );
}

/// macOS only: elsewhere on Unix the platform config dir *is* `$XDG_CONFIG_HOME`,
/// so there is no separate legacy location to fall back to.
#[cfg(target_os = "macos")]
#[test]
fn existing_application_support_config_survives_setting_xdg_config_home() {
    let home = tempfile::tempdir().expect("tempdir");
    let legacy_dir = home.path().join("Library/Application Support/rtk");
    std::fs::create_dir_all(&legacy_dir).expect("legacy dir");
    std::fs::write(legacy_dir.join("config.toml"), "").expect("legacy config");

    let shown = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("config")
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env_remove("RTK_CONFIG_DIR")
        .output()
        .expect("run rtk config");
    let stdout = String::from_utf8_lossy(&shown.stdout);

    assert!(
        stdout.contains(&format!(
            "Config: {}",
            legacy_dir.join("config.toml").display()
        )),
        "an existing macOS config must keep being used: {stdout}"
    );
}
