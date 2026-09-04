//! Regression test for the missing `restore_double_dash` call in dotnet_cmd.rs
//! (see run_dotnet_with_binlog/run_format): without it, `inject_report_trx_into_args`
//! can't tell "the user passed no `--`" from "the user passed `--` but clap's
//! trailing_var_arg stripped it". Since args then look like `["FullyQualifiedName=MyFilter"]`
//! (no `--` at all), it appends a *fresh* `-- --report-trx` at the end instead of reusing the
//! user's own `--` — landing the user's MTP-runtime filter expression BEFORE the separator
//! (misread as a `dotnet test` CLI arg) instead of after it, and stranding `--report-trx` with
//! nothing following it.
//!
//! Stubs `dotnet` on PATH with a script that records its argv, since the real
//! dotnet SDK isn't available in this environment.

#[cfg(unix)]
#[test]
fn dotnet_test_reuses_users_double_dash_for_report_trx_injection() {
    use std::process::Command;

    fn shell_quote(path: &std::path::Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let argv_file = dir.path().join("argv.txt");
    let stub_path = dir.path().join("dotnet");

    // --report-trx is only injected after `--` in VsTestBridge mode (detect_test_runner_mode),
    // which requires a project file with UseMicrosoftTestingPlatformRunner/UseTestingPlatformRunner.
    std::fs::write(
        dir.path().join("MyProject.csproj"),
        r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <UseMicrosoftTestingPlatformRunner>true</UseMicrosoftTestingPlatformRunner>
  </PropertyGroup>
</Project>"#,
    )
    .expect("write csproj");

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

    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("PATH", path_with_stub)
        .current_dir(dir.path())
        .args(["dotnet", "test", "--", "FullyQualifiedName=MyFilter"])
        .output()
        .expect("spawn rtk");
    assert!(
        out.status.success(),
        "rtk dotnet test failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let argv: Vec<String> = std::fs::read_to_string(&argv_file)
        .expect("read captured argv")
        .lines()
        .map(str::to_string)
        .collect();

    let dashdash_count = argv.iter().filter(|a| a.as_str() == "--").count();
    assert_eq!(
        dashdash_count, 1,
        "the user's -- must be reused, not duplicated: {argv:?}"
    );

    let sep = argv.iter().position(|a| a == "--").expect("has --");
    assert_eq!(
        argv.get(sep + 1).map(String::as_str),
        Some("--report-trx"),
        "--report-trx should be injected right after the user's --: {argv:?}"
    );

    let filter_pos = argv
        .iter()
        .position(|a| a == "FullyQualifiedName=MyFilter")
        .expect("filter expression present");
    assert!(
        filter_pos > sep,
        "the user's filter expression must stay AFTER --, not be pulled before it \
         when clap's stripped -- gets a fresh one appended at the end: {argv:?}"
    );
}
