use crate::binlog::{FailedTest, TestSummary};
use chrono::{DateTime, FixedOffset};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|b| *b == b':').next().unwrap_or(name)
}

fn extract_attr_value(
    reader: &Reader<&[u8]>,
    start: &BytesStart<'_>,
    key: &[u8],
) -> Option<String> {
    for attr in start.attributes().flatten() {
        if local_name(attr.key.as_ref()) != key {
            continue;
        }

        if let Ok(value) = attr.decode_and_unescape_value(reader.decoder()) {
            return Some(value.into_owned());
        }
    }

    None
}

fn parse_usize_attr(reader: &Reader<&[u8]>, start: &BytesStart<'_>, key: &[u8]) -> usize {
    extract_attr_value(reader, start, key)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0)
}

fn parse_trx_duration(start: &str, finish: &str) -> Option<String> {
    let start_dt = DateTime::parse_from_rfc3339(start).ok()?;
    let finish_dt = DateTime::parse_from_rfc3339(finish).ok()?;
    format_duration_between(start_dt, finish_dt)
}

fn format_duration_between(
    start_dt: DateTime<FixedOffset>,
    finish_dt: DateTime<FixedOffset>,
) -> Option<String> {
    let diff = finish_dt.signed_duration_since(start_dt);
    let millis = diff.num_milliseconds();
    if millis <= 0 {
        return None;
    }

    if millis >= 1_000 {
        let seconds = millis as f64 / 1_000.0;
        return Some(format!("{seconds:.1} s"));
    }

    Some(format!("{millis} ms"))
}

fn parse_trx_time_bounds(content: &str) -> Option<(DateTime<FixedOffset>, DateTime<FixedOffset>)> {
    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) != b"Times" {
                    buf.clear();
                    continue;
                }

                let start = extract_attr_value(&reader, &e, b"start")?;
                let finish = extract_attr_value(&reader, &e, b"finish")?;
                let start_dt = DateTime::parse_from_rfc3339(&start).ok()?;
                let finish_dt = DateTime::parse_from_rfc3339(&finish).ok()?;
                return Some((start_dt, finish_dt));
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }

        buf.clear();
    }

    None
}

/// Parse TRX (Visual Studio Test Results) file to extract test summary.
/// Returns None if the file doesn't exist or isn't a valid TRX file.
pub fn parse_trx_file(path: &Path) -> Option<TestSummary> {
    let content = std::fs::read_to_string(path).ok()?;
    parse_trx_content(&content)
}

pub fn parse_trx_file_since(path: &Path, since: SystemTime) -> Option<TestSummary> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    if modified < since {
        return None;
    }

    parse_trx_file(path)
}

pub fn parse_trx_files_in_dir(dir: &Path) -> Option<TestSummary> {
    parse_trx_files_in_dir_since(dir, None)
}

pub fn parse_trx_files_in_dir_since(dir: &Path, since: Option<SystemTime>) -> Option<TestSummary> {
    if !dir.exists() || !dir.is_dir() {
        return None;
    }

    let mut summaries = Vec::new();
    let mut min_start: Option<DateTime<FixedOffset>> = None;
    let mut max_finish: Option<DateTime<FixedOffset>> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|e| !e.eq_ignore_ascii_case("trx"))
        {
            continue;
        }

        if let Some(since) = since {
            let modified = match entry.metadata().ok().and_then(|m| m.modified().ok()) {
                Some(modified) => modified,
                None => continue,
            };
            if modified < since {
                continue;
            }
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };

        if let Some((start, finish)) = parse_trx_time_bounds(&content) {
            min_start = Some(min_start.map_or(start, |prev| prev.min(start)));
            max_finish = Some(max_finish.map_or(finish, |prev| prev.max(finish)));
        }

        if let Some(summary) = parse_trx_content(&content) {
            summaries.push(summary);
        }
    }

    if summaries.is_empty() {
        return None;
    }

    let mut merged = TestSummary::default();
    for summary in summaries {
        merged.passed += summary.passed;
        merged.failed += summary.failed;
        merged.skipped += summary.skipped;
        merged.total += summary.total;
        merged.failed_tests.extend(summary.failed_tests);
        merged.project_count += summary.project_count.max(1);
        if merged.duration_text.is_none() {
            merged.duration_text = summary.duration_text;
        }
    }

    if let (Some(start), Some(finish)) = (min_start, max_finish) {
        merged.duration_text = format_duration_between(start, finish);
    }

    Some(merged)
}

