//! Parses Maven Surefire/Failsafe XML test reports (`TEST-*.xml`) with the
//! quick-xml streaming parser. `parse_dir` is mtime-gated so reports left
//! behind by a previous run are never mistaken for this run's.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::path::Path;
use std::time::SystemTime;

/// Head cap on the stack trace kept per failure: the first lines are kept
/// verbatim, the rest is folded into a `... (+N lines)` tail.
const MAX_STACK_TRACE_LINES: usize = 30;

#[derive(Debug, Default)]
pub struct TestSummary {
    pub run: u32,
    pub failures: u32,
    pub errors: u32,
    pub skipped: u32,
}

impl TestSummary {
    fn add(&mut self, other: &Self) {
        self.run += other.run;
        self.failures += other.failures;
        self.errors += other.errors;
        self.skipped += other.skipped;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FailureKind {
    Failure,
    Error,
}

#[derive(Debug)]
pub struct TestFailure {
    pub test_class: String,
    pub test_method: String,
    pub kind: FailureKind,
    /// `message` attribute of the `<failure>`/`<error>` element.
    pub message: Option<String>,
    /// `type` attribute: the exception class.
    pub failure_type: Option<String>,
    /// Element body: the Java stack trace, capped to `MAX_STACK_TRACE_LINES`.
    pub stack_trace: Option<String>,
}

#[derive(Debug, Default)]
pub struct SurefireResult {
    pub summary: TestSummary,
    pub failures: Vec<TestFailure>,
}

impl SurefireResult {
    pub(crate) fn merge(&mut self, other: Self) {
        self.summary.add(&other.summary);
        self.failures.extend(other.failures);
    }
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|b| *b == b':').next().unwrap_or(name)
}

/// Non-empty value of the attribute `key`, namespace prefix ignored.
fn attr(reader: &Reader<&[u8]>, start: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    start
        .attributes()
        .flatten()
        .find(|a| local_name(a.key.as_ref()) == key)
        .and_then(|a| a.decode_and_unescape_value(reader.decoder()).ok())
        .map(|v| v.into_owned())
        .filter(|v| !v.is_empty())
}

fn count_attr(reader: &Reader<&[u8]>, start: &BytesStart<'_>, key: &[u8]) -> u32 {
    attr(reader, start, key)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Keep the first `max` lines verbatim and fold the rest into a count tail.
/// `None` for empty text.
fn cap_lines(text: &str, max: usize) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let total = text.lines().count();
    if total <= max {
        return Some(text.to_string());
    }
    let mut kept = text.lines().take(max).collect::<Vec<_>>().join("\n");
    kept.push_str(&format!("\n... (+{} lines)", total - max));
    Some(kept)
}

/// Parse one `TEST-*.xml` document. `None` when the XML is malformed or has
/// no `<testsuite>`; otherwise a best-effort result.
pub(crate) fn parse_content(xml: &str) -> Option<SurefireResult> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();

    let mut result = SurefireResult::default();
    let mut saw_testsuite = false;
    let mut class = String::new();
    let mut method = String::new();
    // The `<failure>`/`<error>` being read; its text body is the stack trace.
    let mut pending: Option<TestFailure> = None;
    let mut trace = String::new();

