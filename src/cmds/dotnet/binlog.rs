//! Reads MSBuild binary log files and extracts errors and test results.

use crate::core::utils::strip_ansi;
use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinlogIssue {
    pub code: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct BuildSummary {
    pub succeeded: bool,
    pub project_count: usize,
    pub errors: Vec<BinlogIssue>,
    pub warnings: Vec<BinlogIssue>,
    pub duration_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedTest {
    pub name: String,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TestSummary {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub total: usize,
    pub project_count: usize,
    pub failed_tests: Vec<FailedTest>,
    pub duration_text: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RestoreSummary {
    pub restored_projects: usize,
    pub warnings: usize,
    pub errors: usize,
    pub duration_text: Option<String>,
}

lazy_static! {
    static ref ISSUE_RE: Regex = Regex::new(
        r"(?m)^\s*(?P<file>[^\r\n:(]+)\((?P<line>\d+),(?P<column>\d+)\):\s*(?P<kind>error|warning)\s*(?:(?P<code>[A-Za-z]+\d+)\s*:\s*)?(?P<msg>.*)$"
    )
    .expect("valid regex");
    static ref BUILD_SUMMARY_RE: Regex = Regex::new(r"(?mi)^\s*(?P<count>\d+)\s+(?P<kind>warning|error)\(s\)")
        .expect("valid regex");
    static ref ERROR_COUNT_RE: Regex =
        Regex::new(r"(?i)\b(?P<count>\d+)\s+error\(s\)").expect("valid regex");
    static ref WARNING_COUNT_RE: Regex =
        Regex::new(r"(?i)\b(?P<count>\d+)\s+warning\(s\)").expect("valid regex");
    static ref FALLBACK_ERROR_LINE_RE: Regex =
        Regex::new(r"(?mi)^.+\(\d+,\d+\):\s*error(?:\s+[A-Za-z]{2,}\d{3,})?(?:\s*:.*)?$")
            .expect("valid regex");
    static ref FALLBACK_WARNING_LINE_RE: Regex =
        Regex::new(r"(?mi)^.+\(\d+,\d+\):\s*warning(?:\s+[A-Za-z]{2,}\d{3,})?(?:\s*:.*)?$")
            .expect("valid regex");
    static ref DURATION_RE: Regex =
        Regex::new(r"(?m)^\s*Time Elapsed\s+(?P<duration>[^\r\n]+)$").expect("valid regex");
    static ref TEST_RESULT_RE: Regex = Regex::new(
        r"(?m)(?:Passed!|Failed!)\s*-\s*Failed:\s*(?P<failed>\d+),\s*Passed:\s*(?P<passed>\d+),\s*Skipped:\s*(?P<skipped>\d+),\s*Total:\s*(?P<total>\d+),\s*Duration:\s*(?P<duration>[^\r\n-]+)"
    )
    .expect("valid regex");
    static ref TEST_SUMMARY_RE: Regex = Regex::new(
        r"(?mi)^\s*Test summary:\s*total:\s*(?P<total>\d+),\s*failed:\s*(?P<failed>\d+),\s*(?:succeeded|passed):\s*(?P<passed>\d+),\s*skipped:\s*(?P<skipped>\d+),\s*duration:\s*(?P<duration>[^\r\n]+)$"
    )
    .expect("valid regex");
    static ref FAILED_TEST_HEAD_RE: Regex = Regex::new(
        r"(?m)^\s*Failed\s+(?P<name>[^\r\n\[]+)\s+\[[^\]\r\n]+\]\s*$"
    )
    .expect("valid regex");
    static ref RESTORE_PROJECT_RE: Regex =
        Regex::new(r"(?m)^\s*Restored\s+.+\.csproj\s*\(").expect("valid regex");
    static ref RESTORE_DIAGNOSTIC_RE: Regex = Regex::new(
        r"(?mi)^\s*(?:(?P<file>.+?)\s+:\s+)?(?P<kind>warning|error)\s+(?P<code>[A-Za-z]{2,}\d{3,})\s*:\s*(?P<msg>.+)$"
    )
    .expect("valid regex");
    static ref PROJECT_PATH_RE: Regex =
        Regex::new(r"(?m)^\s*([A-Za-z]:)?[^\r\n]*\.csproj(?:\s|$)").expect("valid regex");
    static ref PRINTABLE_RUN_RE: Regex = Regex::new(r"[\x20-\x7E]{5,}").expect("valid regex");
    static ref DIAGNOSTIC_CODE_RE: Regex =
        Regex::new(r"^[A-Za-z]{2,}\d{3,}$").expect("valid regex");
    static ref SOURCE_FILE_RE: Regex = Regex::new(r"(?i)([A-Za-z]:)?[/\\][^\s]+\.(cs|vb|fs)")
        .expect("valid regex");
    static ref SENSITIVE_ENV_RE: Regex = {
        let keys = SENSITIVE_ENV_VARS
            .iter()
            .map(|key| regex::escape(key))
            .collect::<Vec<_>>()
            .join("|");
        Regex::new(&format!(
            r"(?P<prefix>\b(?:{})\s*(?:=|:)\s*)(?P<value>[^\s;]+)",
            keys
        ))
        .expect("valid regex")
    };
}

const SENSITIVE_ENV_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "USERNAME",
    "USER",
    "APPDATA",
    "LOCALAPPDATA",
    "TEMP",
    "TMP",
    "SSH_AUTH_SOCK",
    "SSH_AGENT_LAUNCHER",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GITHUB_PAT",
    "NUGET_API_KEY",
    "NUGET_AUTH_TOKEN",
    "VSS_NUGET_EXTERNAL_FEED_ENDPOINTS",
    "AZURE_DEVOPS_TOKEN",
    "AZURE_CLIENT_SECRET",
    "AZURE_TENANT_ID",
    "AZURE_CLIENT_ID",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "API_TOKEN",
    "AUTH_TOKEN",
    "ACCESS_TOKEN",
    "BEARER_TOKEN",
    "PASSWORD",
    "CONNECTION_STRING",
    "DATABASE_URL",
    "DOCKER_CONFIG",
    "KUBECONFIG",
];

const RECORD_END_OF_FILE: i32 = 0;
const RECORD_BUILD_STARTED: i32 = 1;
const RECORD_BUILD_FINISHED: i32 = 2;
const RECORD_PROJECT_STARTED: i32 = 3;
const RECORD_PROJECT_FINISHED: i32 = 4;
const RECORD_ERROR: i32 = 9;
const RECORD_WARNING: i32 = 10;
const RECORD_MESSAGE: i32 = 11;
const RECORD_CRITICAL_BUILD_MESSAGE: i32 = 13;
const RECORD_PROJECT_IMPORT_ARCHIVE: i32 = 17;
const RECORD_NAME_VALUE_LIST: i32 = 23;
const RECORD_STRING: i32 = 24;

const FLAG_BUILD_EVENT_CONTEXT: i32 = 1 << 0;
const FLAG_MESSAGE: i32 = 1 << 2;
const FLAG_TIMESTAMP: i32 = 1 << 5;
const FLAG_ARGUMENTS: i32 = 1 << 14;
const FLAG_IMPORTANCE: i32 = 1 << 15;
const FLAG_EXTENDED: i32 = 1 << 16;

const STRING_RECORD_START_INDEX: i32 = 10;

pub fn parse_build(binlog_path: &Path) -> Result<BuildSummary> {
    let parsed = parse_events_from_binlog(binlog_path)
        .with_context(|| format!("Failed to parse binlog at {}", binlog_path.display()))?;
    let strings_blob = parsed.string_records.join("\n");
    let text_fallback = parse_build_from_text(&strings_blob);

    let duration_text = match (parsed.build_started_ticks, parsed.build_finished_ticks) {
        (Some(start), Some(end)) if end >= start => Some(format_ticks_duration(end - start)),
        _ => None,
    };

    let parsed_project_count = parsed.project_files.len();

    Ok(BuildSummary {
        succeeded: parsed.build_succeeded.unwrap_or(false),
        project_count: if parsed_project_count > 0 {
            parsed_project_count
        } else {
            text_fallback.project_count
        },
        errors: select_best_issues(parsed.errors, text_fallback.errors),
        warnings: select_best_issues(parsed.warnings, text_fallback.warnings),
        duration_text,
    })
}

fn select_best_issues(primary: Vec<BinlogIssue>, fallback: Vec<BinlogIssue>) -> Vec<BinlogIssue> {
    if primary.is_empty() {
        return fallback;
    }
    if fallback.is_empty() {
        return primary;
    }
    if primary.iter().all(is_suspicious_issue) && fallback.iter().any(is_contextual_issue) {
        return fallback;
    }
    if issues_quality_score(&fallback) > issues_quality_score(&primary) {
        fallback
    } else {
        primary
    }
}

fn issues_quality_score(issues: &[BinlogIssue]) -> usize {
    issues.iter().map(issue_quality_score).sum()
}

fn issue_quality_score(issue: &BinlogIssue) -> usize {
    let mut score = 0;
    if is_contextual_issue(issue) {
        score += 4;
    }
    if !issue.code.is_empty() && is_likely_diagnostic_code(&issue.code) {
        score += 2;
    }
    if issue.line > 0 {
        score += 1;
    }
    if issue.column > 0 {
        score += 1;
    }
    if !issue.message.is_empty() && issue.message != "Build issue" {
        score += 1;
    }
    score
}

fn is_contextual_issue(issue: &BinlogIssue) -> bool {
    !issue.file.is_empty() && !is_likely_diagnostic_code(&issue.file)
}

fn is_suspicious_issue(issue: &BinlogIssue) -> bool {
    issue.code.is_empty() && is_likely_diagnostic_code(&issue.file)
}

pub fn parse_test(binlog_path: &Path) -> Result<TestSummary> {
    let parsed = parse_events_from_binlog(binlog_path)
        .with_context(|| format!("Failed to parse binlog at {}", binlog_path.display()))?;
    let blob = parsed.string_records.join("\n");
    let mut summary = parse_test_from_text(&blob);
    let parsed_project_count = parsed.project_files.len();
    if parsed_project_count > 0 {
        summary.project_count = parsed_project_count;
    }
    Ok(summary)
}

pub fn parse_restore(binlog_path: &Path) -> Result<RestoreSummary> {
    let parsed = parse_events_from_binlog(binlog_path)
        .with_context(|| format!("Failed to parse binlog at {}", binlog_path.display()))?;
    let blob = parsed.string_records.join("\n");
    let mut summary = parse_restore_from_text(&blob);
    let parsed_project_count = parsed.project_files.len();
    if parsed_project_count > 0 {
        summary.restored_projects = parsed_project_count;
    }
    Ok(summary)
}

#[derive(Default)]
struct ParsedBinlog {
    string_records: Vec<String>,
    messages: Vec<String>,
    project_files: HashSet<String>,
    errors: Vec<BinlogIssue>,
    warnings: Vec<BinlogIssue>,
    build_succeeded: Option<bool>,
    build_started_ticks: Option<i64>,
    build_finished_ticks: Option<i64>,
}

#[derive(Default)]
struct ParsedEventFields {
    message: Option<String>,
    timestamp_ticks: Option<i64>,
}

fn parse_events_from_binlog(path: &Path) -> Result<ParsedBinlog> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed to read binlog at {}", path.display()))?;
    if bytes.is_empty() {
        anyhow::bail!("Failed to parse binlog at {}: empty file", path.display());
    }

    let mut decoder = GzDecoder::new(bytes.as_slice());
    let mut payload = Vec::new();
    decoder.read_to_end(&mut payload).with_context(|| {
        format!(
            "Failed to parse binlog at {}: gzip decode failed",
            path.display()
        )
    })?;

    let mut reader = BinReader::new(&payload);
    let file_format_version = reader
        .read_i32_le()
        .context("binlog header missing file format version")?;
    let _minimum_reader_version = reader
        .read_i32_le()
        .context("binlog header missing minimum reader version")?;

    if file_format_version < 18 {
        anyhow::bail!(
            "Failed to parse binlog at {}: unsupported binlog format {}",
            path.display(),
            file_format_version
        );
    }

    let mut parsed = ParsedBinlog::default();

    while !reader.is_eof() {
        let kind = reader
            .read_7bit_i32()
            .context("failed to read record kind")?;
        if kind == RECORD_END_OF_FILE {
            break;
        }

        match kind {
            RECORD_STRING => {
                let text = reader
                    .read_dotnet_string()
                    .context("failed to read string record")?;
                parsed.string_records.push(text);
            }
            RECORD_NAME_VALUE_LIST | RECORD_PROJECT_IMPORT_ARCHIVE => {
                let len = reader
                    .read_7bit_i32()
                    .context("failed to read record length")?;
                if len < 0 {
                    anyhow::bail!("negative record length: {}", len);
                }
                reader
                    .skip(len as usize)
                    .context("failed to skip auxiliary record payload")?;
            }
            _ => {
                let len = reader
                    .read_7bit_i32()
                    .context("failed to read event length")?;
                if len < 0 {
                    anyhow::bail!("negative event length: {}", len);
                }

                let payload = reader
                    .read_exact(len as usize)
                    .context("failed to read event payload")?;
                let mut event_reader = BinReader::new(payload);
                let _ =
                    parse_event_record(kind, &mut event_reader, file_format_version, &mut parsed);
            }
        }
    }

    Ok(parsed)
}

fn parse_event_record(
    kind: i32,
    reader: &mut BinReader<'_>,
    file_format_version: i32,
    parsed: &mut ParsedBinlog,
) -> Result<()> {
    match kind {
        RECORD_BUILD_STARTED => {
            let fields = read_event_fields(reader, file_format_version, parsed, false)?;
            parsed.build_started_ticks = fields.timestamp_ticks;
        }
        RECORD_BUILD_FINISHED => {
            let fields = read_event_fields(reader, file_format_version, parsed, false)?;
            parsed.build_finished_ticks = fields.timestamp_ticks;
            parsed.build_succeeded = Some(reader.read_bool()?);
        }
        RECORD_PROJECT_STARTED => {
            let _fields = read_event_fields(reader, file_format_version, parsed, false)?;
            if reader.read_bool()? {
                skip_build_event_context(reader, file_format_version)?;
            }
            if let Some(project_file) = read_optional_string(reader, parsed)? {
                if !project_file.is_empty() {
                    parsed.project_files.insert(project_file);
                }
            }
        }
        RECORD_PROJECT_FINISHED => {
            let _fields = read_event_fields(reader, file_format_version, parsed, false)?;
            if let Some(project_file) = read_optional_string(reader, parsed)? {
                if !project_file.is_empty() {
                    parsed.project_files.insert(project_file);
                }
            }
            let _ = reader.read_bool()?;
        }
        RECORD_ERROR | RECORD_WARNING => {
            let fields = read_event_fields(reader, file_format_version, parsed, false)?;

            let _subcategory = read_optional_string(reader, parsed)?;
            let code = read_optional_string(reader, parsed)?.unwrap_or_default();
            let file = read_optional_string(reader, parsed)?.unwrap_or_default();
            let _project_file = read_optional_string(reader, parsed)?;
            let line = reader.read_7bit_i32()?.max(0) as u32;
            let column = reader.read_7bit_i32()?.max(0) as u32;
            let _ = reader.read_7bit_i32()?;
            let _ = reader.read_7bit_i32()?;

            let issue = BinlogIssue {
                code,
                file,
                line,
                column,
                message: fields.message.unwrap_or_default(),
            };

            if kind == RECORD_ERROR {
                parsed.errors.push(issue);
            } else {
                parsed.warnings.push(issue);
            }
        }
        RECORD_MESSAGE => {
            let fields = read_event_fields(reader, file_format_version, parsed, true)?;
            if let Some(message) = fields.message {
                parsed.messages.push(message);
            }
        }
        RECORD_CRITICAL_BUILD_MESSAGE => {
            let fields = read_event_fields(reader, file_format_version, parsed, false)?;
            if let Some(message) = fields.message {
                parsed.messages.push(message);
            }
        }
        _ => {}
    }

    Ok(())
}

fn read_event_fields(
    reader: &mut BinReader<'_>,
    file_format_version: i32,
    parsed: &ParsedBinlog,
    read_importance: bool,
) -> Result<ParsedEventFields> {
    let flags = reader.read_7bit_i32()?;
    let mut result = ParsedEventFields::default();

    if flags & FLAG_MESSAGE != 0 {
        result.message = read_deduplicated_string(reader, parsed)?;
    }

    if flags & FLAG_BUILD_EVENT_CONTEXT != 0 {
        skip_build_event_context(reader, file_format_version)?;
    }

    if flags & FLAG_TIMESTAMP != 0 {
        result.timestamp_ticks = Some(reader.read_i64_le()?);
        let _ = reader.read_7bit_i32()?;
    }

    if flags & FLAG_EXTENDED != 0 {
        let _ = read_optional_string(reader, parsed)?;
        skip_string_dictionary(reader, file_format_version)?;
        let _ = read_optional_string(reader, parsed)?;
    }

    if flags & FLAG_ARGUMENTS != 0 {
        let count = reader.read_7bit_i32()?.max(0) as usize;
        for _ in 0..count {
            let _ = read_deduplicated_string(reader, parsed)?;
        }
    }

    if (file_format_version < 13 && read_importance) || (flags & FLAG_IMPORTANCE != 0) {
        let _ = reader.read_7bit_i32()?;
    }

    Ok(result)
}

fn skip_build_event_context(reader: &mut BinReader<'_>, file_format_version: i32) -> Result<()> {
    let count = if file_format_version > 1 { 7 } else { 6 };
    for _ in 0..count {
        let _ = reader.read_7bit_i32()?;
    }
    Ok(())
}

fn skip_string_dictionary(reader: &mut BinReader<'_>, file_format_version: i32) -> Result<()> {
    if file_format_version < 10 {
        anyhow::bail!("legacy dictionary format is unsupported");
    }

    let _ = reader.read_7bit_i32()?;
    Ok(())
}

fn read_optional_string(
    reader: &mut BinReader<'_>,
    parsed: &ParsedBinlog,
) -> Result<Option<String>> {
    read_deduplicated_string(reader, parsed)
}

fn read_deduplicated_string(
    reader: &mut BinReader<'_>,
    parsed: &ParsedBinlog,
) -> Result<Option<String>> {
    let index = reader.read_7bit_i32()?;
    if index == 0 {
        return Ok(None);
    }
    if index == 1 {
        return Ok(Some(String::new()));
    }
    if index < STRING_RECORD_START_INDEX {
        return Ok(None);
    }
    let record_idx = (index - STRING_RECORD_START_INDEX) as usize;
    parsed
        .string_records
        .get(record_idx)
        .cloned()
        .map(Some)
        .with_context(|| format!("invalid string record index {}", index))
}

fn format_ticks_duration(ticks: i64) -> String {
    let total_seconds = ticks.div_euclid(10_000_000);
    let centiseconds = ticks.rem_euclid(10_000_000) / 100_000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!(
        "{:02}:{:02}:{:02}.{:02}",
        hours, minutes, seconds, centiseconds
    )
}

struct BinReader<'a> {
    cursor: Cursor<&'a [u8]>,
}

impl<'a> BinReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            cursor: Cursor::new(bytes),
        }
    }

    fn is_eof(&self) -> bool {
        (self.cursor.position() as usize) >= self.cursor.get_ref().len()
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let start = self.cursor.position() as usize;
        let end = start.saturating_add(len);
        if end > self.cursor.get_ref().len() {
            anyhow::bail!("unexpected end of stream");
        }
        self.cursor.set_position(end as u64);
        Ok(&self.cursor.get_ref()[start..end])
    }

    fn skip(&mut self, len: usize) -> Result<()> {
        let _ = self.read_exact(len)?;
        Ok(())
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_bool(&mut self) -> Result<bool> {
        Ok(self.read_u8()? != 0)
    }

    fn read_i32_le(&mut self) -> Result<i32> {
        let b = self.read_exact(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i64_le(&mut self) -> Result<i64> {
        let b = self.read_exact(8)?;
        Ok(i64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_7bit_i32(&mut self) -> Result<i32> {
        let mut value: u32 = 0;
        let mut shift = 0;
        loop {
            let byte = self.read_u8()?;
            value |= ((byte & 0x7F) as u32) << shift;
            if (byte & 0x80) == 0 {
                return Ok(value as i32);
            }

            shift += 7;
            if shift >= 35 {
                anyhow::bail!("invalid 7-bit encoded integer");
            }
        }
    }

    fn read_dotnet_string(&mut self) -> Result<String> {
        let len = self.read_7bit_i32()?;
        if len < 0 {
            anyhow::bail!("negative string length: {}", len);
        }
        let bytes = self.read_exact(len as usize)?;
        String::from_utf8(bytes.to_vec()).context("invalid UTF-8 string")
    }
}

pub fn scrub_sensitive_env_vars(input: &str) -> String {
    SENSITIVE_ENV_RE
        .replace_all(input, "${prefix}[REDACTED]")
        .into_owned()
}

pub fn parse_build_from_text(text: &str) -> BuildSummary {
    let text = text.replace("\r\n", "\n");
    let clean = strip_ansi(&text);
    let scrubbed = scrub_sensitive_env_vars(&clean);
    let mut seen_errors: HashSet<(String, String, u32, u32, String)> = HashSet::new();
    let mut seen_warnings: HashSet<(String, String, u32, u32, String)> = HashSet::new();
    let mut summary = BuildSummary {
        succeeded: scrubbed.contains("Build succeeded") && !scrubbed.contains("Build FAILED"),
        project_count: count_projects(&scrubbed),
        errors: Vec::new(),
        warnings: Vec::new(),
        duration_text: extract_duration(&scrubbed),
    };

    for captures in ISSUE_RE.captures_iter(&scrubbed) {
        let issue = BinlogIssue {
            code: captures
                .name("code")
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            file: captures
                .name("file")
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            line: captures
                .name("line")
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .unwrap_or(0),
            column: captures
                .name("column")
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .unwrap_or(0),
            message: captures
                .name("msg")
                .map(|m| {
                    let msg = m.as_str().trim();
                    if msg.is_empty() {
                        "diagnostic without message".to_string()
                    } else {
                        msg.to_string()
                    }
                })
                .unwrap_or_default(),
        };

        let key = (
            issue.code.clone(),
            issue.file.clone(),
            issue.line,
            issue.column,
            issue.message.clone(),
        );

        match captures.name("kind").map(|m| m.as_str()) {
            Some("error") => {
                if seen_errors.insert(key) {
                    summary.errors.push(issue);
                }
            }
            Some("warning") => {
                if seen_warnings.insert(key) {
                    summary.warnings.push(issue);
                }
            }
            _ => {}
        }
    }

    if summary.errors.is_empty() || summary.warnings.is_empty() {
        let mut warning_count_from_summary = 0;
        let mut error_count_from_summary = 0;

        for captures in BUILD_SUMMARY_RE.captures_iter(&scrubbed) {
            let count = captures
                .name("count")
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(0);

            match captures
                .name("kind")
                .map(|m| m.as_str().to_ascii_lowercase())
                .as_deref()
            {
                Some("warning") => {
                    warning_count_from_summary = warning_count_from_summary.max(count)
                }
                Some("error") => error_count_from_summary = error_count_from_summary.max(count),
                _ => {}
            }
        }

        let inline_error_count = ERROR_COUNT_RE
            .captures_iter(&scrubbed)
            .filter_map(|captures| {
                captures
                    .name("count")
                    .and_then(|m| m.as_str().parse::<usize>().ok())
            })
            .max()
            .unwrap_or(0);
        let inline_warning_count = WARNING_COUNT_RE
            .captures_iter(&scrubbed)
            .filter_map(|captures| {
                captures
                    .name("count")
                    .and_then(|m| m.as_str().parse::<usize>().ok())
            })
            .max()
            .unwrap_or(0);

        warning_count_from_summary = warning_count_from_summary.max(inline_warning_count);
        error_count_from_summary = error_count_from_summary.max(inline_error_count);

        if summary.errors.is_empty() {
            for idx in 0..error_count_from_summary {
                summary.errors.push(BinlogIssue {
                    code: String::new(),
                    file: String::new(),
                    line: 0,
                    column: 0,
                    message: format!("Build error #{} (details omitted)", idx + 1),
                });
            }
        }

        if summary.warnings.is_empty() {
            for idx in 0..warning_count_from_summary {
                summary.warnings.push(BinlogIssue {
                    code: String::new(),
                    file: String::new(),
                    line: 0,
                    column: 0,
                    message: format!("Build warning #{} (details omitted)", idx + 1),
                });
            }
        }

        if summary.errors.is_empty() {
            let fallback_error_lines = FALLBACK_ERROR_LINE_RE.captures_iter(&scrubbed).count();
            for idx in 0..fallback_error_lines {
                summary.errors.push(BinlogIssue {
                    code: String::new(),
                    file: String::new(),
                    line: 0,
                    column: 0,
                    message: format!("Build error #{} (details omitted)", idx + 1),
                });
            }
        }

        if summary.warnings.is_empty() {
            let fallback_warning_lines = FALLBACK_WARNING_LINE_RE.captures_iter(&scrubbed).count();
            for idx in 0..fallback_warning_lines {
                summary.warnings.push(BinlogIssue {
                    code: String::new(),
                    file: String::new(),
                    line: 0,
                    column: 0,
                    message: format!("Build warning #{} (details omitted)", idx + 1),
                });
            }
        }
    }

    let has_error_signal = scrubbed.contains("Build FAILED")
        || scrubbed.contains(": error ")
        || BUILD_SUMMARY_RE.captures_iter(&scrubbed).any(|captures| {
            let is_error = matches!(
                captures
                    .name("kind")
                    .map(|m| m.as_str().to_ascii_lowercase())
                    .as_deref(),
                Some("error")
            );
            let count = captures
                .name("count")
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(0);
            is_error && count > 0
        });

    if summary.errors.is_empty() || summary.warnings.is_empty() {
        let (diagnostic_errors, diagnostic_warnings) = parse_restore_issues_from_text(&scrubbed);

        if summary.errors.is_empty() {
            summary.errors = diagnostic_errors;
        }

        if summary.warnings.is_empty() {
            summary.warnings = diagnostic_warnings;
        }
    }

    if summary.errors.is_empty() && !summary.succeeded && has_error_signal {
        summary.errors = extract_binary_like_issues(&scrubbed);
    }

    if summary.project_count == 0
        && (scrubbed.contains("Build succeeded")
            || scrubbed.contains("Build FAILED")
            || scrubbed.contains(" -> "))
    {
        summary.project_count = 1;
    }

    summary
}

pub fn parse_test_from_text(text: &str) -> TestSummary {
    let text = text.replace("\r\n", "\n");
    let clean = strip_ansi(&text);
    let scrubbed = scrub_sensitive_env_vars(&clean);
    let mut summary = TestSummary {
        passed: 0,
        failed: 0,
        skipped: 0,
        total: 0,
        project_count: count_projects(&scrubbed).max(1),
        failed_tests: Vec::new(),
        duration_text: extract_duration(&scrubbed),
    };

    let mut found_summary_line = false;
    let mut fallback_duration = None;
    for captures in TEST_RESULT_RE.captures_iter(&scrubbed) {
        found_summary_line = true;
        summary.passed += captures
            .name("passed")
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .unwrap_or(0);
        summary.failed += captures
            .name("failed")
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .unwrap_or(0);
        summary.skipped += captures
            .name("skipped")
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .unwrap_or(0);
        summary.total += captures
            .name("total")
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .unwrap_or(0);

        if let Some(duration) = captures.name("duration") {
            fallback_duration = Some(duration.as_str().trim().to_string());
        }
    }

    if found_summary_line && summary.duration_text.is_none() {
        summary.duration_text = fallback_duration;
    }

    if let Some(captures) = TEST_SUMMARY_RE.captures_iter(&scrubbed).last() {
        summary.passed = captures
            .name("passed")
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .unwrap_or(summary.passed);
        summary.failed = captures
            .name("failed")
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .unwrap_or(summary.failed);
        summary.skipped = captures
            .name("skipped")
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .unwrap_or(summary.skipped);
        summary.total = captures
            .name("total")
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .unwrap_or(summary.total);

        if let Some(duration) = captures.name("duration") {
            summary.duration_text = Some(duration.as_str().trim().to_string());
        }
    }

    let lines: Vec<&str> = scrubbed.lines().collect();
    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx];
        if let Some(captures) = FAILED_TEST_HEAD_RE.captures(line) {
            let name = captures
                .name("name")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let mut details = Vec::new();
            idx += 1;
            while idx < lines.len() {
                let detail_line = lines[idx].trim_end();
                if FAILED_TEST_HEAD_RE.is_match(detail_line) {
                    idx = idx.saturating_sub(1);
                    break;
                }
                let detail_trimmed = detail_line.trim_start();
                if detail_trimmed.starts_with("Failed!  -")
                    || detail_trimmed.starts_with("Passed!  -")
                    || detail_trimmed.starts_with("Test summary:")
                    || detail_trimmed.starts_with("Build ")
                {
                    idx = idx.saturating_sub(1);
                    break;
                }

                if detail_line.trim().is_empty() {
                    if !details.is_empty() {
                        details.push(String::new());
                    }
                } else {
                    details.push(detail_line.trim().to_string());
                }
                if details.len() >= 20 {
                    break;
                }
                idx += 1;
            }
            summary.failed_tests.push(FailedTest { name, details });
        }
        idx += 1;
    }

    if summary.failed == 0 {
        summary.failed = summary.failed_tests.len();
    }
    if summary.total == 0 {
        summary.total = summary.passed + summary.failed + summary.skipped;
    }

    summary
}

pub fn parse_restore_from_text(text: &str) -> RestoreSummary {
    let text = text.replace("\r\n", "\n");
    let (errors, warnings) = parse_restore_issues_from_text(&text);
    let clean = strip_ansi(&text);
    let scrubbed = scrub_sensitive_env_vars(&clean);

    RestoreSummary {
        restored_projects: RESTORE_PROJECT_RE.captures_iter(&scrubbed).count(),
        warnings: warnings.len(),
        errors: errors.len(),
        duration_text: extract_duration(&scrubbed),
    }
}

pub fn parse_restore_issues_from_text(text: &str) -> (Vec<BinlogIssue>, Vec<BinlogIssue>) {
    let text = text.replace("\r\n", "\n");
    let clean = strip_ansi(&text);
    let scrubbed = scrub_sensitive_env_vars(&clean);
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut seen_errors: HashSet<(String, String, u32, u32, String)> = HashSet::new();
    let mut seen_warnings: HashSet<(String, String, u32, u32, String)> = HashSet::new();

    for captures in RESTORE_DIAGNOSTIC_RE.captures_iter(&scrubbed) {
        let issue = BinlogIssue {
            code: captures
                .name("code")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default(),
            file: captures
                .name("file")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default(),
            line: 0,
            column: 0,
            message: captures
                .name("msg")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default(),
        };

        let key = (
            issue.code.clone(),
            issue.file.clone(),
            issue.line,
            issue.column,
            issue.message.clone(),
        );

        match captures
            .name("kind")
            .map(|m| m.as_str().to_ascii_lowercase())
        {
            Some(kind) if kind == "error" => {
                if seen_errors.insert(key) {
                    errors.push(issue);
                }
            }
            Some(kind) if kind == "warning" => {
                if seen_warnings.insert(key) {
                    warnings.push(issue);
                }
            }
            _ => {}
        }
    }

    (errors, warnings)
}

fn count_projects(text: &str) -> usize {
    PROJECT_PATH_RE.captures_iter(text).count()
}

fn extract_duration(text: &str) -> Option<String> {
    DURATION_RE
        .captures(text)
        .and_then(|c| c.name("duration"))
        .map(|m| m.as_str().trim().to_string())
}

fn extract_printable_runs(text: &str) -> Vec<String> {
    let mut runs = Vec::new();
    for captures in PRINTABLE_RUN_RE.captures_iter(text) {
        let Some(matched) = captures.get(0) else {
            continue;
        };

        let run = matched.as_str().trim();
        if run.len() < 5 {
            continue;
        }
        runs.push(run.to_string());
    }
    runs
}

fn extract_binary_like_issues(text: &str) -> Vec<BinlogIssue> {
    let runs = extract_printable_runs(text);
    if runs.is_empty() {
        return Vec::new();
    }

    let mut issues = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    for idx in 0..runs.len() {
        let code = runs[idx].trim();
        if !DIAGNOSTIC_CODE_RE.is_match(code) || !is_likely_diagnostic_code(code) {
            continue;
        }

        let message = (1..=4)
            .filter_map(|delta| idx.checked_sub(delta))
            .map(|j| runs[j].trim())
            .find(|candidate| {
                !DIAGNOSTIC_CODE_RE.is_match(candidate)
                    && !SOURCE_FILE_RE.is_match(candidate)
                    && candidate.chars().any(|c| c.is_ascii_alphabetic())
                    && candidate.contains(' ')
                    && !candidate.contains("Copyright")
                    && !candidate.contains("Compiler version")
            })
            .unwrap_or("Build issue")
            .to_string();

        let file = (1..=4)
            .filter_map(|delta| runs.get(idx + delta))
            .find_map(|candidate| {
                SOURCE_FILE_RE
                    .captures(candidate)
                    .and_then(|caps| caps.get(0))
                    .map(|m| m.as_str().to_string())
            })
            .unwrap_or_default();

        if file.is_empty() && message == "Build issue" {
            continue;
        }

        let key = (code.to_string(), file.clone(), message.clone());
        if !seen.insert(key) {
            continue;
        }

        issues.push(BinlogIssue {
            code: code.to_string(),
            file,
            line: 0,
            column: 0,
            message,
        });
    }

    issues
}

fn is_likely_diagnostic_code(code: &str) -> bool {
    const ALLOWED_PREFIXES: &[&str] = &[
        "CS", "MSB", "NU", "FS", "BC", "CA", "SA", "IDE", "IL", "VB", "AD", "TS", "C", "LNK",
    ];

    ALLOWED_PREFIXES
        .iter()
        .any(|prefix| code.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn write_7bit_i32(buf: &mut Vec<u8>, value: i32) {
        let mut v = value as u32;
        while v >= 0x80 {
            buf.push(((v as u8) & 0x7F) | 0x80);
            v >>= 7;
        }
        buf.push(v as u8);
    }

    fn write_dotnet_string(buf: &mut Vec<u8>, value: &str) {
        write_7bit_i32(buf, value.len() as i32);
        buf.extend_from_slice(value.as_bytes());
    }

    fn write_event_record(target: &mut Vec<u8>, kind: i32, payload: &[u8]) {
        write_7bit_i32(target, kind);
        write_7bit_i32(target, payload.len() as i32);
        target.extend_from_slice(payload);
    }

    fn build_minimal_binlog(records: &[u8]) -> Vec<u8> {
        let mut plain = Vec::new();
        plain.extend_from_slice(&25_i32.to_le_bytes());
        plain.extend_from_slice(&18_i32.to_le_bytes());
        plain.extend_from_slice(records);

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&plain).expect("write plain payload");
        encoder.finish().expect("finish gzip")
    }

    #[test]
    fn test_scrub_sensitive_env_vars_masks_values() {
        let input = "PATH=/usr/local/bin HOME: /Users/daniel GITHUB_TOKEN=ghp_123";
        let scrubbed = scrub_sensitive_env_vars(input);

        assert!(scrubbed.contains("PATH=[REDACTED]"));
        assert!(scrubbed.contains("HOME: [REDACTED]"));
        assert!(scrubbed.contains("GITHUB_TOKEN=[REDACTED]"));
        assert!(!scrubbed.contains("/usr/local/bin"));
        assert!(!scrubbed.contains("ghp_123"));
    }

    #[test]
    fn test_scrub_sensitive_env_vars_masks_token_and_connection_values() {
        let input = "GH_TOKEN=ghs_abc AWS_SESSION_TOKEN=aws_xyz CONNECTION_STRING=Server=localhost";
        let scrubbed = scrub_sensitive_env_vars(input);

        assert!(scrubbed.contains("GH_TOKEN=[REDACTED]"));
        assert!(scrubbed.contains("AWS_SESSION_TOKEN=[REDACTED]"));
        assert!(scrubbed.contains("CONNECTION_STRING=[REDACTED]"));
        assert!(!scrubbed.contains("ghs_abc"));
        assert!(!scrubbed.contains("aws_xyz"));
        assert!(!scrubbed.contains("Server=localhost"));
    }

    #[test]
    fn test_parse_build_from_text_extracts_issues() {
        let input = r#"
Build FAILED.
src/Program.cs(42,15): error CS0103: The name 'foo' does not exist
src/Program.cs(25,10): warning CS0219: Variable 'x' is assigned but never used
    1 Warning(s)
    1 Error(s)
Time Elapsed 00:00:03.45
"#;

        let summary = parse_build_from_text(input);
        assert!(!summary.succeeded);
        assert_eq!(summary.errors.len(), 1);
        assert_eq!(summary.warnings.len(), 1);
        assert_eq!(summary.errors[0].code, "CS0103");
        assert_eq!(summary.warnings[0].code, "CS0219");
        assert_eq!(summary.duration_text.as_deref(), Some("00:00:03.45"));
    }

    #[test]
    fn test_parse_build_from_text_extracts_warning_without_code() {
        let input = r#"
/Users/dev/sdk/Microsoft.TestPlatform.targets(48,5): warning
Build succeeded with 1 warning(s) in 0.5s
"#;

        let summary = parse_build_from_text(input);
        assert_eq!(summary.warnings.len(), 1);
        assert_eq!(
            summary.warnings[0].file,
            "/Users/dev/sdk/Microsoft.TestPlatform.targets"
        );
        assert_eq!(summary.warnings[0].code, "");
    }

    #[test]
    fn test_parse_build_from_text_extracts_inline_warning_counts() {
        let input = r#"
Build failed with 1 error(s) and 4 warning(s) in 4.7s
"#;

        let summary = parse_build_from_text(input);
        assert_eq!(summary.errors.len(), 1);
        assert_eq!(summary.warnings.len(), 4);
    }

    #[test]
    fn test_parse_build_from_text_extracts_msbuild_global_error() {
        let input = r#"
MSBUILD : error MSB1009: Project file does not exist.
Switch: /tmp/nonexistent.csproj
"#;

        let summary = parse_build_from_text(input);
        assert_eq!(summary.errors.len(), 1);
        assert_eq!(summary.errors[0].code, "MSB1009");
        assert_eq!(summary.errors[0].file, "MSBUILD");
        assert!(summary.errors[0]
            .message
            .contains("Project file does not exist"));
    }

    #[test]
    fn test_parse_test_from_text_extracts_failure_summary() {
        let input = r#"
Failed!  - Failed:     2, Passed:   245, Skipped:     0, Total:   247, Duration: 1 s
  Failed MyApp.Tests.UnitTests.CalculatorTests.Add_ShouldReturnSum [5 ms]
  Error Message:
   Assert.Equal() Failure: Expected 5, Actual 4

  Failed MyApp.Tests.IntegrationTests.DatabaseTests.CanConnect [20 ms]
  Error Message:
   System.InvalidOperationException: Connection refused
"#;

        let summary = parse_test_from_text(input);
        assert_eq!(summary.passed, 245);
        assert_eq!(summary.failed, 2);
        assert_eq!(summary.total, 247);
        assert_eq!(summary.failed_tests.len(), 2);
        assert!(summary.failed_tests[0]
            .name
            .contains("CalculatorTests.Add_ShouldReturnSum"));
    }

    #[test]
    fn test_parse_test_from_text_keeps_multiline_failure_details() {
        let input = r#"
Failed!  - Failed:     1, Passed:   10, Skipped:     0, Total:   11, Duration: 1 s
  Failed MyApp.Tests.SampleTests.ShouldFail [5 ms]
  Error Message:
   Assert.That(messageInstance, Is.Null)
   Expected: null
   But was:  <MyApp.Tests.SampleTests+Impl>

   Stack Trace:
      at MyApp.Tests.SampleTests.ShouldFail() in /repo/SampleTests.cs:line 42
"#;

        let summary = parse_test_from_text(input);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.failed_tests.len(), 1);
        let details = summary.failed_tests[0].details.join("\n");
        assert!(details.contains("Expected: null"));
        assert!(details.contains("But was:"));
        assert!(details.contains("Stack Trace:"));
    }

    #[test]
    fn test_parse_test_from_text_ignores_non_test_failed_prefix_lines() {
        let input = r#"
Passed!  - Failed:     0, Passed:   940, Skipped:     7, Total:   947, Duration: 1 s
  Failed to load prune package data from PrunePackageData folder, loading from targeting packs instead
"#;

        let summary = parse_test_from_text(input);
        assert_eq!(summary.failed, 0);
        assert!(summary.failed_tests.is_empty());
    }

    #[test]
    fn test_parse_test_from_text_aggregates_multiple_project_summaries() {
        let input = r#"
Passed!  - Failed:     0, Passed:   914, Skipped:     7, Total:   921, Duration: 00:00:08.20
Failed!  - Failed:     1, Passed:    26, Skipped:     0, Total:    27, Duration: 00:00:00.54
Time Elapsed 00:00:12.34
"#;

        let summary = parse_test_from_text(input);
        assert_eq!(summary.passed, 940);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 7);
        assert_eq!(summary.total, 948);
        assert_eq!(summary.duration_text.as_deref(), Some("00:00:12.34"));
    }

    #[test]
    fn test_parse_test_from_text_prefers_test_summary_duration_and_counts() {
        let input = r#"
Failed!  - Failed:     1, Passed:   940, Skipped:     7, Total:   948, Duration: 1 s
Test summary: total: 949, failed: 1, succeeded: 940, skipped: 7, duration: 2.7s
Build failed with 1 error(s) and 4 warning(s) in 6.0s
"#;

        let summary = parse_test_from_text(input);
        assert_eq!(summary.passed, 940);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 7);
        assert_eq!(summary.total, 949);
        assert_eq!(summary.duration_text.as_deref(), Some("2.7s"));
    }

    #[test]
    fn test_parse_restore_from_text_extracts_project_count() {
        let input = r#"
  Restored /tmp/App/App.csproj (in 1.1 sec).
  Restored /tmp/App.Tests/App.Tests.csproj (in 1.2 sec).
"#;

        let summary = parse_restore_from_text(input);
        assert_eq!(summary.restored_projects, 2);
        assert_eq!(summary.errors, 0);
    }

    #[test]
    fn test_parse_restore_from_text_extracts_nuget_error_diagnostic() {
        let input = r#"
/Users/dev/src/App/App.csproj : error NU1101: Unable to find package Foo.Bar. No packages exist with this id in source(s): nuget.org

Restore failed with 1 error(s) in 1.0s
"#;

        let summary = parse_restore_from_text(input);
        assert_eq!(summary.errors, 1);
        assert_eq!(summary.warnings, 0);
    }

    #[test]
    fn test_parse_restore_issues_ignores_summary_warning_error_counts() {
        let input = r#"
  0 Warning(s)
  1 Error(s)

  Time Elapsed 00:00:01.23
"#;

        let (errors, warnings) = parse_restore_issues_from_text(input);
        assert_eq!(errors.len(), 0);
        assert_eq!(warnings.len(), 0);
    }

    #[test]
    fn test_parse_build_fails_when_binlog_is_unparseable() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let binlog_path = temp_dir.path().join("build.binlog");
        std::fs::write(&binlog_path, [0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00])
            .expect("write binary file");

        let err = parse_build(&binlog_path).expect_err("parse should fail");
        assert!(
            err.to_string().contains("Failed to parse binlog"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_parse_build_fails_when_binlog_missing() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let binlog_path = temp_dir.path().join("build.binlog");

        let err = parse_build(&binlog_path).expect_err("parse should fail");
        assert!(
            err.to_string().contains("Failed to parse binlog"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_parse_build_reads_structured_events() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let binlog_path = temp_dir.path().join("build.binlog");

        let mut records = Vec::new();

        // String records (index starts at 10)
        write_7bit_i32(&mut records, RECORD_STRING);
        write_dotnet_string(&mut records, "Build started"); // 10
        write_7bit_i32(&mut records, RECORD_STRING);
        write_dotnet_string(&mut records, "Build finished"); // 11
        write_7bit_i32(&mut records, RECORD_STRING);
        write_dotnet_string(&mut records, "src/App.csproj"); // 12
        write_7bit_i32(&mut records, RECORD_STRING);
        write_dotnet_string(&mut records, "The name 'foo' does not exist"); // 13
        write_7bit_i32(&mut records, RECORD_STRING);
        write_dotnet_string(&mut records, "CS0103"); // 14
        write_7bit_i32(&mut records, RECORD_STRING);
        write_dotnet_string(&mut records, "src/Program.cs"); // 15

        // BuildStarted (message + timestamp)
        let mut build_started = Vec::new();
        write_7bit_i32(&mut build_started, FLAG_MESSAGE | FLAG_TIMESTAMP);
        write_7bit_i32(&mut build_started, 10);
        build_started.extend_from_slice(&1_000_000_000_i64.to_le_bytes());
        write_7bit_i32(&mut build_started, 1);
        write_event_record(&mut records, RECORD_BUILD_STARTED, &build_started);

        // ProjectFinished
        let mut project_finished = Vec::new();
        write_7bit_i32(&mut project_finished, 0);
        write_7bit_i32(&mut project_finished, 12);
        project_finished.push(1);
        write_event_record(&mut records, RECORD_PROJECT_FINISHED, &project_finished);

        // Error event
        let mut error_event = Vec::new();
        write_7bit_i32(&mut error_event, FLAG_MESSAGE);
        write_7bit_i32(&mut error_event, 13);
        write_7bit_i32(&mut error_event, 0); // subcategory
        write_7bit_i32(&mut error_event, 14); // code
        write_7bit_i32(&mut error_event, 15); // file
        write_7bit_i32(&mut error_event, 0); // project file
        write_7bit_i32(&mut error_event, 42);
        write_7bit_i32(&mut error_event, 10);
        write_7bit_i32(&mut error_event, 42);
        write_7bit_i32(&mut error_event, 10);
        write_event_record(&mut records, RECORD_ERROR, &error_event);

        // BuildFinished (message + timestamp + succeeded)
        let mut build_finished = Vec::new();
        write_7bit_i32(&mut build_finished, FLAG_MESSAGE | FLAG_TIMESTAMP);
        write_7bit_i32(&mut build_finished, 11);
        build_finished.extend_from_slice(&1_010_000_000_i64.to_le_bytes());
        write_7bit_i32(&mut build_finished, 1);
        build_finished.push(1);
        write_event_record(&mut records, RECORD_BUILD_FINISHED, &build_finished);

        write_7bit_i32(&mut records, RECORD_END_OF_FILE);

        let binlog_bytes = build_minimal_binlog(&records);
        std::fs::write(&binlog_path, binlog_bytes).expect("write binlog");

        let summary = parse_build(&binlog_path).expect("parse should succeed");
        assert!(summary.succeeded);
        assert_eq!(summary.project_count, 1);
        assert_eq!(summary.errors.len(), 1);
        assert_eq!(summary.errors[0].code, "CS0103");
        assert_eq!(summary.duration_text.as_deref(), Some("00:00:01.00"));
    }

    #[test]
    fn test_parse_test_reads_message_events() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let binlog_path = temp_dir.path().join("test.binlog");

        let mut records = Vec::new();
        write_7bit_i32(&mut records, RECORD_STRING);
        write_dotnet_string(
            &mut records,
            "Failed!  - Failed:     1, Passed:     2, Skipped:     0, Total:     3, Duration: 1 s",
        ); // 10

        let mut message_event = Vec::new();
        write_7bit_i32(&mut message_event, FLAG_MESSAGE | FLAG_IMPORTANCE);
        write_7bit_i32(&mut message_event, 10);
        write_7bit_i32(&mut message_event, 1);
        write_event_record(&mut records, RECORD_MESSAGE, &message_event);

        write_7bit_i32(&mut records, RECORD_END_OF_FILE);
        let binlog_bytes = build_minimal_binlog(&records);
        std::fs::write(&binlog_path, binlog_bytes).expect("write binlog");

        let summary = parse_test(&binlog_path).expect("parse should succeed");
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.passed, 2);
        assert_eq!(summary.total, 3);
    }

    #[test]
    fn test_parse_test_fails_when_binlog_missing() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let binlog_path = temp_dir.path().join("test.binlog");

        let err = parse_test(&binlog_path).expect_err("parse should fail");
        assert!(
            err.to_string().contains("Failed to parse binlog"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_parse_restore_fails_when_binlog_missing() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let binlog_path = temp_dir.path().join("restore.binlog");

        let err = parse_restore(&binlog_path).expect_err("parse should fail");
        assert!(
            err.to_string().contains("Failed to parse binlog"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_parse_build_from_fixture_text() {
        let input = include_str!("../../../tests/fixtures/dotnet/build_failed.txt");
        let summary = parse_build_from_text(input);

        assert_eq!(summary.errors.len(), 1);
        assert_eq!(summary.errors[0].code, "CS1525");
        assert_eq!(summary.duration_text.as_deref(), Some("00:00:00.76"));
    }

    #[test]
    fn test_parse_build_sets_project_count_floor() {
        let input = r#"
RtkDotnetSmoke -> /tmp/RtkDotnetSmoke.dll

Build succeeded.
    0 Warning(s)
    0 Error(s)

Time Elapsed 00:00:00.12
"#;

        let summary = parse_build_from_text(input);
        assert_eq!(summary.project_count, 1);
        assert!(summary.succeeded);
    }

    #[test]
    fn test_parse_build_does_not_infer_binary_errors_on_successful_build() {
        let input = "\x0bInvalid expression term ';'\x18\x06CS1525\x18%/tmp/App/Broken.cs\x09\nBuild succeeded.\n    0 Warning(s)\n    0 Error(s)\n";

        let summary = parse_build_from_text(input);
        assert!(summary.succeeded);
        assert!(summary.errors.is_empty());
    }

    #[test]
    fn test_parse_test_from_fixture_text() {
        let input = include_str!("../../../tests/fixtures/dotnet/test_failed.txt");
        let summary = parse_test_from_text(input);

        assert_eq!(summary.failed, 1);
        assert_eq!(summary.passed, 0);
        assert_eq!(summary.total, 1);
        assert_eq!(summary.failed_tests.len(), 1);
        assert!(summary.failed_tests[0]
            .name
            .contains("RtkDotnetSmoke.UnitTest1.Test1"));
    }

    #[test]
    fn test_extract_binary_like_issues_recovers_code_message_and_path() {
        let noisy =
            "\x0bInvalid expression term ';'\x18\x06CS1525\x18%/tmp/RtkDotnetSmoke/Broken.cs\x09";
        let issues = extract_binary_like_issues(noisy);

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "CS1525");
        assert_eq!(issues[0].file, "/tmp/RtkDotnetSmoke/Broken.cs");
        assert!(issues[0].message.contains("Invalid expression term"));
    }

    #[test]
    fn test_is_likely_diagnostic_code_filters_framework_monikers() {
        assert!(is_likely_diagnostic_code("CS1525"));
        assert!(is_likely_diagnostic_code("MSB4018"));
        assert!(!is_likely_diagnostic_code("NET451"));
        assert!(!is_likely_diagnostic_code("NET10"));
    }

    #[test]
    fn test_select_best_issues_prefers_fallback_when_primary_loses_context() {
        let primary = vec![BinlogIssue {
            code: String::new(),
            file: "CS1525".to_string(),
            line: 51,
            column: 1,
            message: "Invalid expression term ';'".to_string(),
        }];

        let fallback = vec![BinlogIssue {
            code: "CS1525".to_string(),
            file: "/Users/dev/project/src/NServiceBus.Core/Class1.cs".to_string(),
            line: 1,
            column: 9,
            message: "Invalid expression term ';'".to_string(),
        }];

        let selected = select_best_issues(primary, fallback.clone());
        assert_eq!(selected, fallback);
    }

    #[test]
    fn test_select_best_issues_keeps_primary_when_context_is_good() {
        let primary = vec![BinlogIssue {
            code: "CS0103".to_string(),
            file: "src/Program.cs".to_string(),
            line: 42,
            column: 15,
            message: "The name 'foo' does not exist".to_string(),
        }];

        let fallback = vec![BinlogIssue {
            code: "CS0103".to_string(),
            file: String::new(),
            line: 0,
            column: 0,
            message: "Build error #1 (details omitted)".to_string(),
        }];

        let selected = select_best_issues(primary.clone(), fallback);
        assert_eq!(selected, primary);
    }
}
