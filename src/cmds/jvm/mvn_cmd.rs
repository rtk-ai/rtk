//! `rtk mvn` — Maven wrapper with compact output.
//!
//! Modes:
//!   * `mvn test` / `mvn verify` / `mvn integration-test` — run Maven, parse
//!     `target/surefire-reports/*.xml` (and `failsafe-reports/*.xml` for IT),
//!     emit a single-header summary + one line per failure/error.
//!   * Other phases (`compile`, `package`, `clean`, `install`, …) — fall back to
//!     a line-streaming filter that strips Maven's `[INFO]` chatter and keeps
//!     `[ERROR]` / `BUILD …` / timing lines.
//!   * `-X`, `--debug`, `--verbose` — passthrough raw output.
//!
//! Mirrors `gradlew_cmd::run`'s contract: returns the underlying mvn exit code,
//! tee's raw output on failure for re-reading, fail-soft on any parser error.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use regex::Regex;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

// ── Task detection ───────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum MvnTask {
    /// Includes the test or verify lifecycle phases — Surefire/Failsafe XML
    /// reports are expected on disk afterwards.
    Test,
    /// Build-only phases (compile / package / clean / install without test).
    Build,
    /// Unknown phase — passthrough.
    Other,
}

fn detect_task(args: &[String]) -> MvnTask {
    // Walk args left-to-right (skipping flags) and pick the most "powerful" phase
    // seen. Maven invocations are usually short; this is O(n) once.
    let mut has_test = false;
    let mut has_build = false;
    let mut has_other = false;

    for a in args.iter().filter(|a| !a.starts_with('-')) {
        let lower = a.to_lowercase();
        // Plugin goals like "failsafe:integration-test" or "surefire:test"
        let phase = lower.rsplit(':').next().unwrap_or(&lower);

        match phase {
            "test" | "verify" | "integration-test" => has_test = true,
            "compile" | "package" | "clean" | "install" => has_build = true,
            // Skipping the test lifecycle still produces no Surefire reports.
            _ => has_other = true,
        }
    }

    if has_test {
        MvnTask::Test
    } else if has_build {
        MvnTask::Build
    } else if has_other {
        MvnTask::Other
    } else {
        // Bare `mvn` with no phases — treat as Other (passthrough).
        MvnTask::Other
    }
}

// ── Binary resolution: mvnw > mvn ────────────────────────────────────────────

fn mvn_binary() -> &'static str {
    if cfg!(windows) {
        if Path::new(".\\mvnw.cmd").exists() {
            ".\\mvnw.cmd"
        } else {
            "mvn"
        }
    } else if Path::new("./mvnw").exists() {
        "./mvnw"
    } else {
        "mvn"
    }
}

fn new_mvn_command(args: &[String]) -> Command {
    let mut cmd = if cfg!(windows) {
        if Path::new(".\\mvnw.cmd").exists() {
            Command::new(".\\mvnw.cmd")
        } else {
            resolved_command("mvn")
        }
    } else if Path::new("./mvnw").exists() {
        Command::new("./mvnw")
    } else {
        resolved_command("mvn")
    };
    cmd.args(args);
    cmd
}

// ── Public entry point ───────────────────────────────────────────────────────

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    // Verbose flags bypass filtering — user wants the full firehose.
    if args
        .iter()
        .any(|a| a == "-X" || a == "--debug" || a == "--verbose" || a == "-e" || a == "--errors")
    {
        let osargs: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough(mvn_binary(), &osargs, verbose);
    }

    let cmd = new_mvn_command(args);
    let args_display = args.join(" ");
    let tool = mvn_binary();

    match detect_task(args) {
        MvnTask::Test => {
            // Run mvn fully; then post-process by reading Surefire/Failsafe XML
            // from disk. We use run_filtered to capture the raw stdout for
            // fallback display, then layer the XML summary on top.
            runner::run_filtered(
                cmd,
                tool,
                &args_display,
                |raw| filter_test_output(raw, Path::new(".")),
                RunOptions::with_tee("mvn_test"),
            )
        }
        MvnTask::Build => runner::run_filtered(
            cmd,
            tool,
            &args_display,
            filter_build_output,
            RunOptions::with_tee("mvn_build"),
        ),
        MvnTask::Other => {
            let osargs: Vec<OsString> = args.iter().map(OsString::from).collect();
            runner::run_passthrough(mvn_binary(), &osargs, verbose)
        }
    }
}

// ── Build-phase line filter ──────────────────────────────────────────────────

