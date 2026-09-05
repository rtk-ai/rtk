//! Synchronous stdio MCP adapter for RTK.

use super::{
    debug_enabled, redact_sensitive, redact_sensitive_lines, rewrite, run_filtered_with_request,
    OutputRequest, DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_TIMEOUT_MS,
};
use crate::core::config::Config;
use crate::core::tracking::Tracker;
use crate::hooks::agent_policy;
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const PROTOCOL_VERSION: &str = "2024-11-05";
const DEFAULT_LIST_LIMIT: usize = 50;
const MIN_SEMANTIC_OUTPUT_TOKENS: usize = 64;
const MAX_SEMANTIC_OUTPUT_TOKENS: usize = 65_536;
const MAX_RECOVERY_PAGE_BYTES: usize = 1_048_576;
const MAX_RECOVERY_LINE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseMode {
    Compact,
    Legacy,
}

pub fn run() -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut output = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line.context("Failed to read MCP stdin")?;
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle_request(&request),
            Err(error) => Some(rpc_error(None, -32700, &format!("Invalid JSON: {error}"))),
        };

        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response)
                .context("Failed to encode MCP response")?;
            output
                .write_all(b"\n")
                .context("Failed to write MCP response")?;
            output.flush().context("Failed to flush MCP response")?;
        }
    }
    Ok(())
}

fn handle_request(request: &Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str);

    if debug_enabled() {
        eprintln!(
            "[rtk-debug] mcp.request method={} notification={}",
            method.unwrap_or("<missing>"),
            id.is_none()
        );
    }

    let Some(method) = method else {
        return Some(rpc_error(id, -32600, "Request method is required"));
    };

    match method {
        "notifications/initialized" => None,
        "initialize" => Some(rpc_result(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "rtk", "version": env!("CARGO_PKG_VERSION") },
                "instructions": agent_policy::MCP_INSTRUCTIONS
            }),
        )),
        "tools/list" => Some(rpc_result(id, json!({ "tools": tool_definitions() }))),
        "tools/call" => Some(handle_tool_call(id, request.get("params"))),
        _ if id.is_none() => None,
        _ => Some(rpc_error(id, -32601, &format!("Unknown method: {method}"))),
    }
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "rewrite_command",
            "description": "Inspect how RTK would rewrite shell command text. For execution, prefer run_filtered with typed RTK argv.",
            "inputSchema": { "type": "object", "required": ["command"], "properties": {
                "command": { "type": "string" }
            }}
        }),
        json!({
            "name": "run_cmd",
            "description": agent_policy::RUN_CMD_DESCRIPTION,
            "inputSchema": { "type": "object", "required": ["expression"], "properties": {
                "expression": { "type": "string", "minLength": 1 },
                "cwd": { "type": "string" },
                "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 600000 },
                "max_output_bytes": { "type": "integer", "minimum": 1, "maximum": 10485760 },
                "tee_on_failure": { "type": "boolean" },
                "max_tokens": { "type": "integer", "minimum": 64, "maximum": 65536 },
                "max_output_tokens": { "type": "integer", "minimum": 64, "maximum": 65536 },
                "response_mode": { "type": "string", "enum": ["compact", "legacy"], "default": "compact" }
            }}
        }),
        json!({
            "name": "run_powershell",
            "description": agent_policy::RUN_POWERSHELL_DESCRIPTION,
            "inputSchema": { "type": "object", "required": ["host", "expression"], "properties": {
                "host": { "type": "string", "enum": ["powershell", "pwsh"] },
                "expression": { "type": "string", "minLength": 1 },
                "cwd": { "type": "string" },
                "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 600000 },
                "max_output_bytes": { "type": "integer", "minimum": 1, "maximum": 10485760 },
                "tee_on_failure": { "type": "boolean" },
                "max_tokens": { "type": "integer", "minimum": 64, "maximum": 65536 },
                "max_output_tokens": { "type": "integer", "minimum": 64, "maximum": 65536 },
                "response_mode": { "type": "string", "enum": ["compact", "legacy"], "default": "compact" }
            }}
        }),
        json!({
            "name": "run_filtered",
            "description": agent_policy::RUN_FILTERED_DESCRIPTION,
            "inputSchema": { "type": "object", "required": ["rtk_args"], "properties": {
                "rtk_args": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                "cwd": { "type": "string" },
                "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 600000 },
                "max_output_bytes": { "type": "integer", "minimum": 1, "maximum": 10485760 },
                "tee_on_failure": { "type": "boolean" },
                "max_tokens": { "type": "integer", "minimum": 64, "maximum": 65536 },
                "max_output_tokens": { "type": "integer", "minimum": 64, "maximum": 65536 },
                "response_mode": { "type": "string", "enum": ["compact", "legacy"], "default": "compact" }
            }}
        }),
        json!({
            "name": "gain_summary",
            "description": "Return RTK token savings statistics.",
            "inputSchema": { "type": "object", "properties": {
                "project": { "type": "boolean" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 }
            }}
        }),
        json!({
            "name": "discover_unhandled",
            "description": "Find RTK rewrite candidates in Claude pre-hook transcripts. Candidate counts are not confirmed misses; use gain_summary for executed RTK usage.",
            "inputSchema": { "type": "object", "properties": {
                "project": { "type": "string" },
                "since_days": { "type": "integer", "minimum": 1, "maximum": 3650 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 }
            }}
        }),
        json!({
            "name": "list_tee_artifacts",
            "description": "List RTK raw-output recovery files.",
            "inputSchema": { "type": "object", "properties": {
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 }
            }}
        }),
        json!({
            "name": "read_tee_file",
            "description": "Read a bounded RTK raw-output recovery file.",
            "inputSchema": { "type": "object", "required": ["path"], "properties": {
                "path": { "type": "string" },
                "max_bytes": { "type": "integer", "minimum": 1, "maximum": 10485760 }
            }}
        }),
        json!({
            "name": "read_recovery",
            "description": "Read a bounded page from a prior RTK recovery artifact by ID. This tool never executes the producer or accepts filesystem paths.",
            "inputSchema": { "type": "object", "required": ["recovery_id"], "properties": {
                "recovery_id": { "type": "string", "minLength": 1 },
                "lines": { "type": "string", "description": "Inclusive one-based range START:END." },
                "cursor": { "type": "integer", "minimum": 1 },
                "max_lines": { "type": "integer", "minimum": 1, "maximum": 500 }
            }}
        }),
        json!({
            "name": "search_recovery",
            "description": "Search a prior RTK recovery artifact by ID without rerunning its producer.",
            "inputSchema": { "type": "object", "required": ["recovery_id", "pattern"], "properties": {
                "recovery_id": { "type": "string", "minLength": 1 },
                "pattern": { "type": "string", "minLength": 1 },
                "regex": { "type": "boolean" },
                "context": { "type": "integer", "minimum": 0, "maximum": 20 },
                "max_matches": { "type": "integer", "minimum": 1, "maximum": 100 }
            }}
        }),
    ]
}

