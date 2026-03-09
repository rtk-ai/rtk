use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::{metadata, utils};

const RTK_OPERATIONAL_COMMAND_BYPASS_ENV: &str = "RTK_BYPASS_OPERATIONAL_COMMAND_SHIMS";
const RTK_RECURSION_DEPTH_ENV: &str = "RTK_RECURSION_DEPTH";
const RTK_RECURSION_DEPTH_LIMIT: u32 = 32;

pub(crate) fn operational_command_name_from_argv0(argv0: &OsStr) -> Option<String> {
    let basename = Path::new(argv0).file_name()?.to_string_lossy();
    let trimmed = basename.strip_suffix(".exe").unwrap_or(&basename);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn build_parse_argv(raw_argv: &[OsString]) -> Vec<OsString> {
    if raw_argv.is_empty() {
        return vec![OsString::from("rtk")];
    }

    let Some(operational_command) = operational_command_name_from_argv0(&raw_argv[0]) else {
        return raw_argv.to_vec();
    };

    if !metadata::is_shim_eligible_top_level_command(&operational_command) {
        return raw_argv.to_vec();
    }

    let mut parse_argv = Vec::with_capacity(raw_argv.len() + 1);
    parse_argv.push(OsString::from("rtk"));
    parse_argv.push(OsString::from(operational_command));
    parse_argv.extend(raw_argv.iter().skip(1).cloned());
    parse_argv
}

fn current_recursion_depth() -> u32 {
    std::env::var(RTK_RECURSION_DEPTH_ENV)
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
}

fn install_child_bypass_env(depth: u32) {
    std::env::set_var(RTK_OPERATIONAL_COMMAND_BYPASS_ENV, "1");
    std::env::set_var(RTK_RECURSION_DEPTH_ENV, (depth + 1).to_string());
}

fn maybe_exec_native_operational_command(raw_argv: &[OsString]) -> Result<bool> {
    if std::env::var(RTK_OPERATIONAL_COMMAND_BYPASS_ENV).unwrap_or_default() != "1" {
        return Ok(false);
    }
    if raw_argv.is_empty() {
        return Ok(false);
    }

    let Some(operational_command) = operational_command_name_from_argv0(&raw_argv[0]) else {
        return Ok(false);
    };
    if !metadata::is_shim_eligible_top_level_command(&operational_command) {
        return Ok(false);
    }

    let mut cmd = utils::native_command(&operational_command).with_context(|| {
        format!(
            "Failed to resolve native command for '{}'",
            operational_command
        )
    })?;
    cmd.args(raw_argv.iter().skip(1))
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());

    let status = cmd
        .status()
        .with_context(|| format!("Failed to execute native '{}'", operational_command))?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(true)
}

pub(crate) fn prepare_runtime_parse_argv() -> Result<Option<Vec<OsString>>> {
    let raw_argv: Vec<OsString> = std::env::args_os().collect();
    let recursion_depth = current_recursion_depth();
    if recursion_depth >= RTK_RECURSION_DEPTH_LIMIT {
        anyhow::bail!(
            "Detected recursive operational_command-shim invocation (depth={}). Refusing to continue.",
            recursion_depth
        );
    }

    if maybe_exec_native_operational_command(&raw_argv)? {
        return Ok(None);
    }

    // Child subprocesses should bypass operational_command rewrite and resolve native commands directly.
    install_child_bypass_env(recursion_depth);

    Ok(Some(build_parse_argv(&raw_argv)))
}

pub(crate) fn should_block_fallback_for_excluded_shim_command(parse_argv: &[OsString]) -> bool {
    let Some(operational_command) = parse_argv
        .first()
        .and_then(|s| operational_command_name_from_argv0(s))
    else {
        return false;
    };

    metadata::is_supported_top_level_command(&operational_command)
        && !metadata::is_shim_eligible_top_level_command(&operational_command)
}