lazy_static! {
    /// Strip noisy lines emitted on every Maven run.
    static ref INFO_NOISE: Regex = Regex::new(
        r"^\[INFO\]\s*(---|Building\s|Downloading\s|Downloaded\s|Installing\s|\s*$)"
    ).unwrap();
    static ref DOWNLOAD_LEGACY: Regex = Regex::new(r"^(Downloading|Downloaded|Progress)\b").unwrap();
    /// Keep these for diagnostics.
    static ref KEEP: Regex = Regex::new(
        r"^\[(ERROR|WARNING)\]|^BUILD\s+(SUCCESS|FAILURE)|^\[INFO\]\s+BUILD\s+(SUCCESS|FAILURE)|^\[INFO\]\s+Total time|^\[INFO\]\s+Finished at"
    ).unwrap();
}

/// Predicate version, exposed for streaming/line tests.
fn filter_build_line(line: &str) -> bool {
    if INFO_NOISE.is_match(line) || DOWNLOAD_LEGACY.is_match(line) || line.trim().is_empty() {
        return false;
    }
    if KEEP.is_match(line) {
        return true;
    }
    // Default: drop other [INFO] lines, keep everything else (compile errors etc.)
    !line.starts_with("[INFO]")
}

fn filter_build_output(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() / 4);
    for line in raw.lines() {
        if filter_build_line(line) {
            out.push_str(line);
            out.push('\n');
        }
    }
    if out.is_empty() {
        "mvn: ok\n".to_string()
    } else {
        out
    }
}

// ── Test-phase Surefire/Failsafe XML parser ──────────────────────────────────

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct SuiteTotals {
    pub tests: usize,
    pub failures: usize,
    pub errors: usize,
    pub skipped: usize,
}

impl SuiteTotals {
    fn ok(&self) -> usize {
        self.tests
            .saturating_sub(self.failures)
            .saturating_sub(self.errors)
            .saturating_sub(self.skipped)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FailedCase {
    pub classname: String,
    pub name: String,
    /// "FAIL" (assertion failure) or "ERR" (test errored / threw).
    pub kind: &'static str,
    pub message: String,
}

/// Discover Surefire + Failsafe XML reports under `root`, walking module subdirs.
fn collect_report_paths(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !root.exists() {
        return found;
    }
    // walkdir for portability and proper handling of symlinks (already a dep).
    for entry in walkdir::WalkDir::new(root)
        .max_depth(8)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let parent = path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());
        if matches!(parent, Some("surefire-reports") | Some("failsafe-reports")) {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("TEST-") && name.ends_with(".xml") {
                    found.push(path.to_path_buf());
                }
            }
        }
    }
    found
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|b| *b == b':').next().unwrap_or(name)
}

fn attr_value(reader: &Reader<&[u8]>, start: &BytesStart<'_>, key: &[u8]) -> Option<String> {
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

fn attr_usize(reader: &Reader<&[u8]>, start: &BytesStart<'_>, key: &[u8]) -> usize {
    attr_value(reader, start, key)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0)
}

