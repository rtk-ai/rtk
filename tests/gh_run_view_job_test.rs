//! Job views must reach `gh` with their original argv and unfiltered output.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
mod common;

fn fake_gh(dir: &Path) {
    let path = dir.join("gh");
    fs::write(
        &path,
        "#!/bin/sh\nprintf 'Job output\\n  step: build\\n'\nfor arg in \"$@\"; do printf 'arg:%s\\n' \"$arg\"; done\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
}

#[test]
fn job_views_passthrough_output_and_original_arguments() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_gh(dir.path());
    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    for args in [
        vec!["--job", "67890", "--repo", "owner/repo"],
        vec!["-j", "67890"],
        vec!["--job=67890"],
        vec!["-j67890"],
        vec!["--job", "67890", "12345"],
        vec!["-vj", "67890"],
        vec!["-vj67890"],
        vec!["--template", "--", "--job", "67890"],
        vec!["--job", "--", "12345"],
    ] {
        let output = common::rtk_command()
            .args(["gh", "run", "view"])
            .args(&args)
            .env("PATH", &path)
            .output()
            .expect("run rtk gh run view");
        assert!(output.status.success(), "failed for {args:?}: {output:?}");

        let expected = format!(
            "Job output\n  step: build\n{}",
            ["run", "view"]
                .into_iter()
                .chain(args.iter().copied())
                .map(|arg| format!("arg:{arg}\n"))
                .collect::<String>()
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    }
}