pub fn find_recent_trx_in_testresults() -> Option<PathBuf> {
    find_recent_trx_in_dir(Path::new("./TestResults"))
}

fn find_recent_trx_in_dir(dir: &Path) -> Option<PathBuf> {
    if !dir.exists() {
        return None;
    }

    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let is_trx = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("trx"));
            if !is_trx {
                return None;
            }

            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

fn parse_trx_content(content: &str) -> Option<TestSummary> {
    #[derive(Clone, Copy)]
    enum CaptureField {
        Message,
        StackTrace,
    }

    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut summary = TestSummary::default();
    let mut saw_test_run = false;
    let mut in_failed_result = false;
    let mut in_error_info = false;
    let mut failed_test_name = String::new();
    let mut message_buf = String::new();
    let mut stack_buf = String::new();
    let mut capture_field: Option<CaptureField> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"TestRun" => saw_test_run = true,
                b"Times" => {
                    let start = extract_attr_value(&reader, &e, b"start");
                    let finish = extract_attr_value(&reader, &e, b"finish");
                    if let (Some(start), Some(finish)) = (start, finish) {
                        summary.duration_text = parse_trx_duration(&start, &finish);
                    }
                }
                b"Counters" => {
                    summary.total = parse_usize_attr(&reader, &e, b"total");
                    summary.passed = parse_usize_attr(&reader, &e, b"passed");
                    summary.failed = parse_usize_attr(&reader, &e, b"failed");
                }
                b"UnitTestResult" => {
                    let outcome = extract_attr_value(&reader, &e, b"outcome")
                        .unwrap_or_else(|| "Unknown".to_string());

                    if outcome == "Failed" {
                        in_failed_result = true;
                        in_error_info = false;
                        capture_field = None;
                        message_buf.clear();
                        stack_buf.clear();
                        failed_test_name = extract_attr_value(&reader, &e, b"testName")
                            .unwrap_or_else(|| "unknown".to_string());
                    }
                }
                b"ErrorInfo" => {
                    if in_failed_result {
                        in_error_info = true;
                    }
                }
                b"Message" => {
                    if in_failed_result && in_error_info {
                        capture_field = Some(CaptureField::Message);
                        message_buf.clear();
                    }
                }
                b"StackTrace" => {
                    if in_failed_result && in_error_info {
                        capture_field = Some(CaptureField::StackTrace);
                        stack_buf.clear();
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => match local_name(e.name().as_ref()) {
                b"Times" => {
                    let start = extract_attr_value(&reader, &e, b"start");
                    let finish = extract_attr_value(&reader, &e, b"finish");
                    if let (Some(start), Some(finish)) = (start, finish) {
                        summary.duration_text = parse_trx_duration(&start, &finish);
                    }
                }
                b"Counters" => {
                    summary.total = parse_usize_attr(&reader, &e, b"total");
                    summary.passed = parse_usize_attr(&reader, &e, b"passed");
                    summary.failed = parse_usize_attr(&reader, &e, b"failed");
                }
                b"UnitTestResult" => {
                    let outcome = extract_attr_value(&reader, &e, b"outcome")
                        .unwrap_or_else(|| "Unknown".to_string());
                    if outcome == "Failed" {
                        let name = extract_attr_value(&reader, &e, b"testName")
                            .unwrap_or_else(|| "unknown".to_string());
                        summary.failed_tests.push(FailedTest {
                            name,
                            details: Vec::new(),
                        });
                    }
                }
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if !in_failed_result {
                    buf.clear();
                    continue;
                }

                let text = String::from_utf8_lossy(e.as_ref());
                match capture_field {
                    Some(CaptureField::Message) => message_buf.push_str(&text),
                    Some(CaptureField::StackTrace) => stack_buf.push_str(&text),
                    None => {}
                }
            }
            Ok(Event::CData(e)) => {
                if !in_failed_result {
                    buf.clear();
                    continue;
                }

                let text = String::from_utf8_lossy(e.as_ref());
                match capture_field {
                    Some(CaptureField::Message) => message_buf.push_str(&text),
                    Some(CaptureField::StackTrace) => stack_buf.push_str(&text),
                    None => {}
                }
            }
            Ok(Event::End(e)) => match local_name(e.name().as_ref()) {
                b"Message" | b"StackTrace" => {
                    capture_field = None;
                }
                b"ErrorInfo" => {
                    in_error_info = false;
                }
                b"UnitTestResult" => {
                    if in_failed_result {
                        let mut details = Vec::new();

                        let message = message_buf.trim();
                        if !message.is_empty() {
                            details.push(message.to_string());
                        }

                        let stack = stack_buf.trim();
                        if !stack.is_empty() {
                            let stack_lines: Vec<&str> = stack.lines().take(3).collect();
                            if !stack_lines.is_empty() {
                                details.push(stack_lines.join("\n"));
                            }
                        }

                        summary.failed_tests.push(FailedTest {
                            name: failed_test_name.clone(),
                            details,
                        });

                        in_failed_result = false;
                        in_error_info = false;
                        capture_field = None;
                        message_buf.clear();
                        stack_buf.clear();
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }

        buf.clear();
    }

    if !saw_test_run {
        return None;
    }

    // Calculate skipped from counters if available
    if summary.total > 0 {
        summary.skipped = summary
            .total
            .saturating_sub(summary.passed + summary.failed);
    }

    // Set project count to at least 1 if there were any tests
    if summary.total > 0 {
        summary.project_count = 1;
    }

    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_parse_trx_content_extracts_passed_counts() {
        let trx = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun xmlns="http://microsoft.com/schemas/VisualStudio/TeamTest/2010">
  <Times creation="2026-02-21T12:57:28.3323710+01:00" queuing="2026-02-21T12:57:28.3323710+01:00" start="2026-02-21T12:57:27.7149650+01:00" finish="2026-02-21T12:57:30.2214710+01:00" />
  <ResultSummary outcome="Completed">
    <Counters total="42" executed="42" passed="40" failed="2" error="0" timeout="0" aborted="0" inconclusive="0" />
  </ResultSummary>
</TestRun>"#;

        let summary = parse_trx_content(trx).expect("valid TRX");
        assert_eq!(summary.total, 42);
        assert_eq!(summary.passed, 40);
        assert_eq!(summary.failed, 2);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.duration_text.as_deref(), Some("2.5 s"));
    }

    #[test]
    fn test_parse_trx_content_extracts_failed_tests_with_details() {
        let trx = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun>
  <Results>
    <UnitTestResult testName="MyTests.Calculator.Add_ShouldFail" outcome="Failed">
      <Output>
        <ErrorInfo>
          <Message>Expected: 5, Actual: 4</Message>
          <StackTrace>at MyTests.Calculator.Add_ShouldFail()\nat line 42</StackTrace>
        </ErrorInfo>
      </Output>
    </UnitTestResult>
  </Results>
  <ResultSummary><Counters total="1" executed="1" passed="0" failed="1" /></ResultSummary>
</TestRun>"#;

        let summary = parse_trx_content(trx).expect("valid TRX");
        assert_eq!(summary.failed_tests.len(), 1);
        assert_eq!(
            summary.failed_tests[0].name,
            "MyTests.Calculator.Add_ShouldFail"
        );
        assert!(summary.failed_tests[0].details[0].contains("Expected: 5, Actual: 4"));
    }

    #[test]
    fn test_parse_trx_content_extracts_counters_when_attribute_order_varies() {
        let trx = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun>
  <ResultSummary outcome="Completed">
    <Counters failed="3" passed="7" executed="10" total="10" />
  </ResultSummary>
</TestRun>"#;

        let summary = parse_trx_content(trx).expect("valid TRX");
        assert_eq!(summary.total, 10);
        assert_eq!(summary.passed, 7);
        assert_eq!(summary.failed, 3);
    }

    #[test]
    fn test_parse_trx_content_extracts_failed_tests_when_attribute_order_varies() {
        let trx = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun>
  <Results>
    <UnitTestResult outcome="Failed" testName="MyTests.Ordering.ShouldStillParse">
      <Output>
        <ErrorInfo>
          <Message>Boom</Message>
          <StackTrace>at MyTests.Ordering.ShouldStillParse()</StackTrace>
        </ErrorInfo>
      </Output>
    </UnitTestResult>
  </Results>
  <ResultSummary><Counters failed="1" passed="0" executed="1" total="1" /></ResultSummary>
</TestRun>"#;

        let summary = parse_trx_content(trx).expect("valid TRX");
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.failed_tests.len(), 1);
        assert_eq!(
            summary.failed_tests[0].name,
            "MyTests.Ordering.ShouldStillParse"
        );
    }

    #[test]
    fn test_parse_trx_content_returns_none_for_invalid_xml() {
        let not_trx = "This is not a TRX file";
        assert!(parse_trx_content(not_trx).is_none());
    }

    #[test]
    fn test_find_recent_trx_in_dir_returns_none_when_missing() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let missing_dir = temp_dir.path().join("TestResults");

        let found = find_recent_trx_in_dir(&missing_dir);
        assert!(found.is_none());
    }

    #[test]
    fn test_find_recent_trx_in_dir_picks_newest_trx() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let testresults_dir = temp_dir.path().join("TestResults");
        std::fs::create_dir_all(&testresults_dir).expect("create TestResults");

        let old_trx = testresults_dir.join("old.trx");
        let new_trx = testresults_dir.join("new.trx");
        std::fs::write(&old_trx, "old").expect("write old");
        std::thread::sleep(Duration::from_millis(5));
        std::fs::write(&new_trx, "new").expect("write new");

        let found = find_recent_trx_in_dir(&testresults_dir).expect("should find newest trx");
        assert_eq!(found, new_trx);
    }

    #[test]
    fn test_find_recent_trx_in_dir_ignores_non_trx_files() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let testresults_dir = temp_dir.path().join("TestResults");
        std::fs::create_dir_all(&testresults_dir).expect("create TestResults");

        let txt = testresults_dir.join("notes.txt");
        std::fs::write(&txt, "noop").expect("write txt");

        let found = find_recent_trx_in_dir(&testresults_dir);
        assert!(found.is_none());
    }

    #[test]
    fn test_parse_trx_files_in_dir_aggregates_counts_and_wall_clock_duration() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let trx_dir = temp_dir.path().join("TestResults");
        std::fs::create_dir_all(&trx_dir).expect("create TestResults");

        let trx_one = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun>
  <Times start="2026-02-21T12:57:27.0000000+01:00" finish="2026-02-21T12:57:30.0000000+01:00" />
  <ResultSummary outcome="Completed">
    <Counters total="10" executed="10" passed="9" failed="1" />
  </ResultSummary>
