//! Filters aube output — install logs and script runners.
//!
//! [aube](https://github.com/jdx/aube) is a Rust pnpm-compatible package manager.
//! `aubr` / `aubx` are multicall shims for `aube run` and `aube dlx`.

use crate::core::utils::{join_or_ok, resolved_command, strip_ansi};
use anyhow::Result;
use std::ffi::OsString;

/// Filter aube install/ci/add output — strip banners and progress noise.
pub fn filter_aube_pkg(output: &str) -> String {
    let cleaned = strip_ansi(output);
    let mut result = Vec::new();

    for line in cleaned.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("aube ") && trimmed.contains("by jdx") {
            continue;
        }
        if trimmed.starts_with("Auto-installing:") {
            continue;
        }
        if trimmed.starts_with("Resolving")
            || trimmed.starts_with("Downloading")
            || trimmed.contains("Progress")
            || trimmed.contains('│')
        {
            continue;
        }
        result.push(line);
    }

    join_or_ok(&result)
}

/// Filter script runner output (`aube test`, `aube run`) — strip echo lines.
pub fn filter_aube_script(output: &str) -> String {
    let cleaned = strip_ansi(output);
    let mut result = Vec::new();

    for line in cleaned.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("aube ") && trimmed.contains("by jdx") {
            continue;
        }
        if trimmed.starts_with("Auto-installing:") {
            continue;
        }
        if trimmed.starts_with("$ ") {
            continue;
        }
        result.push(line);
    }

    join_or_ok(&result)
}

fn run_pkg_subcmd(subcmd: &str, args: &[String], verbose: u8, tee: &str) -> Result<i32> {
    let mut cmd = resolved_command("aube");
    cmd.arg(subcmd);
    for arg in args {
        cmd.arg(arg);
    }
    if verbose > 0 {
        eprintln!("Running: aube {} {}", subcmd, args.join(" "));
    }
    let display = format!("{} {}", subcmd, args.join(" "));
    crate::core::runner::run_filtered(
        cmd,
        "aube",
        display.trim_end(),
        filter_aube_pkg,
        crate::core::runner::RunOptions::with_tee(tee),
    )
}

pub fn run_install(args: &[String], verbose: u8) -> Result<i32> {
    run_pkg_subcmd("install", args, verbose, "aube_install")
}

pub fn run_ci(args: &[String], verbose: u8) -> Result<i32> {
    run_pkg_subcmd("ci", args, verbose, "aube_ci")
}

pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    if crate::core::runner::is_watch_mode(args) {
        let mut passthrough: Vec<OsString> = vec![OsString::from("test")];
        passthrough.extend(args.iter().map(OsString::from));
        return run_passthrough(&passthrough, verbose);
    }

    let mut cmd = resolved_command("aube");
    cmd.arg("test").args(args);
    let display = format!("test {}", args.join(" "));
    crate::core::runner::run_filtered(
        cmd,
        "aube",
        display.trim_end(),
        filter_aube_script,
        crate::core::runner::RunOptions::with_tee("aube_test"),
    )
}

/// `aubr` is the multicall shim for `aube run <script>`.
pub fn run_aubr(args: &[String], verbose: u8) -> Result<i32> {
    if crate::core::runner::is_watch_mode(args) {
        let os_args: Vec<OsString> = std::iter::once(OsString::from("run"))
            .chain(args.iter().map(OsString::from))
            .collect();
        return run_passthrough(&os_args, verbose);
    }

    let mut cmd = resolved_command("aube");
    cmd.arg("run").args(args);
    let display = format!("run {}", args.join(" "));
    crate::core::runner::run_filtered(
        cmd,
        "aube",
        display.trim_end(),
        filter_aube_script,
        crate::core::runner::RunOptions::with_tee("aube_run"),
    )
}

/// `aubx` is the multicall shim for `aube dlx <tool>`.
pub fn run_aubx(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    if crate::core::runner::is_watch_mode(args) {
        let passthrough: Vec<OsString> = args.iter().map(OsString::from).collect();
        return crate::core::runner::run_passthrough("aubx", &passthrough, verbose);
    }
    super::npm_cmd::exec_with("aubx", args, verbose, skip_env)
}

pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("aube", args, verbose)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_aube_script_strips_banner_and_echo() {
        let raw = "Auto-installing: install state not found\naube 2.2.0 by jdx.dev · ✓ Already up to date\n$ node -e \"console.log(1)\"\n1\n";
        let filtered = filter_aube_script(raw);
        assert_eq!(filtered.trim(), "1");
    }

    #[test]
    fn test_filter_aube_pkg_keeps_errors() {
        let raw = "aube 2.2.0 by jdx.dev\nResolving dependencies\nERR_PNPM_SOMETHING went wrong\n";
        let filtered = filter_aube_pkg(raw);
        assert!(filtered.contains("ERR_PNPM"));
    }
}
