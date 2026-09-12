//! Google Cloud CLI output compression for a narrow set of high-volume commands.
//!
//! An explicit `--format` is always a passthrough: format choice is an API
//! contract with the caller. Unknown commands are also passthroughs.

use crate::core::stream::{exec_capture, CaptureResult};
use crate::core::tee::{force_tee_hint, force_tee_tail_hint, tee_and_hint};
use crate::core::tracking;
use crate::core::truncate::{CAP_INVENTORY, CAP_LIST};
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use serde_json::Value;

const MAX_LIST_ITEMS: usize = CAP_LIST;
const MAX_LOG_EVENTS: usize = CAP_INVENTORY;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SupportedCommand {
    ComputeInstances,
    ContainerClusters,
    RunServices,
    LoggingRead,
    StorageLs,
    StorageTransfer,
}

impl SupportedCommand {
    fn find(args: &[String]) -> Option<Self> {
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        if contains_words(&words, &["compute", "instances", "list"]) {
            Some(Self::ComputeInstances)
        } else if contains_words(&words, &["container", "clusters", "list"]) {
            Some(Self::ContainerClusters)
        } else if contains_words(&words, &["run", "services", "list"]) {
            Some(Self::RunServices)
        } else if contains_words(&words, &["logging", "read"]) {
            Some(Self::LoggingRead)
        } else if contains_words(&words, &["storage", "ls"]) {
            Some(Self::StorageLs)
        } else if contains_words(&words, &["storage", "cp"])
            || contains_words(&words, &["storage", "rsync"])
            || contains_words(&words, &["storage", "mv"])
        {
            Some(Self::StorageTransfer)
        } else {
            None
        }
    }

    fn format(self) -> Option<&'static str> {
        match self {
            Self::ComputeInstances => Some("json(name,zone,machineType,status,scheduling.preemptible)"),
            Self::ContainerClusters => Some("json(name,location,status,currentMasterVersion,currentNodeVersion)"),
            Self::RunServices => Some("json(metadata.name,status.latestReadyRevisionName,status.conditions)"),
            Self::LoggingRead => Some(
                "json(timestamp,severity,logName,textPayload,jsonPayload.message,protoPayload.status.message)",
            ),
            // `gcloud storage ls` only accepts its special `gsutil` format.
            // Its default is already one path per line, so preserve that API.
            Self::StorageLs => None,
            Self::StorageTransfer => None,
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::ComputeInstances => "gcloud_compute_instances_list",
            Self::ContainerClusters => "gcloud_container_clusters_list",
            Self::RunServices => "gcloud_run_services_list",
            Self::LoggingRead => "gcloud_logging_read",
            Self::StorageLs => "gcloud_storage_ls",
            Self::StorageTransfer => "gcloud_storage_transfer",
        }
    }
}

struct FilteredOutput {
    text: String,
    recall: Option<Recall>,
}

enum Recall {
    Tail(String, usize),
    Full(String),
}

/// Executes `gcloud` without changing a caller-selected format or an unsupported command.
pub fn run(subcommand: &str, args: &[String], verbose: u8) -> Result<i32> {
    let mut all_args = vec![subcommand.to_string()];
    all_args.extend(crate::core::args_utils::restore_double_dash(args));

    match (has_explicit_format(&all_args), SupportedCommand::find(&all_args)) {
        (false, Some(command)) => run_supported(&all_args, command, verbose),
        _ => crate::core::runner::run_passthrough(
            "gcloud",
            &all_args.iter().map(Into::into).collect::<Vec<_>>(),
            verbose,
        ),
    }
}

