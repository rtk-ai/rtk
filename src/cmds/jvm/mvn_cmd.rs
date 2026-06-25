//! Apache Maven filter — Surefire/Failsafe block collapse, compile error/warning
//! dedup, package/install pipeline with mode-toggle.
//!
//! Replaces the previous `src/filters/mvn-build.toml` filter with a Rust module
//! capable of state-machine parsing (block collapse, continuation tracking,
//! mode toggle) that TOML DSL cannot express.

use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// Cap on emitted failing test-class blocks and `[ERROR] Failures:` summary
/// entries — test-failure cap class, same binding as pytest/rspec/rake/runner.
const MAX_MVN_FAILING_CLASSES: usize = CAP_WARNINGS;

// ── Shared regex patterns ────────────────────────────────────────────────────

lazy_static! {
    /// `[INFO] Running com.example.app.FooTest`
    static ref RUNNING: Regex = Regex::new(r"^\[INFO\] Running ").unwrap();

    /// Surefire/Failsafe per-class close line. Captures `Failures` and `Errors`.
    /// Tolerates the optional `<<< FAILURE!` / `<<< ERROR!` marker (3.5.5 emits
    /// `<<< FAILURE!` even for errors-only classes — see
    /// `mvn_test_multifail_slice_raw.txt`; `ERROR!` accepted defensively for
    /// other Surefire versions; failure detection is via the captured counts,
    /// not the marker). Separator is `-` (Surefire 2.x) or `--` (Surefire 3.x).
    /// Prefix INFO/ERROR/WARNING (3.x emits WARNING for classes with only
    /// skipped tests).
    static ref CLOSE: Regex = Regex::new(
        r"^\[(?:INFO|ERROR|WARNING)\] Tests run: \d+, Failures: (\d+), Errors: (\d+), Skipped: \d+, Time elapsed: [^ ]+ s(?:\s+<<<\s*(?:FAILURE|ERROR)!)?\s+--?\s+in (.+)$"
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

    /// Reactor summary header that opens the per-module pass/fail block at
    /// the end of a multi-module build.
    static ref REACTOR_SUMMARY: Regex = Regex::new(r"^\[INFO\] Reactor Summary for ").unwrap();

    /// Compile-error coordinate substring to strip when deduping warnings/errors.
    static ref FILE_COORD: Regex = Regex::new(r"/[^:]+\.java:\[\d+,\d+\]").unwrap();
}

// ── Quiet-mode detection ────────────────────────────────────────────────────

/// `mvn -q` / `mvn --quiet` suppresses all `[INFO]` lines: no `BUILD SUCCESS`
/// footer, no `[INFO] Running` markers, no module banners. A passing run emits
/// **zero bytes**; a failing run emits only `[ERROR]`-prefixed lines plus the
/// stack trace. The standard filters key off `[INFO]` markers and the footer
/// guard, so they can't fire here — `filter_quiet` handles this case instead.
fn is_quiet(args: &[String]) -> bool {
    args.iter().any(|a| a == "-q" || a == "--quiet")
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

/// Boilerplate `[ERROR]` lines Maven emits after `Failed to execute goal` —
/// pure noise pointing at log files and help URLs, no signal for the user/LLM.
/// Deliberately excludes `[ERROR] After correcting the problems` and
/// `[ERROR]   mvn <args> -rf :…` (the resume hint is actionable signal for a
/// multi-module build) and `[ERROR] Failed to execute goal` (signal).
const BOILER_PREFIXES: &[&str] = &[
    "[ERROR] See ",
    "[ERROR] -> [Help",
    "[ERROR] To see the full stack trace",
    "[ERROR] Re-run Maven",
    "[ERROR] For more information",
    "[ERROR] [Help",
];

/// Post-failure help boilerplate, plus the bare `[ERROR]` divider lines Maven
/// emits between boilerplate blocks (same drop rules as `filter_quiet`).
fn is_boilerplate(line: &str) -> bool {
    BOILER_PREFIXES.iter().any(|p| line.starts_with(p)) || line.trim_end() == "[ERROR]"
}

/// `[ERROR] FQN.method -- Time elapsed: 0.030 s <<< FAILURE!` (or `<<< ERROR!`).
/// Distinguished from CLOSE by call position: only consulted when
/// `in_block == false` (CLOSE only occurs while a block is open). A
/// CLOSE-shaped line outside a block would match too — acceptable: the
/// disarm-on-take guard limits the effect to one stray line.
/// Note: the `[ERROR]   Class.test:25 …` failures-summary entries (3-space
/// indent, no `<<<` marker) do NOT match.
fn is_per_test_subline(line: &str) -> bool {
    line.starts_with("[ERROR] ")
        && (line.contains("<<< FAILURE!") || line.contains("<<< ERROR!"))
}

// ── English-footer guard ────────────────────────────────────────────────────

fn has_english_footer(stripped: &str) -> bool {
    stripped.lines().any(|l| {
        let t = l.trim();
        t.ends_with(" BUILD SUCCESS") || t.ends_with(" BUILD FAILURE")
    })
}

// ── Outside-block keep list (shared by surefire + package) ──────────────────

/// Multi-module reactor summary keeper. Reads `in_reactor_summary` and toggles
/// it on `[INFO] Reactor Summary for …` (enter) and `BUILD SUCCESS`/`BUILD
/// FAILURE` (exit). Returns `true` for every line while the flag is set so the
/// per-module status rows (`[INFO] foo ...... SUCCESS [  1.234 s]`, plain
/// `[INFO]` separators inside the summary, etc.) survive. Returns `false`
/// otherwise — the caller's outside-block keep-list still applies.
///
/// Designed to be called **before** `keep_outside_block` so the `BUILD_FOOT`
/// clears-flag side effect always runs regardless of `||` short-circuit.
fn reactor_summary_keep(line: &str, in_reactor_summary: &mut bool) -> bool {
    if REACTOR_SUMMARY.is_match(line) {
        *in_reactor_summary = true;
        return true;
    }
    if BUILD_FOOT.is_match(line) {
        *in_reactor_summary = false;
        return false;
    }
    *in_reactor_summary
}

fn keep_outside_block(line: &str) -> bool {
    // Help boilerplate must be rejected before the `[ERROR]` catch-all below
    // (non-quiet parity with `filter_quiet`'s boilerplate stripping).
    if is_boilerplate(line) {
        return false;
    }
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

/// Shared state machine driving the inner Surefire block + failure-trail
/// behaviour for `filter_surefire` and `filter_package`. Each filter wraps it
/// with its own outside-block keep logic (`[WARNING]` dedup, module-banner
/// keep, `keep_continuation` for compile-error continuations, etc.) which is
/// applied on the [`SurefireStep::Passthrough`] arm.
///
/// Inner machine responsibilities:
///   - `[INFO] --- … @ … ---` plugin banner skip
///   - `[INFO] Running <FQN>` opens a buffered block (flushes any prior open
///     block as keep — happens on truncated output)
///   - in-block buffering until the next CLOSE line
///   - CLOSE with `Failures > 0` or `Errors > 0` → yields
///     [`SurefireStep::FailingClose`] so the outer loop can decide whether to
///     emit (this seam enforces [`MAX_MVN_FAILING_CLASSES`])
///   - failure-trail handling for the exception/user-frame trail Surefire 3.x
///     emits **after** the close line, terminated by a blank line. Framework
///     frames (junit, jdk.proxy, java.base, etc.) are stripped from both the
///     buffered block and the trail; user-code frames are preserved.
///   - multi-failure classes: Surefire 3.x emits one blank-separated detail
///     block per failing test under a single CLOSE line. When a trail ends at
///     a blank line, `trail_rearm` remembers the keep/drop decision so the
///     next per-test subline re-enters the trail with the same decision.
///     End-of-input with `trail_rearm` still `Some` is harmless (nothing
///     pending in `out`); `finish()` / `flush_open_block_as_keep` need no
///     special handling.
struct SurefireBlock<'a> {
    block_lines: Vec<&'a str>,
    block_running: Option<&'a str>,
    in_block: bool,
    failure_trail: bool,
    /// When set together with `failure_trail`, consumes the trail (per-test
    /// `<<< FAILURE!` subline, exception, user frames) without writing it to
    /// `out`. Used when the caller capped a failing block via `drop_failing`.
    drop_trail: bool,
    /// Set when a trail ends at a blank line; holds the `drop_trail` value so
    /// the next per-test subline of the same class re-enters the trail with
    /// the same keep/drop decision (a capped class must drop **all** its
    /// per-test blocks, not just the first). Cleared by any non-blank
    /// non-subline line, by `RUNNING`, and by `commit_failing`/`drop_failing`.
    trail_rearm: Option<bool>,
}

enum SurefireStep<'a> {
    /// Inner machine consumed the line; outer loop should `continue;`.
    Consumed,
    /// A CLOSE line with `Failures > 0` or `Errors > 0` was reached. Outer
    /// loop decides whether to commit (via [`SurefireBlock::commit_failing`]).
    FailingClose {
        running: Option<&'a str>,
        lines: Vec<&'a str>,
        close: &'a str,
    },
    /// Inner machine did not handle the line; outer loop applies its own
    /// outside-block keep logic.
    Passthrough,
}

impl<'a> SurefireBlock<'a> {
    fn new() -> Self {
        Self {
            block_lines: Vec::new(),
            block_running: None,
            in_block: false,
            failure_trail: false,
            drop_trail: false,
            trail_rearm: None,
        }
    }

    fn step(&mut self, line: &'a str, out: &mut String) -> SurefireStep<'a> {
        if PLUGIN_BANNER.is_match(line) {
            return SurefireStep::Consumed;
        }

        if RUNNING.is_match(line) {
            if self.in_block {
                self.flush_open_block_as_keep(out);
            }
            self.block_lines.clear();
            self.block_running = Some(line);
            self.in_block = true;
            self.failure_trail = false;
            // Load-bearing: a capped multi-failure class followed by a kept
            // class must not re-arm into the new class's trail decision.
            self.trail_rearm = None;
            return SurefireStep::Consumed;
        }

        if self.in_block {
            if let Some(caps) = CLOSE.captures(line) {
                let fail = caps.get(1).map(|m| m.as_str() != "0").unwrap_or(false);
                let err = caps.get(2).map(|m| m.as_str() != "0").unwrap_or(false);
                if fail || err {
                    let lines = std::mem::take(&mut self.block_lines);
                    let running = self.block_running.take();
                    self.in_block = false;
                    return SurefireStep::FailingClose {
                        running,
                        lines,
                        close: line,
                    };
                }
                self.block_lines.clear();
                self.block_running = None;
                self.in_block = false;
                return SurefireStep::Consumed;
            }
            self.block_lines.push(line);
            return SurefireStep::Consumed;
        }

        if self.failure_trail {
            if line.is_empty() {
                if !self.drop_trail {
                    out.push('\n');
                }
                // Arm re-entry: a following per-test subline belongs to the
                // same class and must inherit this trail's keep/drop decision.
                self.trail_rearm = Some(self.drop_trail);
                self.failure_trail = false;
                self.drop_trail = false;
                return SurefireStep::Consumed;
            }
            let t = line.trim_start();
            if t.starts_with("at ") && is_framework_frame(t) {
                return SurefireStep::Consumed;
            }
            if self.drop_trail {
                return SurefireStep::Consumed;
            }
            out.push_str(line);
            out.push('\n');
            return SurefireStep::Consumed;
        }

        if let Some(dropped) = self.trail_rearm {
            if line.is_empty() {
                // Tolerate extra blanks between per-test blocks: stay armed,
                // let the blank fall through (outer keep-lists drop it).
                return SurefireStep::Passthrough;
            }
            self.trail_rearm = None; // disarm unconditionally on non-blank (load-bearing)
            if is_per_test_subline(line) {
                self.failure_trail = true;
                self.drop_trail = dropped;
                if !dropped {
                    out.push_str(line);
                    out.push('\n');
                }
                return SurefireStep::Consumed;
            }
            // Non-subline: trail is over; already disarmed — fall through.
        }

        SurefireStep::Passthrough
    }

    /// Mark a `FailingClose` as dropped (cap exceeded). The block itself is
    /// already extracted by `step()`; this sets `failure_trail` so the
    /// post-close trail (per-test subline, exception, user frames) is
    /// consumed and silently dropped until the next blank line.
    fn drop_failing(&mut self) {
        self.failure_trail = true;
        self.drop_trail = true;
        // Belt-and-suspenders: a CLOSE can only follow a RUNNING (which
        // already cleared `trail_rearm`), but keep the invariant local too.
        self.trail_rearm = None;
    }

    /// Commit a `FailingClose` to `out`: writes `running`, then `lines` (with
    /// framework frames stripped), then `close`. Enables `failure_trail` so
    /// the post-close exception/user-frame trail is preserved.
    fn commit_failing(
        &mut self,
        out: &mut String,
        running: Option<&str>,
        lines: &[&str],
        close: &str,
    ) {
        if let Some(r) = running {
            out.push_str(r);
            out.push('\n');
        }
        for l in lines {
            let t = l.trim_start();
            if t.starts_with("at ") && is_framework_frame(t) {
                continue;
            }
            out.push_str(l);
            out.push('\n');
        }
        out.push_str(close);
        out.push('\n');
        self.failure_trail = true;
        // Belt-and-suspenders: see `drop_failing`.
        self.trail_rearm = None;
    }

    /// End-of-stream flush: if a block opened and never closed (truncated
    /// output), surface what we have rather than dropping it silently.
    fn finish(&mut self, out: &mut String) {
        if self.in_block {
            self.flush_open_block_as_keep(out);
        }
    }

    fn flush_open_block_as_keep(&mut self, out: &mut String) {
        if let Some(r) = self.block_running.take() {
            out.push_str(r);
            out.push('\n');
        }
        for l in self.block_lines.drain(..) {
            out.push_str(l);
            out.push('\n');
        }
        self.in_block = false;
    }
}

/// `[ERROR] Failures:` summary block cap. Maven emits a summary at the end of
/// a failing test run:
///
/// ```text
/// [ERROR] Failures:
/// [ERROR]   ClassA.testFoo:25 expected: <a> but was: <b>
/// [ERROR]   ClassB.testBar:42 expected: <c> but was: <d>
/// [INFO]
/// [ERROR] Tests run: 100, Failures: 50, Errors: 0, Skipped: 0
/// ```
///
/// The aggregate `[ERROR] Tests run:` line is matched by `AGG` and kept; the
/// `[ERROR]   ` entries are kept by the catch-all `[ERROR]` keeper. On builds
/// with hundreds of failures this can be quite large. Cap entries at
/// [`MAX_MVN_FAILING_CLASSES`] and emit `\n… +N more failures\n` immediately
/// before the `Tests run:` aggregate when entries were dropped.
struct FailuresSummaryCap {
    cap: usize,
    in_summary: bool,
    emitted: usize,
    dropped: usize,
}

impl FailuresSummaryCap {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            in_summary: false,
            emitted: 0,
            dropped: 0,
        }
    }

    /// If `line` is an `[ERROR]   ` entry inside the failures summary, write
    /// it (or count it as dropped) and return `true` so the caller skips its
    /// own keep-list. Returns `false` otherwise.
    fn handle_entry(&mut self, line: &str, out: &mut String) -> bool {
        if !self.in_summary || !line.starts_with("[ERROR]   ") {
            return false;
        }
        // Per core cap policy, `0` means summary-only: no entries, tail still counts.
        if self.emitted < self.cap {
            out.push_str(line);
            out.push('\n');
            self.emitted += 1;
        } else {
            self.dropped += 1;
        }
        true
    }

    /// Detect the `[ERROR] Failures:` header so subsequent `[ERROR]   ` lines
    /// get capped. Caller is responsible for writing the header to `out`.
    fn handle_header(&mut self, line: &str) {
        if line.starts_with("[ERROR] Failures:") {
            self.in_summary = true;
            self.emitted = 0;
            self.dropped = 0;
        }
    }

    /// Pre-emit the `… +N more failures` tail when the aggregate
    /// `[ERROR] Tests run:` line is about to be written, then close the
    /// summary. Caller writes the AGG line itself afterwards.
    fn handle_aggregate(&mut self, line: &str, out: &mut String) {
        if !self.in_summary || !AGG.is_match(line) {
            return;
        }
        if self.dropped > 0 {
            out.push_str(&format!("\n… +{} more failures\n", self.dropped));
        }
        self.in_summary = false;
        self.emitted = 0;
        self.dropped = 0;
    }

    /// End-of-stream tail emission for cases where the AGG line never arrives
    /// (truncated output). Emits the tail with no trailing newline guard so
    /// the resulting filtered output is still well-formed.
    fn finish(&mut self, out: &mut String) {
        if self.in_summary && self.dropped > 0 {
            out.push_str(&format!("\n… +{} more failures\n", self.dropped));
        }
    }
}

