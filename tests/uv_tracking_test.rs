//! End-to-end coverage for the output that `uv run` records in the tracker.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn isolated_env(home: &Path, bin_dir: &Path) -> Vec<(String, std::ffi::OsString)> {
    vec![
        ("HOME".into(), home.as_os_str().to_owned()),
        ("PATH".into(), bin_dir.as_os_str().to_owned()),
        ("RTK_DB_PATH".into(), home.join("rtk.db").into_os_string()),
        ("RTK_TEE_DIR".into(), home.join("tee").into_os_string()),
        (
            "XDG_CONFIG_HOME".into(),
            home.join(".config").into_os_string(),
        ),
        (
            "XDG_DATA_HOME".into(),
            home.join(".local").join("share").into_os_string(),
        ),
        ("RTK_SUPPRESS_HOOK_WARNING".into(), "true".into()),
    ]
}

#[test]
fn uv_run_tracks_the_output_that_was_actually_printed() {
    let home = tempfile::tempdir().expect("tempdir");
    let bin_dir = home.path().join("bin");
    std::fs::create_dir(&bin_dir).expect("bin dir");

    // A silent failing run produces an empty filtered body. `print_with_hint`
    // still emits a recovery hint, which must be included in output_tokens.
    let uv = bin_dir.join("uv");
    std::fs::write(
        &uv,
        "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt 600 ]; do\n  printf '\\n'\n  i=$((i + 1))\ndone\nexit 1\n",
    )
    .expect("uv shim");
    std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o755))
        .expect("make uv executable");

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env_clear()
        .args(["uv", "run", "python", "-c", "raise SystemExit(1)"])
        .envs(isolated_env(home.path(), &bin_dir))
        .current_dir(home.path())
        .output()
        .expect("run rtk uv");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        !output.stdout.is_empty(),
        "the tee recovery hint should be printed"
    );

    let db = rusqlite::Connection::open(home.path().join("rtk.db")).expect("open tracker DB");
    let (input_tokens, output_tokens): (i64, i64) = db
        .query_row(
            "SELECT input_tokens, output_tokens FROM commands WHERE rtk_cmd LIKE 'rtk uv run %' ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("tracked uv run row");

    assert!(input_tokens > 0);
    assert!(
        output_tokens > 0,
        "the emitted recovery hint must count toward output tokens"
    );
    assert!(
        output_tokens < input_tokens,
        "tracking the displayed hint should still report savings"
    );
}
