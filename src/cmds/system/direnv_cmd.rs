//! Filters `direnv exec` output using the shared TOML registry.

use crate::core::runner;
use crate::core::toml_filter;
use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command};
use anyhow::{bail, Context, Result};
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::process::Stdio;

pub fn run(args: &[OsString], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        bail!("direnv: no arguments specified");
    }

    let toml_disabled = std::env::var("RTK_NO_TOML").ok().as_deref() == Some("1");
    if toml_disabled || !matches!(args.first().and_then(|arg| arg.to_str()), Some("exec")) {
        return runner::run_passthrough("direnv", args, verbose);
    }

    let args_str = tracking::args_display(args);
    let original_cmd = format!("direnv {}", args_str);
    let rtk_cmd = format!("rtk direnv {}", args_str);

    if verbose > 0 {
        eprintln!("Running: {}", original_cmd);
    }

    let lookup_cmd = render_lookup_command(args);
    let Some(filter) = toml_filter::find_matching_filter(&lookup_cmd) else {
        return runner::run_passthrough("direnv", args, verbose);
    };

    let timer = tracking::TimedExecution::start();
    let output = resolved_command("direnv")
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to run direnv exec")?;

    let stdout_raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr_raw = String::from_utf8_lossy(&output.stderr).into_owned();
    let filtered_stdout = toml_filter::apply_filter(filter, &stdout_raw);
    let filtered_stderr = toml_filter::apply_filter(filter, &stderr_raw);

    write_filtered(&mut io::stdout().lock(), &filtered_stdout, &stdout_raw)?;
    write_filtered(&mut io::stderr().lock(), &filtered_stderr, &stderr_raw)?;

    timer.track(
        &original_cmd,
        &rtk_cmd,
        &format!("{}{}", stdout_raw, stderr_raw),
        &format!("{}{}", filtered_stdout, filtered_stderr),
    );

    Ok(exit_code_from_output(&output, "direnv"))
}

fn render_lookup_command(args: &[OsString]) -> String {
    let mut rendered = Vec::with_capacity(args.len() + 1);
    rendered.push("direnv".to_string());
    rendered.extend(args.iter().map(|arg| shell_quote(arg)));
    rendered.join(" ")
}

fn shell_quote(arg: &OsStr) -> String {
    let value = arg.to_string_lossy();
    if value.is_empty() {
        return "''".to_string();
    }

    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return value.into_owned();
    }

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn write_filtered<W: Write>(writer: &mut W, filtered: &str, raw: &str) -> io::Result<()> {
    if filtered.is_empty() {
        return Ok(());
    }

    writer.write_all(filtered.as_bytes())?;
    if raw.ends_with('\n') && !filtered.ends_with('\n') {
        writer.write_all(b"\n")?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_lookup_command_quotes_shell_fragments() {
        let args = vec![
            OsString::from("exec"),
            OsString::from("."),
            OsString::from("sh"),
            OsString::from("-lc"),
            OsString::from("printenv >&2"),
        ];

        assert_eq!(
            render_lookup_command(&args),
            "direnv exec . sh -lc 'printenv >&2'"
        );
    }
}