fn handle_tool_call(id: Option<Value>, params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return rpc_error(id, -32602, "tools/call params are required");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return rpc_error(id, -32602, "tools/call requires a tool name");
    };
    let arguments = params.get("arguments").unwrap_or(&Value::Null);

    let mode = match response_mode(arguments) {
        Ok(mode) => mode,
        Err(error) => return rpc_error(id, -32602, &error.to_string()),
    };

    match call_tool(name, arguments) {
        Ok(value) => {
            let value = match mode {
                ResponseMode::Compact => compact_tool_value(name, value),
                ResponseMode::Legacy => value,
            };
            rpc_result(
                id,
                json!({
                    "content": [{ "type": "text", "text": value.to_string() }],
                    "structuredContent": value
                }),
            )
        }
        Err(error) => {
            let message = error.to_string();
            let code = if !tool_definitions()
                .iter()
                .any(|tool| tool["name"].as_str() == Some(name))
            {
                -32601
            } else if message.contains("timed out") {
                -32002
            } else if is_invalid_tool_argument(&message) {
                -32602
            } else {
                -32001
            };
            rpc_error(id, code, &message)
        }
    }
}

fn response_mode(args: &Value) -> Result<ResponseMode> {
    match args.get("response_mode").and_then(Value::as_str) {
        None => Ok(default_response_mode()),
        Some("compact") => Ok(ResponseMode::Compact),
        Some("legacy") => Ok(ResponseMode::Legacy),
        Some(_) => anyhow::bail!("response_mode must be compact or legacy"),
    }
}

fn default_response_mode() -> ResponseMode {
    match std::env::var("RTK_MCP_RESPONSE_MODE").ok().as_deref() {
        Some("legacy") => ResponseMode::Legacy,
        _ => ResponseMode::Compact,
    }
}

fn compact_tool_value(name: &str, value: Value) -> Value {
    if !matches!(name, "run_cmd" | "run_powershell" | "run_filtered") {
        return value;
    }
    let Some(object) = value.as_object() else {
        return value;
    };
    let mut compact = Map::new();
    for key in ["exit_code", "stdout", "stderr"] {
        if let Some(value) = object.get(key) {
            compact.insert(key.to_string(), value.clone());
        }
    }
    if object.get("truncated") == Some(&Value::Bool(true)) {
        compact.insert("truncated".to_string(), Value::Bool(true));
    }
    Value::Object(compact)
}

