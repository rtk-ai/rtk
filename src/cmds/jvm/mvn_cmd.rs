//! Apache Maven filter — Surefire/Failsafe block collapse, compile error/warning
//! dedup, package/install pipeline with mode-toggle.
//!
//! Replaces the previous `src/filters/mvn-build.toml` filter with a Rust module
//! capable of state-machine parsing (block collapse, continuation tracking,
//! mode toggle) that TOML DSL cannot express.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

// ── Shared regex patterns ────────────────────────────────────────────────────

lazy_static! {
    /// `[INFO] Running com.example.app.FooTest`
    static ref RUNNING: Regex = Regex::new(r"^\[INFO\] Running ").unwrap();

    /// Surefire/Failsafe per-class close line. Captures `Failures` and `Errors`.
    /// Tolerates the optional `<<< FAILURE!` marker between duration and ` - in `.
    static ref CLOSE: Regex = Regex::new(
        r"^\[(?:INFO|ERROR)\] Tests run: \d+, Failures: (\d+), Errors: (\d+), Skipped: \d+, Time elapsed: [^ ]+ s(?:\s+<<<\s*FAILURE!)? - in (.+)$"
    ).unwrap();

    /// Final BUILD footer.
    static ref BUILD_FOOT: Regex = Regex::new(r"^\[(?:INFO|ERROR)\] BUILD (?:SUCCESS|FAILURE)$").unwrap();

    /// `[INFO] Results:` separator before the aggregate.
    static ref RESULTS: Regex = Regex::new(r"^\[INFO\] Results:\s*$").unwrap();

    /// Aggregate counts line (no `Time elapsed`, no ` - in `).
    static ref AGG: Regex = Regex::new(
        r"^\[(?:INFO|ERROR)\] Tests run: \d+, Failures: \d+, Errors: \d+, Skipped: \d+\s*$"
    ).unwrap();

    /// Plugin banner line: `[INFO] --- plugin:goal (id) @ module ---`.
    static ref PLUGIN_BANNER: Regex = Regex::new(r"^\[INFO\] --- .* @ .* ---$").unwrap();

    /// Module banner with project name in brackets.
    static ref MODULE_BANNER: Regex = Regex::new(r"^\[INFO\] -+< .+ >-+$").unwrap();

    /// Duration normaliser for deterministic snapshots.
    static ref TIME_DURATION: Regex = Regex::new(r"Time elapsed: [0-9.]+ s").unwrap();

    /// Total time line — also normalised.
    static ref TOTAL_TIME: Regex = Regex::new(r"^\[INFO\] Total time:\s+[0-9.]+ s").unwrap();

    /// Compile-error coordinate substring to strip when deduping warnings/errors.
    static ref FILE_COORD: Regex = Regex::new(r"/[^:]+\.java:\[\d+,\d+\]").unwrap();
}

// ── Phase detection ─────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum MvnPhase {
    Test,        // test, integration-test (Failsafe = Surefire shape)
    Compile,     // compile, test-compile
    Package,     // package, install, verify, deploy
    Passthrough, // clean, site, plugin goals, version/help, empty
}

/// Scan args left-to-right, skip flags + `-D…` system props, pick the LAST
/// remaining token. If empty, plugin-form (`:`), or `clean`/`site` → Passthrough.
pub fn detect_phase(args: &[String]) -> MvnPhase {
    let last = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .next_back()
        .unwrap_or("");

    if last.is_empty() || last.contains(':') {
        return MvnPhase::Passthrough;
    }
    match last {
        "clean" | "site" | "site-deploy" => MvnPhase::Passthrough,
        "test" | "integration-test" => MvnPhase::Test,
        "compile" | "test-compile" => MvnPhase::Compile,
        "package" | "install" | "verify" | "deploy" => MvnPhase::Package,
        _ => MvnPhase::Passthrough,
    }
}

// ── Stack-frame deny-list ────────────────────────────────────────────────────

const FRAMEWORK_FRAME_PREFIXES: &[&str] = &[
    "at org.junit.",
    "at junit.",
    "at org.apache.maven.surefire.",
    "at sun.reflect.",
    "at jdk.internal.reflect.",
    "at jdk.proxy",
    "at java.base/",
    "at java.lang.reflect.",
    "at java.util.",
];