/// Parse one Surefire / Failsafe XML report.
///
/// Returns `(suite_totals, failed_cases)`. Best-effort: malformed XML yields
/// zero-valued totals and no cases, the caller skips silently.
pub(crate) fn parse_report(content: &str) -> (SuiteTotals, Vec<FailedCase>) {
    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut totals = SuiteTotals::default();
    let mut cases: Vec<FailedCase> = Vec::new();

    // Active testcase context — populated on <testcase>, drained on </testcase>
    let mut current_class = String::new();
    let mut current_name = String::new();
    let mut current_kind: Option<&'static str> = None;
    let mut current_message = String::new();
    let mut in_failure_text = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                match local_name(e.name().as_ref()) {
                    // First testsuite element wins for totals; some reports
                    // wrap multiple suites but each XML file is generated
                    // per-class so totals are stable.
                    b"testsuite" if totals.tests == 0 => {
                        totals.tests = attr_usize(&reader, &e, b"tests");
                        totals.failures = attr_usize(&reader, &e, b"failures");
                        totals.errors = attr_usize(&reader, &e, b"errors");
                        totals.skipped = attr_usize(&reader, &e, b"skipped");
                    }
                    b"testcase" => {
                        current_class = attr_value(&reader, &e, b"classname").unwrap_or_default();
                        current_name = attr_value(&reader, &e, b"name").unwrap_or_default();
                        current_kind = None;
                        current_message.clear();
                    }
                    b"failure" => {
                        current_kind = Some("FAIL");
                        if let Some(msg) = attr_value(&reader, &e, b"message") {
                            current_message = msg;
                        }
                        in_failure_text = true;
                    }
                    b"error" => {
                        current_kind = Some("ERR");
                        if let Some(msg) = attr_value(&reader, &e, b"message") {
                            current_message = msg;
                        }
                        in_failure_text = true;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) if in_failure_text && current_message.is_empty() => {
                if let Ok(s) = t.unescape() {
                    // First line of the stack/text only — keeps output dense.
                    let first = s.lines().next().unwrap_or("").trim();
                    if !first.is_empty() {
                        current_message.push_str(first);
                    }
                }
            }
            Ok(Event::End(e)) => match local_name(e.name().as_ref()) {
                b"failure" | b"error" => {
                    in_failure_text = false;
                }
                b"testcase" => {
                    if let Some(kind) = current_kind.take() {
                        let snippet = truncate(&current_message, 80);
                        cases.push(FailedCase {
                            classname: std::mem::take(&mut current_class),
                            name: std::mem::take(&mut current_name),
                            kind,
                            message: snippet,
                        });
                    } else {
                        current_class.clear();
                        current_name.clear();
                    }
                    current_message.clear();
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => break, // malformed → return what we have
            _ => {}
        }
        buf.clear();
    }

    (totals, cases)
}

fn truncate(s: &str, max: usize) -> String {
    let cleaned: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if cleaned.chars().count() <= max {
        cleaned
    } else {
        let truncated: String = cleaned.chars().take(max).collect();
        format!("{truncated}…")
    }
}

/// Build the final compact summary by aggregating all reports under `cwd`.
/// `raw` is the captured stdout/stderr from `mvn` — falls back to a slim subset
/// of it (BUILD … lines + last 20 lines) when no reports exist (e.g. a compile
/// failure before tests ran).
fn filter_test_output(raw: &str, cwd: &Path) -> String {
    let reports = collect_report_paths(cwd);

    if reports.is_empty() {
        // No surefire reports → compile failed or no tests configured.
        // Fall back to the build filter + tail of stderr/stdout.
        let trimmed = filter_build_output(raw);
        return trimmed;
    }

    let mut agg = SuiteTotals::default();
    let mut all_cases: Vec<FailedCase> = Vec::new();

    for path in &reports {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let (totals, cases) = parse_report(&content);
        agg.tests += totals.tests;
        agg.failures += totals.failures;
        agg.errors += totals.errors;
        agg.skipped += totals.skipped;
        all_cases.extend(cases);
    }

    format_summary(&agg, &all_cases, raw)
}

/// Public for tests — formats the rolled-up summary block.
pub(crate) fn format_summary(totals: &SuiteTotals, cases: &[FailedCase], raw: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "TESTS: {} OK={} FAIL={} ERR={} SKIP={}\n",
        totals.tests,
        totals.ok(),
        totals.failures,
        totals.errors,
        totals.skipped,
    ));

    for c in cases {
        out.push_str(&format!(
            "{}: {}.{} — {}\n",
            c.kind, c.classname, c.name, c.message
        ));
    }

    // Surface BUILD SUCCESS/FAILURE + Total time if present.
    for line in raw.lines() {
        let l = line.trim_start_matches("[INFO] ").trim_start_matches("[ERROR] ");
        if l.starts_with("BUILD SUCCESS")
            || l.starts_with("BUILD FAILURE")
            || l.starts_with("Total time")
        {
            out.push_str(l);
            out.push('\n');
        }
    }

    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn detect_task_recognises_test_and_verify() {
        assert_eq!(detect_task(&["test".into()]), MvnTask::Test);
        assert_eq!(detect_task(&["verify".into()]), MvnTask::Test);
        assert_eq!(detect_task(&["clean".into(), "test".into()]), MvnTask::Test);
        assert_eq!(
            detect_task(&["failsafe:integration-test".into()]),
            MvnTask::Test
        );
        assert_eq!(detect_task(&["package".into()]), MvnTask::Build);
        assert_eq!(detect_task(&["compile".into()]), MvnTask::Build);
        assert_eq!(detect_task(&["dependency:tree".into()]), MvnTask::Other);
    }

    #[test]
    fn detect_task_skips_flags() {
        assert_eq!(
            detect_task(&["-Dtest=Foo".into(), "test".into()]),
            MvnTask::Test
        );
        assert_eq!(
            detect_task(&["-pl".into(), "moduleA".into(), "test".into()]),
            MvnTask::Test
        );
    }

    #[test]
    fn parse_all_pass_report() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="com.example.AllPass" tests="3" failures="0" errors="0" skipped="0" time="0.05">
  <testcase classname="com.example.AllPass" name="t1" time="0.01"/>
  <testcase classname="com.example.AllPass" name="t2" time="0.02"/>
  <testcase classname="com.example.AllPass" name="t3" time="0.02"/>
