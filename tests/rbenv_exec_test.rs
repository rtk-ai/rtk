//! End-to-end tests for `rbenv exec` rewriting and execution, `env` prefix forms,
//! and `grep -rn` execution (#5307).
//!
//! Uses a stub `rbenv` script on `PATH` that mirrors real `rbenv-exec` behavior:
//! invoking `rbenv exec rtk ...` fails with exit 127 (`rbenv: rtk: command not found`),
//! while `rtk rbenv exec ...` runs `rbenv` directly with the caller's exact arguments
//! and filters the inner Ruby tool's output.

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn write_config(home: &Path, contents: &str) {
    for dir in [
        home.join(".config/rtk"),
        home.join("Library/Application Support/rtk"),
    ] {
        std::fs::create_dir_all(&dir).expect("mkdir rtk config");
        std::fs::write(dir.join("config.toml"), contents).expect("write rtk config");
    }
}

fn write_rbenv_stub(dir: &Path, argv_log: &Path) {
    let stub = dir.join("rbenv");
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$*" > '{log}'
if [ "$1" = "exec" ] && [ "$2" = "rtk" ]; then
    echo "rbenv: rtk: command not found" >&2
    exit 127
fi
for arg in "$@"; do
    case "$arg" in
        --help|-h)
            echo "usage-help-output"
            exit 0
            ;;
        --version)
            echo "tool-version-1.0.0"
            exit 0
            ;;
    esac
done
if [ "$1" = "versions" ]; then
    echo "* 3.3.6 (set by .ruby-version)"
    exit 0
fi
if [ "$1" = "exec" ] && [ "$2" = "rails" ] && [ "$3" = "runner" ]; then
    echo "runner-output-42"
    exit 7
fi
if [ "$1" = "exec" ] && [ "$2" = "bundle" ] && [ "$3" = "install" ]; then
    cat <<'EOF'
Using bundler 2.5.6
Using rake 13.1.0
Using rspec 3.13.0
Fetching minitest 5.22.2
Installing minitest 5.22.2
Bundle complete! 85 Gemfile dependencies, 200 gems now installed.
Use `bundle info [gemname]` to see where a bundled gem is installed.
EOF
    exit 0
fi
for arg in "$@"; do
    case "$arg" in
        rspec|bin/rspec)
            cat <<'EOF'
{{"examples":[{{"full_description":"User validates email","status":"passed","file_path":"./spec/foo_spec.rb","line_number":10,"exception":null}},{{"full_description":"User rejects blank email","status":"failed","file_path":"./spec/foo_spec.rb","line_number":18,"exception":{{"class":"RSpec::Expectations::ExpectationNotMetError","message":"expected false to be truthy","backtrace":["./spec/foo_spec.rb:19:in `block'"]}}}}],"summary":{{"duration":0.12,"example_count":2,"failure_count":1,"pending_count":0,"errors_outside_of_examples_count":0}}}}
EOF
            exit 1
            ;;
        rubocop)
            cat <<'EOF'
{{"files":[{{"path":"app/models/user.rb","offenses":[{{"cop_name":"Style/StringLiterals","severity":"convention","message":"Prefer single-quoted strings","correctable":true,"location":{{"start_line":4}}}}]}}],"summary":{{"offense_count":1,"target_file_count":1,"inspected_file_count":1,"correctable_offense_count":1}}}}
EOF
            exit 1
            ;;
        rails|rake|bin/rails|bin/rake)
            cat <<'EOF'
Run options: --seed 12345

# Running:

..F..

Finished in 0.123456s, 40.5 runs/s

  1) Failure:
UserTest#test_email [test/models/user_test.rb:12]:
Expected false to be truthy.

5 runs, 5 assertions, 1 failures, 0 errors, 0 skips
EOF
            exit 1
            ;;
    esac
done
exit 0
"#,
        log = argv_log.display()
    );
    std::fs::write(&stub, script).expect("write rbenv stub");
    let mut perms = std::fs::metadata(&stub).expect("stat stub").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&stub, perms).expect("chmod stub");
}

