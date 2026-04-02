use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

#[cfg(unix)]
fn fake_direnv_script(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("direnv");
    let script = r#"#!/bin/sh
if [ "$1" = "exec" ]; then
  shift
  DIR="$1"
  shift
  echo "direnv: loading ${DIR}/.envrc" >&2
  exec "$@"
fi
printf 'real direnv passthrough %s\n' "$*"
"#;
    fs::write(&path, script).expect("write fake direnv");
    let mut perms = fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
}

#[cfg(unix)]
fn test_path(dir: &Path) -> std::ffi::OsString {
    test_path_with_prefixes(&[dir])
}

#[cfg(unix)]
fn test_path_with_prefixes(prefixes: &[&Path]) -> std::ffi::OsString {
    let mut paths: Vec<_> = prefixes.iter().map(|path| path.to_path_buf()).collect();
    paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    env::join_paths(paths).expect("join PATH")
}

#[cfg(unix)]
fn fake_gh_script(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("gh");
    let script = r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "token" ]; then
  printf '%s\n' "$GITHUB_TOKEN"
  exit 0
fi
printf 'fake gh passthrough %s\n' "$*"
"#;
    fs::write(&path, script).expect("write fake gh");
    let mut perms = fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
}

fn rtk_bin() -> &'static str {
    env!("CARGO_BIN_EXE_rtk")
}

#[cfg(unix)]
#[test]
fn direnv_non_exec_passthrough_without_filters() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_direnv_script(temp.path());

    let output = Command::new(rtk_bin())
        .args(["direnv", "status"])
        .env("PATH", test_path(temp.path()))
        .output()
        .expect("run rtk direnv status");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "real direnv passthrough status"
    );
}

#[cfg(unix)]
#[test]
fn direnv_exec_uses_builtin_filter_without_user_global_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_direnv_script(temp.path());

    let output = Command::new(rtk_bin())
        .args(["direnv", "exec", ".", "env"])
        .env("PATH", test_path(temp.path()))
        .env("GITHUB_TOKEN", "ghp_secret_value")
        .output()
        .expect("run rtk direnv exec");

    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("GITHUB_TOKEN=***"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[cfg(unix)]
#[test]
fn direnv_exec_redacts_stdout_and_stderr() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_direnv_script(temp.path());

    let output = Command::new(rtk_bin())
        .args(["direnv", "exec", ".", "sh", "-lc", "printenv >&2; env"])
        .env("PATH", test_path(temp.path()))
        .env("GITHUB_TOKEN", "ghp_secret_value")
        .env("OPENAI_API_KEY", "sk-secret_value")
        .output()
        .expect("run rtk direnv exec");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("GITHUB_TOKEN=***"));
    assert!(stderr.contains("GITHUB_TOKEN=***"));
    assert!(!stdout.contains("ghp_secret_value"));
    assert!(!stderr.contains("ghp_secret_value"));
    assert!(stderr.contains("direnv: loading ./.envrc"));
}

#[cfg(unix)]
#[test]
fn direnv_exec_unmatched_command_passthrough() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_direnv_script(temp.path());

    let output = Command::new(rtk_bin())
        .args(["direnv", "exec", ".", "sh", "-lc", "printf 1"])
        .env("PATH", test_path(temp.path()))
        .output()
        .expect("run rtk direnv passthrough");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("direnv: loading ./.envrc"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn direnv_exec_user_global_override_applies() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_direnv_script(temp.path());

    #[cfg(target_os = "macos")]
    let filters_path = temp
        .path()
        .join("Library")
        .join("Application Support")
        .join("rtk")
        .join("filters.toml");
    #[cfg(not(target_os = "macos"))]
    let filters_path = temp.path().join(".config").join("rtk").join("filters.toml");

    fs::create_dir_all(filters_path.parent().expect("config dir")).expect("mkdir config");
    fs::write(
        &filters_path,
        r#"schema_version = 1

[filters.direnv-user-override]
description = "Override built-in direnv env filtering"
match_command = "^direnv\\s+exec\\s+\\S+\\s+env(?:\\s|$)"
keep_lines_matching = ["^GITHUB_TOKEN="]
replace = [
  { pattern = "^([A-Za-z_][A-Za-z0-9_]*)=.*$", replacement = "$1=USER" },
]
"#,
    )
    .expect("write user-global override");

    let output = Command::new(rtk_bin())
        .args(["direnv", "exec", ".", "env"])
        .env("PATH", test_path(temp.path()))
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", temp.path().join(".config"))
        .env("GITHUB_TOKEN", "ghp_secret_value")
        .env("OPENAI_API_KEY", "sk-secret_value")
        .output()
        .expect("run rtk direnv env with user-global override");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("GITHUB_TOKEN=USER"));
    assert!(!stdout.contains("OPENAI_API_KEY"));
}

