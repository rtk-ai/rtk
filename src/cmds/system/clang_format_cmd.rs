//! Runs clang-format and summarizes files changed by in-place formatting.

use crate::core::runner;
use crate::core::stream::status_to_exit_code;
use crate::core::tracking;
use crate::core::truncate::CAP_LIST;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;

const MAX_CHANGED_FILES: usize = CAP_LIST;

#[derive(Debug, PartialEq, Eq)]
enum InvocationPlan {
    Passthrough,
    Summarize { files: Vec<PathBuf> },
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedArgs {
    in_place: bool,
    dry_run: bool,
    output_replacements_xml: bool,
    stdin_seen: bool,
    response_file_seen: bool,
    files: Vec<PathBuf>,
    file_lists: Vec<PathBuf>,
}

#[derive(Debug)]
struct FileSnapshot {
    path: PathBuf,
    display: String,
    before: Vec<u8>,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    match plan_invocation(args) {
        InvocationPlan::Passthrough => run_passthrough(args, verbose),
        InvocationPlan::Summarize { files } => run_in_place_with_summary(args, &files, verbose),
    }
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
    runner::run_passthrough("clang-format", &os_args, verbose)
}

fn run_in_place_with_summary(args: &[String], files: &[PathBuf], verbose: u8) -> Result<i32> {
    let snapshots = snapshot_files(files);
    if snapshots.is_empty() {
        return run_passthrough(args, verbose);
    }

    if verbose > 0 {
        eprintln!("Running: clang-format {}", args.join(" "));
    }

    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("clang-format");
    cmd.args(args);

    let output = cmd.output().context("Failed to run clang-format")?;
    write_all_preserving_broken_pipe(io::stdout(), &output.stdout)?;
    write_all_preserving_broken_pipe(io::stderr(), &output.stderr)?;

    let exit_code = status_to_exit_code(output.status);
    let changed = changed_files(&snapshots);
    let summary = format_changed_summary(&changed);

    if !output.stdout.is_empty() && !output.stdout.ends_with(b"\n") {
        write_all_preserving_broken_pipe(io::stdout(), b"\n")?;
    }
    write_all_preserving_broken_pipe(io::stdout(), summary.as_bytes())?;
    write_all_preserving_broken_pipe(io::stdout(), b"\n")?;

    let args_display = args.join(" ");
    timer.track_passthrough(
        &format!("clang-format {}", args_display),
        &format!("rtk clang-format {} (summary)", args_display),
    );

    Ok(exit_code)
}

fn write_all_preserving_broken_pipe(mut writer: impl Write, bytes: &[u8]) -> io::Result<()> {
    if let Err(err) = writer.write_all(bytes) {
        if err.kind() == io::ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(err);
    }
    if let Err(err) = writer.flush() {
        if err.kind() == io::ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(err);
    }
    Ok(())
}

fn plan_invocation(args: &[String]) -> InvocationPlan {
    let parsed = parse_args(args);
    if !parsed.in_place
        || parsed.dry_run
        || parsed.output_replacements_xml
        || parsed.stdin_seen
        || parsed.response_file_seen
    {
        return InvocationPlan::Passthrough;
    }

    let Some(files) = expand_files(parsed.files, &parsed.file_lists) else {
        return InvocationPlan::Passthrough;
    };

    if files.is_empty() {
        InvocationPlan::Passthrough
    } else {
        InvocationPlan::Summarize { files }
    }
}

fn parse_args(args: &[String]) -> ParsedArgs {
    let mut parsed = ParsedArgs::default();
    let mut i = 0;
    let mut end_of_options = false;

    while i < args.len() {
        let arg = &args[i];

        if end_of_options {
            observe_positional(arg, &mut parsed);
            i += 1;
            continue;
        }

        if arg == "--" {
            end_of_options = true;
            i += 1;
            continue;
        }

        if arg.starts_with('@') {
            parsed.response_file_seen = true;
            i += 1;
            continue;
        }

        if is_in_place_flag(arg) {
            parsed.in_place = true;
            i += 1;
            continue;
        }

        if is_dry_run_flag(arg) {
            parsed.dry_run = true;
            i += 1;
            continue;
        }

        if is_output_replacements_xml_flag(arg) {
            parsed.output_replacements_xml = true;
            i += 1;
            continue;
        }

        if let Some(path) = inline_files_arg(arg) {
            parsed.file_lists.push(PathBuf::from(path));
            i += 1;
            continue;
        }

        if is_files_arg(arg) {
            if let Some(next) = args.get(i + 1) {
                parsed.file_lists.push(PathBuf::from(next));
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        if option_takes_next_value(arg) {
            i += 2;
            continue;
        }

        if arg.starts_with('-') {
            if arg == "-" {
                parsed.stdin_seen = true;
            }
            i += 1;
            continue;
        }

        observe_positional(arg, &mut parsed);
        i += 1;
    }

    parsed
}

fn observe_positional(arg: &str, parsed: &mut ParsedArgs) {
    if arg == "-" {
        parsed.stdin_seen = true;
    } else {
        parsed.files.push(PathBuf::from(arg));
    }
}

fn is_in_place_flag(arg: &str) -> bool {
    arg == "-i" || bool_option_enabled(arg, "-i") || bool_option_enabled(arg, "--inplace")
}

fn is_dry_run_flag(arg: &str) -> bool {
    arg == "-n" || bool_option_enabled(arg, "--dry-run") || bool_option_enabled(arg, "-dry-run")
}

fn is_output_replacements_xml_flag(arg: &str) -> bool {
    bool_option_enabled(arg, "--output-replacements-xml")
        || bool_option_enabled(arg, "-output-replacements-xml")
}

fn bool_option_enabled(arg: &str, name: &str) -> bool {
    if arg == name {
        return true;
    }

    let Some(value) = arg.strip_prefix(&format!("{}=", name)) else {
        return false;
    };
    !matches!(
        value.to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

fn inline_files_arg(arg: &str) -> Option<&str> {
    arg.strip_prefix("--files=")
        .or_else(|| arg.strip_prefix("-files="))
}

fn is_files_arg(arg: &str) -> bool {
    arg == "--files" || arg == "-files"
}

fn option_takes_next_value(arg: &str) -> bool {
    matches!(
        arg,
        "--assume-filename"
            | "-assume-filename"
            | "--cursor"
            | "-cursor"
            | "--fallback-style"
            | "-fallback-style"
            | "--ferror-limit"
            | "-ferror-limit"
            | "--length"
            | "-length"
            | "--lines"
            | "-lines"
            | "--offset"
            | "-offset"
            | "--qualifier-alignment"
            | "-qualifier-alignment"
            | "--style"
            | "-style"
            | "--Wno-error"
            | "-Wno-error"
    )
}

fn expand_files(mut files: Vec<PathBuf>, file_lists: &[PathBuf]) -> Option<Vec<PathBuf>> {
    for list_path in file_lists {
        let content = std::fs::read_to_string(list_path).ok()?;
        for line in content.lines() {
            let entry = line.trim();
            if entry.is_empty() {
                continue;
            }
            if entry == "-" {
                return None;
            }
            files.push(PathBuf::from(entry));
        }
    }
    Some(files)
}

fn snapshot_files(files: &[PathBuf]) -> Vec<FileSnapshot> {
    let mut snapshots = Vec::new();
    let mut seen = HashSet::new();

    for path in files {
        let Ok(before) = std::fs::read(path) else {
            continue;
        };
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if !seen.insert(key) {
            continue;
        }
        snapshots.push(FileSnapshot {
            path: path.clone(),
            display: path.display().to_string(),
            before,
        });
    }

    snapshots
}

fn changed_files(snapshots: &[FileSnapshot]) -> Vec<String> {
    snapshots
        .iter()
        .filter_map(|snapshot| match std::fs::read(&snapshot.path) {
            Ok(after) if after != snapshot.before => Some(snapshot.display.clone()),
            Err(_) => Some(snapshot.display.clone()),
            _ => None,
        })
        .collect()
}

fn format_changed_summary(changed_files: &[String]) -> String {
    if changed_files.is_empty() {
        return "clang-format: no changes".to_string();
    }

    let mut output = format!(
        "clang-format: changed {} {}",
        changed_files.len(),
        if changed_files.len() == 1 {
            "file"
        } else {
            "files"
        }
    );

    let all_lines: Vec<String> = changed_files
        .iter()
        .enumerate()
        .map(|(index, file)| format!("{}. {}", index + 1, file))
        .collect();

    for line in all_lines.iter().take(MAX_CHANGED_FILES) {
        output.push('\n');
        output.push_str(line);
    }

    if changed_files.len() > MAX_CHANGED_FILES {
        output.push_str(&format!(
            "\n... +{} more files",
            changed_files.len() - MAX_CHANGED_FILES
        ));
        let all_text = all_lines.join("\n");
        if let Some(hint) = crate::core::tee::force_tee_tail_hint(
            &all_text,
            "clang-format-files",
            MAX_CHANGED_FILES + 1,
        ) {
            output.push(' ');
            output.push_str(&hint);
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn test_parse_in_place_files() {
        let parsed = parse_args(&s(&["-i", "--style", "file", "a.cpp", "b.hpp"]));
        assert!(parsed.in_place);
        assert!(!parsed.dry_run);
        assert_eq!(parsed.files, vec![PathBuf::from("a.cpp"), PathBuf::from("b.hpp")]);
    }

    #[test]
    fn test_parse_assume_filename_is_not_input_file() {
        let parsed = parse_args(&s(&["-i", "--assume-filename", "virtual.cpp"]));
        assert!(parsed.in_place);
        assert!(parsed.files.is_empty());
        assert!(!parsed.stdin_seen);
    }

    #[test]
    fn test_parse_stdin_passthrough_marker() {
        let parsed = parse_args(&s(&["-i", "--assume-filename=virtual.cpp", "-"]));
        assert!(parsed.in_place);
        assert!(parsed.stdin_seen);
    }

    #[test]
    fn test_parse_dry_run_and_xml_flags() {
        let parsed = parse_args(&s(&[
            "-i",
            "--dry-run",
            "--Werror",
            "--output-replacements-xml",
            "a.cpp",
        ]));
        assert!(parsed.in_place);
        assert!(parsed.dry_run);
        assert!(parsed.output_replacements_xml);
    }

    #[test]
    fn test_plan_passes_through_stdout_producing_mode() {
        assert_eq!(
            plan_invocation(&s(&["file.cpp"])),
            InvocationPlan::Passthrough
        );
    }

    #[test]
    fn test_plan_passes_through_stdin_mode() {
        assert_eq!(
            plan_invocation(&s(&["-i", "--assume-filename", "file.cpp", "-"])),
            InvocationPlan::Passthrough
        );
    }

    #[test]
    fn test_plan_passes_through_dry_run_werror() {
        assert_eq!(
            plan_invocation(&s(&["--dry-run", "--Werror", "file.cpp"])),
            InvocationPlan::Passthrough
        );
    }

    #[test]
    fn test_plan_passes_through_dash_n_dry_run() {
        assert_eq!(
            plan_invocation(&s(&["-i", "-n", "file.cpp"])),
            InvocationPlan::Passthrough
        );
    }

    #[test]
    fn test_plan_passes_through_output_replacements_xml() {
        assert_eq!(
            plan_invocation(&s(&["-i", "--output-replacements-xml", "file.cpp"])),
            InvocationPlan::Passthrough
        );
    }

    #[test]
    fn test_plan_passes_through_response_file() {
        assert_eq!(
            plan_invocation(&s(&["-i", "@clang-format.args"])),
            InvocationPlan::Passthrough
        );
    }

    #[test]
    fn test_sort_includes_does_not_hide_input_file() {
        assert_eq!(
            plan_invocation(&s(&["-i", "--sort-includes", "file.cpp"])),
            InvocationPlan::Summarize {
                files: vec![PathBuf::from("file.cpp")]
            }
        );
    }

    #[test]
    fn test_expand_files_list() {
        let mut list = NamedTempFile::new().unwrap();
        writeln!(list, "a.cpp").unwrap();
        writeln!(list, "b.hpp").unwrap();

        let plan = plan_invocation(&s(&[
            "-i",
            "--files",
            list.path().to_str().unwrap(),
            "c.cc",
        ]));

        assert_eq!(
            plan,
            InvocationPlan::Summarize {
                files: vec![
                    PathBuf::from("c.cc"),
                    PathBuf::from("a.cpp"),
                    PathBuf::from("b.hpp"),
                ]
            }
        );
    }

    #[test]
    fn test_expand_inline_files_list() {
        let mut list = NamedTempFile::new().unwrap();
        writeln!(list, "a.cpp").unwrap();

        let plan = plan_invocation(&s(&[
            "-i",
            &format!("--files={}", list.path().display()),
        ]));

        assert_eq!(
            plan,
            InvocationPlan::Summarize {
                files: vec![PathBuf::from("a.cpp")]
            }
        );
    }

    #[test]
    fn test_unreadable_files_list_passes_through() {
        assert_eq!(
            plan_invocation(&s(&["-i", "--files=/missing/clang-format-list"])),
            InvocationPlan::Passthrough
        );
    }

    #[test]
    fn test_format_no_changes_summary() {
        assert_eq!(
            format_changed_summary(&[]),
            "clang-format: no changes".to_string()
        );
    }

    #[test]
    fn test_format_changed_files_summary() {
        let changed = vec!["a.cpp".to_string(), "src/b.hpp".to_string()];
        let summary = format_changed_summary(&changed);
        assert!(summary.starts_with("clang-format: changed 2 files"));
        assert!(summary.contains("1. a.cpp"));
        assert!(summary.contains("2. src/b.hpp"));
    }

    #[test]
    fn test_partial_failure_summary_still_reports_changed_files() {
        let changed = vec!["formatted-before-error.cpp".to_string()];
        let summary = format_changed_summary(&changed);
        assert!(summary.contains("clang-format: changed 1 file"));
        assert!(summary.contains("formatted-before-error.cpp"));
        assert!(!summary.contains("no changes"));
    }

    #[test]
    fn test_format_changed_files_truncates_with_hint_text() {
        let changed: Vec<String> = (0..(MAX_CHANGED_FILES + 2))
            .map(|index| format!("file{index}.cpp"))
            .collect();
        let summary = format_changed_summary(&changed);
        assert!(summary.contains("clang-format: changed 22 files"));
        assert!(summary.contains("... +2 more files"));
    }

    #[test]
    fn test_changed_files_detects_byte_changes_for_partial_failure_reporting() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "int main(){{return 0;}}").unwrap();

        let snapshots = snapshot_files(&[file.path().to_path_buf()]);
        std::fs::write(file.path(), "int main() { return 0; }\n").unwrap();

        let changed = changed_files(&snapshots);
        assert_eq!(changed, vec![file.path().display().to_string()]);
    }

    #[test]
    fn test_snapshot_ignores_unreadable_files() {
        let snapshots = snapshot_files(&[PathBuf::from("/missing/file.cpp")]);
        assert!(snapshots.is_empty());
    }
}