fn is_framework_frame(trimmed: &str) -> bool {
    FRAMEWORK_FRAME_PREFIXES
        .iter()
        .any(|p| trimmed.starts_with(p))
}

// ── English-footer guard ────────────────────────────────────────────────────

fn has_english_footer(stripped: &str) -> bool {
    stripped.lines().any(|l| {
        let t = l.trim();
        t.ends_with(" BUILD SUCCESS") || t.ends_with(" BUILD FAILURE")
    })
}

// ── Duration normaliser ─────────────────────────────────────────────────────

fn normalise(s: &str) -> String {
    let s = TIME_DURATION.replace_all(s, "Time elapsed: T s").into_owned();
    TOTAL_TIME
        .replace_all(&s, "[INFO] Total time: T s")
        .into_owned()
}

// ── Outside-block keep list (shared by surefire + package) ──────────────────

fn keep_outside_block(line: &str) -> bool {
    RESULTS.is_match(line)
        || AGG.is_match(line)
        || BUILD_FOOT.is_match(line)
        || MODULE_BANNER.is_match(line)
        || line.starts_with("[INFO] Total time:")
        || line.starts_with("[INFO] Finished at:")
        || line.starts_with("[INFO] Building ")
        || line.starts_with("[INFO] Scanning ")
        || line.starts_with("[INFO] Installing ")
        || line.starts_with("[ERROR] Failures:")
        || line.starts_with("[ERROR] Errors:")
        || (line.starts_with("[ERROR]") && !line.starts_with("[ERROR] Tests run:"))
        || line.starts_with("[INFO] Building war:")
        || line.starts_with("[INFO] Building jar:")
        || line.starts_with("[INFO] Building ear:")
}

// ── Surefire block filter ───────────────────────────────────────────────────

/// Buffered single-pass filter for `mvn test` / `mvn integration-test`.
///
/// State machine: when a `[INFO] Running <FQN>` line is seen, start buffering
/// the block. When the close line arrives, decide:
/// - Failures == 0 && Errors == 0 → drop the block silently.
/// - Else → emit the block with framework frames stripped, durations normalised.
///
/// English-footer guard: if no `BUILD SUCCESS`/`BUILD FAILURE` line is present,
/// return the ANSI-stripped raw input (non-English locale or truncated output).
pub fn filter_surefire(raw: &str) -> String {
    let stripped = strip_ansi(raw);
    if !has_english_footer(&stripped) {
        return stripped;
    }

    let mut out = String::new();
    let mut block_lines: Vec<String> = Vec::new();
    let mut block_running: Option<String> = None;
    let mut in_block = false;

    for line in stripped.lines() {
        if PLUGIN_BANNER.is_match(line) {
            continue;
        }

        if RUNNING.is_match(line) {
            if in_block {
                flush_block_as_keep(&mut out, &block_running, &block_lines);
            }
            block_lines.clear();
            block_running = Some(line.to_string());
            in_block = true;
            continue;
        }

        if in_block {
            if let Some(caps) = CLOSE.captures(line) {
                let fail = caps.get(1).map(|m| m.as_str() != "0").unwrap_or(false);
                let err = caps.get(2).map(|m| m.as_str() != "0").unwrap_or(false);
                if fail || err {
                    emit_block(&mut out, &block_running, &block_lines);
                    out.push_str(&normalise(line));
                    out.push('\n');
                }
                block_lines.clear();
                block_running = None;
                in_block = false;
                continue;
            }
            block_lines.push(line.to_string());
            continue;
        }

        if keep_outside_block(line) {
            out.push_str(&normalise(line));
            out.push('\n');
        }
    }

    if in_block {
        flush_block_as_keep(&mut out, &block_running, &block_lines);
    }
    out
}

fn flush_block_as_keep(out: &mut String, running: &Option<String>, lines: &[String]) {
    if let Some(r) = running {
        out.push_str(&normalise(r));
        out.push('\n');
    }
    for l in lines {
        out.push_str(&normalise(l));
        out.push('\n');
    }
}

