//! Regression test for rtk-ai/rtk#2663: `rtk npm <args>` must hand npm the argv
//! it was given, with nothing inserted.
//!
//! RTK used to prepend `run` whenever the first argument was missing from a
//! hardcoded `NPM_SUBCOMMANDS` allowlist, so every subcommand or alias the list
//! did not know about (`npm query`, `npm sbom`, `npm add`, `npm x`, …) became
//! `npm run <that>` and failed. The allowlist is gone; this test pins the
//! replacement contract so it cannot come back — a script name like `build` is
//! forwarded as `npm build` and fails the way it would without RTK, instead of
//! being silently rewritten.
//!
//! Stubs `npm` on PATH with a script that records its argv, since a real npm
//! project isn't available in this environment.

#[cfg(unix)]
#[test]
fn npm_args_are_forwarded_verbatim() {
    use std::process::Command;

    fn shell_quote(path: &std::path::Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let argv_file = dir.path().join("argv.txt");
    let stub_path = dir.path().join("npm");

    std::fs::write(
        &stub_path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nexit 0\n",
            shell_quote(&argv_file)
        ),
    )
    .expect("write stub");
    let mut perms = std::fs::metadata(&stub_path)
        .expect("stat stub")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&stub_path, perms).expect("chmod stub");

    let path_with_stub = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // One case per shape the old allowlist got wrong, plus the two it got right.
    let cases: &[&[&str]] = &[
        &["query", ".name"], // npm 8 subcommand, was missing from the list
        &["sbom", "--sbom-format=cyclonedx"], // npm 10 subcommand, ditto
        &["add", "lodash"],  // alias of install
        &["x", "cowsay"],    // alias of exec
        &["build"],          // a package.json script: must NOT become `npm run build`
        &["run", "build"],   // explicit run: must not gain a second `run`
        &["--version"],      // a bare flag
    ];

    for case in cases {
        let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .env("PATH", &path_with_stub)
            .current_dir(dir.path())
            .arg("npm")
            .args(*case)
            .output()
            .expect("spawn rtk");
        assert!(
            out.status.success(),
            "rtk npm {case:?} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        let argv: Vec<String> = std::fs::read_to_string(&argv_file)
            .expect("read captured argv")
            .lines()
            .map(str::to_string)
            .collect();

        assert_eq!(
            argv,
            case.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            "npm must receive the user's argv unchanged"
        );
    }
}
