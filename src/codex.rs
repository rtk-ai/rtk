use anyhow::{Context, Result};
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::discover::registry;

const RTK_HOST_ENV: &str = "RTK_HOST";
const RTK_HOST_CODEX: &str = "codex";
const RTK_BYPASS_ENV: &str = "RTK_BYPASS";
const RTK_DISABLED_ENV: &str = "RTK_DISABLED";

pub fn maybe_run_as_codex_shim(argv0: &OsStr, args: &[OsString]) -> Result<Option<i32>> {
    let invoked_as = basename(argv0);
    if invoked_as == "rtk" {
        return Ok(None);
    }

    if env::var(RTK_HOST_ENV).ok().as_deref() != Some(RTK_HOST_CODEX)
        || env::var(RTK_BYPASS_ENV).ok().as_deref() == Some("1")
        || env::var(RTK_DISABLED_ENV).ok().as_deref() == Some("1")
        || !registry::entrypoints().contains(invoked_as.as_str())
    {
        return Ok(Some(exec_real_binary(&invoked_as, args)?));
    }

    let excluded = crate::config::Config::load()
        .map(|c| c.hooks.exclude_commands)
        .unwrap_or_default();

    if let Some(rewritten) = registry::rewrite_argv(&invoked_as, args, &excluded) {
        return Ok(Some(exec_rewritten(&rewritten)?));
    }

    Ok(Some(exec_real_binary(&invoked_as, args)?))
}

fn exec_rewritten(argv: &[OsString]) -> Result<i32> {
    let current_exe = env::current_exe().context("Failed to resolve current rtk binary")?;
    let mut command = Command::new(&current_exe);
    command.args(argv.iter().skip(1)).env(RTK_BYPASS_ENV, "1");
    run_command(command)
}

fn exec_real_binary(invoked_as: &str, args: &[OsString]) -> Result<i32> {
    let current_exe = env::current_exe().context("Failed to resolve current rtk binary")?;
    let clean_path = clean_path_without_shims(invoked_as, &current_exe)?;
    let cwd = env::current_dir().context("Failed to resolve current working directory")?;
    let resolved = which::which_in(invoked_as, Some(&clean_path), cwd)
        .with_context(|| format!("Failed to resolve real binary for {invoked_as}"))?;

    let mut command = Command::new(resolved);
    command
        .args(args)
        .env("PATH", &clean_path)
        .env(RTK_BYPASS_ENV, "1");
    run_command(command)
}

fn run_command(mut command: Command) -> Result<i32> {
    let status = command.status().context("Failed to spawn child process")?;
    Ok(exit_status_code(status))
}

fn clean_path_without_shims(invoked_as: &str, current_exe: &Path) -> Result<OsString> {
    let original = env::var_os("PATH").unwrap_or_default();
    let filtered: Vec<PathBuf> = env::split_paths(&original)
        .filter(|entry| !is_shim_dir(entry, invoked_as, current_exe))
        .collect();
    env::join_paths(filtered).context("Failed to rebuild PATH without Codex shim entries")
}

fn is_shim_dir(dir: &Path, invoked_as: &str, current_exe: &Path) -> bool {
    let candidate = dir.join(invoked_as);
    if !candidate.exists() {
        return false;
    }

    match (candidate.canonicalize(), current_exe.canonicalize()) {
        (Ok(candidate), Ok(current_exe)) => candidate == current_exe,
        _ => false,
    }
}

fn basename(path: &OsStr) -> String {
    Path::new(path)
        .file_name()
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(unix)]
fn exit_status_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;

    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

#[cfg(not(unix))]
fn exit_status_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_entrypoints_cover_wrapper_binaries() {
        let entrypoints = registry::entrypoints();
        assert!(entrypoints.contains("python"));
        assert!(entrypoints.contains("python3"));
        assert!(entrypoints.contains("npx"));
        assert!(entrypoints.contains("uv"));
    }

    #[test]
    fn test_rewrite_git_status_argv() {
        let args = vec![OsString::from("status")];
        let rewritten = registry::rewrite_argv("git", &args, &[]).unwrap();
        let actual: Vec<String> = rewritten
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(actual, vec!["rtk", "git", "status"]);
    }

    #[test]
    fn test_rewrite_python_m_pytest_argv() {
        let args = vec![
            OsString::from("-m"),
            OsString::from("pytest"),
            OsString::from("-x"),
            OsString::from("tests/"),
        ];
        let rewritten = registry::rewrite_argv("python", &args, &[]).unwrap();
        let actual: Vec<String> = rewritten
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(actual, vec!["rtk", "pytest", "-x", "tests/"]);
    }

    #[test]
    fn test_rewrite_gh_json_argv_skipped() {
        let args = vec![
            OsString::from("pr"),
            OsString::from("list"),
            OsString::from("--json"),
            OsString::from("number"),
        ];
        assert!(registry::rewrite_argv("gh", &args, &[]).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_clean_path_without_shims_removes_matching_entry() {
        let temp = tempfile::tempdir().unwrap();
        let shim_dir = temp.path().join("shims");
        fs::create_dir_all(&shim_dir).unwrap();

        let current_exe = env::current_exe().unwrap();
        std::os::unix::fs::symlink(&current_exe, shim_dir.join("git")).unwrap();

        let original_path =
            env::join_paths([shim_dir.as_path(), Path::new("/usr/bin"), Path::new("/bin")])
                .unwrap();

        let _guard = temp_env::with_var("PATH", Some(original_path));
        let cleaned = clean_path_without_shims("git", &current_exe).unwrap();
        let parts: Vec<PathBuf> = env::split_paths(&cleaned).collect();

        assert!(!parts.contains(&shim_dir));
        assert!(parts.contains(&PathBuf::from("/usr/bin")));
    }

    mod temp_env {
        use std::ffi::OsString;

        pub struct Guard {
            key: &'static str,
            prev: Option<OsString>,
        }

        impl Drop for Guard {
            fn drop(&mut self) {
                match &self.prev {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }

        pub fn with_var(key: &'static str, value: Option<OsString>) -> Guard {
            let prev = std::env::var_os(key);
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
            Guard { key, prev }
        }
    }
}