fn run_supported(args: &[String], command: SupportedCommand, verbose: u8) -> Result<i32> {
    let label = format!("gcloud {}", args.join(" "));
    let timer = tracking::TimedExecution::start();
    let mut child = resolved_command("gcloud");
    child.args(args);
    if let Some(format) = command.format() {
        child.arg(format!("--format={format}"));
    }
    if verbose > 0 {
        eprintln!("Running: {}", label);
    }

    let CaptureResult {
        stdout,
        stderr,
        exit_code,
    } = exec_capture(&mut child).context("Failed to run gcloud")?;
    let raw = combine_output(&stdout, &stderr);

    if exit_code != 0 {
        print!("{stdout}");
        eprint!("{stderr}");
        if let Some(hint) = tee_and_hint(&raw, command.slug(), exit_code) {
            if needs_hint_separator(&stdout, &stderr) {
                eprintln!();
            }
            eprintln!("{hint}");
        }
        timer.track(&label, &format!("rtk {label}"), &raw, &raw);
        return Ok(exit_code);
    }

    let filtered = match command {
        SupportedCommand::StorageLs => filter_storage_ls(&stdout),
        SupportedCommand::StorageTransfer => filter_storage_transfer(&stdout),
        _ => filter_json(command, &stdout),
    };
    let filtered = filtered.unwrap_or_else(|| {
        eprintln!("rtk: filter warning: gcloud output was not recognized; passing through raw output");
        FilteredOutput {
            text: stdout.clone(),
            recall: None,
        }
    });
    let hint = match filtered.recall.as_ref() {
        Some(Recall::Tail(lines, offset)) => force_tee_tail_hint(lines, command.slug(), *offset),
        Some(Recall::Full(content)) => force_tee_hint(content, command.slug()),
        None => None,
    };
    let shown = crate::core::runner::emit_guarded(&filtered.text, hint.as_deref(), &stdout);
    eprint!("{stderr}");
    timer.track(
        &label,
        &format!("rtk {label}"),
        &raw,
        &combine_output(&shown, &stderr),
    );
    Ok(0)
}

fn combine_output(stdout: &str, stderr: &str) -> String {
    if stderr.is_empty() {
        stdout.to_string()
    } else if stdout.is_empty() {
        stderr.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    }
}

fn needs_hint_separator(stdout: &str, stderr: &str) -> bool {
    let last_output = if stderr.is_empty() { stdout } else { stderr };
    !last_output.is_empty() && !last_output.ends_with('\n')
}

fn contains_words(args: &[&str], command: &[&str]) -> bool {
    args.windows(command.len()).any(|words| words == command)
}

fn has_explicit_format(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--format" || arg.starts_with("--format="))
}

fn filter_json(command: SupportedCommand, stdout: &str) -> Option<FilteredOutput> {
    let parsed = serde_json::from_str::<Value>(stdout).ok()?;
    let items = parsed.as_array()?;
    let max_items = match command {
        SupportedCommand::LoggingRead => MAX_LOG_EVENTS,
        _ => MAX_LIST_ITEMS,
    };
    let lines: Vec<String> = items.iter().map(|item| format_item(command, item)).collect();
    let text = if lines.is_empty() {
        stdout.trim_end().to_string()
    } else {
        lines.iter().take(max_items).cloned().collect::<Vec<_>>().join("\n")
    };
    let recall = (lines.len() > max_items)
        .then(|| Recall::Tail(lines.join("\n"), max_items + 1));
    Some(FilteredOutput { text, recall })
}

fn format_item(command: SupportedCommand, item: &Value) -> String {
    match command {
        SupportedCommand::ComputeInstances => format!(
            "{} {} {} {} preemptible={}",
            field(item, "name"),
            basename(&field(item, "zone")),
            basename(&field(item, "machineType")),
            field(item, "status"),
            nested(item, &["scheduling", "preemptible"]),
        ),
        SupportedCommand::ContainerClusters => format!(
            "{} {} {} control-plane={} node={}",
            field(item, "name"),
            field(item, "location"),
            field(item, "status"),
            field(item, "currentMasterVersion"),
            field(item, "currentNodeVersion"),
        ),
        SupportedCommand::RunServices => format!(
            "{} revision={} {}",
            nested(item, &["metadata", "name"]),
            nested(item, &["status", "latestReadyRevisionName"]),
            ready_condition(item),
        ),
        SupportedCommand::LoggingRead => format!(
            "{} {} {}: {}",
            field(item, "timestamp"),
            field(item, "severity"),
            basename(&field(item, "logName")),
            log_message(item),
        ),
        SupportedCommand::StorageLs => unreachable!("storage ls is line-oriented output"),
        SupportedCommand::StorageTransfer => unreachable!("storage transfers are text output"),
    }
}

fn field(value: &Value, key: &str) -> String {
    display(value.get(key))
}

fn nested(value: &Value, keys: &[&str]) -> String {
    display(keys.iter().try_fold(value, |current, key| current.get(*key)))
}

