//! STS identity projections must preserve the caller's AWS CLI output and status.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

mod common;

const ACCOUNT: &str = "123456789012";
const ARN: &str = "arn:aws:iam::123456789012:user/example";

fn fake_aws(dir: &Path) {
    let path = dir.join("aws");
    fs::write(
        &path,
        r##"#!/bin/sh
if [ "$*" = "sts get-caller-identity --query [Account,Arn] --output text" ]; then
  printf '123456789012\tarn:aws:iam::123456789012:user/example\n'
elif [ "$*" = "sts get-caller-identity --query Account" ]; then
  printf '"123456789012"\n'
elif [ "$*" = "sts get-caller-identity --query=Arn" ]; then
  printf '"arn:aws:iam::123456789012:user/example"\n'
elif [ "$*" = "sts get-caller-identity --output text" ]; then
  printf '123456789012\tarn:aws:iam::123456789012:user/example\tAIDAEXAMPLE\n'
elif [ "$*" = "sts get-caller-identity --output json" ]; then
  printf '{"UserId":"AIDAEXAMPLE","Account":"123456789012","Arn":"arn:aws:iam::123456789012:user/example"}\n'
elif [ "$*" = "sts get-caller-identity --query Bad" ]; then
  printf 'Invalid --query expression\n' >&2
  exit 2
else
  printf 'unexpected aws argv: %s\n' "$*" >&2
  exit 99
fi
"##,
    )
    .expect("write fake aws");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake aws");
}

fn rtk_aws(dir: &Path, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    common::rtk_command()
        .arg("aws")
        .arg("sts")
        .arg("get-caller-identity")
        .args(args)
        .env("PATH", path)
        .output()
        .expect("run rtk aws")
}

fn assert_output(out: Output, code: i32, stdout: &str, stderr: &str) {
    assert_eq!(out.status.code(), Some(code));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), stdout);
    assert_eq!(String::from_utf8(out.stderr).unwrap(), stderr);
}

#[test]
fn array_query_keeps_explicit_text_output() {
    let dir = common::tempdir();
    fake_aws(dir.path());
    assert_output(
        rtk_aws(
            dir.path(),
            &["--query", "[Account,Arn]", "--output", "text"],
        ),
        0,
        &format!("{ACCOUNT}\t{ARN}\n"),
        "",
    );
}

#[test]
fn scalar_query_keeps_the_account() {
    let dir = common::tempdir();
    fake_aws(dir.path());
    assert_output(
        rtk_aws(dir.path(), &["--query", "Account"]),
        0,
        &format!("\"{ACCOUNT}\"\n"),
        "",
    );
}

#[test]
fn attached_query_keeps_the_arn() {
    let dir = common::tempdir();
    fake_aws(dir.path());
    assert_output(
        rtk_aws(dir.path(), &["--query=Arn"]),
        0,
        &format!("\"{ARN}\"\n"),
        "",
    );
}

#[test]
fn explicit_text_output_without_query_is_preserved() {
    let dir = common::tempdir();
    fake_aws(dir.path());
    assert_output(
        rtk_aws(dir.path(), &["--output", "text"]),
        0,
        &format!("{ACCOUNT}\t{ARN}\tAIDAEXAMPLE\n"),
        "",
    );
}

#[test]
fn ordinary_identity_still_uses_the_summary() {
    let dir = common::tempdir();
    fake_aws(dir.path());
    assert_output(
        rtk_aws(dir.path(), &[]),
        0,
        &format!("AWS: {ACCOUNT} {ARN}\n"),
        "",
    );
}

#[test]
fn query_failure_keeps_the_aws_error_and_exit_status() {
    let dir = common::tempdir();
    fake_aws(dir.path());
    assert_output(
        rtk_aws(dir.path(), &["--query", "Bad"]),
        2,
        "",
        "Invalid --query expression\n",
    );
}
