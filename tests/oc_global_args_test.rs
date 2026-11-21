//! Pin the argv received by oc, not just clap's parsed fields.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

mod common;

fn run_oc(args: &[&str], stdout: &str, stderr: &str, exit: i32) -> (Vec<String>, Output) {
    let dir = common::tempdir();
    let argv_file = dir.path().join("argv");
    let stdout_file = dir.path().join("stdout");
    let stderr_file = dir.path().join("stderr");
    fs::write(&stdout_file, stdout).expect("write stub stdout");
    fs::write(&stderr_file, stderr).expect("write stub stderr");
    let stub = dir.path().join("oc");
    fs::write(
        &stub,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$RTK_TEST_ARGV\"\n\
         cat \"$RTK_TEST_STDOUT\"\ncat \"$RTK_TEST_STDERR\" >&2\nexit \"$RTK_TEST_EXIT\"\n",
    )
    .expect("write oc stub");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod oc stub");
    let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("stub PATH");
    let output = common::rtk_command()
        .args(args)
        .env("PATH", path)
        .env("RTK_TEST_ARGV", &argv_file)
        .env("RTK_TEST_STDOUT", &stdout_file)
        .env("RTK_TEST_STDERR", &stderr_file)
        .env("RTK_TEST_EXIT", exit.to_string())
        .output()
        .expect("run rtk");
    let argv = fs::read_to_string(argv_file)
        .unwrap_or_else(|e| panic!("read oc argv: {e}; output: {output:?}"))
        .lines()
        .map(str::to_string)
        .collect();
    (argv, output)
}

fn oc_argv(args: &[&str]) -> Vec<String> {
    let (argv, output) = run_oc(args, "", "", 0);
    assert!(output.status.success(), "{output:?}");
    argv
}

#[test]
fn oc_aliases_forward_global_flags_before_or_after_the_subcommand() {
    for alias in ["pods", "services"] {
        for args in [
            vec!["oc", "--context", "prod", "-n", "staging", alias],
            vec!["oc", alias, "--context", "prod", "-n", "staging"],
        ] {
            assert_eq!(
                oc_argv(&args),
                [
                    "get",
                    alias,
                    "-o",
                    "json",
                    "--context",
                    "prod",
                    "-n",
                    "staging"
                ],
                "{args:?}"
            );
        }
    }
}

#[test]
fn oc_aliases_keep_all_targets_and_argument_boundaries() {
    assert_eq!(
        oc_argv(&[
            "oc",
            "--context=prod",
            "--kubeconfig=/tmp/cluster config",
            "-nstaging",
            "--as=system:serviceaccount:staging:reader",
            "pods",
        ]),
        [
            "get",
            "pods",
            "-o",
            "json",
            "--context",
            "prod",
            "--kubeconfig",
            "/tmp/cluster config",
            "-n",
            "staging",
            "--as",
            "system:serviceaccount:staging:reader",
        ]
    );
}

#[test]
fn oc_all_namespaces_still_takes_precedence_over_namespace() {
    assert_eq!(
        oc_argv(&["oc", "-n", "staging", "pods", "-A", "--context", "prod"]),
        ["get", "pods", "-o", "json", "--context", "prod", "-A"]
    );
}

#[test]
fn oc_logs_forward_globals_and_the_container() {
    assert_eq!(
        oc_argv(&[
            "oc",
            "logs",
            "api",
            "-n",
            "staging",
            "--context",
            "dev",
            "-c",
            "app",
            "--as",
            "viewer",
        ]),
        [
            "logs",
            "--tail",
            "100",
            "api",
            "-c",
            "app",
            "--context",
            "dev",
            "-n",
            "staging",
            "--as",
            "viewer",
        ]
    );
}

#[test]
fn oc_get_does_not_insert_globals_between_an_output_flag_and_its_value() {
    assert_eq!(
        oc_argv(&[
            "oc",
            "--context",
            "dev",
            "get",
            "-o",
            "wide",
            "pods",
            "-n",
            "staging",
        ]),
        [
            "get",
            "-o",
            "wide",
            "pods",
            "-n",
            "staging",
            "--context",
            "dev"
        ]
    );
    assert_eq!(
        oc_argv(&["oc", "--context", "dev", "get"]),
        ["get", "--context", "dev"]
    );
}

#[test]
fn oc_external_subcommands_keep_globals_and_their_own_arguments() {
    assert_eq!(
        oc_argv(&[
            "oc",
            "--context",
            "dev",
            "--as",
            "viewer",
            "describe",
            "deployment",
            "api",
            "--namespace",
            "other",
        ]),
        [
            "--context",
            "dev",
            "--as",
            "viewer",
            "describe",
            "deployment",
            "api",
            "--namespace",
            "other",
        ]
    );
}

#[test]
fn oc_help_and_explicit_boundaries_are_preserved() {
    assert_eq!(
        oc_argv(&["oc", "--context", "dev", "pods", "--help"]),
        ["get", "pods", "-o", "json", "--context", "dev", "--help"]
    );
    assert_eq!(
        oc_argv(&["oc", "--context", "dev", "pods", "--", "--help"]),
        ["--context", "dev", "pods", "--", "--help"]
    );
    assert_eq!(
        oc_argv(&[
            "oc",
            "--context",
            "dev",
            "get",
            "deployments",
            "--",
            "--help"
        ]),
        ["get", "deployments", "--context", "dev", "--", "--help"]
    );
    assert_eq!(
        oc_argv(&["oc", "pods", "--", "--context", "literal"]),
        ["pods", "--", "--context", "literal"]
    );
}

#[test]
fn oc_global_flags_do_not_disable_pod_compression() {
    let fixture = include_str!("fixtures/oc_pods.json");
    let (argv, output) = run_oc(&["oc", "pods", "--context", "prod"], fixture, "", 0);
    assert_eq!(argv, ["get", "pods", "-o", "json", "--context", "prod"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("19 pods:"), "{stdout}");
    assert!(
        stdout.len() * 5 < fixture.len() * 4,
        "at least 20% reduction: {stdout}"
    );
}

#[test]
fn oc_forbidden_identity_keeps_stderr_and_exit_status() {
    let diagnostic = "Error from server (Forbidden): pods is forbidden for viewer\n";
    let (argv, output) = run_oc(&["oc", "pods", "--as", "viewer"], "", diagnostic, 1);
    assert_eq!(argv, ["get", "pods", "-o", "json", "--as", "viewer"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), diagnostic);
}
