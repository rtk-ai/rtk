//! Generic Python proxy that summarizes only large, successful output.

use crate::cmds::system::summary;
use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::ffi::OsString;

const SUMMARY_THRESHOLD_BYTES: usize = 1024;

pub fn run(executable: &str, args: &[String], verbose: u8) -> Result<i32> {
    if should_passthrough(args) {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough(executable, &args, verbose);
    }

    let mut cmd = resolved_command(executable);
    cmd.args(args);
    let command_label = format!("{} {}", executable, args.join(" "));
    if verbose > 0 {
        eprintln!("Running: {}", command_label.trim_end());
    }

    runner::run_filtered(
        cmd,
        executable,
        &args.join(" "),
        |raw| filter_output(raw, &command_label, None),
        RunOptions::default()
            .inherit_stdin()
            .early_exit_on_failure(),
    )
}

fn should_passthrough(args: &[String]) -> bool {
    if args.is_empty() {
        return true;
    }

    if args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "-V" | "-VV" | "--version" | "-h" | "--help"
        )
            || arg == "-i"
            || arg == "--inspect"
            || (arg.starts_with('-')
                && !arg.starts_with("--")
                && arg.chars().skip(1).any(|flag| flag == 'i'))
    }) {
        return true;
    }

    args.windows(2).any(|pair| {
        pair[0] == "-m"
            && matches!(
                pair[1].as_str(),
                "code" | "pdb" | "idlelib" | "IPython" | "ipython"
            )
    })
}

fn filter_output(raw: &str, command_label: &str, recovery_hint: Option<&str>) -> String {
    if raw.len() <= SUMMARY_THRESHOLD_BYTES {
        return raw.trim_end_matches(['\r', '\n']).to_string();
    }

    let mut output = summary::summarize_output(raw, command_label, true);
    let hint = recovery_hint
        .map(str::to_string)
        .or_else(|| crate::core::tee::force_tee_hint(raw, "python"));
    if let Some(hint) = hint {
        output.push('\n');
        output.push_str(&hint);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_and_version_modes_pass_through() {
        for args in [
            vec![],
            vec!["-i".into()],
            vec!["-iq".into(), "script.py".into()],
            vec!["--version".into()],
            vec!["-m".into(), "pdb".into(), "script.py".into()],
        ] {
            assert!(should_passthrough(&args), "expected passthrough for {args:?}");
        }
    }

    #[test]
    fn scripts_and_inline_commands_are_filterable() {
        for args in [
            vec!["script.py".into()],
            vec!["-c".into(), "print(1)".into()],
            vec!["-m".into(), "http.server".into()],
            vec!["-".into()],
        ] {
            assert!(!should_passthrough(&args), "expected filtering for {args:?}");
        }
    }

    #[test]
    fn short_output_is_preserved() {
        assert_eq!(filter_output("one\ntwo\n", "python3 demo.py", None), "one\ntwo");
    }

    #[test]
    fn large_output_is_summarized_with_recovery() {
        let raw = (1..=200)
            .map(|line| format!("debug value {line}: {}", "x".repeat(20)))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_output(
            &raw,
            "python3 demo.py",
            Some("[full output: /tmp/python.log]"),
        );

        assert!(filtered.contains("Command: python3 demo.py"));
        assert!(filtered.contains("200 lines of output"));
        assert!(filtered.ends_with("[full output: /tmp/python.log]"));
        assert!(filtered.len() < raw.len());
    }
}
