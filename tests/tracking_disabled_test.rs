use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run_log(root: &Path, tracking_enabled: bool, track_override: Option<&str>) -> PathBuf {
    let config_home = root.join("config");
    let data_home = root.join("data");
    let config_dir = config_home.join("rtk");
    fs::create_dir_all(&config_dir).expect("create isolated config directory");
    fs::create_dir_all(&data_home).expect("create isolated data directory");
    fs::write(
        config_dir.join("config.toml"),
        format!(
            "[tracking]\nenabled = {tracking_enabled}\nhistory_days = 90\n\n\
             [telemetry]\nenabled = false\n"
        ),
    )
    .expect("write isolated config");

    let input = root.join("input.log");
    fs::write(&input, "repeated line\nrepeated line\n").expect("write input");

    let mut command = Command::new(env!("CARGO_BIN_EXE_rtk"));
    command
        .args(["log", input.to_str().expect("UTF-8 temp path")])
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env_remove("RTK_DB_PATH")
        .env_remove("RTK_TRACK");
    if let Some(value) = track_override {
        command.env("RTK_TRACK", value);
    }

    let output = command.output().expect("run rtk log");
    assert!(
        output.status.success(),
        "rtk log failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    data_home.join("rtk").join("history.db")
}

#[test]
fn disabled_tracking_never_creates_history_database() {
    let config_disabled = tempfile::tempdir().expect("config-disabled tempdir");
    let db_path = run_log(config_disabled.path(), false, None);
    assert!(
        !db_path.exists(),
        "tracking.enabled=false created {}",
        db_path.display()
    );
    let parse_failure = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("rtk-command-that-does-not-exist")
        .env("XDG_CONFIG_HOME", config_disabled.path().join("config"))
        .env("XDG_DATA_HOME", config_disabled.path().join("data"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env_remove("RTK_DB_PATH")
        .env_remove("RTK_TRACK")
        .output()
        .expect("run fallback parse failure");
    assert_eq!(parse_failure.status.code(), Some(127));
    assert!(
        !db_path.exists(),
        "disabled parse-failure tracking created {}",
        db_path.display()
    );

    let env_disabled = tempfile::tempdir().expect("env-disabled tempdir");
    let db_path = run_log(env_disabled.path(), true, Some("0"));
    assert!(
        !db_path.exists(),
        "RTK_TRACK=0 created {}",
        db_path.display()
    );

    let enabled = tempfile::tempdir().expect("enabled tempdir");
    let db_path = run_log(enabled.path(), true, None);
    assert!(
        db_path.is_file(),
        "enabled tracking did not create {}",
        db_path.display()
    );
}