</testsuite>"#;
        let (totals, cases) = parse_report(xml);
        assert_eq!(totals.tests, 3);
        assert_eq!(totals.failures, 0);
        assert_eq!(totals.errors, 0);
        assert_eq!(totals.skipped, 0);
        assert_eq!(cases.len(), 0);
    }

    #[test]
    fn parse_one_failure_report() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="com.example.OneFail" tests="2" failures="1" errors="0" skipped="0" time="0.05">
  <testcase classname="com.example.OneFail" name="passes" time="0.01"/>
  <testcase classname="com.example.OneFail" name="fails" time="0.02">
    <failure type="java.lang.AssertionError" message="expected:&lt;42&gt; but was:&lt;41&gt;">
java.lang.AssertionError: expected:&lt;42&gt; but was:&lt;41&gt;
        at com.example.OneFail.fails(OneFail.java:12)
    </failure>
  </testcase>
</testsuite>"#;
        let (totals, cases) = parse_report(xml);
        assert_eq!(totals.tests, 2);
        assert_eq!(totals.failures, 1);
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].classname, "com.example.OneFail");
        assert_eq!(cases[0].name, "fails");
        assert_eq!(cases[0].kind, "FAIL");
        assert!(
            cases[0].message.contains("expected:") && cases[0].message.contains("41"),
            "got: {}",
            cases[0].message
        );
    }

    #[test]
    fn parse_one_error_report() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="com.example.OneErr" tests="1" failures="0" errors="1" skipped="0" time="0.05">
  <testcase classname="com.example.OneErr" name="throws" time="0.01">
    <error type="java.lang.RuntimeException" message="kaboom">
java.lang.RuntimeException: kaboom
        at com.example.OneErr.throws(OneErr.java:9)
    </error>
  </testcase>
</testsuite>"#;
        let (totals, cases) = parse_report(xml);
        assert_eq!(totals.errors, 1);
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].kind, "ERR");
        assert!(cases[0].message.contains("kaboom"));
    }

    #[test]
    fn parse_skipped_report() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="com.example.Skipped" tests="2" failures="0" errors="0" skipped="2" time="0.0">
  <testcase classname="com.example.Skipped" name="a" time="0">
    <skipped/>
  </testcase>
  <testcase classname="com.example.Skipped" name="b" time="0">
    <skipped/>
  </testcase>
</testsuite>"#;
        let (totals, cases) = parse_report(xml);
        assert_eq!(totals.tests, 2);
        assert_eq!(totals.skipped, 2);
        assert_eq!(cases.len(), 0);
    }

    #[test]
    fn parse_failsafe_it_report() {
        // Failsafe writes the same TEST-*.xml shape; the parser is format-agnostic.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="com.example.IT" tests="1" failures="0" errors="0" skipped="0" time="3.5">
  <testcase classname="com.example.IT" name="it_smoke" time="3.5"/>
</testsuite>"#;
        let (totals, _) = parse_report(xml);
        assert_eq!(totals.tests, 1);
    }

    #[test]
    fn format_summary_one_failure() {
        let totals = SuiteTotals {
            tests: 4,
            failures: 1,
            errors: 0,
            skipped: 0,
        };
        let cases = vec![FailedCase {
            classname: "com.example.A".into(),
            name: "bad".into(),
            kind: "FAIL",
            message: "expected 1 was 2".into(),
        }];
        let raw = "[INFO] BUILD FAILURE\n[INFO] Total time:  2.5 s\n";
        let s = format_summary(&totals, &cases, raw);
        assert!(s.starts_with("TESTS: 4 OK=3 FAIL=1 ERR=0 SKIP=0\n"));
        assert!(s.contains("FAIL: com.example.A.bad — expected 1 was 2"));
        assert!(s.contains("BUILD FAILURE"));
    }

    #[test]
    fn build_filter_strips_info_noise() {
        let raw = "[INFO] Building myapp 1.0\n[INFO] Downloading foo\n[INFO] \n[ERROR] /src/Main.java:[10,5] cannot find symbol\n[INFO] BUILD FAILURE\n[INFO] Total time: 2.5 s\n";
        let out = filter_build_output(raw);
        assert!(!out.contains("Downloading"));
        assert!(out.contains("[ERROR]"));
        assert!(out.contains("BUILD FAILURE"));
    }

    /// Aggregate token-savings sanity check on a representative noisy Maven build log.
    #[test]
    fn build_filter_meets_token_savings_target() {
        let raw = include_str!("../../../tests/fixtures/mvn_build_noisy_raw.txt");
        let filtered = filter_build_output(raw);
        let raw_tokens = count_tokens(raw) as f64;
        let filt_tokens = count_tokens(&filtered) as f64;
        let savings = 100.0 - (filt_tokens / raw_tokens * 100.0);
        assert!(
            savings >= 60.0,
            "mvn build filter savings {:.1}% < 60% (raw_tokens={}, filt_tokens={})",
            savings,
            raw_tokens,
            filt_tokens
        );
    }
}
