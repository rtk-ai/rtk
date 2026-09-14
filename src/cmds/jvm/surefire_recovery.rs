//! Post-run recovery of test failures from Surefire/Failsafe XML reports.
//!
//! Maven's stdout is not always a complete record of what failed: a forked-VM
//! crash, parallel forks interleaving output, `-q`, redirected test output
//! (`<redirectTestOutputToFile>`) or `-Dmaven.test.failure.ignore=true` can
//! hide per-test detail that `target/surefire-reports/TEST-*.xml` still
//! carries.
//!
//! This pass runs **after** the stdout filter and compares the XML-reported
//! failures against the filtered text. Failures the text already mentions are
//! left alone; only the ones stdout missed are appended, in `[ERROR]`-prefixed
//! native style. When stdout already tells the whole story — the common case —
//! nothing is appended and the filtered output stays byte-identical. Reports
//! older than the run's start are ignored (`parse_dir` mtime gate), so files
//! left by a previous run never leak in.

use crate::cmds::jvm::surefire_reports::{self, FailureKind, SurefireResult, TestFailure};
use crate::core::truncate::CAP_WARNINGS;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Cap on appended failure blocks — the same test-failure cap class as
/// `MAX_MVN_FAILING_CLASSES` in `mvn_cmd.rs`.
const MAX_RECOVERED_FAILURES: usize = CAP_WARNINGS;

/// `surefire-reports` / `failsafe-reports` under `cwd/target` and under each
/// depth-1 reactor module's `target`.
fn report_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut targets = vec![cwd.join("target")];
    if let Ok(entries) = std::fs::read_dir(cwd) {
        targets.extend(
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .map(|e| e.path().join("target")),
        );
    }
    targets.sort();
    targets
        .iter()
        .flat_map(|t| [t.join("surefire-reports"), t.join("failsafe-reports")])
        .filter(|d| d.is_dir())
        .collect()
}

/// Parse and merge every report dir. `None` when no fresh report was found.
fn collect(cwd: &Path, since: SystemTime) -> Option<SurefireResult> {
    let mut merged: Option<SurefireResult> = None;
    for dir in report_dirs(cwd) {
        if let Some(parsed) = surefire_reports::parse_dir(&dir, since) {
            merged
                .get_or_insert_with(SurefireResult::default)
                .merge(parsed);
        }
    }
    merged
}

/// A failure counts as already visible when the text mentions its
/// `Class.method`, either fully qualified (per-test lines:
/// `[ERROR] com.example.FooTest.bar -- Time elapsed …`) or by short class
/// name (failures summary: `[ERROR]   FooTest.bar:42 …`).
fn text_mentions(text: &str, f: &TestFailure) -> bool {
    if f.test_class.is_empty() || f.test_method.is_empty() {
        // Malformed entry — never treat it as missing.
        return true;
    }
    let short = f.test_class.rsplit('.').next().unwrap_or(&f.test_class);
    text.contains(&format!("{}.{}", f.test_class, f.test_method))
        || text.contains(&format!("{}.{}", short, f.test_method))
}

fn push_prefixed(out: &mut String, text: &str) {
    for line in text.lines() {
        out.push_str("[ERROR]     ");
        out.push_str(line);
        out.push('\n');
    }
}

