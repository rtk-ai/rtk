mod common;

fn assert_rewrite(command: &str, expected: Option<&str>) {
    let output = common::rtk_command()
        .args(["rewrite", command])
        .output()
        .unwrap();
    if let Some(expected) = expected {
        assert!(
            matches!(output.status.code(), Some(0 | 3)),
            "{command}: {output:?}"
        );
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .trim_end_matches('\n'),
            expected,
            "{command}"
        );
    } else {
        assert_eq!(output.status.code(), Some(1), "{command}: {output:?}");
        assert!(output.stdout.is_empty(), "{command}: {output:?}");
    }
}

#[test]
fn toolchain_rewrites_preserve_the_original_command() {
    for toolchain in ["+stable", "+nightly", "+1.75.0", "+nightly-2026-08-01"] {
        for command in ["build", "check", "clippy", "test", "install", "fmt"] {
            let original = format!("cargo {toolchain} {command} -p demo");
            assert_rewrite(&original, Some(&format!("rtk {original}")));
        }
    }
    for original in [
        "cargo --locked check",
        "cargo --offline test",
        "cargo --frozen build",
        "cargo --color never check",
        "cargo --color=never check",
        "cargo --config check build",
        "cargo --config=check build",
        "cargo -C check build",
        "cargo -C=check build",
        "cargo -Ccheck build",
        "cargo -Z check build",
        "cargo -Z=check build",
        "cargo -Zcheck build",
        "cargo -q check",
        "cargo -vv build",
        "cargo --verbose --quiet check",
        "cargo +nightly -vvZunstable-options -C 'some path' check",
        "cargo +nightly -Zscript check",
        "cargo +nightly -Z script check",
        r#"cargo  +stable  --config 'build.rustflags=["--cfg", "demo"]' check -p "some package""#,
        "cargo\t+stable\tcheck",
    ] {
        assert_rewrite(original, Some(&format!("rtk {original}")));
    }
    assert_rewrite(
        "RUSTUP_TOOLCHAIN=nightly cargo --locked check",
        Some("RUSTUP_TOOLCHAIN=nightly rtk cargo --locked check"),
    );
}

#[test]
fn ambiguous_prefixes_and_unsupported_commands_are_not_rewritten() {
    for original in [
        "cargo",
        "cargo +stable",
        "cargo --config",
        "cargo --config check metadata",
        "cargo --config=check metadata",
        "cargo --color check metadata",
        "cargo --color=check metadata",
        "cargo -C check metadata",
        "cargo -C=check metadata",
        "cargo -Ccheck metadata",
        "cargo -Z check metadata",
        "cargo -Z=check metadata",
        "cargo -Zcheck metadata",
        "cargo --locked +stable check",
        "cargo +stable +nightly check",
        "cargo --unknown check",
        "cargo -vx check",
        "cargo --version check",
        "cargo -V check",
        "cargo --list check",
        "cargo --explain E0425 check",
        "cargo --explain=E0425 check",
        "cargo --help check",
        "cargo -h check",
        "cargo -Zhelp check",
        "cargo -- check",
        "cargo +stable -- check",
        "cargo --config -- check",
        "cargo --config --help check",
        "cargo --color -- check",
        "cargo -C -- check",
        "cargo -Z -- check",
        "cargo +nightly -Zscript file.rs check",
        "cargo +stable metadata",
        "cargo +stable checker",
        "cargo b",
        "cargo c",
        "cargo t",
        "cargo +stable t",
        "cargo --locked c",
        "cargo nextest run",
        "cargo nextest list",
        "cargo nextest show-config test-groups",
        "cargo +stable nextest list",
    ] {
        assert_rewrite(original, None);
    }
}

#[cfg(unix)]
mod execution {
    use super::common;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::PermissionsExt;
    use std::process::Output;