fn is_invalid_tool_argument(message: &str) -> bool {
    [
        "must be ",
        "cannot ",
        "not supported",
        "between ",
        "outside the trusted",
        "does not exist",
        "is not a directory",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

fn call_tool(name: &str, args: &Value) -> Result<Value> {
    match name {
        "rewrite_command" => {
            let command = required_string(args, "command")?;
            Ok(serde_json::to_value(rewrite(command))?)
        }
        "run_cmd" => {
            let expression = required_string(args, "expression")?;
            #[cfg(not(windows))]
            {
                let _ = expression;
                anyhow::bail!("run_cmd is supported only on Windows");
            }
            #[cfg(windows)]
            {
                let (cwd, timeout, max_output, tee_on_failure) = execution_options(args)?;
                let rtk_args = vec!["cmd".to_string(), expression.to_string()];
                Ok(serde_json::to_value(run_filtered_with_request(
                    &rtk_args,
                    cwd.as_deref(),
                    timeout,
                    max_output,
                    tee_on_failure,
                    output_request(args)?,
                )?)?)
            }
        }
        "run_powershell" => {
            let host = required_string(args, "host")?;
            let expression = required_string(args, "expression")?;
            let route = match host {
                "powershell" | "pwsh" => host,
                _ => anyhow::bail!("host must be either powershell or pwsh"),
            };
            #[cfg(not(windows))]
            {
                let _ = (route, expression);
                anyhow::bail!("run_powershell is supported only on Windows");
            }
            #[cfg(windows)]
            {
                let (cwd, timeout, max_output, tee_on_failure) = execution_options(args)?;
                let rtk_args = vec![route.to_string(), expression.to_string()];
                Ok(serde_json::to_value(run_filtered_with_request(
                    &rtk_args,
                    cwd.as_deref(),
                    timeout,
                    max_output,
                    tee_on_failure,
                    output_request(args)?,
                )?)?)
            }
        }
        "run_filtered" => {
            let rtk_args = required_string_array(args, "rtk_args")?;
            let (cwd, timeout, max_output, tee_on_failure) = execution_options(args)?;
            Ok(serde_json::to_value(run_filtered_with_request(
                &rtk_args,
                cwd.as_deref(),
                timeout,
                max_output,
                tee_on_failure,
                output_request(args)?,
            )?)?)
        }
        "gain_summary" => {
            let project = args
                .get("project")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let limit = bounded_usize(args, "limit", 10, 1, 500)?;
            let tracker = Tracker::new().context("Failed to initialize tracking database")?;
            let project_path = crate::core::tracking::current_project_path_string();
            let mut summary = tracker.get_summary_filtered(if project {
                Some(project_path.as_str())
            } else {
                None
            })?;
            summary.by_command.truncate(limit);
            Ok(json!({
                "total_commands": summary.total_commands,
                "total_input": summary.total_input,
                "total_output": summary.total_output,
                "total_saved": summary.total_saved,
                "avg_savings_pct": summary.avg_savings_pct,
                "total_time_ms": summary.total_time_ms,
                "avg_time_ms": summary.avg_time_ms,
                "by_command": summary.by_command,
                "by_day": summary.by_day
            }))
        }
        "discover_unhandled" => {
            let command = discover_command(args)?;
            let result = run_filtered_with_request(
                &command,
                None,
                Duration::from_millis(DEFAULT_TIMEOUT_MS),
                DEFAULT_MAX_OUTPUT_BYTES,
                true,
                OutputRequest::agent(),
            )?;
            if result.exit_code != 0 {
                anyhow::bail!(
                    "discover execution failed with exit code {}: {}",
                    result.exit_code,
                    result.stderr
                );
            }
            let mut report: Value = serde_json::from_str(&result.stdout)
                .context("discover returned invalid structured JSON")?;
            if let Some(object) = report.as_object_mut() {
                object.insert(
                    "execution".to_string(),
                    json!({
                        "exit_code": result.exit_code,
                        "truncated": result.truncated,
                        "tee_path": result.tee_path
                    }),
                );
            }
            Ok(report)
        }
        "list_tee_artifacts" => {
            let limit = bounded_usize(args, "limit", DEFAULT_LIST_LIMIT, 1, 500)?;
            Ok(json!({ "artifacts": list_tee_artifacts(limit)? }))
        }
        "read_tee_file" => {
            let path = PathBuf::from(required_string(args, "path")?);
            let max_bytes = bounded_usize(
                args,
                "max_bytes",
                DEFAULT_MAX_OUTPUT_BYTES,
                1,
                10 * 1024 * 1024,
            )?;
            let content = read_tee_file(&path, max_bytes)?;
            Ok(json!({
                "path": redact_sensitive(&path.to_string_lossy()),
                "content": redact_sensitive(&content)
            }))
        }
        "read_recovery" => {
            let recovery_id = required_string(args, "recovery_id")?;
            let path = crate::core::tee::resolve_lossless_recovery(recovery_id)
                .ok_or_else(|| anyhow::anyhow!("recovery artifact not found: {recovery_id}"))?;
            let line_range = args
                .get("lines")
                .map(|value| {
                    value
                        .as_str()
                        .context("lines must use START:END syntax")
                        .and_then(crate::cmds::system::read::parse_line_range)
                })
                .transpose()?;
            let cursor = args
                .get("cursor")
                .map(|value| {
                    value
                        .as_u64()
                        .filter(|value| *value > 0)
                        .map(|value| value as usize)
                        .context("cursor must be a positive integer")
                })
                .transpose()?;
            let max_lines = bounded_usize(args, "max_lines", 100, 1, 500)?;
            Ok(read_recovery_page(
                recovery_id,
                &path,
                line_range,
                cursor,
                max_lines,
            )?)
        }
        "search_recovery" => {
            let recovery_id = required_string(args, "recovery_id")?;
            let pattern = required_string(args, "pattern")?;
            let regex = args.get("regex").and_then(Value::as_bool).unwrap_or(false);
            let context = bounded_usize(args, "context", 1, 0, 20)?;
            let max_matches = bounded_usize(args, "max_matches", 20, 1, 100)?;
            let path = crate::core::tee::resolve_lossless_recovery(recovery_id)
                .ok_or_else(|| anyhow::anyhow!("recovery artifact not found: {recovery_id}"))?;
            Ok(search_recovery_file(
                recovery_id,
                &path,
                pattern,
                regex,
                context,
                max_matches,
            )?)
        }
        _ => anyhow::bail!("Unknown tool: {name}"),
    }
}

fn discover_command(args: &Value) -> Result<Vec<String>> {
    let mut command = vec!["discover".to_string()];
    match args.get("project") {
        Some(Value::String(project)) if !project.trim().is_empty() => {
            command.extend(["--project".to_string(), project.to_string()]);
            if debug_enabled() {
                eprintln!("[rtk-debug] mcp.discover scope=project");
            }
        }
        Some(Value::Null) | None => {
            // A global stdio MCP server usually inherits the AI client's launch
            // directory, not the active project. Defaulting to all projects
            // keeps since_days meaningful and matches the tool's global role.
            command.push("--all".to_string());
            if debug_enabled() {
                eprintln!("[rtk-debug] mcp.discover scope=all-projects");
            }
        }
        Some(Value::String(_)) => anyhow::bail!("project must be a non-empty string"),
        Some(_) => anyhow::bail!("project must be a string"),
    }
    if let Some(days) = args.get("since_days") {
        let days = days.as_u64().context("since_days must be an integer")?;
        if !(1..=3650).contains(&days) {
            anyhow::bail!("since_days must be between 1 and 3650");
        }
        command.extend(["--since".to_string(), days.to_string()]);
    }
    let limit = bounded_usize(args, "limit", DEFAULT_LIST_LIMIT, 1, 500)?;
    command.extend([
        "--limit".to_string(),
        limit.to_string(),
        "--format".to_string(),
        "json".to_string(),
    ]);
    Ok(command)
}

fn list_tee_artifacts(limit: usize) -> Result<Vec<Value>> {
    let dir = tee_dir()?;
    list_tee_artifacts_in(&dir, limit)
}

fn list_tee_artifacts_in(dir: &Path, limit: usize) -> Result<Vec<Value>> {
    let read_dir = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if debug_enabled() {
                eprintln!(
                    "[rtk-debug] mcp.tee.list decision=empty-directory path={}",
                    redact_sensitive(&dir.to_string_lossy())
                );
            }
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read tee directory: {}", dir.display()));
        }
    };
    let mut entries = read_dir
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
    Ok(entries
        .into_iter()
        .take(limit)
        .map(|entry| {
            json!({
                "path": entry.path(),
                "size": entry.metadata().map(|metadata| metadata.len()).unwrap_or(0)
            })
        })
        .collect())
}

