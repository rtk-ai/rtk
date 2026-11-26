//! Compact output for mount-related system commands.
//!
//! The no-argument `mount` listing is reduced to source, mountpoint and filesystem
//! type. Commands with explicit options and mutation commands are passed through
//! unchanged so RTK does not alter their requested output or behavior.

use crate::core::runner::{self, RunOptions};
use crate::core::shell::display_args;
use crate::core::utils::{ChildArgExt, resolved_command};
use anyhow::Result;

pub fn run(command: &str, args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command(command);
    cmd.child_args(args);

    if verbose > 0 {
        eprintln!("Running: {} {}", command, args.join(" "));
    }

    let compact_listing = command == "mount" && args.is_empty();
    runner::run_filtered(
        cmd,
        command,
        &display_args(args),
        |stdout| if compact_listing { compact_mounts(stdout) } else { stdout.to_string() },
        RunOptions::stdout_only().early_exit_on_failure().no_trailing_newline(),
    )
}

fn compact_mounts(raw: &str) -> String {
    let mut output = Vec::new();
    for line in raw.lines() {
        let Some((source, rest)) = line.split_once(" on ") else {
            output.push(line.to_string());
            continue;
        };
        let Some((target, rest)) = rest.split_once(" type ") else {
            output.push(line.to_string());
            continue;
        };
        let filesystem = rest.split_once(" (").map_or(rest, |(fs, _)| fs);
        output.push(format!("{source} -> {target} [{filesystem}]"));
    }
    if output.is_empty() { String::new() } else { output.join("\\n") + "\\n" }
}

#[cfg(test)]
mod tests {
    use super::compact_mounts;

    #[test]
    fn compact_listing_keeps_source_target_and_filesystem() {
        let raw = "/dev/sda1 on / type ext4 (rw,relatime)\\n/dev/sdb1 on /media/usb type vfat (rw,nosuid)\\n";
        assert_eq!(compact_mounts(raw), "/dev/sda1 -> / [ext4]\\n/dev/sdb1 -> /media/usb [vfat]\\n");
    }

    #[test]
    fn unknown_mount_format_is_preserved() {
        let raw = "custom mount output\\n";
        assert_eq!(compact_mounts(raw), raw);
    }
}