fn emit_block(out: &mut String, running: &Option<String>, lines: &[String]) {
    if let Some(r) = running {
        out.push_str(&normalise(r));
        out.push('\n');
    }
    for l in lines {
        let t = l.trim_start();
        if t.starts_with("at ") && is_framework_frame(t) {
            continue;
        }
        out.push_str(&normalise(l));
        out.push('\n');
    }
}

// ── Compile filter ──────────────────────────────────────────────────────────

/// Buffered single-pass filter for `mvn compile` / `test-compile`.
///
/// Keeps module banners, `[INFO] Building …`, `[INFO] BUILD …`, totals, finish
/// time, scanning line, install lines, and `[ERROR]` blocks with indented
/// continuation (`  symbol:`, `  ^`, `  required:`). Deduplicates `[WARNING]`
/// lines by normalised message (strip file coordinates).
pub fn filter_compile(raw: &str) -> String {
    let stripped = strip_ansi(raw);
    if !has_english_footer(&stripped) {
        return stripped;
    }

    let mut out = String::new();
    let mut keep_continuation = false;
    let mut seen_warnings: HashSet<String> = HashSet::new();

    for line in stripped.lines() {
        if MODULE_BANNER.is_match(line) {
            out.push_str(line);
            out.push('\n');
            keep_continuation = false;
            continue;
        }
        if BUILD_FOOT.is_match(line)
            || line.starts_with("[INFO] Building ")
            || line.starts_with("[INFO] Total time:")
            || line.starts_with("[INFO] Finished at:")
            || line.starts_with("[INFO] Scanning ")
        {
            out.push_str(&normalise(line));
            out.push('\n');
            keep_continuation = false;
            continue;
        }
        if line.starts_with("[ERROR]") {
            out.push_str(line);
            out.push('\n');
            keep_continuation = true;
            continue;
        }
        if keep_continuation && (line.starts_with(' ') || line.starts_with('\t')) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if line.starts_with("[WARNING]") {
            let norm = FILE_COORD
                .replace_all(&line["[WARNING] ".len().min(line.len())..], "")
                .to_string();
            if seen_warnings.insert(norm) {
                out.push_str(line);
                out.push('\n');
            }
            keep_continuation = false;
            continue;
        }
        // Drop everything else
        keep_continuation = false;
    }

    out
}

// ── Package filter ──────────────────────────────────────────────────────────

/// Buffered single-pass filter for `mvn package`/`install`/`verify`/`deploy`.
///
/// Mode toggle: starts in `Compile` mode, switches to `Surefire` when a
/// `[INFO] Running …` line is seen, switches back on `Tests run:` close.
/// Outside any Surefire block, applies the unified keep-list (compile keepers
/// + install/artifact lines).
pub fn filter_package(raw: &str) -> String {
    let stripped = strip_ansi(raw);
    if !has_english_footer(&stripped) {
        return stripped;
    }

    let mut out = String::new();
    let mut block_lines: Vec<String> = Vec::new();
    let mut block_running: Option<String> = None;
    let mut in_block = false;
    let mut keep_continuation = false;
    let mut seen_warnings: HashSet<String> = HashSet::new();

    for line in stripped.lines() {
        if PLUGIN_BANNER.is_match(line) {
            continue;
        }

        if RUNNING.is_match(line) {
            if in_block {
                flush_block_as_keep(&mut out, &block_running, &block_lines);
            }
            block_lines.clear();
            block_running = Some(line.to_string());
            in_block = true;
            keep_continuation = false;
            continue;
        }

        if in_block {
            if let Some(caps) = CLOSE.captures(line) {
                let fail = caps.get(1).map(|m| m.as_str() != "0").unwrap_or(false);
                let err = caps.get(2).map(|m| m.as_str() != "0").unwrap_or(false);
                if fail || err {
                    emit_block(&mut out, &block_running, &block_lines);
                    out.push_str(&normalise(line));
                    out.push('\n');
                }
                block_lines.clear();
                block_running = None;
                in_block = false;
                continue;
            }
            block_lines.push(line.to_string());
            continue;
        }

        // Outside any Surefire block: compile-keep AND surefire-outside-keep merge.
        if MODULE_BANNER.is_match(line) || keep_outside_block(line) {
            out.push_str(&normalise(line));
            out.push('\n');
            keep_continuation = line.starts_with("[ERROR]")
                && !line.starts_with("[ERROR] Tests run:")
                && !line.starts_with("[ERROR] Failures:")
                && !line.starts_with("[ERROR] Errors:");
            continue;
        }
        if keep_continuation && (line.starts_with(' ') || line.starts_with('\t')) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if line.starts_with("[WARNING]") {
            let norm = FILE_COORD
                .replace_all(&line["[WARNING] ".len().min(line.len())..], "")
                .to_string();
            if seen_warnings.insert(norm) {
                out.push_str(line);
                out.push('\n');
            }
            keep_continuation = false;
            continue;
        }
        keep_continuation = false;
    }

    if in_block {
        flush_block_as_keep(&mut out, &block_running, &block_lines);
    }
    out
}