fn read_tee_file(path: &Path, max_bytes: usize) -> Result<String> {
    let dir = tee_dir()?
        .canonicalize()
        .context("Failed to canonicalize tee directory")?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Tee file does not exist: {}", path.display()))?;
    if !canonical.starts_with(&dir) || canonical.extension().is_none_or(|ext| ext != "log") {
        anyhow::bail!("Tee path is outside the trusted tee directory");
    }
    let bytes = fs::read(&canonical).context("Failed to read tee file")?;
    Ok(String::from_utf8_lossy(&bytes[..bytes.len().min(max_bytes)]).into_owned())
}

struct BoundedRecoveryLine {
    prefix: Vec<u8>,
    total_bytes: usize,
    truncated: bool,
    literal_match: bool,
}

/// Consume exactly one line while retaining only a bounded prefix. This keeps
/// a single pathological producer line from turning recovery navigation into
/// an unbounded allocation.
fn read_bounded_recovery_line<R: BufRead>(
    reader: &mut R,
    retain_limit: usize,
    literal_pattern: Option<&[u8]>,
) -> io::Result<Option<BoundedRecoveryLine>> {
    let mut prefix = Vec::with_capacity(retain_limit.min(8192));
    let mut total_bytes = 0usize;
    let mut literal_match = false;
    let mut search_tail = Vec::new();

    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            break;
        }
        let consumed = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |position| position + 1);
        let line_terminated = buffer[..consumed].last() == Some(&b'\n');
        let chunk = buffer[..consumed].to_vec();

        if let Some(pattern) = literal_pattern.filter(|pattern| !pattern.is_empty()) {
            let mut searchable = Vec::with_capacity(search_tail.len() + chunk.len());
            searchable.extend_from_slice(&search_tail);
            searchable.extend_from_slice(&chunk);
            if searchable
                .windows(pattern.len())
                .any(|window| window == pattern)
            {
                literal_match = true;
            }
            let tail_len = pattern.len().saturating_sub(1);
            search_tail = searchable
                .get(searchable.len().saturating_sub(tail_len)..)
                .unwrap_or_default()
                .to_vec();
        }

        let remaining = retain_limit.saturating_sub(prefix.len());
        prefix.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        total_bytes = total_bytes.saturating_add(chunk.len());
        reader.consume(consumed);

        if line_terminated {
            break;
        }
    }

    if total_bytes == 0 {
        return Ok(None);
    }
    Ok(Some(BoundedRecoveryLine {
        truncated: total_bytes > prefix.len(),
        prefix,
        total_bytes,
        literal_match,
    }))
}

