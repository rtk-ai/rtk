//! Atlassian CLI (acli) proxy with token-optimized output.
//!
//! Routes `rtk acli <product> [args...]` to product-specific filter modules.
//! Unknown products passthrough to the real acli binary unchanged.

use anyhow::Result;
use crate::core::utils::{exit_code_from_output, resolved_command};
use crate::core::tracking;

/// Entry point called from main.rs routing arm.
pub fn run(product: &str, args: &[String], verbose: u8) -> Result<i32> {
    match product {
        "jira" => super::jira::run(args, verbose),
        "confluence" => super::confluence::run(args, verbose),
        _ => run_passthrough(product, args, verbose),
    }
}

/// Execute acli without filtering for unrecognised products (admin, rovodev, auth, etc.)
fn run_passthrough(product: &str, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    if verbose > 0 {
        eprintln!("rtk acli: passthrough for product '{}'", product);
    }
    let mut cmd_args: Vec<String> = vec![product.to_string()];
    cmd_args.extend_from_slice(args);

    let output = resolved_command("acli")
        .args(&cmd_args)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run acli: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_code = exit_code_from_output(&output, "acli");

    if !stdout.is_empty() {
        print!("{}", stdout);
    }
    if !stderr.is_empty() {
        eprint!("{}", stderr);
    }

    let raw = format!("{}\n{}", stdout, stderr);
    timer.track(
        &format!("acli {} {}", product, args.join(" ")),
        &format!("rtk acli {} {} (passthrough)", product, args.join(" ")),
        &raw,
        &raw,
    );

    Ok(exit_code)
}