#[test]
fn rbenv_exec_rewrite_and_execution_end_to_end() {
    let dir = common::tempdir();
    let home = dir.path().join("home");
    let bin = dir.path().join("bin");
    let argv_log = dir.path().join("rbenv_argv.txt");
    std::fs::create_dir_all(&home).expect("mkdir home");
    std::fs::create_dir_all(&bin).expect("mkdir bin");
    write_rbenv_stub(&bin, &argv_log);

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // 1. Default config rewrites `rbenv exec bundle exec rspec ...` with rtk outside rbenv.
    let out = common::rtk_command()
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .args(["rewrite", "rbenv exec bundle exec rspec spec/foo_spec.rb"])
        .output()
        .expect("rtk rewrite");
    assert!(
        matches!(out.status.code(), Some(0) | Some(3)),
        "expected rewrite exit 0 or 3, got {:?}",
        out.status.code()
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "rtk rbenv exec bundle exec rspec spec/foo_spec.rb"
    );

    // 2. Even when `transparent_prefixes = ["rbenv exec"]` is configured, never emit
    //    `rbenv exec rtk ...` (which fails with `rbenv: rtk: command not found`).
    write_config(&home, "[hooks]\ntransparent_prefixes = [\"rbenv exec\"]\n");
    let out = common::rtk_command()
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .args(["rewrite", "rbenv exec bundle exec rspec spec/foo_spec.rb"])
        .output()
        .expect("rtk rewrite with transparent_prefixes");
    assert!(matches!(out.status.code(), Some(0) | Some(3)));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "rtk rbenv exec bundle exec rspec spec/foo_spec.rb"
    );

    let out = common::rtk_command()
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .args(["rewrite", "rbenv exec git status"])
        .output()
        .expect("rtk rewrite unsupported rbenv exec");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

    // 3. `rtk rbenv exec bundle exec rspec` runs rbenv directly and filters JSON output.
    let out = common::rtk_command()
        .env("PATH", &path)
        .current_dir(dir.path())
        .args([
            "rbenv",
            "exec",
            "bundle",
            "exec",
            "rspec",
            "spec/foo_spec.rb",
        ])
        .output()
        .expect("rtk rbenv rspec");
    assert_eq!(out.status.code(), Some(1), "must propagate rspec exit code");
    assert_eq!(
        std::fs::read_to_string(&argv_log)
            .expect("read argv log")
            .trim(),
        "exec bundle exec rspec --format json spec/foo_spec.rb"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("User rejects blank email"),
        "expected filtered failure in stdout, got: {stdout}"
    );
    assert!(
        !stdout.contains("rbenv: rtk: command not found"),
        "must not run rtk under rbenv"
    );

    // 4. Direct `rtk rbenv exec rspec` preserves the caller's argv without injecting `bundle exec`.
    let out = common::rtk_command()
        .env("PATH", &path)
        .current_dir(dir.path())
        .args(["rbenv", "exec", "rspec", "spec/foo_spec.rb"])
        .output()
        .expect("rtk rbenv direct rspec");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(&argv_log)
            .expect("read argv log")
            .trim(),
        "exec rspec --format json spec/foo_spec.rb"
    );

    // 5. `rtk rbenv exec bundle exec rubocop` runs under rbenv and filters JSON offenses.
    let out = common::rtk_command()
        .env("PATH", &path)
        .current_dir(dir.path())
        .args(["rbenv", "exec", "bundle", "exec", "rubocop", "app/"])
        .output()
        .expect("rtk rbenv rubocop");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(&argv_log)
            .expect("read argv log")
            .trim(),
        "exec bundle exec rubocop --format json app/"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Style/StringLiterals"),
        "expected rubocop offense summary, got: {stdout}"
    );

    // 6. `rtk rbenv exec bundle exec rails test` preserves `rails` and filters Minitest.
    let out = common::rtk_command()
        .env("PATH", &path)
        .current_dir(dir.path())
        .args([
            "rbenv",
            "exec",
            "bundle",
            "exec",
            "rails",
            "test",
            "test/models/user_test.rb",
        ])
        .output()
        .expect("rtk rbenv rails test");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(&argv_log)
            .expect("read argv log")
            .trim(),
        "exec bundle exec rails test test/models/user_test.rb"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("UserTest#test_email"),
        "expected minitest failure in stdout, got: {stdout}"
    );

    // 7. `rtk rbenv exec bundle install` applies bundle-install.toml compaction,
    //    and `RTK_NO_TOML=1` bypasses TOML filtering cleanly.
    let out = common::rtk_command()
        .env("PATH", &path)
        .current_dir(dir.path())
        .args(["rbenv", "exec", "bundle", "install"])
        .output()
        .expect("rtk rbenv bundle install");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(&argv_log)
            .expect("read argv log")
            .trim(),
        "exec bundle install"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ok bundle: complete"),
        "expected compacted bundle install output, got: {stdout}"
    );

    let out = common::rtk_command()
        .env("PATH", &path)
        .env("RTK_NO_TOML", "1")
        .current_dir(dir.path())
        .args(["rbenv", "exec", "bundle", "install"])
        .output()
        .expect("rtk rbenv bundle install with RTK_NO_TOML=1");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Using bundler 2.5.6") && !stdout.contains("ok bundle: complete"),
        "RTK_NO_TOML=1 must bypass bundle-install TOML filter, got: {stdout}"
    );

    // 7b. Help/version queries pass through verbatim without injecting `--format json`.
    let out = common::rtk_command()
        .env("PATH", &path)
        .current_dir(dir.path())
        .args(["rbenv", "exec", "bundle", "exec", "rspec", "--help"])
        .output()
        .expect("rtk rbenv rspec --help");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(&argv_log)
            .expect("read argv log")
            .trim(),
        "exec bundle exec rspec --help"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "usage-help-output"
    );

    let out = common::rtk_command()
        .env("PATH", &path)
        .current_dir(dir.path())
        .args(["rbenv", "exec", "rubocop", "--version"])
        .output()
        .expect("rtk rbenv rubocop --version");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(&argv_log)
            .expect("read argv log")
            .trim(),
        "exec rubocop --version"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "tool-version-1.0.0"
    );

    // 7c. `--` separator is preserved in forwarded argv for filtered runs.
    let out = common::rtk_command()
        .env("PATH", &path)
        .current_dir(dir.path())
        .args([
            "rbenv",
            "exec",
            "bundle",
            "exec",
            "rspec",
            "--",
            "spec/foo_spec.rb",
        ])
        .output()
        .expect("rtk rbenv rspec --");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(&argv_log)
            .expect("read argv log")
            .trim(),
        "exec bundle exec rspec --format json -- spec/foo_spec.rb"
    );

    // 8. Non-target rbenv commands pass through verbatim with exit codes preserved.
    let out = common::rtk_command()
        .env("PATH", &path)
        .current_dir(dir.path())
        .args(["rbenv", "versions"])
        .output()
        .expect("rtk rbenv versions");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("3.3.6"));

    let out = common::rtk_command()
        .env("PATH", &path)
        .current_dir(dir.path())
        .args(["rbenv", "exec", "rails", "runner", "puts 1"])
        .output()
        .expect("rtk rbenv rails runner");
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "runner-output-42"
    );

    // 9. `env` prefix forms (`--` and lowercase vars) and `grep -rn` work end-to-end.
    for (input, expected) in [
        (
            "env -- SOME_VAR=dummy git status",
            "env -- SOME_VAR=dummy rtk git status",
        ),
        ("env foo=bar git status", "env foo=bar rtk git status"),
        ("grep -rn needle src", "rtk grep -rn needle src"),
    ] {
        let out = common::rtk_command()
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .args(["rewrite", input])
            .output()
            .expect("rtk rewrite");
        assert!(matches!(out.status.code(), Some(0) | Some(3)), "{input}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            expected,
            "{input}"
        );
    }

    let sample = dir.path().join("sample.txt");
    std::fs::write(&sample, "alpha\nneedle line here\nomega\n").expect("write sample");
    let out = common::rtk_command()
        .current_dir(dir.path())
        .args(["grep", "-rn", "needle", "sample.txt"])
        .output()
        .expect("rtk grep -rn");
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("needle line here"),
        "expected grep -rn match, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}
