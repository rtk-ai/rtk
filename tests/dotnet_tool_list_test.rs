//! Exercise dotnet tool routing without depending on an installed SDK or tool store.
mod common;

use std::ffi::OsStr;
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

const GLOBAL: &str = include_str!("fixtures/dotnet/tool_list_global.txt");
const LOCAL: &str = include_str!("fixtures/dotnet/tool_list_local.txt");
const EMPTY: &str = include_str!("fixtures/dotnet/tool_list_empty.txt");
const COMPACT_GLOBAL: &str = "Package Id\tVersion\tCommands\ncsharp-ls\t0.28.0\tcsharp-ls\ndotnet-outdated-tool\t4.8.1\tdotnet-outdated\n";

fn run(args: &[impl AsRef<OsStr>], stdout: &str, stderr: &str, code: i32) -> Output {
    let dir = common::tempdir();
    #[cfg(unix)]
    let stub = dir.path().join("dotnet");
    #[cfg(windows)]
    let stub = dir.path().join("dotnet.cmd");
    let captured = dir.path().join("argv");
    let stdout_file = dir.path().join("stdout");
    let stderr_file = dir.path().join("stderr");
    std::fs::write(&stdout_file, stdout).expect("write native stdout");
    std::fs::write(&stderr_file, stderr).expect("write native stderr");
    #[cfg(unix)]
    std::fs::write(
        &stub,
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$TOOL_ARGS\"\n/bin/cat \"$TOOL_STDOUT_FILE\"\n/bin/cat \"$TOOL_STDERR_FILE\" >&2\nexit \"$TOOL_EXIT\"\n",
    )
    .expect("write dotnet stub");
    #[cfg(windows)]
    std::fs::write(
        &stub,
        "@echo off\r\ntype \"%TOOL_STDOUT_FILE%\"\r\ntype \"%TOOL_STDERR_FILE%\" 1>&2\r\nexit /b %TOOL_EXIT%\r\n",
    )
    .expect("write dotnet stub");
    #[cfg(unix)]
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
        .expect("make dotnet stub executable");

    let output = common::rtk_command()
        .env("PATH", dir.path())
        .env("PATHEXT", ".COM;.EXE;.BAT;.CMD")
        .env("TOOL_ARGS", &captured)
        .env("TOOL_STDOUT_FILE", &stdout_file)
        .env("TOOL_STDERR_FILE", &stderr_file)
        .env("TOOL_EXIT", code.to_string())
        .current_dir(dir.path())
        .arg("dotnet")
        .args(args)
        .output()
        .expect("run rtk with dotnet stub");

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let expected: Vec<u8> = args
            .iter()
            .flat_map(|arg| arg.as_ref().as_bytes().iter().copied().chain([0]))
            .collect();
        assert_eq!(
            std::fs::read(captured).expect("read captured arguments"),
            expected,
            "dotnet must receive the original arguments without injected flags"
        );
    }
    assert_eq!(output.status.code(), Some(code));
    output
}

