//! The CLI must forward the separator clap consumes before passthrough arguments.

#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};

mod common;

fn assert_forwarded(
    launcher_args: &[&str],
    route: &[&str],
    injected: &[&str],
    user_args: &[&OsStr],
    exit: i32,
) {
    let dir = common::tempdir();
    let capture = dir.path().join("argv");
    let stub = dir.path().join(route[0]);
    fs::write(
        &stub,
        "#!/bin/sh\nfor arg do printf '%s\\000' \"$arg\"; done > \"$RTK_TEST_ARGV\"\nexit \"$RTK_TEST_EXIT\"\n",
    )
    .expect("write stub");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod stub");

    let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("stub PATH");
    let out = common::rtk_command()
        .args(launcher_args)
        .args(route)
        .args(user_args)
        .env("PATH", path)
        .env("RTK_TEST_ARGV", &capture)
        .env("RTK_TEST_EXIT", exit.to_string())
        .current_dir(dir.path())
        .output()
        .expect("run rtk");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(exit),
        "rtk {launcher_args:?} {route:?} {user_args:?}: {stderr}"
    );
    let mut argv = fs::read(capture).unwrap_or_else(|e| {
        panic!("rtk {route:?} {user_args:?} did not record argv: {e}: {stderr}")
    });
    assert_eq!(argv.pop(), Some(0), "argv is NUL terminated");
    let expected: Vec<_> = route[1..]
        .iter()
        .chain(injected)
        .map(|arg| arg.as_bytes())
        .chain(user_args.iter().map(|arg| arg.as_bytes()))
        .collect();
    assert_eq!(
        argv.split(|byte| *byte == 0).collect::<Vec<_>>(),
        expected,
        "rtk {launcher_args:?} {route:?} {user_args:?}"
    );
}

const ROUTES: &[(&[&str], &[&str])] = &[
    (&["mvnd"], &[]),
    (&["oc"], &[]),
    (&["sbt"], &[]),
    (&["gt"], &[]),
    (&["bun"], &[]),
    (&["deno"], &[]),
    (&["docker", "compose"], &[]),
    (&["go", "test"], &["-json"]),
    (&["go", "build"], &[]),
    (&["go", "vet"], &[]),
    (&["kubectl", "get"], &[]),
];

#[test]
fn forwards_the_leading_separator_for_every_affected_route() {
    for &(route, injected) in ROUTES {
        for (launcher, exit) in [(&[][..], 0), (&["-vv"][..], 7)] {
            let args = ["--", "fixture", "", "two words", "雪\nline"].map(OsStr::new);
            assert_forwarded(launcher, route, injected, &args, exit);
        }
    }
}

#[test]
fn does_not_duplicate_the_separator_or_change_ordinary_argv() {
    for &(route, injected) in ROUTES {
        for args in [
            &["fixture", "--", "next"][..],
            &["fixture", "two words"][..],
            &["--"][..],
            &["--", "--", "fixture"][..],
        ] {
            let args: Vec<_> = args.iter().map(OsStr::new).collect();
            assert_forwarded(&[], route, injected, &args, 0);
        }
    }
}

#[test]
fn preserves_literal_subcommand_names_and_non_utf8_operands() {
    // Without the separator, po is routed through the filtered pods alias.
    assert_forwarded(
        &[],
        &["docker", "compose"],
        &[],
        &[OsStr::new("--"), OsStr::new("ps")],
        7,
    );
    assert_forwarded(
        &[],
        &["kubectl", "get"],
        &[],
        &[OsStr::new("--"), OsStr::new("po")],
        7,
    );
    assert_forwarded(
        &[],
        &["oc"],
        &[],
        &[
            OsStr::new("--"),
            OsStr::new("fixture"),
            OsStr::from_bytes(b"operand-\xff"),
        ],
        7,
    );
}
