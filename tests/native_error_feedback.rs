//! Black-box contracts for useful native-command failure diagnostics.

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

struct TestRtk {
    _temp: TempDir,
    tracking_db: PathBuf,
}

impl TestRtk {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let tracking_db = temp.path().join("tracking.db");
        Self {
            _temp: temp,
            tracking_db,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(args)
            .env("RTK_DB_PATH", &self.tracking_db)
            .output()
            .expect("spawn rtk")
    }

    fn tracking_db(&self) -> &Path {
        &self.tracking_db
    }
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn which_path_like_name_is_actionable_and_returns_one() {
    let rtk = TestRtk::new();
    let output = rtk.run(&["which", "not/a-command"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        combined_output(&output)
            .contains("rtk which: path-like name 'not/a-command' is unsupported"),
        "which must explain why path-like input is unsupported: {}",
        combined_output(&output)
    );
}

#[test]
fn grep_missing_path_has_a_nonempty_diagnostic_and_nonzero_exit() {
    let rtk = TestRtk::new();
    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("missing-input.txt");
    let missing = missing.to_str().expect("UTF-8 test path");
    let output = rtk.run(&["grep", "needle", missing]);

    assert_ne!(output.status.code(), Some(0));
    assert!(
        !combined_output(&output).trim().is_empty(),
        "a missing grep path must not be silent"
    );
}

#[test]
fn find_missing_root_reports_access_error_and_returns_nonzero() {
    let rtk = TestRtk::new();
    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("missing-search-root");
    let missing = missing.to_str().expect("UTF-8 test path");
    let output = rtk.run(&["find", "*.rs", missing]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        combined_output(&output).contains("rtk find: cannot access:"),
        "find must report an inaccessible search root: {}",
        combined_output(&output)
    );

    let connection = Connection::open(rtk.tracking_db()).expect("open tracking database");
    let (raw_tokens, shown_tokens): (i64, i64) = connection
        .query_row(
            "SELECT input_tokens, output_tokens FROM commands WHERE rtk_cmd = 'rtk find'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("missing-root find must track its diagnostic as raw and shown output");
    assert!(
        raw_tokens > 0 && shown_tokens > 0,
        "the tracked raw and shown outputs must include the access diagnostic"
    );
}

#[test]
fn find_no_match_is_silent_and_returns_zero() {
    let rtk = TestRtk::new();
    let temp = tempfile::tempdir().expect("tempdir");
    let empty_root = temp.path().to_str().expect("UTF-8 test path");
    let output = rtk.run(&["find", "*.rs", empty_root]);

    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "a no-match find must not print output"
    );
    assert!(
        output.stderr.is_empty(),
        "a no-match find must not print a diagnostic: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn find_unsupported_option_reports_proxy_recovery_and_returns_two() {
    let rtk = TestRtk::new();
    let output = rtk.run(&["find", ".", "-not", "-name", "*.rs"]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim_end(),
        "rtk find: unsupported option '-not'; use `rtk proxy find . -not -name *.rs`"
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn find_unsupported_option_tracks_the_native_find_command() {
    let rtk = TestRtk::new();
    let output = rtk.run(&["find", ".", "-not", "-name", "*.rs"]);

    assert_eq!(output.status.code(), Some(2));

    let connection = Connection::open(rtk.tracking_db()).expect("open tracking database");
    let (original_cmd, rtk_cmd): (String, String) = connection
        .query_row("SELECT original_cmd, rtk_cmd FROM commands", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("unsupported find must be tracked");

    assert_eq!(original_cmd, "find . -not -name *.rs");
    assert_eq!(rtk_cmd, "rtk find");
}

#[test]
fn fallback_missing_command_reports_platform_specific_diagnostic() {
    let rtk = TestRtk::new();
    let program = "rtk-definitely-missing-fallback-command";
    let output = rtk.run(&[program]);
    let diagnostic = combined_output(&output);

    #[cfg(windows)]
    {
        assert_eq!(output.status.code(), Some(2));
        assert!(
            diagnostic.contains(program),
            "Windows diagnostic must identify the rejected command: {diagnostic}"
        );
        assert!(
            diagnostic.contains("ambiguous Windows command"),
            "Windows diagnostic must explain the ambiguity: {diagnostic}"
        );
    }

    #[cfg(not(windows))]
    {
        assert_eq!(output.status.code(), Some(127));
        assert!(
            !diagnostic.trim().is_empty(),
            "Unix shell fallback must produce a diagnostic"
        );
    }
}