#[cfg(unix)]
#[test]
fn direnv_exec_respects_rtk_no_toml_bypass() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_direnv_script(temp.path());

    let output = Command::new(rtk_bin())
        .args(["direnv", "exec", ".", "sh", "-lc", "printenv >&2; env"])
        .env("PATH", test_path(temp.path()))
        .env("RTK_NO_TOML", "1")
        .env("GITHUB_TOKEN", "ghp_secret_value")
        .output()
        .expect("run rtk direnv exec with RTK_NO_TOML");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("GITHUB_TOKEN=ghp_secret_value"));
    assert!(stderr.contains("GITHUB_TOKEN=ghp_secret_value"));
    assert!(!stdout.contains("GITHUB_TOKEN=***"));
    assert!(!stderr.contains("GITHUB_TOKEN=***"));
}

#[cfg(unix)]
#[test]
fn direnv_exec_real_source_up_redacts_gh_auth_token() {
    let temp = tempfile::tempdir().expect("tempdir");
    let parent = temp.path().join("parent");
    let child = parent.join("child");
    let bin = temp.path().join("bin");
    let xdg_config = temp.path().join(".config");
    let xdg_data = temp.path().join(".local").join("share");

    fs::create_dir_all(&child).expect("mkdir child");
    fs::create_dir_all(&bin).expect("mkdir bin");
    fs::create_dir_all(&xdg_config).expect("mkdir xdg config");
    fs::create_dir_all(&xdg_data).expect("mkdir xdg data");
    fs::write(
        parent.join(".envrc"),
        "export GITHUB_TOKEN=ghp_source_up_secret\n",
    )
    .expect("write parent .envrc");
    fs::write(child.join(".envrc"), "source_up\n").expect("write child .envrc");
    fake_gh_script(&bin);

    let path = test_path_with_prefixes(&[&bin]);
    let home = temp.path();

    let parent_allow = Command::new("direnv")
        .current_dir(&parent)
        .arg("allow")
        .env("PATH", &path)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", &xdg_config)
        .env("XDG_DATA_HOME", &xdg_data)
        .output()
        .expect("direnv allow parent");
    assert!(
        parent_allow.status.success(),
        "parent allow stderr: {}",
        String::from_utf8_lossy(&parent_allow.stderr)
    );

    let child_allow = Command::new("direnv")
        .current_dir(&child)
        .arg("allow")
        .env("PATH", &path)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", &xdg_config)
        .env("XDG_DATA_HOME", &xdg_data)
        .output()
        .expect("direnv allow child");
    assert!(
        child_allow.status.success(),
        "child allow stderr: {}",
        String::from_utf8_lossy(&child_allow.stderr)
    );

    let output = Command::new(rtk_bin())
        .current_dir(&child)
        .args(["direnv", "exec", ".", "gh", "auth", "token"])
        .env("PATH", &path)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", &xdg_config)
        .env("XDG_DATA_HOME", &xdg_data)
        .output()
        .expect("run rtk direnv exec gh auth token");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("***"), "stdout: {}", stdout);
    assert!(
        !stdout.contains("ghp_source_up_secret"),
        "stdout: {}",
        stdout
    );
    assert!(
        !stderr.contains("ghp_source_up_secret"),
        "stderr: {}",
        stderr
    );
}
