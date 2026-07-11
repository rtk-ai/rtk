//! Process listing command with Windows-native compact output.

use anyhow::Result;
#[cfg(not(target_os = "windows"))]
use anyhow::Context;
#[cfg(not(target_os = "windows"))]
use crate::core::utils::{exit_code_from_status, resolved_command};

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    #[cfg(target_os = "windows")]
    {
        run_native(args, verbose)
    }

    #[cfg(not(target_os = "windows"))]
    {
        run_external(args, verbose)
    }
}

#[cfg(not(target_os = "windows"))]
fn run_external(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("Running: ps {}", args.join(" "));
    }
    let status = resolved_command("ps").args(args).status().context("Failed to run ps")?;
    Ok(exit_code_from_status(&status, "ps"))
}

#[cfg(target_os = "windows")]
fn run_native(args: &[String], verbose: u8) -> Result<i32> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(0);
    }
    if !args.is_empty() {
        eprintln!(
            "rtk ps: unsupported arguments '{}'; use rtk proxy ps ... for native ps semantics",
            args.join(" ")
        );
        return Ok(2);
    }

    if verbose > 0 {
        eprintln!("Running native ps");
    }

    print!("{}", native_ps_output());
    Ok(0)
}

#[cfg(target_os = "windows")]
fn native_ps_output() -> String {
    let mut system = sysinfo::System::new();
    system.refresh_processes();

    let rows: Vec<(u32, String)> = system
        .processes()
        .iter()
        .map(|(pid, process)| (pid.as_u32(), process.name().to_string()))
        .collect();
    format_ps_rows(rows)
}

#[cfg(any(target_os = "windows", test))]
fn format_ps_rows(mut rows: Vec<(u32, String)>) -> String {
    rows.sort_by_key(|(pid, _)| *pid);

    let mut output = String::from("PID NAME\n");
    for (pid, name) in rows {
        output.push_str(&format!("{pid} {name}\n"));
    }
    output
}

#[cfg(target_os = "windows")]
fn print_help() {
    println!(
        "Process list with compact output (native Windows)\n\n\
Usage: rtk ps [--help]\n\n\
Windows support:\n  no args       list PID and process name\n  -h, --help    print help\n\n\
Unsupported: ps aux, ps -ef, and other Unix ps arguments.\n\
Use `rtk proxy ps ...` for native shell semantics."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_ps_rows_sorts_and_uses_two_columns() {
        let output = format_ps_rows(vec![
            (42, "beta.exe".to_string()),
            (7, "alpha.exe".to_string()),
        ]);

        assert_eq!(output, "PID NAME\n7 alpha.exe\n42 beta.exe\n");
    }

    #[test]
    fn native_process_listing_uses_the_lightweight_system_constructor() {
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/cmds/system/ps.rs"));
        let forbidden_constructor = ["System::new", "_all"].concat();

        assert!(source.contains("System::new();"));
        assert!(!source.contains(&forbidden_constructor));
    }

    #[cfg(not(windows))]
    #[test]
    fn external_ps_spawn_has_actionable_context() {
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/cmds/system/ps.rs"));
        assert!(source.contains(".status().context(\"Failed to run ps\")?"));
    }
}
