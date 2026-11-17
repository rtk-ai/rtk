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

mod common;

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(unix)]
fn install_dotnet_stub(dir: &std::path::Path, body: &str) -> String {
    let stub_path = dir.join("dotnet");
    std::fs::write(&stub_path, format!("#!/bin/sh\n{body}\n")).expect("write dotnet stub");
    let mut perms = std::fs::metadata(&stub_path)
        .expect("stat dotnet stub")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&stub_path, perms).expect("chmod dotnet stub");

    format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

#[cfg(unix)]
fn pin_classic_runner(dir: &std::path::Path) {
    // Keep these fixtures independent of any parent global.json or props file.
    std::fs::write(dir.join("global.json"), "{}").expect("write classic global.json");
    std::fs::write(dir.join("Directory.Build.props"), "")
        .expect("write classic Directory.Build.props");
}

#[cfg(unix)]
#[test]
fn dotnet_test_reuses_users_double_dash_for_report_trx_injection() {
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
    // The mode search walks up for `global.json` and stops at the first; one
    // here keeps a `global.json` above the temporary directory from deciding it.
    std::fs::write(dir.path().join("global.json"), "{}").expect("write global.json");

    std::fs::write(
        &stub_path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nexit 0\n",
            shell_quote(&argv_file.display().to_string())
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

    let out = common::rtk_command()
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

#[cfg(unix)]
#[test]
fn dotnet_test_discovery_keeps_classic_native_names_and_exit_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    pin_classic_runner(dir.path());
    let argv_file = dir.path().join("argv.txt");
    let path_with_stub = install_dotnet_stub(
        dir.path(),
        &format!(
            "printf '%s\\n' \"$@\" > {}\nprintf '%s\\n' 'The following Tests are available:' '    RtkDotnetSmoke.UnitTest1.Test1' '    RtkDotnetSmoke.UnitTest2.Test2'\nexit 0",
            shell_quote(&argv_file.display().to_string())
        ),
    );

    for discovery_arg in ["--list-tests", "-t", "--LIST-TESTS", "-T"] {
        let out = common::rtk_command()
            .env("PATH", &path_with_stub)
            .current_dir(dir.path())
            .args(["dotnet", "test", discovery_arg])
            .output()
            .expect("spawn rtk");
        let stdout = String::from_utf8_lossy(&out.stdout);

        assert_eq!(
            out.status.code(),
            Some(0),
            "discovery must preserve native exit status for {discovery_arg}"
        );
        assert!(
            stdout.contains("RtkDotnetSmoke.UnitTest1.Test1")
                && stdout.contains("RtkDotnetSmoke.UnitTest2.Test2"),
            "discovery names must survive RTK output filtering for {discovery_arg}: {stdout}"
        );

        let argv: Vec<String> = std::fs::read_to_string(&argv_file)
            .expect("read captured argv")
            .lines()
            .map(str::to_string)
            .collect();
        for injected in ["--logger", "--results-directory", "--report-trx"] {
            assert!(
                !argv.iter().any(|arg| arg == injected),
                "discovery must not receive report injection {injected}: {argv:?}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn dotnet_test_does_not_passthrough_non_discovery_switch_mutations() {
    for switch in ["--t", "/t", "-list-tests", "--list-tests:true", "-t:true"] {
        let dir = tempfile::tempdir().expect("tempdir");
        pin_classic_runner(dir.path());
        let argv_file = dir.path().join("argv.txt");
        let path_with_stub = install_dotnet_stub(
            dir.path(),
            &format!(
                "printf '%s\\n' \"$@\" > {}\nexit 0",
                shell_quote(&argv_file.display().to_string())
            ),
        );

        let out = common::rtk_command()
            .env("PATH", &path_with_stub)
            .current_dir(dir.path())
            .args(["dotnet", "test", switch])
            .output()
            .expect("spawn rtk");
        assert!(
            out.status.success(),
            "normal test switch should run through RTK: switch={switch} stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        let argv = std::fs::read_to_string(&argv_file).expect("read captured argv");
        assert!(
            argv.lines().any(|arg| arg == "--logger"),
            "normal test switch must receive the classic TRX logger: switch={switch} argv={argv}"
        );
        assert!(
            argv.lines().any(|arg| arg == "--results-directory"),
            "normal test switch must receive the classic results directory: switch={switch} argv={argv}"
        );
    }
}

#[cfg(unix)]
#[test]
fn dotnet_build_target_switch_still_receives_binlog() {
    let dir = tempfile::tempdir().expect("tempdir");
    let argv_file = dir.path().join("argv.txt");
    let path_with_stub = install_dotnet_stub(
        dir.path(),
        &format!(
            "printf '%s\\n' \"$@\" > {}\nexit 0",
            shell_quote(&argv_file.display().to_string())
        ),
    );

    let out = common::rtk_command()
        .env("PATH", &path_with_stub)
        .current_dir(dir.path())
        .args(["dotnet", "build", "-t"])
        .output()
        .expect("spawn rtk");
    assert!(out.status.success());

    let argv = std::fs::read_to_string(&argv_file).expect("read captured argv");
    assert!(
        argv.lines().any(|arg| arg.starts_with("-bl:")),
        "dotnet build -t must stay on the build path and receive a binlog: {argv}"
    );
}

#[cfg(unix)]
#[test]
fn dotnet_test_discovery_keeps_native_mtp_names_and_exit_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("global.json"),
        r#"{"sdk":{"version":"10.0.100"},"test":{"runner":"Microsoft.Testing.Platform"}}"#,
    )
    .expect("write global.json");
    let path_with_stub = install_dotnet_stub(
        dir.path(),
        &format!(
            "printf '%s\\n' \"$@\" > {}\nprintf '%s\\n' 'RtkDotnetSmoke.MtpTest1.Test1' 'RtkDotnetSmoke.MtpTest2.Test2'\nexit 0",
            shell_quote(&dir.path().join("argv.txt").display().to_string())
        ),
    );

    let out = common::rtk_command()
        .env("PATH", path_with_stub)
        .current_dir(dir.path())
        .args(["dotnet", "test", "--list-tests"])
        .output()
        .expect("spawn rtk");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(
        out.status.code(),
        Some(0),
        "MTP discovery must preserve native exit status"
    );
    assert!(
        stdout.contains("RtkDotnetSmoke.MtpTest1.Test1")
            && stdout.contains("RtkDotnetSmoke.MtpTest2.Test2"),
        "MTP discovery names must survive RTK output filtering: {stdout}"
    );
    let argv: Vec<String> = std::fs::read_to_string(dir.path().join("argv.txt"))
        .expect("read captured argv")
        .lines()
        .map(str::to_string)
        .collect();
    for injected in ["--logger", "--results-directory", "--report-trx"] {
        assert!(
            !argv.iter().any(|arg| arg == injected),
            "MTP discovery must not receive report injection {injected}: {argv:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn dotnet_test_discovery_preserves_native_error_and_exit_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path_with_stub = install_dotnet_stub(
        dir.path(),
        "printf '%s\\n' 'native discovery error' >&2\nexit 9",
    );

    let out = common::rtk_command()
        .env("PATH", path_with_stub)
        .current_dir(dir.path())
        .args(["dotnet", "test", "--list-tests"])
        .output()
        .expect("spawn rtk");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(out.status.code(), Some(9));
    assert!(
        stdout.contains("native discovery error") || stderr.contains("native discovery error"),
        "native discovery error must survive RTK output handling: stdout={stdout} stderr={stderr}"
    );
}

#[cfg(unix)]
#[test]
fn dotnet_test_normal_run_keeps_existing_failure_summary_control() {
    let dir = tempfile::tempdir().expect("tempdir");
    pin_classic_runner(dir.path());
    let fixture = include_str!("fixtures/dotnet/test_failed.txt");
    let path_with_stub = install_dotnet_stub(
        dir.path(),
        &format!("printf '%s' {} >&2\nexit 1", shell_quote(fixture)),
    );

    let out = common::rtk_command()
        .env("PATH", path_with_stub)
        .current_dir(dir.path())
        .args(["dotnet", "test"])
        .output()
        .expect("spawn rtk");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout.contains("RtkDotnetSmoke.UnitTest1.Test1"),
        "{stdout}"
    );
    assert!(stdout.contains("fail dotnet test:"), "{stdout}");
}

#[cfg(unix)]
#[test]
fn dotnet_test_explicit_results_directory_is_retained_control() {
    let dir = tempfile::tempdir().expect("tempdir");
    pin_classic_runner(dir.path());
    let results_dir = dir.path().join("user-results");
    let path_with_stub = install_dotnet_stub(
        dir.path(),
        &format!(
            "mkdir -p {}\nprintf '%s\\n' user-report > {}/user-report.trx\nexit 0",
            shell_quote(&results_dir.display().to_string()),
            shell_quote(&results_dir.display().to_string())
        ),
    );

    let out = common::rtk_command()
        .env("PATH", path_with_stub)
        .current_dir(dir.path())
        .args([
            "dotnet",
            "test",
            "--results-directory",
            results_dir.to_str().expect("UTF-8 results path"),
        ])
        .output()
        .expect("spawn rtk");

    assert!(out.status.success());
    assert!(results_dir.join("user-report.trx").exists());
}

#[cfg(unix)]
#[test]
fn dotnet_test_relative_logger_report_keeps_native_default_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    pin_classic_runner(dir.path());
    let argv_file = dir.path().join("argv.txt");
    let path_with_stub = install_dotnet_stub(
        dir.path(),
        &format!(
            "printf '%s\\n' \"$@\" > {}\nresults_dir=\"$PWD/TestResults\"\nlogger=''\nprevious=''\nfor arg in \"$@\"; do\n  if [ \"$previous\" = '--results-directory' ]; then results_dir=\"$arg\"; fi\n  if [ \"$previous\" = '--logger' ]; then logger=\"$arg\"; fi\n  previous=\"$arg\"\ndone\ncase \"$logger\" in\n  'trx;LogFileName='*)\n    report_name=\"${{logger#*LogFileName=}}\"\n    mkdir -p \"$results_dir\"\n    printf '%s\\n' user-requested-report > \"$results_dir/$report_name\"\n    ;;\nesac\nexit 0",
            shell_quote(&argv_file.display().to_string())
        ),
    );

    let out = common::rtk_command()
        .env("PATH", path_with_stub)
        .current_dir(dir.path())
        .args([
            "dotnet",
            "test",
            "--logger",
            "trx;LogFileName=rtk-user-report.trx",
        ])
        .output()
        .expect("spawn rtk");
    assert!(out.status.success());

    let argv: Vec<String> = std::fs::read_to_string(&argv_file)
        .expect("read captured argv")
        .lines()
        .map(str::to_string)
        .collect();
    let logger_index = argv
        .iter()
        .position(|arg| arg == "--logger")
        .expect("user logger should be forwarded");
    assert_eq!(
        argv.get(logger_index + 1).map(String::as_str),
        Some("trx;LogFileName=rtk-user-report.trx")
    );
    assert!(
        !argv.iter().any(|arg| arg == "--results-directory"),
        "a user logger without an explicit results directory must keep the native default: {argv:?}"
    );
    let results_dir = dir.path().join("TestResults");
    assert!(
        results_dir.join("rtk-user-report.trx").exists(),
        "user-requested logger output was deleted or redirected from the native default: {}",
        results_dir.display()
    );
}

#[cfg(unix)]
#[test]
fn dotnet_test_solution_root_reads_reported_trx_paths_and_retains_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    pin_classic_runner(dir.path());
    let first_trx = dir.path().join("Tests/TestResults/Tests.trx");
    let second_trx = dir.path().join("Tests2/TestResults/Tests2.trx");
    let fallback_trx = dir.path().join("TestResults/fallback.trx");
    let first_contents = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun>
  <Times start="2026-10-20T12:00:00.0000000+00:00" finish="2026-10-20T12:00:00.1000000+00:00" />
  <ResultSummary><Counters total="1" executed="1" passed="0" failed="1" /></ResultSummary>
  <Results><UnitTestResult testName="Tests.UnitTest1.FailsOnDemand" outcome="Failed"><Output><ErrorInfo><Message>asked to fail one</Message><StackTrace>at Tests.UnitTest1.FailsOnDemand()</StackTrace></ErrorInfo></Output></UnitTestResult></Results>
</TestRun>"#;
    let second_contents = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun>
  <Times start="2026-10-20T12:00:00.0000000+00:00" finish="2026-10-20T12:00:00.1000000+00:00" />
  <ResultSummary><Counters total="1" executed="1" passed="0" failed="1" /></ResultSummary>
  <Results><UnitTestResult testName="Tests2.UnitTest1.FailsOnDemand" outcome="Failed"><Output><ErrorInfo><Message>asked to fail two</Message><StackTrace>at Tests2.UnitTest1.FailsOnDemand()</StackTrace></ErrorInfo></Output></UnitTestResult></Results>
</TestRun>"#;
    let fallback_contents = r#"<?xml version="1.0" encoding="utf-8"?><TestRun><ResultSummary><Counters total="1" executed="1" passed="0" failed="1" /></ResultSummary><Results><UnitTestResult testName="Fallback.Tests.ShouldNotWin" outcome="Failed"><Output><ErrorInfo><Message>fallback must not win</Message></ErrorInfo></Output></UnitTestResult></Results></TestRun>"#;
    let path_with_stub = install_dotnet_stub(
        dir.path(),
        &format!(
            // RTK skips a report whose mtime is older than the time it took before spawning
            // dotnet, and a file's mtime can trail that clock by a scheduler tick on Linux. A
            // real test run writes its reports long after; the stub waits to do the same.
            "sleep 0.1\nmkdir -p {} {} {}\nprintf '%s' {} > {}\nprintf '%s' {} > {}\nprintf '%s' {} > {}\nprintf '%s\\n' 'Failed!' 'Total tests: 2' 'Results File: {}' 'Results File: {}' 'Results File: {}' 'Results File: {}'\nprintf '%s\\n' '[xUnit.net 00:00:00.01] Tests.UnitTest1.FailsOnDemand [FAIL]' 'asked to fail one' '[xUnit.net 00:00:00.01] Tests2.UnitTest1.FailsOnDemand [FAIL]' 'asked to fail two' >&2\nexit 1",
            shell_quote(
                &first_trx
                    .parent()
                    .expect("first parent")
                    .display()
                    .to_string()
            ),
            shell_quote(
                &second_trx
                    .parent()
                    .expect("second parent")
                    .display()
                    .to_string()
            ),
            shell_quote(
                &fallback_trx
                    .parent()
                    .expect("fallback parent")
                    .display()
                    .to_string()
            ),
            shell_quote(first_contents),
            shell_quote(&first_trx.display().to_string()),
            shell_quote(second_contents),
            shell_quote(&second_trx.display().to_string()),
            shell_quote(fallback_contents),
            shell_quote(&fallback_trx.display().to_string()),
            shell_quote(&first_trx.display().to_string()),
            shell_quote(&first_trx.display().to_string()),
            shell_quote(&second_trx.display().to_string()),
            shell_quote(&second_trx.display().to_string()),
        ),
    );

    let out = common::rtk_command()
        .env("PATH", &path_with_stub)
        .current_dir(dir.path())
        .args(["dotnet", "test", "--logger", "trx", "-v", "q"])
        .output()
        .expect("spawn rtk");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let shown = format!("{stdout}\n{stderr}");

    assert_eq!(out.status.code(), Some(1), "{shown}");
    assert!(shown.contains("Tests.UnitTest1.FailsOnDemand"), "{shown}");
    assert!(shown.contains("Tests2.UnitTest1.FailsOnDemand"), "{shown}");
    assert!(shown.contains("asked to fail one"), "{shown}");
    assert!(shown.contains("asked to fail two"), "{shown}");
    assert!(shown.contains("0 passed, 2 failed"), "{shown}");
    assert!(shown.contains("in 2 projects"), "{shown}");
    assert!(!shown.contains("Fallback.Tests.ShouldNotWin"), "{shown}");
    assert!(first_trx.exists(), "native first TRX was removed");
    assert!(second_trx.exists(), "native second TRX was removed");
    assert!(fallback_trx.exists(), "fallback TRX was removed");
}

#[cfg(unix)]
#[test]
fn dotnet_test_owned_results_directory_is_cleaned_up_control() {
    let dir = tempfile::tempdir().expect("tempdir");
    pin_classic_runner(dir.path());
    let argv_file = dir.path().join("argv.txt");
    let path_with_stub = install_dotnet_stub(
        dir.path(),
        &format!(
            "printf '%s\\n' \"$@\" > {}\nresults_dir=''\nprevious=''\nfor arg in \"$@\"; do\n  if [ \"$previous\" = '--results-directory' ]; then results_dir=\"$arg\"; fi\n  previous=\"$arg\"\ndone\nmkdir -p \"$results_dir\"\nprintf '%s\\n' owned-report > \"$results_dir/owned.trx\"\nexit 0",
            shell_quote(&argv_file.display().to_string())
        ),
    );

    let out = common::rtk_command()
        .env("PATH", path_with_stub)
        .current_dir(dir.path())
        .args(["dotnet", "test"])
        .output()
        .expect("spawn rtk");
    assert!(out.status.success());

    let argv: Vec<String> = std::fs::read_to_string(&argv_file)
        .expect("read captured argv")
        .lines()
        .map(str::to_string)
        .collect();
    let results_index = argv
        .iter()
        .position(|arg| arg == "--results-directory")
        .expect("ordinary run should use RTK-owned results directory");
    let results_dir = std::path::Path::new(&argv[results_index + 1]);
    assert!(
        !results_dir.exists(),
        "RTK-owned report directory must be cleaned up: {}",
        results_dir.display()
    );
}