</TestRun>"#;

        let trx_two = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun>
  <Times start="2026-02-21T12:57:28.0000000+01:00" finish="2026-02-21T12:57:29.0000000+01:00" />
  <ResultSummary outcome="Completed">
    <Counters total="20" executed="20" passed="20" failed="0" />
  </ResultSummary>
</TestRun>"#;

        std::fs::write(trx_dir.join("a.trx"), trx_one).expect("write first trx");
        std::fs::write(trx_dir.join("b.trx"), trx_two).expect("write second trx");

        let summary = parse_trx_files_in_dir(&trx_dir).expect("merged summary");
        assert_eq!(summary.total, 30);
        assert_eq!(summary.passed, 29);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.duration_text.as_deref(), Some("3.0 s"));
    }

    #[test]
    fn test_parse_trx_files_in_dir_since_ignores_older_files() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let trx_dir = temp_dir.path().join("TestResults");
        std::fs::create_dir_all(&trx_dir).expect("create TestResults");

        let trx_old = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun><ResultSummary><Counters total="2" executed="2" passed="2" failed="0" /></ResultSummary></TestRun>"#;
        std::fs::write(trx_dir.join("old.trx"), trx_old).expect("write old trx");
        std::thread::sleep(Duration::from_millis(5));
        let since = SystemTime::now();
        std::thread::sleep(Duration::from_millis(5));

        let trx_new = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun><ResultSummary><Counters total="3" executed="3" passed="2" failed="1" /></ResultSummary></TestRun>"#;
        std::fs::write(trx_dir.join("new.trx"), trx_new).expect("write new trx");

        let summary = parse_trx_files_in_dir_since(&trx_dir, Some(since)).expect("merged summary");
        assert_eq!(summary.total, 3);
        assert_eq!(summary.failed, 1);
    }

    #[test]
    fn test_parse_trx_files_in_dir_since_handles_uppercase_extension() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let trx_dir = temp_dir.path().join("TestResults");
        std::fs::create_dir_all(&trx_dir).expect("create TestResults");

        let trx = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun><ResultSummary><Counters total="3" executed="3" passed="2" failed="1" /></ResultSummary></TestRun>"#;
        std::fs::write(trx_dir.join("UPPER.TRX"), trx).expect("write trx");

        let summary = parse_trx_files_in_dir_since(&trx_dir, None).expect("summary");
        assert_eq!(summary.total, 3);
        assert_eq!(summary.failed, 1);
    }
}
