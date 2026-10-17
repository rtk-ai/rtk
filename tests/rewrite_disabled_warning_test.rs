//! #508's `RTK_DISABLED` warning and the exclude-pattern warnings are stderr,
//! so no rewrite test that compares stdout can see them move. One test pins
//! that the `RTK_DISABLED` warning prints before the config's own
//! exclude-pattern warnings; the other pins that the pattern warnings print
//! for a line with nothing to rewrite.
//!
//! Unix only: the binary resolves its config directory through `$HOME`, and
//! only Unix takes that from the environment.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::Stdio;

const WARNING: &str = "RTK_DISABLED=1 detected";

/// Runs `rtk rewrite <cmd>` (one argv) against a home holding `config`, and
/// returns stderr.
fn stderr_of(home: &Path, cmd: &str) -> String {
    let out = common::rtk_command()
        .args(["rewrite", cmd])
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .stdin(Stdio::null())
        .output()
        .expect("run rtk");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A home holding `config` as `config.toml`, in both places the config
/// directory resolves to: `.config` on Linux, `Library/Application Support` on
/// macOS.
fn home_with(config: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    for dir in [
        home.path().join(".config").join("rtk"),
        home.path()
            .join("Library")
            .join("Application Support")
            .join("rtk"),
    ] {
        std::fs::create_dir_all(&dir).expect("config dir");
        std::fs::write(dir.join("config.toml"), config).expect("config");
    }
    home
}

#[test]
fn the_warning_prints_before_the_exclude_patterns_own_warnings() {
    let home = home_with("[hooks]\nexclude_commands = [\"^\", \"^(\", \"  \"]\n");
    let stderr = stderr_of(home.path(), "RTK_DISABLED=1 git status");
    let at_warning = stderr.find(WARNING).expect("the RTK_DISABLED warning");
    let at_pattern = stderr
        .find("exclude_commands pattern")
        .expect("a pattern warning");
    assert!(
        at_warning < at_pattern,
        "the RTK_DISABLED warning comes first: {stderr}"
    );
}

#[test]
fn the_patterns_warnings_still_print_for_a_line_with_nothing_to_rewrite() {
    let home = home_with("[hooks]\nexclude_commands = [\"^\"]\n");
    for cmd in ["", "   ", "echo hi", "echo a\necho b"] {
        let stderr = stderr_of(home.path(), cmd);
        assert!(
            stderr.contains("exclude_commands pattern"),
            "{cmd:?} compiles the config first: {stderr}"
        );
    }
}
