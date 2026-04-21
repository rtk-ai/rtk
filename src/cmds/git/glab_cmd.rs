//! GitLab CLI (glab) passthrough support.
//!
//! This command is intentionally passthrough-only for now. It enables
//! first-class CLI routing (`rtk glab ...`) and discover rewrite support.

use anyhow::Result;
use std::ffi::OsString;

/// Run a glab subcommand in passthrough mode.
pub fn run(subcommand: &str, args: &[String], _verbose: u8) -> Result<i32> {
    let mut os_args: Vec<OsString> = vec![OsString::from(subcommand)];
    os_args.extend(args.iter().map(OsString::from));
    crate::core::runner::run_passthrough("glab", &os_args, 0)
}
