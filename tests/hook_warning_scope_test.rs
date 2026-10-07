//! The "hook missing/outdated" reminder is a once-a-day nudge, so where it is
//! spent matters: a command the user ran to inspect or install the hook would
//! both duplicate that command's own reporting and burn the daily marker,
//! leaving the next filtered command silent.
//!
//! Unix only: the check resolves the Claude directory through the home
//! directory, and only Unix takes that from `$HOME`.
#![cfg(unix)]

use std::path::Path;

mod common;

const REMINDER: &str = "No hook installed";

/// Pins every directory the binary resolves from the environment, so a runner
/// that exports `XDG_*` cannot reach past the temporary home.
fn isolating_env(home: &Path) -> Vec<(String, std::ffi::OsString)> {
    vec![
        ("HOME".into(), home.as_os_str().to_owned()),
        ("RTK_DB_PATH".into(), home.join("rtk.db").into_os_string()),
        (
            "XDG_CONFIG_HOME".into(),
            home.join(".config").into_os_string(),
        ),
        (
            "XDG_DATA_HOME".into(),
            home.join(".local").join("share").into_os_string(),
        ),
    ]
}

/// Runs `rtk <args>` against a home directory whose Claude config directory
/// exists but registers no hook, and returns stderr.
fn run(home: &Path, args: &[&str]) -> String {
    let out = common::rtk_command()
        .args(args)
        .envs(isolating_env(home))
        .output()
        .expect("run rtk");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn fresh_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    // Absent, the status check reports "not applicable"; present-but-empty is
    // what makes a missing hook detectable.
    std::fs::create_dir_all(home.path().join(".claude")).expect("claude dir");
    home
}

#[test]
fn commands_that_report_hook_state_do_not_repeat_the_reminder() {
    for args in [
        vec!["init", "-g"],
        vec!["verify"],
        vec!["verify", "--filter", "cargo"],
        vec!["gain"],
    ] {
        let home = fresh_home();
        let stderr = run(home.path(), &args);
        assert!(
            !stderr.contains(&format!("[rtk] /!\\ {REMINDER}")),
            "rtk {} must not print the daily reminder: {stderr}",
            args.join(" ")
        );
    }
}

#[test]
fn filtered_commands_still_get_the_reminder() {
    for args in [vec!["ls"], vec!["git", "status"]] {
        let home = fresh_home();
        let stderr = run(home.path(), &args);
        assert!(
            stderr.contains(REMINDER),
            "rtk {} must still warn: {stderr}",
            args.join(" ")
        );
    }
}

#[test]
fn a_meta_command_leaves_the_daily_reminder_for_the_next_filtered_command() {
    // The regression this guards: `maybe_warn` touches the once-a-day marker,
    // so warning on a meta command would spend the reminder there and leave
    // the next filtered command — the one that should have been hooked —
    // silent for 24 hours.
    let home = fresh_home();
    run(home.path(), &["verify", "--filter", "cargo"]);
    let stderr = run(home.path(), &["ls"]);
    assert!(
        stderr.contains(REMINDER),
        "the reminder must survive a preceding meta command: {stderr}"
    );
}

/// Writes `hooks.suppress_hook_warning` into every location the config loader
/// may pick for this home: `dirs::config_dir` is `$XDG_CONFIG_HOME` on Linux but
/// `~/Library/Application Support` on macOS, so writing only one of them passes
/// on whichever platform the author happens to use.
fn with_suppress_config(home: &Path, value: bool) {
    let body = format!("[hooks]\nsuppress_hook_warning = {value}\n");
    for dir in [
        home.join(".config").join("rtk"),
        home.join("Library").join("Application Support").join("rtk"),
    ] {
        std::fs::create_dir_all(&dir).expect("config dir");
        std::fs::write(dir.join("config.toml"), &body).expect("config");
    }
}

/// Like [`run`], but pins `RTK_SUPPRESS_HOOK_WARNING` — `None` unsets it, so an
/// exported value in the developer's own shell cannot decide the outcome.
fn run_with_env(home: &Path, args: &[&str], value: Option<&str>) -> String {
    let mut cmd = common::rtk_command();
    cmd.args(args).envs(isolating_env(home));
    match value {
        Some(v) => cmd.env("RTK_SUPPRESS_HOOK_WARNING", v),
        None => cmd.env_remove("RTK_SUPPRESS_HOOK_WARNING"),
    };
    let out = cmd.output().expect("run rtk");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn the_env_var_decides_only_when_it_parses() {
    // Pins the composition of env and config, not the parser: a value that
    // parses wins in both directions, and one that does not parse leaves the
    // config file in charge. Folding the two together with `||` — so config
    // `true` could no longer be overridden — passes every other test here.
    for (config, value, warns) in [
        (false, None, true),
        (true, None, false),
        (false, Some("1"), false),
        (false, Some("true"), false),
        (false, Some("on"), false),
        (true, Some("0"), true),
        (true, Some("false"), true),
        (true, Some("off"), true),
        (true, Some(""), false),
        (true, Some("   "), false),
        (true, Some("maybe"), false),
        (false, Some("maybe"), true),
    ] {
        let home = fresh_home();
        with_suppress_config(home.path(), config);
        let stderr = run_with_env(home.path(), &["ls"], value);
        assert_eq!(
            stderr.contains(REMINDER),
            warns,
            "config={config} env={value:?} must {} the reminder: {stderr}",
            if warns { "print" } else { "suppress" }
        );
    }
}

#[test]
fn gain_honours_the_suppression_flag() {
    // `rtk gain` prints its own missing-hook line after the report, outside
    // `maybe_warn`, so the flag has to reach it by a separate path. The report
    // only renders once there is tracked data, hence the seeding run.
    let home = fresh_home();
    run(home.path(), &["ls"]);

    let reported = run(home.path(), &["gain"]);
    assert!(
        reported.contains(REMINDER),
        "gain must report a missing hook by default: {reported}"
    );

    with_suppress_config(home.path(), true);
    let suppressed = run(home.path(), &["gain"]);
    assert!(
        !suppressed.contains(REMINDER),
        "gain must honour suppress_hook_warning: {suppressed}"
    );
}
