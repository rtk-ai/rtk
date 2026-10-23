//! A `Bash(rtk:*)` allow rule grants every command rtk filters, not just rtk's
//! own subcommands. `rtk init` and `rtk verify` report it; the hook path must
//! stay silent, because its stdout is the protocol with the host.
//!
//! Unix only: the settings path is resolved through `$HOME`.
#![cfg(unix)]

use std::path::Path;
use std::process::Output;

mod common;

const BROAD_RULE: &str = "Bash(rtk:*)";
const WARNING: &str = "allows every command rtk filters";

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

/// A home whose Claude settings hold `rules` as allow rules.
fn home_with_allow_rules(rules: &[&str]) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    let claude = home.path().join(".claude");
    std::fs::create_dir_all(&claude).expect("claude dir");
    let body = serde_json::json!({ "permissions": { "allow": rules } });
    std::fs::write(claude.join("settings.json"), body.to_string()).expect("settings");
    home
}

/// Runs `rtk <args>` against `home`. `common::rtk_command` already starts the
/// child in an empty project directory of its own, so no `.claude/` from
/// whatever repository the tests run from can add rules.
fn run(home: &Path, args: &[&str]) -> Output {
    common::rtk_command()
        .args(args)
        .envs(isolating_env(home))
        .output()
        .expect("run rtk")
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn init_and_verify_report_a_broad_allow_rule() {
    for args in [
        vec!["init", "-g"],
        // Gemini is its own flag, and its init branch returns before the
        // shared path: it still has to warn.
        vec!["init", "-g", "--gemini"],
        vec!["verify"],
    ] {
        let home = home_with_allow_rules(&[BROAD_RULE]);
        let stderr = stderr_of(&run(home.path(), &args));
        assert!(
            stderr.contains(BROAD_RULE) && stderr.contains(WARNING),
            "rtk {} must report the broad rule: {stderr}",
            args.join(" ")
        );
    }
}

#[test]
fn a_narrow_allow_rule_is_not_reported() {
    // Each is narrow for its own reason: `Bash(rtk git push)` is a single
    // command, `Bash(rtk git:*)` covers one subcommand, `Bash(*rtk)` only
    // matches the bare word, and `Bash(*)` is not about rtk at all.
    for narrow in [
        "Bash(rtk git push)",
        "Bash(rtk git:*)",
        "Bash(*rtk)",
        "Bash(*)",
    ] {
        let home = home_with_allow_rules(&[narrow]);
        let stderr = stderr_of(&run(home.path(), &["verify"]));
        assert!(
            !stderr.contains(WARNING),
            "{narrow} must not be reported: {stderr}"
        );
    }
}

#[test]
fn a_prefix_rule_without_a_wildcard_is_reported() {
    // Claude Code reads a wildcard-free pattern as a word-boundary prefix, so
    // `Bash(rtk)` grants `rtk git push` just like `Bash(rtk:*)` does.
    let home = home_with_allow_rules(&["Bash(rtk)"]);
    let stderr = stderr_of(&run(home.path(), &["verify"]));
    assert!(
        stderr.contains("Bash(rtk)") && stderr.contains(WARNING),
        "a wildcard-free prefix rule must be reported: {stderr}"
    );
}

#[test]
fn the_hook_path_and_filter_mode_stay_silent() {
    // The hook's stdout is the host protocol, and `verify --filter` runs one
    // filter's tests: neither may carry a settings warning. The same home
    // warns under plain `verify`, so silence here is the feature and not a
    // command that failed before reaching the check.
    let home = home_with_allow_rules(&[BROAD_RULE]);
    assert!(
        stderr_of(&run(home.path(), &["verify"])).contains(WARNING),
        "the positive control must warn, or this test proves nothing"
    );

    for args in [vec!["hook", "claude"], vec!["verify", "--filter", "cargo"]] {
        let out = run(home.path(), &args);
        assert!(
            out.status.success(),
            "rtk {} must exit 0: {}",
            args.join(" "),
            stderr_of(&out)
        );
        assert!(
            !stderr_of(&out).contains(WARNING),
            "rtk {} must stay silent: {}",
            args.join(" "),
            stderr_of(&out)
        );
    }
}

#[test]
fn a_rule_is_reported_once_per_file() {
    let home = home_with_allow_rules(&[BROAD_RULE, BROAD_RULE]);
    let stderr = stderr_of(&run(home.path(), &["verify"]));
    assert_eq!(
        stderr.matches(WARNING).count(),
        1,
        "a repeated rule must warn once: {stderr}"
    );
}
