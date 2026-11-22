#![cfg(unix)]
//! `rtk init -g --opencode` installs the Claude Code setup and the OpenCode plugin
//! together, from a home that has neither, and re-running it repairs a missing plugin.

use std::path::Path;
use std::process::{Command, Stdio};

mod common;

/// `PATH` with an `opencode` stub under `home` ahead of everything else, so
/// `rtk init` never reads the developer's own OpenCode. With `version` the stub
/// prints it; without, it fails, which reads as no OpenCode at all.
fn path_with_opencode(home: &Path, version: Option<&str>) -> std::ffi::OsString {
    use std::os::unix::fs::PermissionsExt;

    // One directory per answer, so a test's own stub survives `rtk(home)`.
    let bin = home.join(format!("opencode-{}", version.unwrap_or("none")));
    std::fs::create_dir_all(&bin).expect("create stub bin");
    let opencode = bin.join("opencode");
    let body = match version {
        Some(v) => format!("#!/bin/sh\necho {v}\n"),
        None => "#!/bin/sh\nexit 1\n".to_string(),
    };
    std::fs::write(&opencode, body).expect("write opencode stub");
    std::fs::set_permissions(&opencode, std::fs::Permissions::from_mode(0o755))
        .expect("make opencode stub executable");
    let mut path = std::ffi::OsString::from(&bin);
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    path
}

/// Runs rtk with `home` as its home, and its configuration, data and history
/// database inside it; its tee spool and recall database stay in scratch. Its
/// `opencode` answers nothing unless a test puts another one first.
fn rtk(home: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.env("HOME", home)
        .env("PATH", path_with_opencode(home, None))
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    cmd
}

/// Runs `rtk init <args>`, asserts it succeeded, and returns stdout.
fn init(home: &Path, args: &[&str]) -> String {
    let out = rtk(home).arg("init").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "rtk init {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn plugin(home: &Path) -> std::path::PathBuf {
    home.join(".config")
        .join("opencode")
        .join("plugins")
        .join("rtk.ts")
}

fn claude_hook_registered(home: &Path) -> bool {
    std::fs::read_to_string(home.join(".claude").join("settings.json"))
        .is_ok_and(|settings| settings.contains("rtk hook claude"))
}

#[test]
fn an_old_opencode_gets_no_plugin_and_no_plugin_path_in_any_install_mode() {
    // Every way `--opencode` reaches the plugin step, run twice so the second
    // pass takes the "hook already present" branch.
    let modes: [&[&str]; 4] = [
        &["-g", "--opencode", "--auto-patch"],
        &["-g", "--opencode", "--hook-only", "--auto-patch"],
        &["-g", "--opencode", "--claude-md"],
        &["-g", "--opencode", "--agent", "cursor"],
    ];
    for args in modes {
        let home = tempfile::tempdir().unwrap();
        let path = path_with_opencode(home.path(), Some("1.1.4"));

        for run in 1..=2 {
            let out = rtk(home.path())
                .env("PATH", &path)
                .arg("init")
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                !plugin(home.path()).exists(),
                "{args:?} run {run}: no plugin below the floor"
            );
            assert!(
                stdout.contains("OpenCode 1.1.4 is older than"),
                "{args:?} run {run}: {stdout}"
            );
            for claim in [
                "plugins/rtk.ts",
                "OpenCode plugin installed",
                "Restart Claude Code and OpenCode",
            ] {
                assert!(
                    !stdout.contains(claim),
                    "{args:?} run {run} reports a plugin it did not write ({claim}): {stdout}"
                );
            }
        }
    }
}

#[test]
fn opencode_installs_claude_hook_and_plugin() {
    let home = tempfile::tempdir().unwrap();
    init(home.path(), &["-g", "--opencode", "--auto-patch"]);
    assert!(
        claude_hook_registered(home.path()),
        "--opencode must also register the Claude hook"
    );
    assert!(
        home.path().join(".claude").join("RTK.md").exists(),
        "--opencode must also write RTK.md"
    );
    assert!(plugin(home.path()).exists(), "OpenCode plugin missing");
}

#[test]
fn opencode_hook_only_installs_claude_hook_and_plugin() {
    let home = tempfile::tempdir().unwrap();
    init(
        home.path(),
        &["-g", "--opencode", "--hook-only", "--auto-patch"],
    );
    assert!(
        claude_hook_registered(home.path()),
        "--opencode --hook-only must also register the Claude hook"
    );
    assert!(
        !home.path().join(".claude").join("RTK.md").exists(),
        "--hook-only must not write RTK.md"
    );
    assert!(plugin(home.path()).exists(), "OpenCode plugin missing");
}

#[test]
fn hook_only_without_patching_leaves_no_claude_dir() {
    let home = tempfile::tempdir().unwrap();
    init(home.path(), &["-g", "--hook-only", "--no-patch"]);
    assert!(
        !home.path().join(".claude").exists(),
        "nothing is written under ~/.claude, so it must not be created"
    );
}

#[test]
fn opencode_claude_md_rerun_restores_missing_plugin() {
    let home = tempfile::tempdir().unwrap();
    init(home.path(), &["-g", "--opencode", "--claude-md"]);
    std::fs::remove_file(plugin(home.path())).unwrap();

    // The CLAUDE.md block is unchanged now; the plugin must still come back.
    let stdout = init(home.path(), &["-g", "--opencode", "--claude-md"]);
    assert!(stdout.contains("already up to date"), "{stdout}");
    assert!(
        !stdout.contains("will now use rtk"),
        "an unchanged block stays silent:\n{stdout}"
    );
    assert!(
        plugin(home.path()).exists(),
        "re-running init must restore the OpenCode plugin"
    );
}