    const HUMAN: &str = concat!(
        "   Compiling dependency v0.1.0\n",
        "   Compiling demo v0.1.0\n",
        "error[E0308]: mismatched types\n",
        " --> src/lib.rs:1:1\n",
        "  |\n",
        "1 | let value: i32 = \"wrong\";\n",
        "  |                  ^^^^^^^ expected i32\n\n",
        "error: could not compile demo due to 1 previous error\n",
    );
    const NATIVE_STDOUT: &str = "Native Cargo output: preserve every line of this information request or unsupported command.\nThis output is neither a compiler diagnostic nor a test result.\n";
    const NATIVE_STDERR: &str =
        "Native Cargo diagnostic: preserve this separate stream and the original exit status.\n";
    const JSON: &str = concat!(
        "{\"reason\":\"compiler-artifact\",\"package_id\":\"dependency 0.1.0\",\"filenames\":[\"target/dependency.rlib\"]}\n",
        "{\"reason\":\"compiler-message\",\"message\":{\"level\":\"error\",\"code\":{\"code\":\"E0308\"},\"rendered\":\"error[E0308]: JSON diagnostic marker\\n --> src/lib.rs:1:1\\n\"}}\n",
        "{\"reason\":\"build-finished\",\"success\":false}\n",
    );

    struct CargoStub {
        dir: tempfile::TempDir,
    }

    impl CargoStub {
        fn new() -> Self {
            let dir = common::tempdir();
            let executable = dir.path().join("cargo");
            fs::write(
                &executable,
                r#"#!/bin/sh
: > "$CARGO_ARGV_FILE"
if [ "$#" -gt 0 ]; then
    printf '%s\0' "$@" > "$CARGO_ARGV_FILE"
fi
cat "$CARGO_STDOUT_FILE"
cat "$CARGO_STDERR_FILE" >&2
exit "$CARGO_EXIT_CODE"
"#,
            )
            .unwrap();
            fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }

        fn run<T: AsRef<OsStr>>(
            &self,
            args: &[T],
            stdout: &str,
            stderr: &str,
            exit: i32,
            verbose: bool,
        ) -> Output {
            let stdout_file = self.dir.path().join("stdout");
            let stderr_file = self.dir.path().join("stderr");
            let argv_file = self.dir.path().join("argv");
            fs::write(&stdout_file, stdout).unwrap();
            fs::write(&stderr_file, stderr).unwrap();
            let path = std::env::join_paths(std::iter::once(self.dir.path().to_path_buf()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ))
            .unwrap();
            let mut cmd = common::rtk_command();
            if verbose {
                cmd.arg("-v");
            }
            let output = cmd
                .arg("cargo")
                .args(args.iter().map(AsRef::as_ref))
                .env("PATH", path)
                .env("CARGO_ARGV_FILE", &argv_file)
                .env("CARGO_STDOUT_FILE", stdout_file)
                .env("CARGO_STDERR_FILE", stderr_file)
                .env("CARGO_EXIT_CODE", exit.to_string())
                .env("RTK_DB_PATH", self.dir.path().join("tracking.db"))
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(exit), "{output:?}");
            let expected: Vec<u8> = args
                .iter()
                .flat_map(|arg| arg.as_ref().as_bytes().iter().copied().chain([0]))
                .collect();
            assert_eq!(fs::read(argv_file).unwrap(), expected, "{output:?}");
            output
        }