/// Buffered single-pass filter for `mvn test` / `mvn integration-test`.
///
/// Drives [`SurefireBlock`] for the inner block/trail machine; applies the
/// outside-block keep-list with `keep_continuation` for indented compile-error
/// continuations (`symbol:` / `location:` after a `[ERROR] cannot find symbol`
/// line).
///
/// English-footer guard: if no `BUILD SUCCESS`/`BUILD FAILURE` line is present,
/// return the ANSI-stripped raw input (non-English locale or truncated output).
pub fn filter_surefire(raw: &str) -> String {
    filter_surefire_with_cap(raw, MAX_MVN_FAILING_CLASSES)
}

fn filter_surefire_with_cap(raw: &str, cap: usize) -> String {
    let stripped = strip_ansi(raw);
    if !has_english_footer(&stripped) {
        return stripped;
    }

    let mut out = String::new();
    let mut block = SurefireBlock::new();
    let mut keep_continuation = false;
    let mut in_reactor_summary = false;
    let mut emitted_failing: usize = 0;
    let mut dropped_failing: usize = 0;
    let mut summary = FailuresSummaryCap::new(cap);

    for line in stripped.lines() {
        match block.step(line, &mut out) {
            SurefireStep::Consumed => continue,
            SurefireStep::FailingClose {
                running,
                lines,
                close,
            } => {
                if emitted_failing < cap {
                    block.commit_failing(&mut out, running, &lines, close);
                    emitted_failing += 1;
                } else {
                    block.drop_failing();
                    dropped_failing += 1;
                }
                keep_continuation = false;
                continue;
            }
            SurefireStep::Passthrough => {}
        }

        if keep_continuation && (line.starts_with(' ') || line.starts_with('\t')) {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Failures-summary cap: gate `[ERROR]   ` entries, emit `+N more` tail
        // before AGG. The helper consumes only summary entries — other lines
        // (header, AGG) fall through to the keep-list below.
        if summary.handle_entry(line, &mut out) {
            continue;
        }

        // Order matters: call reactor_summary_keep first so its BUILD_FOOT
        // clears-flag side effect always runs regardless of `||` short-circuit.
        let reactor_keep = reactor_summary_keep(line, &mut in_reactor_summary);
        if reactor_keep || keep_outside_block(line) {
            // Pre-emit the summary tail when we're about to write AGG.
            summary.handle_aggregate(line, &mut out);
            // Detect summary header so subsequent `[ERROR]   ` entries get capped.
            summary.handle_header(line);
            out.push_str(line);
            out.push('\n');
            keep_continuation = line.starts_with("[ERROR]")
                && !line.starts_with("[ERROR] Tests run:")
                && !line.starts_with("[ERROR] Failures:")
                && !line.starts_with("[ERROR] Errors:");
            continue;
        }
        // Dropped line (e.g. help boilerplate): reset so a stale flag can't
        // keep an indented line that follows a dropped `[ERROR]` line.
        // Parity with filter_package's fall-through reset.
        keep_continuation = false;
    }

    block.finish(&mut out);
    summary.finish(&mut out);
    if dropped_failing > 0 {
        out.push_str(&format!(
            "\n… +{} more failing test classes\n",
            dropped_failing
        ));
    }
    out
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
            out.push_str(line);
            out.push('\n');
            keep_continuation = false;
            continue;
        }
        // Help boilerplate: drop before the `[ERROR]` catch-all (parity with
        // keep_outside_block / filter_quiet).
        if is_boilerplate(line) {
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
            let payload = line.strip_prefix("[WARNING] ").unwrap_or(line);
            let norm = FILE_COORD.replace_all(payload, "").to_string();
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
    filter_package_with_cap(raw, MAX_MVN_FAILING_CLASSES)
}

fn filter_package_with_cap(raw: &str, cap: usize) -> String {
    let stripped = strip_ansi(raw);
    if !has_english_footer(&stripped) {
        return stripped;
    }

    let mut out = String::new();
    let mut block = SurefireBlock::new();
    let mut keep_continuation = false;
    let mut in_reactor_summary = false;
    let mut seen_warnings: HashSet<String> = HashSet::new();
    let mut emitted_failing: usize = 0;
    let mut dropped_failing: usize = 0;
    let mut summary = FailuresSummaryCap::new(cap);

    for line in stripped.lines() {
        match block.step(line, &mut out) {
            SurefireStep::Consumed => continue,
            SurefireStep::FailingClose {
                running,
                lines,
                close,
            } => {
                if emitted_failing < cap {
                    block.commit_failing(&mut out, running, &lines, close);
                    emitted_failing += 1;
                } else {
                    block.drop_failing();
                    dropped_failing += 1;
                }
                keep_continuation = false;
                continue;
            }
            SurefireStep::Passthrough => {}
        }

        // Failures-summary cap (see filter_surefire_with_cap for details).
        if summary.handle_entry(line, &mut out) {
            continue;
        }

        // Order matters: call reactor_summary_keep first so its BUILD_FOOT
        // clears-flag side effect always runs regardless of `||` short-circuit.
        let reactor_keep = reactor_summary_keep(line, &mut in_reactor_summary);
        // Outside any Surefire block: compile-keep AND surefire-outside-keep merge.
        if reactor_keep || MODULE_BANNER.is_match(line) || keep_outside_block(line) {
            summary.handle_aggregate(line, &mut out);
            summary.handle_header(line);
            out.push_str(line);
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
            let payload = line.strip_prefix("[WARNING] ").unwrap_or(line);
            let norm = FILE_COORD.replace_all(payload, "").to_string();
            if seen_warnings.insert(norm) {
                out.push_str(line);
                out.push('\n');
            }
            keep_continuation = false;
            continue;
        }
        keep_continuation = false;
    }

    block.finish(&mut out);
    summary.finish(&mut out);
    if dropped_failing > 0 {
        out.push_str(&format!(
            "\n… +{} more failing test classes\n",
            dropped_failing
        ));
    }
    out
}

// ── Quiet-mode filter ───────────────────────────────────────────────────────

/// Filter for `mvn -q` invocations.
///
/// Under `-q`, Maven 3.x suppresses all `[INFO]` lines, so the standard
/// `filter_surefire` / `filter_compile` / `filter_package` pipelines (which
/// key off the English `BUILD SUCCESS` footer and `[INFO] Running` markers)
/// can't fire. This filter handles the residual `-q` output shape:
///
/// - Green run: input is empty → output is empty (0 → 0, no overhead).
/// - Failure run: keeps the Surefire close-line (`[ERROR] Tests run: …
///   <<< FAILURE! -- in FQN`), the per-test failure subline, exception class,
///   user-code stack frames, the failure summary block (`[ERROR] Failures:`,
///   indented entries, aggregate `Tests run: N, Failures: F, …`), and the
///   `[ERROR] Failed to execute goal` terminator. Drops framework stack
///   frames and the post-failure boilerplate block (`See …`, `[Help 1]`,
///   `Re-run Maven`, `To see the full stack trace`, etc.).
pub fn filter_quiet(raw: &str) -> String {
    let stripped = strip_ansi(raw);
    if stripped.trim().is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let mut failure_trail = false;

    for line in stripped.lines() {
        // Surefire close-line for a failed class — keep + enter failure trail.
        if CLOSE.is_match(line) {
            out.push_str(line);
            out.push('\n');
            failure_trail =
                line.contains("<<< FAILURE!") || line.contains("<<< ERROR!");
            continue;
        }

        // Per-test failure subline: `[ERROR] FQN.method -- Time elapsed: … <<< FAILURE!`
        // (or `<<< ERROR!` for thrown exceptions).
        if is_per_test_subline(line) {
            out.push_str(line);
            out.push('\n');
            failure_trail = true;
            continue;
        }

        // Failure-trail body: exception class, user-code frames; drop framework frames.
        if failure_trail {
            if line.trim().is_empty() {
                out.push('\n');
                failure_trail = false;
                continue;
            }
            let t = line.trim_start();
            if t.starts_with("at ") && is_framework_frame(t) {
                continue;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Failure summary keepers.
        if line.starts_with("[ERROR] Tests run:")
            || line.starts_with("[ERROR] Failures:")
            || line.starts_with("[ERROR] Errors:")
            || line.starts_with("[ERROR]   ")
            || line.starts_with("[ERROR] Failed to execute goal")
        {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Drop post-failure help boilerplate and bare `[ERROR]` dividers
        // (shared with the non-quiet filters — see BOILER_PREFIXES).
        if is_boilerplate(line) {
            continue;
        }

        // Safety net: keep anything else (unexpected output under `-q` is rare;
        // do not silently drop signal we haven't classified).
        out.push_str(line);
        out.push('\n');
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

    let tool = mvn_binary();
    let args_display = args.join(" ");

    // Quiet mode: standard footer guard can't fire (no `BUILD SUCCESS` line
    // under `-q`). Route to `filter_quiet` for any non-passthrough phase so
    // failure output gets framework frames + help boilerplate stripped.
    if is_quiet(args) {
        let phase = detect_phase(args);
        if matches!(phase, MvnPhase::Passthrough) {
            let osargs: Vec<OsString> = args.iter().map(OsString::from).collect();
            return runner::run_passthrough(tool, &osargs, verbose);
        }
        return runner::run_filtered(
            new_mvn_command(args),
            tool,
            &args_display,
            filter_quiet,
            RunOptions::with_tee("mvn_quiet"),
        );
    }

    let phase = detect_phase(args);

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
        // Passing fixture has 5 close lines; all should be dropped (no per-class line in output).
        assert!(!o.contains("Running org.apache.commons.cli.help.UtilTest"));
        assert!(!o.contains("Time elapsed: 1.023 s -- in"));
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(i) as f64 * 100.0);
        assert!(
            savings >= 50.0,
            "pass-fixture savings >=50%, got {:.1}%",
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
            !o.contains("Running org.apache.commons.cli.ConverterTests"),
            "passing-test Running line dropped; got:\n{}",
            o
        );
        assert!(
            o.contains("BUILD SUCCESS"),
            "footer preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("Tests run: 977, Failures: 0"),
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
            o.contains("at org.apache.commons.cli.RtkInducedFailTest.rtkInducedFailure"),
            "user-code frame preserved; got:\n{}",
            o
        );
        assert!(
            !o.contains("at org.junit."),
            "framework frames stripped in failing block; got:\n{}",
            o
        );
    }

    /// 2.x compat: CLOSE regex must still match the single-dash separator emitted
    /// by Surefire 2.x. Locks the `--?` regex against accidental tightening.
    #[test]
    fn surefire_matches_legacy_2x_close_line() {
        let i = "[INFO] -----< x >-----\n[INFO] Running x.Foo\n[INFO] Tests run: 3, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.123 s - in x.Foo\n[INFO] BUILD SUCCESS\n";
        let o = filter_surefire(i);
        // CLOSE matched → passing block dropped silently.
        assert!(
            !o.contains("Running x.Foo"),
            "2.x ` - in ` close-line matched; passing block dropped; got:\n{}",
            o
        );
        assert!(
            o.contains("BUILD SUCCESS"),
            "footer preserved; got:\n{}",
            o
        );
    }

    /// 3.x WARNING-prefixed close line (class with only skipped tests) must
    /// match CLOSE so the block is dropped (no failures, no errors).
    #[test]
    fn surefire_matches_warning_skipped_close_line() {
        let i = "[INFO] -----< x >-----\n[INFO] Running x.Skip\n[WARNING] Tests run: 5, Failures: 0, Errors: 0, Skipped: 5, Time elapsed: 0.010 s -- in x.Skip\n[INFO] BUILD SUCCESS\n";
        let o = filter_surefire(i);
        assert!(
            !o.contains("Running x.Skip"),
            "[WARNING] close-line matched; block dropped; got:\n{}",
            o
        );
    }

    /// 3.x failure-trail: after a CLOSE with `<<< FAILURE!`, the exception
    /// class and user-code frames Surefire emits *outside* the block must be
    /// preserved until the next blank line.
    #[test]
    fn surefire_preserves_3x_failure_trail() {
        let i = "[INFO] -----< x >-----\n\
                 [INFO] Running x.Foo\n\
                 [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.033 s <<< FAILURE! -- in x.Foo\n\
                 [ERROR] x.Foo.bar -- Time elapsed: 0.025 s <<< FAILURE!\n\
                 org.opentest4j.AssertionFailedError: expected: <a> but was: <b>\n\
                 \tat x.Foo.bar(Foo.java:25)\n\
                 \tat org.junit.jupiter.api.Assertions.assertEquals(Assertions.java:1)\n\
                 \n\
                 [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i);
        assert!(o.contains("AssertionFailedError"), "exception preserved; got:\n{}", o);
        assert!(o.contains("at x.Foo.bar"), "user frame preserved; got:\n{}", o);
        assert!(
            !o.contains("at org.junit."),
            "framework frame stripped in trail; got:\n{}",
            o
        );
    }

    // ── Multi-failure class (trail re-arm) ──────────────────────────────────

    /// Surefire 3.x emits one blank-separated detail block per failing test
    /// under a single CLOSE line. All per-test exception messages must survive
    /// (not just the first), framework frames must stay stripped throughout.
    /// Real fixture: `CalcTest` (1 failure + 1 error) + `BoomTest` (errors-only).
    #[test]
    fn surefire_keeps_all_failures_in_multi_failure_class() {
        let i = include_str!("../../../tests/fixtures/mvn_test_multifail_slice_raw.txt");
        let o = filter_surefire(i);
        assert!(
            o.contains("AssertionFailedError: failOne: addition should equal five"),
            "first failure message preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("IllegalStateException: failTwo: induced error"),
            "second failure (ERROR! subline) message preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("at com.example.rtk.CalcTest.failOne(CalcTest.java:12)"),
            "first user frame preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("at com.example.rtk.CalcTest.failTwo(CalcTest.java:17)"),
            "second user frame preserved; got:\n{}",
            o
        );
        assert!(
            !o.contains("at org.junit."),
            "junit frames stripped; got:\n{}",
            o
        );
        assert!(
            !o.contains("at java.base/"),
            "jdk frames stripped; got:\n{}",
            o
        );
    }

    /// Same multi-failure fixture through `filter_package` (drift guard —
    /// the install/verify route shares `SurefireBlock` and must not diverge).
    #[test]
    fn package_keeps_all_failures_in_multi_failure_class() {
        let i = include_str!("../../../tests/fixtures/mvn_test_multifail_slice_raw.txt");
        let o = filter_package(i);
        assert!(
            o.contains("AssertionFailedError: failOne: addition should equal five"),
            "first failure message preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("IllegalStateException: failTwo: induced error"),
            "second failure message preserved; got:\n{}",
            o
        );
        assert!(
            !o.contains("at org.junit."),
            "junit frames stripped; got:\n{}",
            o
        );
        assert!(
            !o.contains("at java.base/"),
            "jdk frames stripped; got:\n{}",
            o
        );
    }

    /// A capped (dropped) multi-failure class must drop **all** its per-test
    /// blocks — the re-arm inherits the drop decision — and the tail counts
    /// classes, not failures. The existing `surefire_caps_failing_blocks_emits_tail`
    /// only covers single-failure classes.
    #[test]
    fn surefire_drop_failing_drops_all_sublines_of_capped_class() {
        let i = "[INFO] Scanning for projects...\n\
                 [INFO] -----< x >-----\n\
                 [INFO] Running x.FailA\n\
                 [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.011 s <<< FAILURE! -- in x.FailA\n\
                 [ERROR] x.FailA.one -- Time elapsed: 0.010 s <<< FAILURE!\n\
                 org.opentest4j.AssertionFailedError: boomA\n\
                 \tat x.FailA.one(FailA.java:10)\n\
                 \n\
                 [INFO] Running x.MultiFail\n\
                 [ERROR] Tests run: 2, Failures: 1, Errors: 1, Skipped: 0, Time elapsed: 0.051 s <<< FAILURE! -- in x.MultiFail\n\
                 [ERROR] x.MultiFail.first -- Time elapsed: 0.020 s <<< FAILURE!\n\
                 org.opentest4j.AssertionFailedError: boomFirst\n\
                 \tat x.MultiFail.first(MultiFail.java:20)\n\
                 \n\
                 [ERROR] x.MultiFail.second -- Time elapsed: 0.030 s <<< ERROR!\n\
                 java.lang.IllegalStateException: boomSecond\n\
                 \tat x.MultiFail.second(MultiFail.java:30)\n\
                 \n\
                 [INFO] BUILD FAILURE\n";
        let o = filter_surefire_with_cap(i, 1);

        assert!(o.contains("boomA"), "first class kept; got:\n{}", o);
        assert!(
            !o.contains("Running x.MultiFail") && !o.contains("boomFirst"),
            "capped class first block dropped; got:\n{}",
            o
        );
        assert!(
            !o.contains("x.MultiFail.second") && !o.contains("boomSecond"),
            "capped class second per-test block dropped (re-arm inherits drop); got:\n{}",
            o
        );
        assert!(
            o.contains("… +1 more failing test classes"),
            "tail counts one class, not one per failure; got:\n{}",
            o
        );
    }

    /// A non-subline line (`[INFO] Results:`) immediately after a trail blank
    /// must disarm the re-arm and be kept normally by the outside-block list.
    #[test]
    fn surefire_rearm_disarms_at_results_boundary() {
        let i = "[INFO] -----< x >-----\n\
                 [INFO] Running x.MultiFail\n\
                 [ERROR] Tests run: 2, Failures: 2, Errors: 0, Skipped: 0, Time elapsed: 0.051 s <<< FAILURE! -- in x.MultiFail\n\
                 [ERROR] x.MultiFail.first -- Time elapsed: 0.020 s <<< FAILURE!\n\
                 org.opentest4j.AssertionFailedError: boomFirst\n\
                 \n\
                 [ERROR] x.MultiFail.second -- Time elapsed: 0.030 s <<< FAILURE!\n\
                 org.opentest4j.AssertionFailedError: boomSecond\n\
                 \n\
                 [INFO] Results:\n\
                 [ERROR] Tests run: 2, Failures: 2, Errors: 0, Skipped: 0\n\
                 [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i);
        assert!(o.contains("boomSecond"), "second block kept; got:\n{}", o);
        assert!(
            o.contains("[INFO] Results:"),
            "Results boundary disarms re-arm and is kept; got:\n{}",
            o
        );
        assert!(
            o.contains("[ERROR] Tests run: 2, Failures: 2"),
            "aggregate kept; got:\n{}",
            o
        );
    }

    /// Double blank between per-test blocks: stay armed across the extra
    /// blank, still re-enter the trail — and no spurious blank lines leak.
    #[test]
    fn surefire_tolerates_double_blank_between_failure_blocks() {
        let i = "[INFO] -----< x >-----\n\
                 [INFO] Running x.MultiFail\n\
                 [ERROR] Tests run: 2, Failures: 2, Errors: 0, Skipped: 0, Time elapsed: 0.051 s <<< FAILURE! -- in x.MultiFail\n\
                 [ERROR] x.MultiFail.first -- Time elapsed: 0.020 s <<< FAILURE!\n\
                 org.opentest4j.AssertionFailedError: boomFirst\n\
                 \n\
                 \n\
                 [ERROR] x.MultiFail.second -- Time elapsed: 0.030 s <<< FAILURE!\n\
                 org.opentest4j.AssertionFailedError: boomSecond\n\
                 \n\
                 [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i);
        assert!(o.contains("boomFirst"), "first block kept; got:\n{}", o);
        assert!(
            o.contains("boomSecond"),
            "second block re-enters trail across double blank; got:\n{}",
            o
        );
        assert!(
            !o.contains("\n\n\n"),
            "no spurious blank lines leak; got:\n{:?}",
            o
        );
    }

    /// Byte-exact pin of the single-failure path: the re-arm machinery must
    /// not change output for single-failure fixtures (no extra blank lines,
    /// no reordering). Literal captured from `filter_surefire` at the commit
    /// preceding the trail re-arm change.
    #[test]
    fn surefire_single_failure_output_unchanged() {
        let i = include_str!("../../../tests/fixtures/mvn_test_fail_slice_raw.txt");
        let o = filter_surefire(i);
        let expected = "[INFO] Scanning for projects...\n\
                        [INFO] ----------------------< commons-cli:commons-cli >-----------------------\n\
                        [INFO] Building Apache Commons CLI 1.11.1-SNAPSHOT\n\
                        [INFO] Running org.apache.commons.cli.RtkInducedFailTest\n\
                        [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.033 s <<< FAILURE! -- in org.apache.commons.cli.RtkInducedFailTest\n\
                        [ERROR] org.apache.commons.cli.RtkInducedFailTest.rtkInducedFailure -- Time elapsed: 0.025 s <<< FAILURE!\n\
                        org.opentest4j.AssertionFailedError: expected: <expected> but was: <actual>\n\
                        \tat org.apache.commons.cli.RtkInducedFailTest.rtkInducedFailure(RtkInducedFailTest.java:25)\n\
                        \n\
                        [INFO] Results:\n\
                        [ERROR] Failures:\n\
                        [ERROR]   RtkInducedFailTest.rtkInducedFailure:25 expected: <expected> but was: <actual>\n\
                        [ERROR] Tests run: 978, Failures: 1, Errors: 0, Skipped: 61\n\
                        [INFO] BUILD FAILURE\n\
                        [INFO] Total time:  01:05 min\n\
                        [INFO] Finished at: 2026-05-21T14:57:09Z\n\
                        [ERROR] Failed to execute goal org.apache.maven.plugins:maven-surefire-plugin:3.5.5:test (default-test) on project commons-cli: There are test failures.\n";
        assert_eq!(o, expected, "single-failure output must be byte-identical");
    }

    /// Savings on the multifail slice. Threshold is low by design: the slice
    /// is nearly all kept failure signal (two failing classes, three per-test
    /// detail blocks), so the droppable share is small — measured 42.3% after
    /// non-quiet boilerplate stripping (19.9% before it; precedent:
    /// reactor-fail pins ≥30% with a "short fixture" note).
    #[test]
    fn savings_mvn_test_multifail_slice() {
        let i = include_str!("../../../tests/fixtures/mvn_test_multifail_slice_raw.txt");
        let o = filter_surefire(i);
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(i) as f64 * 100.0);
        assert!(
            savings >= 30.0,
            "multifail slice ≥30% savings (dense failure-signal fixture), got {:.1}%",
            savings
        );
    }

    /// Non-quiet runs must strip the post-failure help boilerplate
    /// (`-> [Help 1]`, `Re-run Maven`, `See …`, bare `[ERROR]` dividers) the
    /// same way `filter_quiet` does, while keeping the `Failed to execute
    /// goal` terminator (signal).
    #[test]
    fn surefire_drops_help_boilerplate_in_nonquiet_mode() {
        let i = include_str!("../../../tests/fixtures/mvn_test_multifail_slice_raw.txt");
        let o = filter_surefire(i);
        assert!(
            o.contains("[ERROR] Failed to execute goal"),
            "goal terminator kept; got:\n{}",
            o
        );
        assert!(!o.contains("[Help 1]"), "help link stripped; got:\n{}", o);
        assert!(
            !o.contains("Re-run Maven"),
            "re-run hint stripped; got:\n{}",
            o
        );
        assert!(
            !o.contains("To see the full stack trace"),
            "stack-trace hint stripped; got:\n{}",
            o
        );
        assert!(
            !o.contains("See dump files"),
            "dump-file pointer stripped; got:\n{}",
            o
        );
        assert!(
            !o.lines().any(|l| l.trim_end() == "[ERROR]"),
            "bare [ERROR] dividers stripped; got:\n{}",
            o
        );
    }

    /// CLOSE regex accepts a `<<< ERROR!` marker (defensive — Surefire 3.5.5
    /// emits `<<< FAILURE!` even for errors-only classes, per the multifail
    /// fixture capture; other versions may emit `ERROR!`).
    #[test]
    fn close_line_matches_error_marker() {
        let line = "[ERROR] Tests run: 1, Failures: 0, Errors: 1, Skipped: 0, Time elapsed: 0.006 s <<< ERROR! -- in com.example.rtk.BoomTest";
        let caps = CLOSE
            .captures(line)
            .expect("CLOSE must match an ERROR!-marked close line");
        assert_eq!(caps.get(1).expect("failures group").as_str(), "0");
        assert_eq!(caps.get(2).expect("errors group").as_str(), "1");
    }

    /// `mvn test` whose compile step fails before Surefire runs must still
    /// keep the `[ERROR]` block's indented `symbol:` / `location:` continuation
    /// lines. Regression guard for the P0 reviewer ask: `filter_surefire`
    /// previously dropped them because it had no `keep_continuation` flag.
    #[test]
    fn surefire_keeps_compile_continuation_on_test_phase() {
        let i = include_str!("../../../tests/fixtures/mvn_test_compile_fail_slice_raw.txt");
        let o = filter_surefire(i);
        assert!(o.contains("cannot find symbol"), "ERROR line preserved; got:\n{}", o);
        assert!(
            o.contains("symbol:   variable bar"),
            "indented `symbol:` continuation preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("location: class org.apache.commons.cli.CompileBreaker"),
            "indented `location:` continuation preserved; got:\n{}",
            o
        );
        assert!(o.contains("BUILD FAILURE"), "footer preserved; got:\n{}", o);
    }

    /// Regression guard on the package path so the install/verify route does
    /// not silently drift the other way after the `filter_surefire` continuation
    /// fix. Uses the existing compile-error slice — `filter_package` is the
    /// `install`-phase entry point and must keep the same continuation lines.
    #[test]
    fn package_still_keeps_compile_error_continuation_after_refactor() {
        let i = include_str!("../../../tests/fixtures/mvn_compile_error_slice_raw.txt");
        let o = filter_package(i);
        assert!(o.contains("cannot find symbol"), "ERROR line preserved; got:\n{}", o);
        assert!(
            o.contains("symbol:   variable bar"),
            "indented `symbol:` continuation preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("location: class org.apache.commons.cli.CompileBreaker"),
            "indented `location:` continuation preserved; got:\n{}",
            o
        );
    }

    #[test]
    fn surefire_keeps_module_banner() {
        let i = "[INFO] Scanning for projects...\n[INFO] -----< com.example:myapp >-----\n[INFO] BUILD SUCCESS\n";
        let o = filter_surefire(i);
        assert!(o.contains("-----< com.example:myapp >-----"));
    }

    /// Production must ship raw `Time elapsed` and `Total time` durations
    /// untouched — the LLM/user needs the actual numbers to diagnose perf
    /// regressions. Earlier revisions normalised these to `T s`; that was
    /// only ever needed for deterministic snapshots and never belonged in
    /// the production path.
    #[test]
    fn surefire_preserves_real_durations() {
        let i = "[INFO] -----< x >-----\n[INFO] Running x.Foo\n[ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 2.341 s <<< FAILURE! - in x.Foo\n[INFO] BUILD FAILURE\n[INFO] Total time:  4.567 s\n";
        let o = filter_surefire(i);
        assert!(
            o.contains("2.341 s"),
            "raw close-line duration preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("Total time:  4.567 s"),
            "raw total time preserved; got:\n{}",
            o
        );
        assert!(
            !o.contains("Time elapsed: T s"),
            "no normalisation in production; got:\n{}",
            o
        );
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

    // ── CRLF line-ending compatibility ───────────────────────────────────────

    /// `str::lines()` strips single `\r\n` pairs entirely, so real Maven CRLF
    /// output filters cleanly. The hazard is a fixture *already checked out
    /// with CRLF* (e.g. `core.autocrlf=true` without `.gitattributes`): the
    /// `\n` → `\r\n` synthesis below would then produce `\r\r\n`, leaving a
    /// stray `\r` per line that `$`-anchored regexes reject. Normalise the
    /// embedded fixture back to LF first — correct regardless of checkout
    /// state (defense-in-depth alongside `tests/fixtures/** -text`).
    #[test]
    fn surefire_handles_crlf_line_endings() {
        let i_lf = include_str!("../../../tests/fixtures/mvn_test_pass_slice_raw.txt")
            .replace("\r\n", "\n");
        let o_lf = filter_surefire(&i_lf);
        let i_crlf = i_lf.replace('\n', "\r\n");
        let o_crlf = filter_surefire(&i_crlf);
        assert_eq!(
            o_lf,
            o_crlf.replace("\r\n", "\n"),
            "CRLF filtered output must match LF (modulo line endings)"
        );
    }

    #[test]
    fn package_handles_crlf_line_endings() {
        let i_lf = include_str!("../../../tests/fixtures/mvn_install_slice_raw.txt")
            .replace("\r\n", "\n");
        let o_lf = filter_package(&i_lf);
        let i_crlf = i_lf.replace('\n', "\r\n");
        let o_crlf = filter_package(&i_crlf);
        assert_eq!(
            o_lf,
            o_crlf.replace("\r\n", "\n"),
            "CRLF filtered output must match LF (modulo line endings)"
        );
    }

    // ── Cap: failing-class blocks ────────────────────────────────────────────

    /// Synthetic fixture with 5 failing classes; with `cap = 3` we expect
    /// the first 3 failing blocks emitted in full and a
    /// `… +2 more failing test classes` tail.
    #[test]
    fn surefire_caps_failing_blocks_emits_tail() {
        let mut i = String::from(
            "[INFO] Scanning for projects...\n\
             [INFO] -----< x >-----\n",
        );
        for n in 1..=5 {
            i.push_str(&format!(
                "[INFO] Running x.Fail{n}\n\
                 [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.0{n}1 s <<< FAILURE! -- in x.Fail{n}\n\
                 [ERROR] x.Fail{n}.bar -- Time elapsed: 0.0{n}0 s <<< FAILURE!\n\
                 org.opentest4j.AssertionFailedError: boom{n}\n\
                 \tat x.Fail{n}.bar(Fail{n}.java:25)\n\
                 \n",
                n = n
            ));
        }
        i.push_str("[INFO] BUILD FAILURE\n");

        let o = filter_surefire_with_cap(&i, 3);

        // First 3 blocks emitted with their close lines.
        for n in 1..=3 {
            assert!(
                o.contains(&format!("Running x.Fail{}", n)),
                "Fail{n} kept; got:\n{}",
                o,
                n = n
            );
            assert!(
                o.contains(&format!("in x.Fail{}", n)),
                "Fail{n} close line kept; got:\n{}",
                o,
                n = n
            );
        }
        // Blocks 4 and 5 dropped.
        for n in 4..=5 {
            assert!(
                !o.contains(&format!("Running x.Fail{}", n)),
                "Fail{n} dropped; got:\n{}",
                o,
                n = n
            );
            assert!(
                !o.contains(&format!("AssertionFailedError: boom{}", n)),
                "Fail{n} exception dropped; got:\n{}",
                o,
                n = n
            );
        }
        assert!(
            o.contains("… +2 more failing test classes"),
            "tail emitted; got:\n{}",
            o
        );
    }

    /// Cap of 0 means summary-only (core cap policy): no failing-class blocks
    /// emitted, tail still counts every dropped class.
    #[test]
    fn surefire_cap_zero_emits_summary_only() {
        let mut i = String::from(
            "[INFO] Scanning for projects...\n\
             [INFO] -----< x >-----\n",
        );
        for n in 1..=5 {
            i.push_str(&format!(
                "[INFO] Running x.Fail{n}\n\
                 [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.0{n}1 s <<< FAILURE! -- in x.Fail{n}\n\
                 \n",
                n = n
            ));
        }
        i.push_str("[INFO] BUILD FAILURE\n");
        let o = filter_surefire_with_cap(&i, 0);
        for n in 1..=5 {
            assert!(
                !o.contains(&format!("Running x.Fail{}", n)),
                "Fail{n} dropped under cap=0; got:\n{}",
                o,
                n = n
            );
        }
        assert!(
            o.contains("+5 more failing test classes"),
            "tail counts all 5 under cap=0; got:\n{}",
            o
        );
    }

    /// `[ERROR] Failures:` summary block cap: with N>cap entries, expect the
    /// first `cap` entries plus a `\n… +(N-cap) more failures\n` tail
    /// emitted before the aggregate `[ERROR] Tests run:` line.
    #[test]
    fn failures_summary_block_is_capped() {
        let mut i = String::from(
            "[INFO] -----< x >-----\n\
             [INFO] Results:\n\
             [INFO]\n\
             [ERROR] Failures:\n",
        );
        for n in 1..=5 {
            i.push_str(&format!(
                "[ERROR]   ClassA.test{n}:25 expected: <a> but was: <b{n}>\n",
                n = n
            ));
        }
        i.push_str(
            "[INFO]\n\
             [ERROR] Tests run: 100, Failures: 5, Errors: 0, Skipped: 0\n\
             [INFO] BUILD FAILURE\n",
        );
        let o = filter_surefire_with_cap(&i, 3);

        // First 3 entries kept.
        for n in 1..=3 {
            assert!(
                o.contains(&format!("ClassA.test{}:25", n)),
                "entry {n} kept; got:\n{}",
                o,
                n = n
            );
        }
        // Entries 4-5 dropped.
        for n in 4..=5 {
            assert!(
                !o.contains(&format!("ClassA.test{}:25", n)),
                "entry {n} dropped; got:\n{}",
                o,
                n = n
            );
        }
        // Tail emitted before aggregate.
        let tail_idx = o
            .find("… +2 more failures")
            .unwrap_or_else(|| panic!("tail must appear; got:\n{}", o));
        let agg_idx = o
            .find("[ERROR] Tests run: 100")
            .unwrap_or_else(|| panic!("aggregate must appear; got:\n{}", o));
        assert!(
            tail_idx < agg_idx,
            "tail must precede aggregate; tail@{} agg@{}; got:\n{}",
            tail_idx,
            agg_idx,
            o
        );
    }

    // ── Multi-module reactor summary ─────────────────────────────────────────

    /// `mvn install` on a multi-module reactor build that passes everywhere
    /// must preserve the `Reactor Summary for <root>` header and the per-module
    /// `[INFO] foo ...... SUCCESS [ 1.234 s]` rows.
    #[test]
    fn reactor_summary_kept_on_multi_module_pass() {
        let i = include_str!("../../../tests/fixtures/mvn_reactor_pass_slice_raw.txt");
        let o = filter_package(i);
        assert!(
            o.contains("Reactor Summary for multi-module-skeleton"),
            "reactor summary header preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("[INFO] child-a ............................................ SUCCESS"),
            "per-module SUCCESS row preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("[INFO] child-b ............................................ SUCCESS"),
            "second per-module SUCCESS row preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("BUILD SUCCESS"),
            "footer preserved; got:\n{}",
            o
        );
    }

    /// `mvn install` on a multi-module reactor build where one module fails
    /// must preserve the Reactor Summary with the `FAILURE` row plus the
    /// `[ERROR] Failed to execute goal …` terminator that already survives
    /// via `keep_outside_block`.
    #[test]
    fn reactor_summary_kept_on_multi_module_fail() {
        let i = include_str!("../../../tests/fixtures/mvn_reactor_fail_slice_raw.txt");
        let o = filter_package(i);
        assert!(
            o.contains("Reactor Summary for multi-module-skeleton"),
            "reactor summary header preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("child-a ............................................ SUCCESS"),
            "successful module row preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("child-b ............................................ FAILURE"),
            "failing module row preserved; got:\n{}",
            o
        );
        assert!(o.contains("BUILD FAILURE"), "footer preserved; got:\n{}", o);
        assert!(
            o.contains("[ERROR] Failed to execute goal"),
            "goal terminator preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("mvn <args> -rf :child-b"),
            "resume hint preserved (actionable signal); got:\n{}",
            o
        );
        assert!(!o.contains("[Help 1]"), "help boilerplate stripped; got:\n{}", o);
        assert!(
            !o.contains("Re-run Maven"),
            "re-run hint stripped; got:\n{}",
            o
        );
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(i) as f64 * 100.0);
        assert!(
            savings >= 30.0,
            "reactor-fail slice savings >=30% (short fixture); got {:.1}%",
            savings
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
        assert!(
            !o.contains("[Help 1]"),
            "help boilerplate stripped in compile path; got:\n{}",
            o
        );
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

    // ── Token-savings (FULL gzipped fixtures) ───────────────────────────────

    #[test]
    #[ignore]
    fn print_savings_summary() {
        let pf = gunzip(include_bytes!("../../../tests/fixtures/mvn_test_pass_full_raw.txt.gz"));
        let pf_out = filter_surefire(&pf);
        let pf_in_tok = count_tokens(&pf);
        let pf_out_tok = count_tokens(&pf_out);
        let pf_s = 100.0 - (pf_out_tok as f64 / pf_in_tok as f64 * 100.0);
        println!(
            "mvn_test_pass_full: {} -> {} tokens ({:.1}% savings)",
            pf_in_tok, pf_out_tok, pf_s
        );

        let inst = gunzip(include_bytes!("../../../tests/fixtures/mvn_install_full_raw.txt.gz"));
        let inst_out = filter_package(&inst);
        let inst_in_tok = count_tokens(&inst);
        let inst_out_tok = count_tokens(&inst_out);
        let inst_s = 100.0 - (inst_out_tok as f64 / inst_in_tok as f64 * 100.0);
        println!(
            "mvn_install_full:   {} -> {} tokens ({:.1}% savings)",
            inst_in_tok, inst_out_tok, inst_s
        );
    }

    #[test]
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

    // ── Quiet mode (`mvn -q`) ────────────────────────────────────────────────

    #[test]
    fn quiet_detects_short_flag() {
        assert!(is_quiet(&s(["-q", "test"])));
        assert!(is_quiet(&s(["test", "-q"])));
        assert!(is_quiet(&s(["-B", "-q", "-DskipFoo", "install"])));
    }

    #[test]
    fn quiet_detects_long_flag() {
        assert!(is_quiet(&s(["--quiet", "test"])));
    }

    #[test]
    fn quiet_does_not_match_unrelated_flags() {
        assert!(!is_quiet(&s(["-Q", "test"])));
        assert!(!is_quiet(&s(["-quiet", "test"])));
        assert!(!is_quiet(&s(["-B", "test"])));
    }

    /// Green `mvn -q test` emits zero bytes; filter must return empty.
    #[test]
    fn quiet_green_run_is_empty() {
        assert_eq!(filter_quiet(""), "");
        assert_eq!(filter_quiet("   \n\n  \n"), "");
    }

    /// Failure under `-q`: keep close-line, exception, user frame, summary,
    /// goal terminator. Drop framework frames + help boilerplate block.
    #[test]
    fn quiet_fail_strips_framework_and_boilerplate() {
        let i = include_str!("../../../tests/fixtures/mvn_quiet_fail_raw.txt");
        let o = filter_quiet(i);

        // Kept: failure signal.
        assert!(
            o.contains("Tests run: 1, Failures: 1, Errors: 0, Skipped: 0"),
            "close-line preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("AssertionFailedError"),
            "exception class preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("at x.FailTest.this_will_fail"),
            "user-code frame preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("[ERROR] Failures:"),
            "failure summary header preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("[ERROR] Tests run: 6, Failures: 1, Errors: 0, Skipped: 0"),
            "aggregate preserved; got:\n{}",
            o
        );
        assert!(
            o.contains("[ERROR] Failed to execute goal"),
            "goal terminator preserved; got:\n{}",
            o
        );

        // Dropped: framework frames.
        assert!(
            !o.contains("at org.junit."),
            "junit frame stripped; got:\n{}",
            o
        );
        assert!(
            !o.contains("at java.base/"),
            "java.base frame stripped; got:\n{}",
            o
        );

        // Dropped: help boilerplate.
        assert!(
            !o.contains("To see the full stack trace"),
            "help boilerplate stripped; got:\n{}",
            o
        );
        assert!(
            !o.contains("[Help 1] http"),
            "help link stripped; got:\n{}",
            o
        );
        assert!(
            !o.contains("See /tmp/") && !o.contains("See dump files"),
            "log-pointer lines stripped; got:\n{}",
            o
        );
    }

    /// Savings target on the real `mvn -q test` fail fixture.
    #[test]
    fn savings_mvn_quiet_fail() {
        let i = include_str!("../../../tests/fixtures/mvn_quiet_fail_raw.txt");
        let o = filter_quiet(i);
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(i) as f64 * 100.0);
        assert!(
            savings >= 50.0,
            "mvn -q fail ≥50% savings, got {:.1}% (raw={} tok, filtered={} tok)",
            savings,
            count_tokens(i),
            count_tokens(&o)
        );
    }

    /// Safety net: if the `[ERROR]` line isn't on the known keep/drop lists,
    /// the filter must NOT silently drop it. Better to leak a line than to
    /// hide signal.
    #[test]
    fn quiet_unknown_error_line_kept_as_safety_net() {
        let i = "[ERROR] Some unexpected error output we don't classify\n";
        let o = filter_quiet(i);
        assert!(
            o.contains("Some unexpected error output"),
            "unclassified ERROR line preserved; got:\n{}",
            o
        );
    }
}