fn render_bounded_recovery_line(line: &BoundedRecoveryLine, max_bytes: usize) -> Vec<u8> {
    if !line.truncated {
        return line.prefix[..line.prefix.len().min(max_bytes)].to_vec();
    }
    let marker = format!("\n[rtk: line truncated after {} bytes]\n", line.total_bytes);
    let keep = max_bytes.saturating_sub(marker.len());
    let mut rendered = line.prefix[..line.prefix.len().min(keep)].to_vec();
    while rendered
        .last()
        .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
    {
        rendered.pop();
    }
    rendered.extend_from_slice(marker.as_bytes());
    rendered.truncate(max_bytes);
    rendered
}

fn recovery_line_text(line: &BoundedRecoveryLine) -> String {
    let rendered = render_bounded_recovery_line(line, MAX_RECOVERY_LINE_BYTES);
    String::from_utf8_lossy(&rendered)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

fn recovery_line_original_text(line: &BoundedRecoveryLine) -> String {
    String::from_utf8_lossy(&line.prefix)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

fn read_recovery_page(
    recovery_id: &str,
    path: &Path,
    line_range: Option<crate::cmds::system::read::LineRange>,
    cursor: Option<usize>,
    max_lines: usize,
) -> Result<Value> {
    let (start, end) = match line_range {
        Some(range) => {
            if cursor.is_some_and(|value| value < range.start.saturating_sub(1)) {
                anyhow::bail!("cursor is before the requested line range");
            }
            (
                cursor.map_or(range.start, |line| line.saturating_add(1)),
                range.end,
            )
        }
        None => (cursor.map_or(1, |line| line.saturating_add(1)), usize::MAX),
    };
    if start == 0 {
        anyhow::bail!("recovery page start must be greater than zero");
    }

    let file = fs::File::open(path)
        .with_context(|| format!("Failed to open recovery artifact: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut line_number = 0usize;
    let mut first_line = None;
    let mut last_line = None;
    let mut output = Vec::new();
    let mut has_more = false;

    while let Some(line) = read_bounded_recovery_line(
        &mut reader,
        MAX_RECOVERY_PAGE_BYTES.saturating_sub(output.len()),
        None,
    )? {
        line_number = line_number.saturating_add(1);
        if line_number < start {
            continue;
        }
        if line_number > end {
            break;
        }
        if last_line.is_some_and(|_| output.len() >= MAX_RECOVERY_PAGE_BYTES) {
            has_more = true;
            break;
        }
        let remaining = MAX_RECOVERY_PAGE_BYTES.saturating_sub(output.len());
        let rendered = render_bounded_recovery_line(&line, remaining);
        first_line.get_or_insert(line_number);
        last_line = Some(line_number);
        output.extend_from_slice(&rendered);
        has_more |= line.truncated;
        if last_line
            .is_some_and(|last| last.saturating_sub(first_line.unwrap_or(last)) + 1 >= max_lines)
            && line_number < end
            && read_bounded_recovery_line(&mut reader, 0, None)?.is_some()
        {
            has_more = true;
        }
        if last_line
            .is_some_and(|last| last.saturating_sub(first_line.unwrap_or(last)) + 1 >= max_lines)
        {
            break;
        }
    }

    let Some(first_line) = first_line else {
        anyhow::bail!("recovery page is empty or starts after the end of the artifact");
    };
    let last_line = last_line.expect("first recovery line implies last line");
    let mut result = json!({
        "recovery_id": recovery_id,
        "start_line": first_line,
        "end_line": last_line,
        "content": redact_sensitive(&String::from_utf8_lossy(&output)),
        "has_more": has_more
    });
    if has_more {
        result["next_cursor"] = json!(last_line);
    }
    Ok(result)
}

struct PendingRecoveryMatch {
    line: usize,
    text: String,
    context: Vec<(usize, String)>,
    until: usize,
}

fn recovery_match_value(line: usize, text: String, context: Vec<(usize, String)>) -> Value {
    let mut texts = Vec::with_capacity(context.len().saturating_add(1));
    texts.push(text);
    texts.extend(context.iter().map(|(_, text)| text.clone()));
    let redacted = redact_sensitive_lines(&texts);
    let match_text = redacted.first().cloned().unwrap_or_default();
    let context = context
        .into_iter()
        .zip(redacted.into_iter().skip(1))
        .map(|((line, _), text)| json!({ "line": line, "text": text }))
        .collect::<Vec<_>>();
    json!({ "line": line, "text": match_text, "context": context })
}

fn search_recovery_file(
    recovery_id: &str,
    path: &Path,
    pattern: &str,
    use_regex: bool,
    context: usize,
    max_matches: usize,
) -> Result<Value> {
    let regex = use_regex
        .then(|| Regex::new(pattern))
        .transpose()
        .context("invalid recovery search regex")?;
    let file = fs::File::open(path)
        .with_context(|| format!("Failed to open recovery artifact: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut line_number = 0usize;
    let mut history = VecDeque::with_capacity(context.saturating_add(1));
    let mut pending: Vec<PendingRecoveryMatch> = Vec::new();
    let mut matches: Vec<Value> = Vec::new();
    let mut truncated = false;

    while let Some(line) = read_bounded_recovery_line(
        &mut reader,
        MAX_RECOVERY_LINE_BYTES,
        (!use_regex).then_some(pattern.as_bytes()),
    )? {
        line_number = line_number.saturating_add(1);
        let original_text = recovery_line_original_text(&line);
        let text = recovery_line_text(&line);
        truncated |= line.truncated;

        let mut remaining = Vec::with_capacity(pending.len());
        for mut found in pending {
            if line_number <= found.until && line_number > found.line {
                found.context.push((line_number, text.clone()));
            }
            if line_number >= found.until {
                matches.push(recovery_match_value(found.line, found.text, found.context));
            } else {
                remaining.push(found);
            }
        }
        pending = remaining;

        let is_match = regex.as_ref().map_or_else(
            || line.literal_match,
            |matcher| matcher.is_match(&original_text),
        );
        if is_match {
            if matches.len() + pending.len() >= max_matches {
                truncated = true;
                break;
            }
            let mut match_context = history
                .iter()
                .filter(|(line, _)| *line + context >= line_number)
                .cloned()
                .collect::<Vec<_>>();
            match_context.push((line_number, text.clone()));
            if context == 0 {
                matches.push(recovery_match_value(
                    line_number,
                    text.clone(),
                    match_context,
                ));
            } else {
                pending.push(PendingRecoveryMatch {
                    line: line_number,
                    text: text.clone(),
                    context: match_context,
                    until: line_number.saturating_add(context),
                });
            }
        }

        history.push_back((line_number, text));
        while history.len() > context.saturating_add(1) {
            history.pop_front();
        }
    }

    for found in pending {
        matches.push(recovery_match_value(found.line, found.text, found.context));
    }

    Ok(json!({
        "recovery_id": recovery_id,
        "pattern": redact_sensitive(pattern),
        "regex": use_regex,
        "match_count": matches.len(),
        "truncated": truncated,
        "matches": matches
    }))
}

fn tee_dir() -> Result<PathBuf> {
    let config = Config::load().context("Failed to load RTK configuration for tee access")?;
    let dir =
        crate::core::tee::get_tee_dir(&config).context("Unable to resolve RTK tee directory")?;
    if debug_enabled() {
        eprintln!(
            "[rtk-debug] mcp.tee.resolve decision=env-config-default path={}",
            redact_sensitive(&dir.to_string_lossy())
        );
    }
    Ok(dir)
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{key} must be a non-empty string"))
}

fn required_string_array(value: &Value, key: &str) -> Result<Vec<String>> {
    let array = value
        .get(key)
        .and_then(Value::as_array)
        .with_context(|| format!("{key} must be an array"))?;
    let mut result = Vec::with_capacity(array.len());
    for item in array {
        result.push(
            item.as_str()
                .with_context(|| format!("{key} entries must be strings"))?
                .to_string(),
        );
    }
    Ok(result)
}

fn execution_options(args: &Value) -> Result<(Option<PathBuf>, Duration, usize, bool)> {
    let cwd = args.get("cwd").and_then(Value::as_str).map(PathBuf::from);
    let timeout_ms = bounded_u64(args, "timeout_ms", DEFAULT_TIMEOUT_MS, 1, 600_000)?;
    let max_output = bounded_usize(
        args,
        "max_output_bytes",
        DEFAULT_MAX_OUTPUT_BYTES,
        1,
        10 * 1024 * 1024,
    )?;
    let tee_on_failure = args
        .get("tee_on_failure")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    Ok((
        cwd,
        Duration::from_millis(timeout_ms),
        max_output,
        tee_on_failure,
    ))
}

fn output_request(args: &Value) -> Result<OutputRequest> {
    let legacy = optional_token_limit(args, "max_tokens")?;
    let documented = optional_token_limit(args, "max_output_tokens")?;
    if legacy.is_some() && documented.is_some() && legacy != documented {
        anyhow::bail!("max_tokens and max_output_tokens must match when both are supplied");
    }
    let max_tokens = documented.or(legacy);
    if max_tokens.is_some_and(|value| {
        !(MIN_SEMANTIC_OUTPUT_TOKENS..=MAX_SEMANTIC_OUTPUT_TOKENS).contains(&value)
    }) {
        anyhow::bail!(
            "max_tokens must be between {MIN_SEMANTIC_OUTPUT_TOKENS} and {MAX_SEMANTIC_OUTPUT_TOKENS}"
        );
    }
    Ok(OutputRequest {
        audience: super::OutputAudience::Agent,
        max_tokens,
    })
}

fn optional_token_limit(args: &Value, key: &str) -> Result<Option<usize>> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let Some(value) = value.as_u64() else {
        anyhow::bail!("{key} must be an integer");
    };
    Ok(Some(value as usize))
}

fn bounded_u64(value: &Value, key: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    let number = value.get(key).and_then(Value::as_u64).unwrap_or(default);
    if !(min..=max).contains(&number) {
        anyhow::bail!("{key} must be between {min} and {max}");
    }
    Ok(number)
}

fn bounded_usize(
    value: &Value,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize> {
    let number = value
        .get(key)
        .and_then(Value::as_u64)
        .unwrap_or(default as u64) as usize;
    if !(min..=max).contains(&number) {
        anyhow::bail!("{key} must be between {min} and {max}");
    }
    Ok(number)
}

fn rpc_result(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Option<Value>, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_protocol_and_server_info() {
        let response = handle_request(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }))
        .expect("initialize response");
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(response["result"]["serverInfo"]["name"], "rtk");
        assert_eq!(
            response["result"]["instructions"],
            agent_policy::MCP_INSTRUCTIONS
        );
        assert!(response["result"]["instructions"]
            .as_str()
            .is_some_and(|instructions| instructions.contains("last-resort fallbacks")));
    }

    #[test]
    fn initialized_notification_has_no_response() {
        assert!(handle_request(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .is_none());
    }

    #[test]
    fn tools_list_contains_typed_execution_tool() {
        let response = handle_request(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }))
        .expect("tools/list response");
        let tools = response["result"]["tools"].as_array().expect("tools array");
        let run_filtered = tools
            .iter()
            .find(|tool| tool["name"] == "run_filtered")
            .expect("run_filtered tool");
        assert!(run_filtered["description"]
            .as_str()
            .is_some_and(|description| description.contains("Do not wrap supported commands")));
        assert_eq!(
            run_filtered["inputSchema"]["properties"]["max_tokens"]["minimum"],
            json!(64)
        );
        assert_eq!(
            run_filtered["inputSchema"]["properties"]["max_tokens"]["maximum"],
            json!(65_536)
        );
        let gain = tools
            .iter()
            .find(|tool| tool["name"] == "gain_summary")
            .expect("gain_summary tool");
        assert_eq!(
            gain["inputSchema"]["properties"]["limit"]["maximum"],
            json!(500)
        );
    }

    #[test]
    fn tools_list_exposes_first_class_cmd_execution_tool() {
        let response = handle_request(&json!({
            "jsonrpc": "2.0",
            "id": 22,
            "method": "tools/list"
        }))
        .expect("tools/list response");
        let tools = response["result"]["tools"].as_array().expect("tools array");
        let run_cmd = tools
            .iter()
            .find(|tool| tool["name"] == "run_cmd")
            .expect("run_cmd tool");
        assert!(run_cmd["description"]
            .as_str()
            .is_some_and(|description| description.contains("rtk cmd")));
        assert_eq!(run_cmd["inputSchema"]["required"], json!(["expression"]));
        assert_eq!(
            run_cmd["inputSchema"]["properties"]["expression"]["type"],
            json!("string")
        );
    }

    #[test]
    fn tools_list_exposes_first_class_powershell_execution_tool() {
        let response = handle_request(&json!({
            "jsonrpc": "2.0",
            "id": 23,
            "method": "tools/list"
        }))
        .expect("tools/list response");
        let tools = response["result"]["tools"].as_array().expect("tools array");
        let run_powershell = tools
            .iter()
            .find(|tool| tool["name"] == "run_powershell")
            .expect("run_powershell tool");
        assert_eq!(
            run_powershell["inputSchema"]["required"],
            json!(["host", "expression"])
        );
        assert_eq!(
            run_powershell["inputSchema"]["properties"]["host"]["enum"],
            json!(["powershell", "pwsh"])
        );
    }

    #[test]
    fn run_powershell_rejects_unknown_hosts() {
        let error = call_tool(
            "run_powershell",
            &json!({ "host": "cmd", "expression": "echo nope" }),
        )
        .expect_err("unknown host");
        assert!(error.to_string().contains("host must be"));
    }

    #[test]
    fn run_cmd_requires_a_raw_expression() {
        let error = call_tool("run_cmd", &json!({})).expect_err("missing expression");
        assert!(error
            .to_string()
            .contains("expression must be a non-empty string"));
    }

    #[cfg(not(windows))]
    #[test]
    fn run_cmd_reports_windows_only() {
        let error = call_tool("run_cmd", &json!({ "expression": "echo should not spawn" }))
            .expect_err("run_cmd must be Windows-only");
        assert!(error.to_string().contains("Windows"));
    }

    #[test]
    fn output_request_accepts_task_semantic_token_bounds() {
        assert!(output_request(&json!({ "max_tokens": 64 })).is_ok());
        assert!(output_request(&json!({ "max_tokens": 65_536 })).is_ok());
        assert!(output_request(&json!({ "max_tokens": 63 })).is_err());
        assert!(output_request(&json!({ "max_tokens": 65_537 })).is_err());
    }

    #[test]
    fn malformed_tool_arguments_return_invalid_params() {
        let response = handle_request(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "run_filtered",
                "arguments": { "rtk_args": ["mcp"] }
            }
        }))
        .expect("tools/call response");
        assert_eq!(response["error"]["code"], -32602);
        assert!(response["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("not supported")));
    }

    #[test]
    fn rewrite_tool_returns_structured_match() {
        let value = call_tool("rewrite_command", &json!({ "command": "git status" }))
            .expect("rewrite tool");
        assert!(value.get("matched").is_some());
    }

    #[test]
    fn missing_tee_directory_is_an_empty_artifact_list() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("not-created");
        assert_eq!(
            list_tee_artifacts_in(&missing, DEFAULT_LIST_LIMIT).expect("empty list"),
            Vec::<Value>::new()
        );
    }

    #[test]
    fn discover_defaults_to_all_projects_and_honors_bounds() {
        let command = discover_command(&json!({ "since_days": 3, "limit": 7 })).expect("command");
        assert_eq!(
            command,
            ["discover", "--all", "--since", "3", "--limit", "7", "--format", "json"]
        );
    }

    #[test]
    fn discover_project_scope_does_not_add_all() {
        let command = discover_command(&json!({ "project": "D:-work-project" })).expect("command");
        assert!(command
            .windows(2)
            .any(|pair| pair == ["--project", "D:-work-project"]));
        assert!(!command.iter().any(|arg| arg == "--all"));
    }
}