// ── Wrapper detection ───────────────────────────────────────────────────────

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

// ── Entry point ─────────────────────────────────────────────────────────────

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    // Verbose flags bypass filtering — user wants full output.
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "-X" | "--debug" | "-e" | "--errors"))
    {
        let osargs: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough(mvn_binary(), &osargs, verbose);
    }

    let phase = detect_phase(args);
    let tool = mvn_binary();
    let args_display = args.join(" ");

    match phase {
        MvnPhase::Test => runner::run_filtered(
            new_mvn_command(args),
            tool,
            &args_display,
            filter_surefire,
            RunOptions::with_tee("mvn_test"),
        ),
        MvnPhase::Compile => runner::run_filtered(
            new_mvn_command(args),
            tool,
            &args_display,
            filter_compile,
            RunOptions::with_tee("mvn_compile"),
        ),
        MvnPhase::Package => runner::run_filtered(
            new_mvn_command(args),
            tool,
            &args_display,
            filter_package,
            RunOptions::with_tee("mvn_package"),
        ),
        MvnPhase::Passthrough => {
            let osargs: Vec<OsString> = args.iter().map(OsString::from).collect();
            runner::run_passthrough(tool, &osargs, verbose)
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    fn gunzip(bytes: &[u8]) -> String {
        let mut s = String::new();
        GzDecoder::new(bytes)
            .read_to_string(&mut s)
            .expect("gunzip");
        s
    }

    fn s<S: Into<String>>(it: impl IntoIterator<Item = S>) -> Vec<String> {
        it.into_iter().map(Into::into).collect()
    }

    // ── Phase detection ──────────────────────────────────────────────────────

    #[test]
    fn phase_test() {
        assert_eq!(detect_phase(&s(["test"])), MvnPhase::Test);
    }
    #[test]
    fn phase_integration_test() {
        assert_eq!(detect_phase(&s(["integration-test"])), MvnPhase::Test);
    }
    #[test]
    fn phase_compile() {
        assert_eq!(detect_phase(&s(["compile"])), MvnPhase::Compile);
    }
    #[test]
    fn phase_test_compile() {
        assert_eq!(detect_phase(&s(["test-compile"])), MvnPhase::Compile);
    }
    #[test]
    fn phase_install() {
        assert_eq!(detect_phase(&s(["install"])), MvnPhase::Package);
    }
    #[test]
    fn phase_package() {
        assert_eq!(detect_phase(&s(["package"])), MvnPhase::Package);
    }
    #[test]
    fn phase_verify() {
        assert_eq!(detect_phase(&s(["verify"])), MvnPhase::Package);
    }
    #[test]
    fn phase_deploy() {
        assert_eq!(detect_phase(&s(["deploy"])), MvnPhase::Package);
    }
    #[test]
    fn phase_clean_install_is_pkg() {
        assert_eq!(detect_phase(&s(["clean", "install"])), MvnPhase::Package);
    }
    #[test]
    fn phase_flags_before_goal() {
        assert_eq!(
            detect_phase(&s(["-B", "-DskipTests", "test"])),
            MvnPhase::Test
        );
    }
    #[test]
    fn phase_clean_only_passthrough() {
        assert_eq!(detect_phase(&s(["clean"])), MvnPhase::Passthrough);
    }
    #[test]
    fn phase_site_passthrough() {
        assert_eq!(detect_phase(&s(["site"])), MvnPhase::Passthrough);
    }
    #[test]
    fn phase_plugin_goal_passthrough() {
        assert_eq!(
            detect_phase(&s(["dependency:tree"])),
            MvnPhase::Passthrough
        );
    }
    #[test]
    fn phase_empty_passthrough() {
        let v: Vec<String> = Vec::new();
        assert_eq!(detect_phase(&v), MvnPhase::Passthrough);
    }
    #[test]
    fn phase_version_long() {
        assert_eq!(detect_phase(&s(["--version"])), MvnPhase::Passthrough);
    }
    #[test]
    fn phase_version_short() {
        assert_eq!(detect_phase(&s(["-v"])), MvnPhase::Passthrough);
    }
    #[test]
    fn phase_version_java_style() {
        assert_eq!(detect_phase(&s(["-version"])), MvnPhase::Passthrough);
    }
    #[test]
    fn phase_help() {
        assert_eq!(detect_phase(&s(["--help"])), MvnPhase::Passthrough);
    }

    // ── Surefire filter ──────────────────────────────────────────────────────

    #[test]
    fn filter_surefire_pass_output_compact() {
        let i = include_str!("../../../tests/fixtures/mvn_test_pass_slice_raw.txt");
        let o = filter_surefire(i);
        // Passing fixture has 2 blocks; both should be dropped.
        assert!(!o.contains("FooService.execute"));
        assert!(!o.contains("ConsultarCidadeUseCaseTest"));
        // Output should be a small fraction of input.
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(i) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "pass-fixture savings >=60%, got {:.1}%",
            savings
        );
    }

    #[test]
    fn filter_surefire_fail_keeps_signal() {
        let i = include_str!("../../../tests/fixtures/mvn_test_fail_slice_raw.txt");
        let o = filter_surefire(i);
        assert!(o.contains("BUILD FAILURE"));
        assert!(o.contains("Failures: 1"));
    }

    #[test]
    fn surefire_drops_passing_block() {
        let i = include_str!("../../../tests/fixtures/mvn_test_pass_slice_raw.txt");
        let o = filter_surefire(i);
        assert!(
            !o.contains("at org.junit."),
            "framework frames stripped; got:\n{}",
            o
        );
        assert!(
            !o.contains("AppException: input must not be null"),
            "passing-test stack dropped; got:\n{}",
            o
        );
        assert!(
            o.contains("BUILD SUCCESS"),
            "footer preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("Tests run: 10, Failures: 0"),
            "aggregate preserved; got:\n{}",
            o
        );
    }

    #[test]
    fn surefire_preserves_failing_signal() {
        let i = include_str!("../../../tests/fixtures/mvn_test_fail_slice_raw.txt");
        let o = filter_surefire(i);
        assert!(
            o.contains("Failures: 1"),
            "failing aggregate preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("AssertionFailedError"),
            "exception class preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("at com.example."),
            "user-code frame preserved; got:\n{}",
            o
        );
        assert!(
            !o.contains("at org.junit."),
            "framework frames stripped in failing block; got:\n{}",
            o
        );
    }

    #[test]
    fn surefire_keeps_module_banner() {
        let i = "[INFO] Scanning for projects...\n[INFO] -----< com.example:myapp >-----\n[INFO] BUILD SUCCESS\n";
        let o = filter_surefire(i);
        assert!(o.contains("-----< com.example:myapp >-----"));
    }

    #[test]
    fn surefire_normalises_durations() {
        let i = "[INFO] -----< x >-----\n[INFO] Running x.Foo\n[ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 2.341 s <<< FAILURE! - in x.Foo\n[INFO] BUILD FAILURE\n";
        let o = filter_surefire(i);
        assert!(
            o.contains("Time elapsed: T s"),
            "duration normalised; got:\n{}",
            o
        );
        assert!(!o.contains("2.341"), "raw duration removed; got:\n{}", o);
    }

    #[test]
    fn footer_guard_french_passthrough() {
        let i = include_str!("../../../tests/fixtures/mvn_locale_fr_raw.txt");
        let o = filter_surefire(i);
        assert!(
            o.contains("BUILD ÉCHEC"),
            "footer-guard must pass through non-English output; got:\n{}",
            o
        );
        // Confirm we did NOT filter — input length preserved (modulo ANSI strip, which is a no-op here)
        assert_eq!(
            o.lines().count(),
            i.lines().count(),
            "footer-guard returns raw input"
        );
    }

    #[test]
    fn footer_guard_no_pom_passthrough() {
        let i = include_str!("../../../tests/fixtures/mvn_no_pom_raw.txt");
        let o = filter_surefire(i);
        // No BUILD footer → passthrough; user sees the `[ERROR] no POM` line.
        assert!(
            o.contains("there is no POM"),
            "no-pom error preserved; got:\n{}",
            o
        );
    }

    // ── Compile filter ───────────────────────────────────────────────────────

    #[test]
    fn filter_compile_error_compact() {
        let i = include_str!("../../../tests/fixtures/mvn_compile_error_slice_raw.txt");
        let o = filter_compile(i);
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(i) as f64 * 100.0);
        assert!(
            savings >= 30.0,
            "compile-error fixture is small; >=30% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn compile_preserves_error_continuation() {
        let i = include_str!("../../../tests/fixtures/mvn_compile_error_slice_raw.txt");
        let o = filter_compile(i);
        assert!(o.contains("cannot find symbol"), "ERROR line preserved");
        assert!(
            o.contains("symbol:   variable bar"),
            "indented continuation preserved"
        );
        assert!(o.contains("BUILD FAILURE"), "footer preserved");
    }

    #[test]
    fn compile_dedupes_warnings() {
        let i = "[INFO] -----< x >-----\n\
                 [WARNING] /a.java:[1,2] uses deprecated API\n\
                 [WARNING] /b.java:[3,4] uses deprecated API\n\
                 [WARNING] /a.java:[5,6] unchecked cast\n\
                 [INFO] BUILD SUCCESS\n";
        let o = filter_compile(i);
        let warns = o.matches("[WARNING]").count();
        assert_eq!(warns, 2, "dedup by normalised message; got:\n{}", o);
    }

    // ── Package filter ───────────────────────────────────────────────────────

    #[test]
    fn filter_package_install_compact() {
        let i = include_str!("../../../tests/fixtures/mvn_install_slice_raw.txt");
        let o = filter_package(i);
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(i) as f64 * 100.0);
        assert!(
            savings >= 50.0,
            "install-slice savings >=50%, got {:.1}%",
            savings
        );
    }

    #[test]
    fn package_keeps_install_lines() {
        let i = include_str!("../../../tests/fixtures/mvn_install_slice_raw.txt");
        let o = filter_package(i);
        assert!(
            o.contains("Installing"),
            "install line preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("Building jar:"),
            "jar line preserved; got:\n{}",
            o
        );
        assert!(
            !o.contains("at org.junit."),
            "framework frames stripped; got:\n{}",
            o
        );
    }

    // ── Token-savings (FULL fixtures, #[ignore]) ────────────────────────────

    #[test]
    #[ignore]
    fn savings_mvn_test_pass_full() {
        let bytes = include_bytes!("../../../tests/fixtures/mvn_test_pass_full_raw.txt.gz");
        let i = gunzip(bytes);
        let o = filter_surefire(&i);
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(&i) as f64 * 100.0);
        assert!(
            savings >= 90.0,
            "mvn test ≥90% savings on full fixture, got {:.1}% (raw={} tok, filtered={} tok)",
            savings,
            count_tokens(&i),
            count_tokens(&o)
        );
    }

    #[test]
    #[ignore]
    fn savings_mvn_install_full() {
        let bytes = include_bytes!("../../../tests/fixtures/mvn_install_full_raw.txt.gz");
        let i = gunzip(bytes);
        let o = filter_package(&i);
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(&i) as f64 * 100.0);
        assert!(
            savings >= 85.0,
            "mvn install ≥85% savings on full fixture, got {:.1}% (raw={} tok, filtered={} tok)",
            savings,
            count_tokens(&i),
            count_tokens(&o)
        );
    }
}