fn render(missing: &[&TestFailure]) -> String {
    let mut out = format!(
        "[ERROR] {} test failure(s) found in Surefire/Failsafe XML reports but missing from build output:\n",
        missing.len()
    );
    for f in missing.iter().take(MAX_RECOVERED_FAILURES) {
        let marker = match f.kind {
            FailureKind::Failure => "<<< FAILURE!",
            FailureKind::Error => "<<< ERROR!",
        };
        out.push_str(&format!(
            "[ERROR]   {}.{} {}\n",
            f.test_class, f.test_method, marker
        ));
        match &f.stack_trace {
            // A Java trace opens with its own `Type: message` line.
            Some(trace) => push_prefixed(&mut out, trace),
            None => {
                let header = [f.failure_type.as_deref(), f.message.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(": ");
                push_prefixed(&mut out, &header);
            }
        }
    }
    if missing.len() > MAX_RECOVERED_FAILURES {
        out.push_str(&format!(
            "… +{} more recovered failures\n",
            missing.len() - MAX_RECOVERED_FAILURES
        ));
    }
    out
}

/// Compare fresh XML reports against the filtered stdout and return the text
/// with the failures stdout missed appended. `None` when there is nothing to
/// add: no fresh reports, no failures, or every failure already visible.
pub(crate) fn recover_missing_failures(
    filtered: &str,
    cwd: &Path,
    since: SystemTime,
) -> Option<String> {
    let reports = collect(cwd, since)?;
    let missing: Vec<&TestFailure> = reports
        .failures
        .iter()
        .filter(|f| !text_mentions(filtered, f))
        .collect();
    if missing.is_empty() {
        return None;
    }

    let mut out = filtered.trim_end_matches('\n').to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&render(&missing));

    // Blind stdout (forked-VM crash, `-q`, truncation): no aggregate
    // `Tests run:` line survived filtering — restore it from the XML.
    if !filtered.contains("Tests run:") {
        let s = &reports.summary;
        out.push_str(&format!(
            "[ERROR] Tests run: {}, Failures: {}, Errors: {}, Skipped: {} (from XML reports)\n",
            s.run, s.failures, s.errors, s.skipped
        ));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/java/surefire-reports"
    );

    /// Fabricate a project dir holding fresh copies of surefire fixtures.
    fn project_with_reports(fixtures: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("target/surefire-reports");
        std::fs::create_dir_all(&dir).unwrap();
        for name in fixtures {
            std::fs::copy(Path::new(FIXTURES).join(name), dir.join(name)).expect("copy fixture");
        }
        tmp
    }

    fn recover(text: &str, cwd: &Path) -> Option<String> {
        recover_missing_failures(text, cwd, SystemTime::UNIX_EPOCH)
    }

    #[test]
    fn nothing_to_add_when_no_reports_exist() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(recover("[INFO] BUILD FAILURE\n", tmp.path()), None);
    }

    #[test]
    fn nothing_to_add_when_stdout_already_covers_failures() {
        let tmp = project_with_reports(&["TEST-com.example.FailingTest.xml"]);
        let text = "[ERROR] com.example.FailingTest.shouldReturnUser -- Time elapsed: 0.1 s <<< FAILURE!\n\
                    [ERROR] com.example.FailingTest.shouldHandleNull -- Time elapsed: 0.1 s <<< FAILURE!\n\
                    [ERROR] Tests run: 7, Failures: 2, Errors: 0, Skipped: 0\n\
                    [INFO] BUILD FAILURE\n";
        assert_eq!(recover(text, tmp.path()), None);
    }

    #[test]
    fn short_class_summary_mention_counts_as_covered() {
        let tmp = project_with_reports(&["TEST-com.example.FailingTest.xml"]);
        // Failures-summary style: short class name + method, no FQN anywhere.
        let text = "[ERROR] Failures:\n\
                    [ERROR]   FailingTest.shouldReturnUser:42 expected:<200> but was:<404>\n\
                    [ERROR]   FailingTest.shouldHandleNull:55 Unexpected exception\n\
                    [ERROR] Tests run: 7, Failures: 2, Errors: 0, Skipped: 0\n\
                    [INFO] BUILD FAILURE\n";
        assert_eq!(recover(text, tmp.path()), None);
    }

    #[test]
    fn appends_failures_missing_from_stdout() {
        let tmp = project_with_reports(&["TEST-com.example.FailingTest.xml"]);
        // Blind stdout: footer only (forked-VM crash or -q).
        let out = recover("[INFO] BUILD FAILURE\n", tmp.path()).expect("appended");
        let expected = "[INFO] BUILD FAILURE\n\
            \n\
            [ERROR] 2 test failure(s) found in Surefire/Failsafe XML reports but missing from build output:\n\
            [ERROR]   com.example.FailingTest.shouldReturnUser <<< FAILURE!\n\
            [ERROR]     org.opentest4j.AssertionFailedError: expected:<200> but was:<404>\n\
            [ERROR]     \tat org.junit.jupiter.api.AssertEquals.assertEquals(AssertEquals.java:150)\n\
            [ERROR]     \tat com.example.FailingTest.shouldReturnUser(FailingTest.java:42)\n\
            [ERROR]     \tat java.base/java.lang.reflect.Method.invoke(Method.java:580)\n\
            [ERROR]   com.example.FailingTest.shouldHandleNull <<< FAILURE!\n\
            [ERROR]     java.lang.AssertionError: Unexpected exception: NullPointerException\n\
            [ERROR]     \tat com.example.FailingTest.shouldHandleNull(FailingTest.java:55)\n\
            [ERROR]     \tat java.base/java.lang.reflect.Method.invoke(Method.java:580)\n\
            [ERROR] Tests run: 4, Failures: 2, Errors: 0, Skipped: 0 (from XML reports)\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn partial_coverage_appends_only_missing() {
        let tmp = project_with_reports(&["TEST-com.example.FailingTest.xml"]);
        // stdout carried the first failure but lost the second (interleave).
        let text = "[ERROR] com.example.FailingTest.shouldReturnUser -- Time elapsed: 0.1 s <<< FAILURE!\n\
                    [ERROR] Tests run: 7, Failures: 2, Errors: 0, Skipped: 0\n\
                    [INFO] BUILD FAILURE\n";
        let out = recover(text, tmp.path()).expect("appended");
        assert!(out.starts_with(text.trim_end()), "{out}");
        assert!(out.contains("[ERROR] 1 test failure(s) found"), "{out}");
        assert!(
            out.contains("[ERROR]   com.example.FailingTest.shouldHandleNull <<< FAILURE!"),
            "{out}"
        );
        // The covered one is not re-rendered in the recovery block.
        assert_eq!(out.matches("shouldReturnUser").count(), 1, "{out}");
        // Aggregate not duplicated — stdout already had one.
        assert!(!out.contains("(from XML reports)"), "{out}");
    }

    #[test]
    fn error_kind_uses_error_marker_and_keeps_trace() {
        let tmp = project_with_reports(&["TEST-com.example.ErrorTest.xml"]);
        let out = recover("[INFO] BUILD FAILURE\n", tmp.path()).expect("appended");
        assert!(
            out.contains("[ERROR]   com.example.ErrorTest.shouldNotThrow <<< ERROR!\n"),
            "{out}"
        );
        assert!(
            out.contains("[ERROR]     java.net.ConnectException: Connection refused\n"),
            "{out}"
        );
        assert!(
            out.contains("Tests run: 2, Failures: 0, Errors: 1, Skipped: 0 (from XML reports)"),
            "{out}"
        );
    }

    #[test]
    fn stale_reports_are_ignored() {
        let tmp = project_with_reports(&["TEST-com.example.FailingTest.xml"]);
        // `since` in the future -> every report is stale.
        let future = SystemTime::now() + std::time::Duration::from_secs(3600);
        assert_eq!(
            recover_missing_failures("[INFO] BUILD FAILURE\n", tmp.path(), future),
            None
        );
    }

    #[test]
    fn discovers_reactor_module_and_failsafe_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("service-a/target/failsafe-reports");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(
            Path::new(FIXTURES).join("TEST-com.example.FailingTest.xml"),
            dir.join("TEST-com.example.FailingTest.xml"),
        )
        .unwrap();

        let out = recover("[INFO] BUILD FAILURE\n", tmp.path()).expect("appended");
        assert!(
            out.contains("com.example.FailingTest.shouldReturnUser"),
            "module-level failsafe reports discovered: {out}"
        );
    }

    #[test]
    fn passing_run_with_reports_is_untouched() {
        let tmp = project_with_reports(&["TEST-com.example.PassingTest.xml"]);
        assert_eq!(recover("[INFO] BUILD SUCCESS\n", tmp.path()), None);
    }

    #[test]
    fn cap_limits_rendered_blocks_and_counts_rest() {
        let failures: Vec<TestFailure> = (0..CAP_WARNINGS + 3)
            .map(|i| TestFailure {
                test_class: format!("com.example.T{i}"),
                test_method: "m".into(),
                kind: FailureKind::Failure,
                message: Some("boom".into()),
                failure_type: Some("java.lang.AssertionError".into()),
                stack_trace: None,
            })
            .collect();
        let refs: Vec<&TestFailure> = failures.iter().collect();
        let out = render(&refs);
        assert_eq!(out.matches("<<< FAILURE!").count(), CAP_WARNINGS);
        // No trace body: the header is built from the attributes instead.
        assert!(
            out.contains("[ERROR]     java.lang.AssertionError: boom\n"),
            "{out}"
        );
        assert!(out.contains("… +3 more recovered failures"), "{out}");
    }
}