    loop {
        let event = reader.read_event_into(&mut buf);
        match event {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                match local_name(e.name().as_ref()) {
                    b"testsuite" => {
                        saw_testsuite = true;
                        result.summary.add(&TestSummary {
                            run: count_attr(&reader, e, b"tests"),
                            failures: count_attr(&reader, e, b"failures"),
                            errors: count_attr(&reader, e, b"errors"),
                            skipped: count_attr(&reader, e, b"skipped"),
                        });
                    }
                    b"testcase" => {
                        class = attr(&reader, e, b"classname").unwrap_or_default();
                        method = attr(&reader, e, b"name").unwrap_or_default();
                    }
                    tag @ (b"failure" | b"error") => {
                        pending = Some(TestFailure {
                            test_class: class.clone(),
                            test_method: method.clone(),
                            kind: if tag == b"failure" {
                                FailureKind::Failure
                            } else {
                                FailureKind::Error
                            },
                            message: attr(&reader, e, b"message"),
                            failure_type: attr(&reader, e, b"type"),
                            stack_trace: None,
                        });
                        trace.clear();
                    }
                    _ => {}
                }
                // `<failure .../>` has no body: nothing more to collect.
                if matches!(event, Ok(Event::Empty(_))) {
                    result.failures.extend(pending.take());
                }
            }
            Ok(Event::Text(ref t)) if pending.is_some() => {
                if let Ok(text) = t.unescape() {
                    trace.push_str(&text);
                }
            }
            Ok(Event::CData(ref c)) if pending.is_some() => {
                trace.push_str(&String::from_utf8_lossy(c));
            }
            Ok(Event::End(ref e))
                if matches!(local_name(e.name().as_ref()), b"failure" | b"error") =>
            {
                if let Some(mut failure) = pending.take() {
                    failure.stack_trace = cap_lines(trace.trim(), MAX_STACK_TRACE_LINES);
                    result.failures.push(failure);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }

    saw_testsuite.then_some(result)
}

/// Parse every `TEST-*.xml` in `dir` modified at or after `since` and merge
/// the results. Stale files are skipped silently; unreadable or malformed
/// ones are reported on stderr and skipped. `None` when nothing was parsed.
pub(crate) fn parse_dir(dir: &Path, since: SystemTime) -> Option<SurefireResult> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("TEST-") && n.ends_with(".xml"))
        })
        .collect();
    paths.sort();

    let mut merged: Option<SurefireResult> = None;
    for path in paths {
        let fresh = path
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|m| m >= since);
        if !fresh {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            eprintln!("rtk mvn: skipping unreadable {}", path.display());
            continue;
        };
        let Some(parsed) = parse_content(&content) else {
            eprintln!("rtk mvn: skipping malformed {}", path.display());
            continue;
        };
        merged
            .get_or_insert_with(SurefireResult::default)
            .merge(parsed);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    const FIXTURES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/java/surefire-reports"
    );

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(Path::new(FIXTURES).join(name)).expect("read fixture")
    }

    fn copy_fixture(tmp: &tempfile::TempDir, name: &str, mtime: Option<SystemTime>) {
        let dst = tmp.path().join(name);
        std::fs::copy(Path::new(FIXTURES).join(name), &dst).expect("copy fixture");
        if let Some(mtime) = mtime {
            std::fs::File::options()
                .append(true)
                .open(&dst)
                .expect("open fixture")
                .set_modified(mtime)
                .expect("set mtime");
        }
    }

    #[test]
    fn parse_dir_missing_returns_none() {
        let dir = Path::new("/definitely/does/not/exist/rtk-test");
        assert!(parse_dir(dir, SystemTime::UNIX_EPOCH).is_none());
    }

    #[test]
    fn parse_dir_aggregates_multi_file_counts() {
        let tmp = tempfile::tempdir().unwrap();
        copy_fixture(&tmp, "TEST-com.example.PassingTest.xml", None);
        copy_fixture(&tmp, "TEST-com.example.FailingTest.xml", None);
        copy_fixture(&tmp, "TEST-com.example.SkippedTest.xml", None);

        let result = parse_dir(tmp.path(), SystemTime::UNIX_EPOCH).expect("parses");
        assert_eq!(result.summary.run, 3 + 4 + 2);
        assert_eq!(result.summary.failures, 2);
        assert_eq!(result.summary.skipped, 1);
        assert_eq!(result.failures.len(), 2);
    }

    #[test]
    fn parse_dir_time_gate_skips_stale_files() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let stale = now - Duration::from_secs(60 * 60);
        let fresh = now + Duration::from_millis(50);
        copy_fixture(&tmp, "TEST-com.example.PassingTest.xml", Some(stale));
        copy_fixture(&tmp, "TEST-com.example.FailingTest.xml", Some(fresh));

        let result = parse_dir(tmp.path(), now).expect("parses");
        assert_eq!(result.summary.run, 4, "only the fresh FailingTest counts");
        assert_eq!(result.summary.failures, 2);
    }

    #[test]
    fn parse_dir_all_stale_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let stale = now - Duration::from_secs(60 * 60);
        copy_fixture(&tmp, "TEST-com.example.FailingTest.xml", Some(stale));
        assert!(parse_dir(tmp.path(), now).is_none());
    }

    #[test]
    fn parse_dir_malformed_file_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        copy_fixture(&tmp, "TEST-com.example.PassingTest.xml", None);
        std::fs::write(
            tmp.path().join("TEST-com.example.Broken.xml"),
            "<not-xml>>>>",
        )
        .unwrap();

        let result = parse_dir(tmp.path(), SystemTime::UNIX_EPOCH).expect("parses");
        assert_eq!(result.summary.run, 3, "PassingTest only");
        assert!(result.failures.is_empty());
    }

    #[test]
    fn parse_content_single_passing() {
        let result = parse_content(&fixture("TEST-com.example.PassingTest.xml")).expect("parses");
        assert_eq!(result.summary.run, 3);
        assert_eq!(result.summary.failures, 0);
        assert_eq!(result.summary.errors, 0);
        assert!(result.failures.is_empty());
    }

    #[test]
    fn parse_content_single_failing_extracts_details() {
        let result = parse_content(&fixture("TEST-com.example.FailingTest.xml")).expect("parses");
        assert_eq!(result.summary.failures, 2);
        assert_eq!(result.failures.len(), 2);
        let first = &result.failures[0];
        assert_eq!(first.test_class, "com.example.FailingTest");
        assert_eq!(first.test_method, "shouldReturnUser");
        assert_eq!(first.kind, FailureKind::Failure);
        assert_eq!(
            first.message.as_deref(),
            Some("expected:<200> but was:<404>")
        );
        assert_eq!(
            first.failure_type.as_deref(),
            Some("org.opentest4j.AssertionFailedError")
        );
        let trace = first.stack_trace.as_deref().expect("trace");
        assert!(
            trace
                .starts_with("org.opentest4j.AssertionFailedError: expected:<200> but was:<404>\n"),
            "{trace}"
        );
        assert!(
            trace.contains("at com.example.FailingTest.shouldReturnUser(FailingTest.java:42)"),
            "{trace}"
        );
    }

    #[test]
    fn parse_content_error_element_marks_failure_kind_error() {
        let result = parse_content(&fixture("TEST-com.example.ErrorTest.xml")).expect("parses");
        assert_eq!(result.summary.errors, 1);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].kind, FailureKind::Error);
        assert_eq!(
            result.failures[0].failure_type.as_deref(),
            Some("java.net.ConnectException")
        );
    }

    #[test]
    fn parse_content_skipped_testsuite_counts_skipped() {
        let result = parse_content(&fixture("TEST-com.example.SkippedTest.xml")).expect("parses");
        assert_eq!(result.summary.skipped, 1);
        assert!(result.failures.is_empty());
    }

    #[test]
    fn parse_content_empty_failure_element_has_no_trace() {
        let xml = r#"<testsuite name="a.T" tests="1" failures="1">
  <testcase name="m" classname="a.T"><failure message="boom" type="java.lang.AssertionError"/></testcase>
</testsuite>"#;
        let result = parse_content(xml).expect("parses");
        assert_eq!(result.failures.len(), 1);
        let f = &result.failures[0];
        assert_eq!(
            (f.test_class.as_str(), f.test_method.as_str()),
            ("a.T", "m")
        );
        assert_eq!(f.message.as_deref(), Some("boom"));
        assert!(f.stack_trace.is_none());
    }

    #[test]
    fn parse_content_caps_stack_trace_lines() {
        let frames: String = (0..MAX_STACK_TRACE_LINES + 5)
            .map(|i| format!("\tat a.T.m{i}(T.java:{i})\n"))
            .collect();
        let xml = format!(
            r#"<testsuite name="a.T" tests="1" failures="1">
  <testcase name="m" classname="a.T"><failure message="boom" type="java.lang.AssertionError">java.lang.AssertionError: boom
{frames}</failure></testcase>
</testsuite>"#
        );
        let result = parse_content(&xml).expect("parses");
        let trace = result.failures[0].stack_trace.as_deref().expect("trace");
        // Header line + frames = MAX + 6 lines in total; MAX kept + the tail.
        assert_eq!(trace.lines().count(), MAX_STACK_TRACE_LINES + 1, "{trace}");
        assert!(trace.ends_with("... (+6 lines)"), "{trace}");
    }

    #[test]
    fn parse_content_no_testsuite_returns_none() {
        assert!(parse_content("<not-xml>>>>").is_none());
        assert!(parse_content("<root><child/></root>").is_none());
    }
}
