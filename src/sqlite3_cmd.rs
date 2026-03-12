//! SQLite3 client wrapper with token-friendly defaults.
//!
//! V1 behavior intentionally stays close to native sqlite3:
//! - pass all args through unchanged
//! - preserve stdout/stderr and exit codes
//! - track command usage for RTK analytics

use crate::tracking;
use anyhow::{Context, Result};

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = std::process::Command::new("sqlite3");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: sqlite3 {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run sqlite3 (is sqlite3 installed?)")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}{}", stdout, stderr);

    if !stdout.is_empty() {
        print!("{}", stdout);
    }
    if !stderr.is_empty() {
        eprint!("{}", stderr);
    }

    let filtered = stdout.to_string();
    timer.track(
        &format!("sqlite3 {}", args.join(" ")),
        &format!("rtk sqlite3 {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}
