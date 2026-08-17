use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Output};

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn shell_quote_path(value: &Path) -> String {
    shell_quote(&value.to_string_lossy())
}

#[cfg(target_os = "linux")]
fn write_probe(temp: &tempfile::TempDir) -> PathBuf {
    let probe = temp.path().join("argv-cwd-probe.sh");
    fs::write(
        &probe,
        r#"#!/usr/bin/env bash
set -euo pipefail
evidence="$1"
shift
printf '%s\n' "$$" > "${evidence}.pid"
readlink "/proc/$$/cwd" > "${evidence}.cwd"
printf '%s\n' "$#" > "${evidence}.argc"
: > "${evidence}.argv"
if (( $# > 0 )); then
    printf '%s\0' "$@" > "${evidence}.argv"
fi
printf '%s\n' "${RTK_ARGV_SENTINEL-}" > "${evidence}.env"
"#,
    )
    .expect("write probe");

    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(&probe).expect("probe metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&probe, permissions).expect("make probe executable");
    probe
}

#[cfg(target_os = "linux")]
fn run_rtk_test(caller: &Path, args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("test")
        .args(args)
        .current_dir(caller)
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env("RTK_ARGV_SENTINEL", "preserved env with spaces > literal")
        .output()
        .expect("run rtk test")
}

#[cfg(target_os = "linux")]
fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "rtk test failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
fn read_probe_cwd(evidence: &Path) -> PathBuf {
    PathBuf::from(
        fs::read_to_string(evidence.with_extension("cwd"))
            .expect("probe cwd evidence")
            .trim(),
    )
}

#[cfg(target_os = "linux")]
fn read_probe_args(evidence: &Path) -> Vec<String> {
    let bytes = fs::read(evidence.with_extension("argv")).expect("probe argv evidence");
    bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8(part.to_vec()).expect("utf8 probe arg"))
        .collect()
}

#[cfg(target_os = "linux")]
#[test]
fn test_preserves_grouped_bash_lc_child_cwd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let caller = temp.path().join("caller");
    let child = temp.path().join("child");
    fs::create_dir_all(&caller).expect("create caller");
    fs::create_dir_all(&child).expect("create child");

    let probe = write_probe(&temp);

    let evidence = temp.path().join("rtk");
    let grouped = format!(
        "cd {} && {} {}",
        shell_quote_path(&child),
        shell_quote_path(&probe),
        shell_quote_path(&evidence)
    );

    let output = run_rtk_test(&caller, &["bash".into(), "-lc".into(), grouped]);
    assert_success(&output);

    let observed_cwd = read_probe_cwd(&evidence);
    let expected_cwd = fs::canonicalize(&child).expect("canonical child");

    assert_eq!(
        observed_cwd, expected_cwd,
        "bash -lc grouped command must reach bash as one argv element"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_preserves_grouped_bash_lc_with_space_in_child_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let caller = temp.path().join("caller");
    let child = temp.path().join("child path with spaces");
    fs::create_dir_all(&caller).expect("create caller");
    fs::create_dir_all(&child).expect("create child");
    let probe = write_probe(&temp);
    let evidence = temp.path().join("space-path");
    let grouped = format!(
        "cd {} && {} {}",
        shell_quote_path(&child),
        shell_quote_path(&probe),
        shell_quote_path(&evidence)
    );

    let output = run_rtk_test(&caller, &["bash".into(), "-lc".into(), grouped]);
    assert_success(&output);

    assert_eq!(
        read_probe_cwd(&evidence),
        fs::canonicalize(&child).expect("canonical child")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_preserves_grouped_bash_lc_metacharacters_quotes_and_backslashes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let caller = temp.path().join("caller");
    let child = temp.path().join("child");
    fs::create_dir_all(&caller).expect("create caller");
    fs::create_dir_all(&child).expect("create child");
    let probe = write_probe(&temp);
    let evidence = temp.path().join("grouped-argv");
    let expected = vec![
        "literal with spaces",
        "greater>than",
        "pipe|value",
        "semi;colon",
        "single'quote",
        "double\"quote",
        r"back\slash",
    ];
    let quoted_args = expected
        .iter()
        .map(|value| shell_quote(value))
        .collect::<Vec<_>>()
        .join(" ");
    let grouped = format!(
        "cd {} && {} {} {}",
        shell_quote_path(&child),
        shell_quote_path(&probe),
        shell_quote_path(&evidence),
        quoted_args
    );

    let output = run_rtk_test(&caller, &["bash".into(), "-lc".into(), grouped]);
    assert_success(&output);

    assert_eq!(
        read_probe_args(&evidence),
        expected,
        "grouped -lc argument owns shell metacharacter and quoting semantics"
    );
    assert_eq!(
        read_probe_cwd(&evidence),
        fs::canonicalize(&child).expect("canonical child")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_preserves_literal_argv_for_non_shell_command() {
    let temp = tempfile::tempdir().expect("tempdir");
    let caller = temp.path().join("caller");
    fs::create_dir_all(&caller).expect("create caller");
    let probe = write_probe(&temp);
    let evidence = temp.path().join("direct-argv");
    let expected = vec![
        "literal with spaces".to_string(),
        "greater > than".to_string(),
        "pipe|value".to_string(),
        "semi;colon".to_string(),
        "single'quote".to_string(),
        "double\"quote".to_string(),
        r"back\slash".to_string(),
    ];
    let mut args = vec![
        probe.to_string_lossy().into_owned(),
        evidence.to_string_lossy().into_owned(),
    ];
    args.extend(expected.iter().cloned());

    let output = run_rtk_test(&caller, &args);
    assert_success(&output);

    assert_eq!(read_probe_args(&evidence), expected);
    assert_eq!(
        fs::read_to_string(evidence.with_extension("env"))
            .expect("probe env evidence")
            .trim(),
        "preserved env with spaces > literal"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_preserves_pnpm_dash_c_target_cwd() {
    if !Command::new("pnpm")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
    {
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let caller = temp.path().join("caller");
    let target = temp.path().join("pnpm-target");
    fs::create_dir_all(&caller).expect("create caller");
    fs::create_dir_all(&target).expect("create target");
    let probe = write_probe(&temp);
    let evidence = temp.path().join("pnpm-cwd");
    let script = format!(
        "{} {}",
        shell_quote_path(&probe),
        shell_quote_path(&evidence)
    );
    let package = serde_json::json!({
        "name": "rtk-test-argv-probe",
        "version": "1.0.0",
        "private": true,
        "scripts": { "probe": script }
    });
    fs::write(
        target.join("package.json"),
        serde_json::to_vec(&package).expect("serialize package"),
    )
    .expect("write package");

    let output = run_rtk_test(
        &caller,
        &[
            "pnpm".into(),
            "-C".into(),
            target.to_string_lossy().into_owned(),
            "run".into(),
            "probe".into(),
        ],
    );
    assert_success(&output);

    assert_eq!(
        read_probe_cwd(&evidence),
        fs::canonicalize(&target).expect("canonical target")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_preserves_direct_command_exit_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let caller = temp.path().join("caller");
    fs::create_dir_all(&caller).expect("create caller");

    let output = run_rtk_test(&caller, &["bash".into(), "-c".into(), "exit 23".into()]);

    assert_eq!(
        output.status.code(),
        Some(23),
        "rtk test must propagate the direct child's exit code"
    );
}