#[test]
fn opencode_dry_run_names_both_and_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let stdout = init(
        home.path(),
        &["-g", "--opencode", "--auto-patch", "--dry-run"],
    );
    assert!(
        stdout.contains("/.claude/settings.json"),
        "dry-run must name the Claude hook patch:\n{stdout}"
    );
    assert!(
        stdout.contains("/opencode/plugins/rtk.ts"),
        "dry-run must name the OpenCode plugin:\n{stdout}"
    );
    // The `opencode` stub `rtk(home)` puts there is the test's, not rtk's.
    let leftovers: Vec<_> = std::fs::read_dir(home.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name != "opencode-none")
        .collect();
    assert!(leftovers.is_empty(), "dry-run wrote: {leftovers:?}");
}

/// Regression #4413: a malformed CLAUDE.md block aborts the Claude side, but
/// the OpenCode plugin is an independent target and must still be installed.
#[test]
fn opencode_plugin_survives_malformed_claude_md_block() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("CLAUDE.md"),
        "<!-- rtk-instructions\nno closing marker\n",
    )
    .unwrap();

    let out = rtk(home.path())
        .args(["init", "-g", "--opencode", "--claude-md"])
        .output()
        .unwrap();

    assert!(
        !out.status.success(),
        "the malformed Claude block must still fail the command"
    );
    assert!(
        plugin(home.path()).exists(),
        "the OpenCode plugin must be installed before the Claude write can fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Refusing to modify malformed"),
        "the Claude-side error must still be reported:\n{stderr}"
    );
}

/// Regression #4413: a read-only `~/.claude` aborts the Claude side at the
/// RTK.md write, but the OpenCode plugin must already be in place.
#[test]
fn opencode_plugin_survives_read_only_claude_dir() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::set_permissions(&claude_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let out = rtk(home.path())
        .args(["init", "-g", "--opencode", "--auto-patch"])
        .output()
        .unwrap();
    // Restore before asserting so the TempDir can clean itself up on failure.
    std::fs::set_permissions(&claude_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        !out.status.success(),
        "the read-only Claude dir must still fail the command"
    );
    assert!(
        plugin(home.path()).exists(),
        "the OpenCode plugin must be installed before the Claude write can fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("RTK.md"),
        "the Claude-side error must still be reported:\n{stderr}"
    );
}

/// The inverse direction: an OpenCode failure must not block the Claude setup.
#[test]
fn claude_setup_survives_opencode_install_failure() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let opencode_dir = home.path().join(".config").join("opencode");
    std::fs::create_dir_all(&opencode_dir).unwrap();
    std::fs::set_permissions(&opencode_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let out = rtk(home.path())
        .args(["init", "-g", "--opencode", "--auto-patch"])
        .output()
        .unwrap();
    std::fs::set_permissions(&opencode_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        !out.status.success(),
        "the failed OpenCode install must still fail the command"
    );
    assert!(
        home.path().join(".claude").join("RTK.md").exists(),
        "the Claude setup must run after the OpenCode install fails"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("OpenCode install failed"),
        "the OpenCode-side error must be reported:\n{stderr}"
    );
}

/// Without `--opencode` the plugin is not written, even with OpenCode on PATH.
/// The same stub, run with `--opencode`, does write it: the negative is live.
#[test]
fn no_opencode_flag_installs_no_plugin() {
    let home = tempfile::tempdir().unwrap();
    let path = path_with_opencode(home.path(), Some("1.18.29"));

    let out = rtk(home.path())
        .env("PATH", &path)
        .args(["init", "-g", "--auto-patch"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        claude_hook_registered(home.path()),
        "the Claude hook must be registered, so the run did its work"
    );
    assert!(
        !plugin(home.path()).exists(),
        "plugin written without --opencode"
    );

    let control = tempfile::tempdir().unwrap();
    let out = rtk(control.path())
        .env("PATH", path_with_opencode(control.path(), Some("1.18.29")))
        .args(["init", "-g", "--opencode", "--auto-patch"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        plugin(control.path()).exists(),
        "the same stub with --opencode must write the plugin"
    );
}

/// Both targets fail: the command reports each cause, and the plugin path is
/// not claimed as installed. A regular file where the OpenCode directory
/// should be makes the plugin write fail for every uid, root included.
#[test]
fn both_failures_are_reported() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("CLAUDE.md"),
        "<!-- rtk-instructions\nno closing marker\n",
    )
    .unwrap();
    let config_dir = home.path().join(".config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("opencode"), "not a directory").unwrap();

    let out = rtk(home.path())
        .args(["init", "-g", "--opencode", "--claude-md"])
        .output()
        .unwrap();

    assert!(!out.status.success(), "both failures must fail the command");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("OpenCode install failed"), "{stderr}");
    assert!(
        stderr.contains("Failed to create OpenCode plugin directory"),
        "the OpenCode cause must be reported:\n{stderr}"
    );
    assert!(stderr.contains("Agent install failed"), "{stderr}");
    assert!(
        stderr.contains("Refusing to modify malformed"),
        "the agent cause must be reported:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for claim in [
        "OpenCode plugin installed",
        "plugins/rtk.ts",
        "Restart OpenCode",
    ] {
        assert!(
            !stdout.contains(claim),
            "a failed plugin write must not be reported ({claim}):\n{stdout}"
        );
    }
}
