//! Compact, bounded diagnostics for embedded and native build tools.

use crate::core::ai_output::{AiDocument, AiRecord, BudgetClass, Omission, Severity};
use crate::core::runner::{self, RunOptions};
use anyhow::Result;
use std::ffi::OsString;

/// Run a build/analysis executable once and summarize only its already-captured
/// output. Structured or script modes remain exact.
pub fn run_tool(program: &str, args: &[String], verbose: u8) -> Result<i32> {
    if requires_exact_output(program, args) {
        return runner::run_passthrough_with_reason(
            program,
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            verbose,
            crate::core::ai_output::ExactReason::Structured,
        );
    }

    let mut command = crate::core::utils::resolved_command(program);
    command.args(args);
    runner::run_ai_filtered_with_exit(
        command,
        program,
        &args.join(" "),
        BudgetClass::Diagnostic,
        build_document,
        RunOptions::with_tee(program).inherit_stdin(),
    )
}

fn requires_exact_output(program: &str, args: &[String]) -> bool {
    if program.eq_ignore_ascii_case("cmake")
        && args
            .iter()
            .any(|arg| arg == "-P" || arg == "--trace-expand")
    {
        return true;
    }
    args.iter().any(|arg| {
        arg == "--xml"
            || arg.starts_with("--xml-version")
            || arg == "--sarif"
            || arg.contains("diagnostics-format=json")
            || arg == "-t"
    })
}

fn build_document(raw: &str, exit_code: i32) -> Result<AiDocument> {
    let mut document = AiDocument::new(Some(if exit_code == 0 { "ok" } else { "failed" }));
    document.fact("exit", exit_code.to_string());
    let mut retained = 0usize;
    let mut omitted = 0usize;

    for line in raw.lines() {
        let lower = line.to_ascii_lowercase();
        let is_error = lower.contains("error")
            || lower.contains("fatal")
            || lower.contains("undefined reference")
            || lower.contains("failed")
            || lower.contains("overflow");
        let is_warning = lower.contains("warning") || lower.contains("deprecated");
        let is_progress = lower.contains("ninja: no work")
            || lower.contains("building ")
            || lower.contains("compiling ")
            || lower.starts_with("[");
        if is_error || is_warning || (!is_progress && retained < 8) {
            let severity = if is_error {
                Severity::Error
            } else if is_warning {
                Severity::Warning
            } else {
                Severity::Info
            };
            document.push(AiRecord::new(severity, line));
            retained += 1;
        } else {
            omitted += 1;
        }
    }

    if exit_code != 0 && retained == 0 {
        document.push(AiRecord::new(
            Severity::Error,
            "producer failed; no diagnostic text was captured",
        ));
    }
    if omitted > 0 {
        document = document.with_omission(Omission {
            items: omitted,
            groups: 0,
        });
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_failure_is_retained_after_progress_noise() {
        let raw = (0..100)
            .map(|index| format!("[{index}/100] compiling component"))
            .chain(["error: missing header".to_string()])
            .collect::<Vec<_>>()
            .join("\n");
        let rendered = crate::core::ai_output::render(
            &build_document(&raw, 1).unwrap(),
            BudgetClass::Diagnostic,
        );
        assert!(rendered.text.contains("missing header"));
        assert!(rendered.text.contains("exit=1"));
    }

    #[test]
    fn machine_modes_are_exact() {
        assert!(requires_exact_output(
            "cmake",
            &["-P".into(), "script.cmake".into()]
        ));
        assert!(requires_exact_output("cppcheck", &["--xml".into()]));
        assert!(!requires_exact_output(
            "ninja",
            &["-C".into(), "build".into()]
        ));
    }
}
