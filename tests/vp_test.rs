//! Process-level contracts for the Vite+ test adapter.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

mod common;

const PASS: &str = "\n RUN  v5.1.1 /fixture\n\n\n Test Files  1 passed (1)\n      Tests  2 passed (2)\n   Start at  11:56:27\n   Duration  274ms (transform 51%, import 31%, worker 9%, tests 9%)\n\n";

fn run_vp(dir: &Path, args: &[&str], stdout: &str, stderr: &str, code: i32) -> Output {
    let exe = dir.join("vp");
    fs::write(&exe, "#!/bin/sh\nprintf '%s\\0' \"$@\" >> \"$VP_ARGV\"\ncat \"$VP_STDOUT\"\ncat \"$VP_STDERR\" >&2\nexit \"$VP_EXIT\"\n")
        .expect("write fake vp");
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).expect("chmod fake vp");
    let argv = dir.join("argv");
    fs::write(&argv, b"").expect("reset argv log");
    fs::write(dir.join("stdout"), stdout).expect("write stdout");
    fs::write(dir.join("stderr"), stderr).expect("write stderr");
    let path = std::env::join_paths(std::iter::once(dir.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("join PATH");
    let out = common::rtk_command()
        .arg("vp")
        .args(args)
        .env("PATH", path)
        .env("VP_ARGV", &argv)
        .env("VP_STDOUT", dir.join("stdout"))
        .env("VP_STDERR", dir.join("stderr"))
        .env("VP_EXIT", code.to_string())
        .current_dir(dir)
        .output()
        .expect("run rtk vp");
    let expected: Vec<u8> = args.iter().flat_map(|arg| arg.bytes().chain([0])).collect();
    assert_eq!(
        fs::read(argv).expect("read argv"),
        expected,
        "native argv must survive exactly once"
    );
    out
}

#[test]
fn successful_one_shot_keeps_native_counts_and_stderr() {
    let dir = common::tempdir();
    let out = run_vp(
        dir.path(),
        &["test", "run", "path with spaces"],
        PASS,
        "warning!\n",
        0,
    );
    assert!(out.status.success());
    assert_eq!(
        out.stdout,
        b" Test Files  1 passed (1)\n      Tests  2 passed (2)\n"
    );
    assert_eq!(out.stderr, b"warning!\n");
}

#[test]
fn failure_and_unrecognized_output_survive_verbatim() {
    let dir = common::tempdir();
    let out = run_vp(dir.path(), &["test"], PASS, "unhandled error\n", 17);
    assert_eq!(out.status.code(), Some(17));
    assert_eq!(out.stdout, PASS.as_bytes());
    assert_eq!(out.stderr, b"unhandled error\n");
    for raw in [
        "{\"numPassedTests\":2}\n",
        "TAP version 13\n1..2\n",
        "nothing recognized\n",
    ] {
        let out = run_vp(dir.path(), &["test"], raw, "", 0);
        assert!(out.status.success());
        assert_eq!(out.stdout, raw.as_bytes());
    }
}

#[test]
fn flags_other_modes_and_other_subcommands_pass_through() {
    let dir = common::tempdir();
    for args in [
        &["--help"][..],
        &["--version"],
        &["test", "--reporter=json"],
        &["test", "--reporter", "verbose"],
        &["test", "--outputFile.json=result.json"],
        &["test", "--help"],
        &["test", "--watch"],
        &["test", "watch"],
        &["test", "dev"],
        &["test", "list"],
        &["test", "bench"],
        &["test", "related", "src/x.ts"],
        &["test", "--coverage"],
        &["-C", "my workspace", "test"],
        &["test", "--", "--watch"],
        &["lint", "--fix"],
    ] {
        let out = run_vp(dir.path(), args, PASS, "", 0);
        assert!(out.status.success());
        assert_eq!(out.stdout, PASS.as_bytes(), "{args:?}");
    }
}

#[test]
fn wants_only_vp_test_rewritten() {
    for (line, expected) in [
        (
            "vp test run src/foo.test.ts",
            Some("rtk vp test run src/foo.test.ts"),
        ),
        ("vp test", Some("rtk vp test")),
        ("vp lint", None),
        ("vp build", None),
        ("vp tester", None),
    ] {
        let out = common::rtk_command()
            .args(["rewrite", line])
            .output()
            .expect("rewrite");
        let output = String::from_utf8(out.stdout).expect("UTF-8 rewrite");
        if let Some(expected) = expected {
            assert!(
                matches!(out.status.code(), Some(0) | Some(3)),
                "{line}: {:?}",
                out.status
            );
            assert_eq!(output.trim(), expected);
        } else {
            assert!(output.trim().is_empty(), "{line}: {output}");
        }
    }
}

#[test]
fn explicit_one_shot_modes_keep_optimization_hint() {
    // Real Vite+ 1.8.0 default reporter output, with path shortened.
    let raw = "\n RUN  v5.1.1 /fixture\n\n\n Test Files  20 passed (20)\n      Tests  20 passed (20)\n   Start at  14:21:05\n   Duration  820ms (transform 45%, import 35%, worker 11%, tests 8%)\n\n    Isolate  20 workers spawned · ~233ms startup each (spawn + environment, per file)\n             at least ~349ms faster with isolate: false — reuses workers across files instead of one per file\n\n";
    let expected = " Test Files  20 passed (20)\n      Tests  20 passed (20)\n\n    Isolate  20 workers spawned · ~233ms startup each (spawn + environment, per file)\n             at least ~349ms faster with isolate: false — reuses workers across files instead of one per file\n\n";
    let dir = common::tempdir();
    for flag in ["--run", "--watch=false"] {
        let out = run_vp(dir.path(), &["test", flag], raw, "", 0);
        assert!(out.status.success());
        assert_eq!(out.stdout, expected.as_bytes());
        assert!(out.stdout.len() * 5 <= raw.len() * 4);
    }
}

#[test]
fn existing_custom_vp_filters_keep_precedence() {
    let dir = common::tempdir();
    fs::create_dir(dir.path().join(".rtk")).expect("filter directory");
    fs::write(
        dir.path().join(".rtk/filters.toml"),
        r#"schema_version = 1

[filters.vp-existing]
match_command = '^vp (lint|test)\b'
strip_lines_matching = ['^CUSTOM_NOISE']
"#,
    )
    .expect("write filters");
    let trust = common::rtk_command()
        .current_dir(dir.path())
        .args(["trust", "--yes"])
        .output()
        .expect("trust fixture filters");
    assert!(trust.status.success(), "{:?}", trust);
    for args in [&["lint"][..], &["test"]] {
        let out = run_vp(dir.path(), args, "CUSTOM_NOISE\nimportant\n", "", 0);
        assert!(out.status.success());
        let stdout = String::from_utf8(out.stdout).expect("UTF-8 stdout");
        assert_eq!(stdout.trim(), "important");
    }
}

#[test]
fn unavailable_vp_keeps_fallback_exit_status() {
    let dir = common::tempdir();
    let out = common::rtk_command()
        .args(["vp", "test"])
        .env("PATH", dir.path())
        .current_dir(dir.path())
        .output()
        .expect("run rtk with no vp on PATH");
    assert_eq!(out.status.code(), Some(127));
    assert!(out.stdout.is_empty());
    assert!(!out.stderr.is_empty());
}