        fn assert_native(&self, args: &[&str]) {
            let output = self.run(args, NATIVE_STDOUT, NATIVE_STDERR, 37, false);
            assert_eq!(output.stdout, NATIVE_STDOUT.as_bytes(), "{args:?}");
            assert_eq!(output.stderr, NATIVE_STDERR.as_bytes(), "{args:?}");
        }
    }

    fn combined(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    #[test]
    fn prefixes_select_the_existing_filters_and_preserve_child_argv() {
        let cargo = CargoStub::new();
        for prefix in [
            vec![],
            vec!["+stable"],
            vec!["+nightly-2026-08-01"],
            vec!["--locked"],
            vec!["--offline"],
            vec!["--frozen"],
            vec!["--quiet"],
            vec!["--verbose"],
            vec!["-q"],
            vec!["-vv"],
            vec!["--color", "never"],
            vec!["--color=never"],
            vec!["--config", "check"],
            vec!["--config=check"],
            vec!["-C", "check"],
            vec!["-Ccheck"],
            vec!["-C=check"],
            vec!["-Z", "check"],
            vec!["-Zcheck"],
            vec!["-Z=check"],
            vec!["+nightly", "-vvZunstable-options", "-C", "some path"],
            vec!["+nightly", "-Zscript"],
        ] {
            for command in ["build", "check", "clippy", "test", "install"] {
                let args: Vec<_> = prefix
                    .iter()
                    .copied()
                    .chain([command, "-p", "demo", "--", "", "--nocapture"])
                    .collect();
                let output = cargo.run(&args, "", HUMAN, 101, false);
                let text = combined(&output);
                assert!(
                    text.contains(&format!("cargo {command}:")),
                    "{args:?}: {text}"
                );
                assert!(text.contains("error[E0308]"), "{args:?}: {text}");
                assert!(!text.contains("Compiling"), "{args:?}: {text}");
            }
        }
    }

    #[test]
    fn unknown_or_action_prefixes_and_aliases_pass_through_unchanged() {
        let cargo = CargoStub::new();
        for args in [
            vec![],
            vec!["+stable"],
            vec!["--locked", "+stable", "check"],
            vec!["+stable", "+nightly", "check"],
            vec!["--config", "check", "metadata"],
            vec!["--config=check", "metadata"],
            vec!["--color", "check", "metadata"],
            vec!["--color=check", "metadata"],
            vec!["-C", "check", "metadata"],
            vec!["-Ccheck", "metadata"],
            vec!["-C=check", "metadata"],
            vec!["-Z", "check", "metadata"],
            vec!["-Zcheck", "metadata"],
            vec!["-Z=check", "metadata"],
            vec!["--config"],
            vec!["--unknown", "check"],
            vec!["--unknown=value", "check"],
            vec!["-vx", "check"],
            vec!["--version", "check"],
            vec!["-V", "check"],
            vec!["--list", "check"],
            vec!["--explain", "E0425", "check"],
            vec!["--explain=E0425", "check"],
            vec!["--help", "check"],
            vec!["-h", "check"],
            vec!["-Zhelp", "check"],
            vec!["-Z=help", "check"],
            vec!["--", "check"],
            vec!["--", "--", "check"],
            vec!["+stable", "--", "check"],
            vec!["--locked", "--", "check"],
            vec!["--config", "--", "check"],
            vec!["--config", "--help", "check"],
            vec!["--color", "--", "check"],
            vec!["-C", "--", "check"],
            vec!["-Z", "--", "check"],
            vec!["+nightly", "-Zscript", "file.rs", "check"],
            vec!["metadata"],
            vec!["fmt", "--check"],
            vec!["b"],
            vec!["c"],
            vec!["t"],
            vec!["+stable", "t"],
        ] {
            cargo.assert_native(&args);
        }
    }

    #[test]
    fn attached_short_values_do_not_disable_filtering_but_help_still_does() {
        let cargo = CargoStub::new();
        for command in ["build", "test"] {
            for prefix in [vec![], vec!["+stable", "--locked"]] {
                for option in ["-pshell", "-Fwith_help"] {
                    let args: Vec<_> = prefix.iter().copied().chain([command, option]).collect();
                    let output = cargo.run(&args, "", HUMAN, 101, false);
                    let text = combined(&output);
                    assert!(
                        text.contains(&format!("cargo {command}:")),
                        "{args:?}: {text}"
                    );
                    assert!(!text.contains("Compiling"), "{args:?}: {text}");
                }
                for option in ["-h", "--help"] {
                    let args: Vec<_> = prefix.iter().copied().chain([command, option]).collect();
                    cargo.assert_native(&args);
                }
            }
        }
        cargo.assert_native(&["+stable", "test", "--", "--help"]);
    }

    #[test]
    fn json_arguments_select_json_filters_for_build_check_and_clippy() {
        let cargo = CargoStub::new();
        for command in ["build", "check", "clippy"] {
            for prefix in [vec![], vec!["+stable", "--config", "check"]] {
                for format in [
                    vec!["--message-format=json"],
                    vec!["--message-format", "json"],
                ] {
                    let args: Vec<_> = prefix
                        .iter()
                        .copied()
                        .chain([command])
                        .chain(format)
                        .collect();
                    let output = cargo.run(&args, JSON, "   Compiling demo v0.1.0\n", 101, false);
                    let text = combined(&output);
                    assert!(
                        text.contains(&format!("cargo {command}:")),
                        "{args:?}: {text}"
                    );
                    assert!(text.contains("JSON diagnostic marker"), "{args:?}: {text}");
                    assert!(!text.contains("compiler-artifact"), "{args:?}: {text}");
                    assert!(!text.contains("\"reason\""), "{args:?}: {text}");
                }
            }
        }
    }

    #[test]
    fn install_version_remains_a_value_with_a_prefix() {
        let cargo = CargoStub::new();
        for prefix in [vec![], vec!["+stable", "--locked"]] {
            for version in [vec!["--version", "1.2.3"], vec!["--version=1.2.3"]] {
                let args: Vec<_> = prefix
                    .iter()
                    .copied()
                    .chain(["install", "demo"])
                    .chain(version)
                    .collect();
                let output = cargo.run(&args, "", HUMAN, 101, false);
                let text = combined(&output);
                assert!(text.contains("cargo install:"), "{args:?}: {text}");
                assert!(!text.contains("Compiling"), "{args:?}: {text}");
            }
        }
    }

    #[test]
    fn direct_nextest_run_keeps_its_existing_filter_and_prefix() {
        let cargo = CargoStub::new();
        let output = cargo.run(
            &["+stable", "--locked", "nextest", "run"],
            "",
            concat!(
                "    Starting 50 tests across 2 binaries\n",
                "        PASS [   0.010s] (1/50) demo::test_one\n",
                "        PASS [   0.010s] (50/50) demo::test_last\n",
                "     Summary [   0.500s] 50 tests run: 50 passed\n",
            ),
            0,
            false,
        );
        assert!(combined(&output).contains("cargo nextest: 50 passed"));
    }

    #[test]
    fn execution_keeps_non_utf8_arguments() {
        let cargo = CargoStub::new();
        let args = [
            OsString::from("+stable"),
            OsString::from("--config"),
            OsString::from_vec(vec![0xff]),
            OsString::from("check"),
            OsString::from("--manifest-path"),
            OsString::from_vec(vec![0xfe]),
        ];
        let output = cargo.run(&args, "", HUMAN, 101, false);
        assert!(combined(&output).contains("cargo check:"));
    }

    #[test]
    fn tracking_and_verbose_output_include_the_complete_original_arguments() {
        let cargo = CargoStub::new();
        let args = [
            "+nightly",
            "-Zunstable-options",
            "-C",
            "some path",
            "--config",
            "build.jobs=2",
            "check",
            "--manifest-path",
            "some workspace/Cargo.toml",
            "--",
            "",
            "--nocapture",
        ];
        let output = cargo.run(&args, "", HUMAN, 101, true);
        let expected = "cargo +nightly -Zunstable-options -C 'some path' --config build.jobs=2 check --manifest-path 'some workspace/Cargo.toml' -- '' --nocapture";
        assert!(String::from_utf8_lossy(&output.stderr).contains(&format!("Running: {expected}")));
        let conn = rusqlite::Connection::open(cargo.dir.path().join("tracking.db")).unwrap();
        let (original, rewritten): (String, String) = conn
            .query_row("SELECT original_cmd, rtk_cmd FROM commands", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(original, expected);
        assert_eq!(rewritten, format!("rtk {expected}"));
    }
}
