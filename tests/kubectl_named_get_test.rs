//! A named get must keep kubectl's native result instead of applying a List filter.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

mod common;

fn run_get(args: &[&str], body: &str) -> (Output, Vec<String>) {
    let dir = common::tempdir();
    let tool = dir.path().join("kubectl");
    let argv = dir.path().join("argv");
    fs::write(
        &tool,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$RTK_TEST_ARGV\"\n{body}\n"),
    )
    .expect("write kubectl");
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).expect("chmod kubectl");
    let out = common::rtk_command()
        .args(["kubectl", "get"])
        .args(args)
        .env(
            "PATH",
            format!(
                "{}:{}",
                dir.path().display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("RTK_TEST_ARGV", &argv)
        .output()
        .expect("run rtk kubectl get");
    let forwarded = fs::read_to_string(argv)
        .expect("kubectl was invoked")
        .lines()
        .map(str::to_owned)
        .collect();
    (out, forwarded)
}

#[test]
fn named_pods_keep_native_output_and_arguments() {
    let native = "NAME READY STATUS RESTARTS AGE\nweb 1/1 Running 0 1d\n";
    let body = format!(
        r#"for arg do
  if [ "$arg" = json ]; then
    printf '%s\n' '{{"kind":"Pod","metadata":{{"name":"web"}},"status":{{"phase":"Running"}}}}'
    exit 0
  fi
done
printf '{native}'"#
    );
    for resource in ["pod", "pods", "po"] {
        let args = [resource, "-n", "production", "web"];
        let (out, forwarded) = run_get(&args, &body);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(out.stdout, native.as_bytes(), "{resource}");
        assert!(out.stderr.is_empty(), "{out:?}");
        assert_eq!(forwarded, ["get", resource, "-n", "production", "web"]);
    }
}

#[test]
fn named_services_keep_native_output_and_arguments() {
    let native = "NAME TYPE CLUSTER-IP PORT(S) AGE\napi ClusterIP 10.0.0.1 443/TCP 1d\n";
    let body = format!(
        r#"for arg do
  if [ "$arg" = json ]; then
    printf '%s\n' '{{"kind":"Service","metadata":{{"name":"api"}},"spec":{{"type":"ClusterIP"}}}}'
    exit 0
  fi
done
printf '{native}'"#
    );
    for resource in ["service", "services", "svc"] {
        let args = [resource, "api", "--context=prod"];
        let (out, forwarded) = run_get(&args, &body);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(out.stdout, native.as_bytes(), "{resource}");
        assert!(out.stderr.is_empty(), "{out:?}");
        assert_eq!(forwarded, ["get", resource, "api", "--context=prod"]);
    }
}

#[test]
fn named_get_preserves_failure_streams_and_exit_status() {
    let (out, forwarded) = run_get(
        &["pods", "missing"],
        "printf 'partial output'; printf 'Error from server (NotFound)' >&2; exit 7",
    );
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(out.stdout, b"partial output");
    assert_eq!(out.stderr, b"Error from server (NotFound)");
    assert_eq!(forwarded, ["get", "pods", "missing"]);
}

#[test]
fn names_after_separator_and_multiple_names_stay_native() {
    for args in [
        vec!["pods", "--", "web"],
        vec!["pods", "web", "api"],
        vec!["services", "api", "-o", "yaml"],
    ] {
        let (out, forwarded) = run_get(&args, "printf 'native result\\n'");
        assert!(out.status.success(), "{out:?}");
        assert_eq!(out.stdout, b"native result\n");
        let expected: Vec<&str> = std::iter::once("get").chain(args).collect();
        assert_eq!(forwarded, expected);
    }
}

#[test]
fn pod_lists_with_namespace_still_use_the_list_filter() {
    let fixture = include_str!("fixtures/oc_pods.json");
    let body = format!("cat <<'RTK_PODS'\n{fixture}\nRTK_PODS");
    let (out, forwarded) = run_get(&["pods", "-n", "default"], &body);
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let shown = String::from_utf8(out.stdout).expect("UTF-8 output");
    assert!(shown.contains("pods:"), "{shown}");
    assert!(!shown.contains("No pods found"), "{shown}");
    assert!(shown.len() < fixture.len(), "list should remain compact");
    assert_eq!(forwarded, ["get", "pods", "-o", "json", "-n", "default"]);
}