fn display(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.replace(['\n', '\r'], "\\n"),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| display(Some(value)))
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[details]".to_string(),
        Some(Value::Null) | None => "-".to_string(),
    }
}

fn basename(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn ready_condition(item: &Value) -> String {
    let mut conditions = item
        .pointer("/status/conditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let ready = conditions
        .find(|condition| condition.get("type").and_then(Value::as_str) == Some("Ready"));
    match ready {
        Some(condition) => format!("ready={}", field(condition, "status")),
        None => "ready=-".to_string(),
    }
}

fn log_message(item: &Value) -> String {
    [
        item.get("textPayload"),
        item.pointer("/jsonPayload/message"),
        item.pointer("/protoPayload/status/message"),
    ]
    .into_iter()
    .find_map(|value| value.and_then(Value::as_str))
    .map(|message| message.replace(['\n', '\r'], "\\n"))
    .unwrap_or_else(|| "-".to_string())
}

fn filter_storage_ls(stdout: &str) -> Option<FilteredOutput> {
    let lines: Vec<String> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect();
    if lines.is_empty() && !stdout.trim().is_empty() {
        return None;
    }
    let text = lines
        .iter()
        .take(MAX_LIST_ITEMS)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let recall = (lines.len() > MAX_LIST_ITEMS)
        .then(|| Recall::Tail(lines.join("\n"), MAX_LIST_ITEMS + 1));
    Some(FilteredOutput { text, recall })
}

fn filter_storage_transfer(stdout: &str) -> Option<FilteredOutput> {
    let mut important = Vec::new();
    let mut last_progress = None;
    for line in stdout.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("error") || lower.contains("warning") || lower.contains("failed") {
            important.push(line.to_string());
        } else if lower.contains("completed")
            || lower.contains("copying")
            || lower.contains("moving")
            || lower.contains("synchronizing")
        {
            last_progress = Some(line.to_string());
        } else {
            important.push(line.to_string());
        }
    }
    if let Some(progress) = last_progress {
        important.push(progress);
    }
    let text = important.join("\n");
    // A transfer's source/destination paths are multi-line progress, not a flat inventory.
    let recall = (text.trim_end() != stdout.trim_end()).then(|| Recall::Full(stdout.to_string()));
    Some(FilteredOutput { text, recall })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::guard::never_worse;
    use crate::core::tracking::estimate_tokens;

    const COMPUTE: &str = include_str!("../../../tests/fixtures/gcloud/compute_instances.json");
    const CLUSTERS: &str = include_str!("../../../tests/fixtures/gcloud/container_clusters.json");
    const SERVICES: &str = include_str!("../../../tests/fixtures/gcloud/run_services.json");
    const LOGS: &str = include_str!("../../../tests/fixtures/gcloud/logging_read.json");
    const STORAGE: &str = include_str!("../../../tests/fixtures/gcloud/storage_ls.txt");

    #[test]
    fn routes_hot_paths_after_global_flags() {
        let args = ["--project", "redacted-project", "compute", "instances", "list"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(SupportedCommand::find(&args), Some(SupportedCommand::ComputeInstances));
    }

    #[test]
    fn explicit_format_forms_are_always_passthroughs() {
        assert!(has_explicit_format(&[
            "compute".into(),
            "--format=value(name)".into(),
        ]));
        assert!(has_explicit_format(&["compute".into(), "--format=json".into()]));
        assert!(has_explicit_format(&[
            "compute".into(),
            "--format".into(),
            "value(name)".into(),
        ]));
        assert!(!has_explicit_format(&["compute".into(), "instances".into()]));
    }

    #[test]
    fn unsupported_commands_are_not_routed_to_a_filter() {
        let args = ["artifacts", "docker", "images", "list"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(SupportedCommand::find(&args), None);
    }

    #[test]
    fn filters_redacted_hot_path_fixtures() {
        let cases = [
            (SupportedCommand::ComputeInstances, COMPUTE, "preemptible=true"),
            (SupportedCommand::ContainerClusters, CLUSTERS, "node=1.30.4"),
            (SupportedCommand::RunServices, SERVICES, "ready=True"),
            (SupportedCommand::LoggingRead, LOGS, "ERROR"),
            (SupportedCommand::StorageLs, STORAGE, "artifact-001"),
        ];
        for (command, raw, expected) in cases {
            let output = match command {
                SupportedCommand::StorageLs => filter_storage_ls(raw),
                _ => filter_json(command, raw),
            }
            .unwrap()
            .text;
            assert!(output.contains(expected), "{command:?}: {output}");
            let savings = 1.0 - estimate_tokens(&output) as f64 / estimate_tokens(raw) as f64;
            assert!(
                savings >= 0.20,
                "{command:?} should save at least 20%, got {:.1}%",
                savings * 100.0
            );
        }
    }

    #[test]
    fn compute_fixture_snapshot() {
        assert_eq!(
            filter_json(SupportedCommand::ComputeInstances, COMPUTE)
                .unwrap()
                .text,
            "redacted-web-01 region-a e2-medium RUNNING preemptible=true\nredacted-batch-02 region-b n2-standard-4 TERMINATED preemptible=false"
        );
    }

    #[test]
    fn malformed_json_falls_back_to_raw() {
        assert!(filter_json(SupportedCommand::ComputeInstances, "not json").is_none());
    }

    #[test]
    fn empty_results_preserve_their_recognizable_raw_form() {
        assert_eq!(filter_json(SupportedCommand::ComputeInstances, "[]").unwrap().text, "[]");
        assert_eq!(filter_storage_ls("").unwrap().text, "");
    }

    #[test]
    fn inventory_cap_has_recovery_lines() {
        let raw = format!("[{}]", (0..=MAX_LIST_ITEMS).map(|n| format!(r#"{{"name":"vm-{n}"}}"#)).collect::<Vec<_>>().join(","));
        let output = filter_json(SupportedCommand::ComputeInstances, &raw).unwrap();
        assert_eq!(output.text.lines().count(), MAX_LIST_ITEMS);
        assert!(matches!(output.recall, Some(Recall::Tail(_, offset)) if offset == MAX_LIST_ITEMS + 1));
    }

    #[test]
    fn storage_inventory_uses_the_list_cap_and_recovery_lines() {
        let output = filter_storage_ls(STORAGE).unwrap();
        assert_eq!(output.text.lines().count(), MAX_LIST_ITEMS);
        assert!(matches!(output.recall, Some(Recall::Tail(_, offset)) if offset == MAX_LIST_ITEMS + 1));
    }

    #[test]
    fn logging_uses_inventory_cap_and_keeps_event_boundaries() {
        let raw = format!("[{}]", (0..=MAX_LOG_EVENTS).map(|n| format!(r#"{{"timestamp":"2026-01-01T00:00:{n:02}Z","textPayload":"event {n}"}}"#)).collect::<Vec<_>>().join(","));
        let output = filter_json(SupportedCommand::LoggingRead, &raw).unwrap();
        assert_eq!(output.text.lines().count(), MAX_LOG_EVENTS);
        assert!(output.text.contains("event 0"));
        assert!(matches!(output.recall, Some(Recall::Tail(_, offset)) if offset == MAX_LOG_EVENTS + 1));
    }

    #[test]
    fn transfer_keeps_warnings_errors_and_final_progress() {
        let raw = "Copying source-a\nCopying source-b\nWARNING: retrying\nERROR: object denied\nCompleted 2 objects";
        assert_eq!(
            filter_storage_transfer(raw).unwrap().text,
            "WARNING: retrying\nERROR: object denied\nCompleted 2 objects"
        );
    }

    #[test]
    fn transfer_does_not_recall_for_a_trailing_newline_alone() {
        let output = filter_storage_transfer("Completed 1 object\n").unwrap();
        assert_eq!(output.text, "Completed 1 object");
        assert!(output.recall.is_none());
    }

    #[test]
    fn guard_falls_back_when_a_hint_would_cost_more_than_raw() {
        assert_eq!(never_worse("[]", "\n[full output: rtk recall abc]"), "[]");
    }

    #[test]
    fn stderr_is_part_of_the_tracked_raw_output() {
        assert_eq!(combine_output("[]", "WARNING: redacted"), "[]\nWARNING: redacted");
    }

    #[test]
    fn failure_hint_separator_respects_the_last_output_stream() {
        assert!(needs_hint_separator("stdout", "stderr"));
        assert!(needs_hint_separator("stdout", ""));
        assert!(!needs_hint_separator("stdout\n", "stderr\n"));
        assert!(!needs_hint_separator("", ""));
    }
}
