//! A leading `--` for the passthrough commands of #4753. clap's `trailing_var_arg` strips a
//! `--` that leads `args` (or an external subcommand), so `rtk tsc -- zzz` forwarded
//! `tsc zzz` and a path after the separator was read as an option or a subcommand. The
//! separator has to reach the tool, after any options rtk injects, and a word after it such
//! as `run` must stay an argument. Stubs each tool on PATH with a script that records its argv.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;

mod common;

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// Runs `rtk <tool> <args>` against a `<tool>` stub and returns the argv the stub received.
fn tool_argv(tool: &str, args: &[&str]) -> Vec<String> {
    let dir = common::tempdir();
    let argv_file = dir.path().join("argv.txt");
    let stub_path = dir.path().join(tool);

    fs::write(
        &stub_path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n",
            shell_quote(&argv_file)
        ),
    )
    .expect("write stub");
    fs::set_permissions(&stub_path, fs::Permissions::from_mode(0o755)).expect("chmod stub");

    let path_with_stub = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = common::rtk_command()
        .arg(tool)
        .args(args)
        .env("PATH", path_with_stub)
        .env("LC_ALL", "C")
        .current_dir(dir.path())
        .output()
        .expect("spawn rtk");

    fs::read_to_string(&argv_file)
        .unwrap_or_else(|e| {
            panic!(
                "read captured argv for {tool}: {e}; rtk stdout={} stderr={}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        })
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn leading_double_dash_reaches_the_tool() {
    let cases: &[(&str, &[&str])] = &[
        ("tsc", &[]),
        ("prettier", &[]),
        ("go", &[]),
        ("cargo", &[]),
        ("phpunit", &[]),
        ("phpstan", &[]),
        ("mvn", &[]),
        ("kubectl", &[]),
        ("docker", &[]),
        ("rspec", &["--format", "json"]),
        ("rubocop", &["--format", "json"]),
        ("vitest", &["run", "--reporter=json"]),
        ("jest", &["--no-watch", "--json"]),
        ("pest", &["--no-progress"]),
    ];
    let mut failures = Vec::new();
    for (tool, injected) in cases {
        let mut expected: Vec<&str> = injected.to_vec();
        expected.extend(["--", "zzz"]);
        let argv = tool_argv(tool, &["--", "zzz"]);
        if argv != expected {
            failures.push(format!("{tool}: got {argv:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_double_dash_after_an_argument_is_forwarded_once() {
    assert_eq!(tool_argv("tsc", &["-p", "--", "zzz"]), ["-p", "--", "zzz"]);
    assert_eq!(tool_argv("go", &["zzz", "--", "zzz"]), ["zzz", "--", "zzz"]);
}

#[test]
fn run_after_double_dash_stays_an_argument() {
    assert_eq!(
        tool_argv("vitest", &["--", "run"]),
        ["run", "--reporter=json", "--", "run"]
    );
    assert_eq!(
        tool_argv("jest", &["--", "run"]),
        ["--no-watch", "--json", "--", "run"]
    );
}