#[test]
fn dotnet_tool_list_compacts_global_and_custom_path_tables() {
    for args in [
        vec!["tool", "list", "--global"],
        vec!["tool", "list", "-g", "csharp-ls"],
        vec!["tool", "list", "--tool-path", "/workspace/tool store"],
        vec!["tool", "list", "--tool-path=/workspace/tool store"],
    ] {
        let output = run(&args, GLOBAL, "", 0);
        assert_eq!(String::from_utf8_lossy(&output.stdout), COMPACT_GLOBAL);
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn dotnet_tool_list_keeps_local_manifest_paths_with_spaces() {
    for args in [vec!["tool", "list"], vec!["tool", "list", "--local"]] {
        let output = run(&args, LOCAL, "", 0);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "Package Id\tVersion\tCommands\tManifest\ncsharp-ls\t0.28.0\tcsharp-ls\t/workspace/sample tools/.config/dotnet-tools.json\ndotnet-outdated-tool\t4.8.1\tdotnet-outdated\t/workspace/sample tools/.config/dotnet-tools.json\n"
        );
    }
}

#[test]
fn dotnet_tool_list_leaves_empty_and_unrecognized_output_unchanged() {
    for stdout in [EMPTY, "", "An unfamiliar SDK message.\n"] {
        let output = run(&["tool", "list"], stdout, "", 0);
        assert_eq!(output.stdout, stdout.as_bytes());
    }
}

#[test]
fn dotnet_tool_list_preserves_failure_output_and_stderr() {
    let output = run(&["tool", "list"], GLOBAL, "fixture failure\n", 7);
    assert_eq!(output.stdout, GLOBAL.as_bytes());
    assert_eq!(output.stderr, b"fixture failure\n");

    let output = run(&["tool", "list"], GLOBAL, "fixture warning\n", 0);
    assert_eq!(output.stdout, COMPACT_GLOBAL.as_bytes());
    assert_eq!(output.stderr, b"fixture warning\n");
}

#[test]
fn dotnet_tool_list_preserves_explicit_json_output() {
    for (args, raw) in [
        (
            vec!["tool", "list", "--global", "--format", "json"],
            include_str!("fixtures/dotnet/tool_list_json_global.txt"),
        ),
        (
            vec!["tool", "list", "--format=json"],
            include_str!("fixtures/dotnet/tool_list_json_local.txt"),
        ),
        (
            vec![
                "tool",
                "list",
                "--tool-path",
                "/workspace/tool store",
                "--format",
                "json",
            ],
            include_str!("fixtures/dotnet/tool_list_json_empty.txt"),
        ),
    ] {
        let output = run(&args, raw, "", 0);
        assert_eq!(output.stdout, raw.as_bytes());
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn dotnet_tool_list_preserves_json_line_endings_and_eof() {
    for raw in [
        "{\"version\":1,\"data\":[]}\r\n",
        "{\"version\":1,\"data\":[]}",
    ] {
        let output = run(&["tool", "list", "--format", "json"], raw, "", 0);
        assert_eq!(output.stdout, raw.as_bytes());
    }
}

#[test]
fn dotnet_tool_list_respects_json_option_values_and_boundaries() {
    let output = run(&["tool", "list", "--format=json"], GLOBAL, "", 0);
    assert_eq!(output.stdout, GLOBAL.as_bytes());
    for args in [
        vec!["tool", "list", "--format", "table"],
        vec!["tool", "list", "--tool-path", "json"],
        vec!["tool", "list", "--tool-path", "--format=json"],
        vec!["tool", "list", "--", "--format=json"],
    ] {
        let output = run(&args, GLOBAL, "", 0);
        assert_eq!(output.stdout, COMPACT_GLOBAL.as_bytes());
    }
}

#[test]
fn dotnet_tool_list_preserves_raw_and_failure_streams() {
    for code in [0, 7] {
        for (stdout, stderr) in [
            ("unfamiliar output\r\n", "diagnostic\r\n"),
            ("unfamiliar output", "diagnostic without newline"),
            (" \r\n", "\r\n"),
        ] {
            let output = run(&["tool", "list"], stdout, stderr, code);
            assert_eq!(output.stdout, stdout.as_bytes());
            assert_eq!(output.stderr, stderr.as_bytes());
        }
    }
}

#[test]
fn dotnet_tool_list_preserves_output_above_the_line_capture_limit() {
    let items = (0..85_000)
        .map(|i| format!(
            "{{\"packageId\":\"sample.tool.n{i:05}\",\"version\":\"1.0.0\",\"commands\":[\"tool{i:05}\"],\"manifest\":\"/workspace/tool store/.config/dotnet-tools.json\"}}"
        ))
        .collect::<Vec<_>>()
        .join(",");
    let raw = format!("{{\"version\":1,\"data\":[{items}]}}\n");
    assert!(raw.len() > 10 * 1024 * 1024);
    for args in [vec!["tool", "list", "--format=json"], vec!["tool", "list"]] {
        let output = run(&args, &raw, "", 0);
        assert!(output.stderr.is_empty());
        assert_eq!(output.stdout.len(), raw.len());
        assert_eq!(output.stdout, raw.as_bytes());
    }
}

#[test]
fn dotnet_tool_other_operations_and_help_stay_unfiltered() {
    for args in [
        vec!["tool"],
        vec!["tool", "--help"],
        vec!["tool", "list", "--help"],
        vec!["tool", "list", "-h"],
        vec!["tool", "list", "-?"],
        vec!["tool", "install", "list"],
        vec!["tool", "restore"],
        vec!["tool", "run", "list"],
        vec!["tool", "list-extra"],
        vec!["tool", "--"],
        vec!["tool", "--", "--help"],
        vec!["tool", "--", "list"],
    ] {
        let output = run(&args, GLOBAL, "", 0);
        assert_eq!(output.stdout, GLOBAL.as_bytes(), "arguments: {args:?}");
    }
}

#[cfg(unix)]
#[test]
fn dotnet_tool_list_preserves_boundaries_and_non_utf8_paths() {
    use std::os::unix::ffi::OsStringExt;

    let args = vec![
        OsString::from("tool"),
        OsString::from("list"),
        OsString::from("--tool-path"),
        OsString::from_vec(b"/workspace/tools-\xff".to_vec()),
        OsString::from("--"),
        OsString::from("--global"),
    ];
    let output = run(&args, GLOBAL, "", 0);
    assert_eq!(output.stdout, COMPACT_GLOBAL.as_bytes());
}
