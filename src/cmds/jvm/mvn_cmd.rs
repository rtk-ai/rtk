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
use regex::Regex;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

/// Cap on emitted failing test-class blocks and `[ERROR] Failures:` summary
/// entries — test-failure cap class, same binding as pytest/rspec/rake/runner.
const MAX_MVN_FAILING_CLASSES: usize = CAP_WARNINGS;

/// Hard cap on [`Lanes`] growth. Real reactors have a bounded module count
/// (dozens, not thousands); 256 is generous headroom above any real build.
/// Invariant: `Lanes::route`'s linear scan is O(`MAX_LANES`), never
/// O(distinct tags seen) — once the cap is hit, no further tag mints a new
/// lane, so neither the lane count nor the per-line scan can grow with a
/// pathological input (e.g. thousands of uniquely-tagged log lines).
const MAX_LANES: usize = 256;

// ── Shared regex patterns ────────────────────────────────────────────────────

/// `[INFO] Running com.example.app.FooTest`
static RUNNING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[INFO\] Running ").unwrap());

/// Genuine mvnd module opener shape for `Running`: Surefire emits `"Running
/// " + clazz.getName()` — a single whitespace-free dotted FQCN, never
/// whitespace (a Java class name cannot contain a space). Confirmed against
/// all 22 `Running` lines across all 16 real fixtures. Used only in
/// [`is_lane_opener`] to narrow the never-seen-tag admission case; every
/// other `RUNNING` call site (block open/reset) is untouched — an app log
/// merely shaped `[main] [INFO] Running the widget pipeline` (multi-word
/// argument) no longer qualifies as a lane opener.
static RUNNING_MODULE_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[INFO\] Running \S+\s*$").unwrap());

/// Surefire/Failsafe per-class close line. Captures `Failures` and `Errors`.
/// Tolerates the optional `<<< FAILURE!` / `<<< ERROR!` marker (3.5.5 emits
/// `<<< FAILURE!` even for errors-only classes — see
/// `mvn_test_multifail_slice_raw.txt`; `ERROR!` accepted defensively for
/// other Surefire versions; failure detection is via the captured counts,
/// not the marker). Separator is `-` (Surefire 2.x) or `--` (Surefire 3.x).
/// Prefix INFO/ERROR/WARNING (3.x emits WARNING for classes with only
/// skipped tests).
static CLOSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\[(?:INFO|ERROR|WARNING)\] Tests run: \d+, Failures: (\d+), Errors: (\d+), Skipped: \d+, Time elapsed: [^ ]+ s(?:\s+<<<\s*(?:FAILURE|ERROR)!)?\s+--?\s+in (.+)$"
    ).unwrap()
});

/// Final BUILD footer.
static BUILD_FOOT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(?:INFO|ERROR)\] BUILD (?:SUCCESS|FAILURE)$").unwrap());

/// `[INFO] Results:` separator before the aggregate.
static RESULTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[INFO\] Results:\s*$").unwrap());

/// Aggregate counts line (no `Time elapsed`, no ` - in `).
static AGG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[(?:INFO|ERROR)\] Tests run: \d+, Failures: \d+, Errors: \d+, Skipped: \d+\s*$")
        .unwrap()
});

/// Plugin banner line: `[INFO] --- plugin:goal (id) @ module ---`.
static PLUGIN_BANNER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[INFO\] --- .* @ .* ---$").unwrap());

/// Surefire/Failsafe plugin execution banner specifically — captures the
/// *plugin family* (`surefire` or `failsafe`) from
/// `[INFO] --- <plugin>:<version>:<goal> (<execution-id>) @ <module> ---`,
/// so a unit-test → integration-test transition can be detected independent
/// of lane/module identity (see
/// [`FailuresSummaryCap::observe_plugin_banner`]).
///
/// The family is the whole key on purpose: version, goal and execution id
/// are all per-module configuration, so keying on them would make two
/// modules that merely pin different Surefire versions — or name their
/// executions differently — look like separate phases and each collect a
/// fresh budget. Both coordinate spellings normalize to the same family, so
/// `maven-surefire-plugin:2.22.2:test` (Maven ≤3.8 / mvnd 0.x) and
/// `surefire:3.5.5:test` are one phase, not two.
///
/// The accepted cost is the other direction: a build that runs two *Surefire*
/// executions (unit tests, then integration tests without Failsafe) shares one
/// budget across both, so the second execution's entries can collapse into its
/// `… +N more failures` tail. The count is still reported; over-reporting a
/// phase boundary that isn't one was the worse failure, since it silently
/// multiplied the reactor-wide cap.
///
/// Neither key bounds a parallel `verify`, where one module can reach
/// Failsafe while another is still in Surefire: the families alternate and
/// each flip starts a fresh budget, so the cap multiplies. Keying on the goal
/// string flips at least as often, so this is no worse than before — but the
/// reactor-wide cap is not a guarantee under interleaved phases.
///
/// Deliberately narrower than [`PLUGIN_BANNER`], which matches *any*
/// plugin's banner (`compiler:…`, `resources:…`, `clean:…`, …) — those never
/// open a failures summary and must never trigger a budget reset.
static TEST_PLUGIN_BANNER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[INFO\] --- (?:maven-)?(surefire|failsafe)(?:-plugin)?:\S+ \([^)]*\) @ .* ---$")
        .unwrap()
});

/// A genuine mvnd reactor module header: `Building <name> <version>
/// [n/m]` — the trailing `[n/m]` reactor-position counter is what a real
/// module's own `Building` line always carries (confirmed against
/// `mvnd_reactor_pass_raw.txt` / `mvnd_reactor_fail_raw.txt`) and an app log
/// coincidentally shaped `[pool-1] [INFO] Building segment N of the data
/// pipeline` never does. Used to gate the `[INFO] Building ` opener/keeper
/// on *tagged* lanes only — see [`is_lane_opener`] and
/// [`keep_outside_block`]. The root lane's own `Building` line (plain `mvn`,
/// single-module — never reactor-numbered) is unaffected: it's kept by the
/// unconditional `starts_with` check, not this one.
static BUILDING_MODULE_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[INFO\] Building .*\[\d+/\d+\]\s*$").unwrap());

/// Module banner with project name in brackets.
static MODULE_BANNER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[INFO\] -+< .+ >-+$").unwrap());

/// Reactor summary header that opens the per-module pass/fail block at
/// the end of a multi-module build.
static REACTOR_SUMMARY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[INFO\] Reactor Summary for ").unwrap());

/// Compile-error coordinate substring to strip when deduping warnings/errors.
static FILE_COORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/[^:]+\.java:\[\d+,\d+\]").unwrap());

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

/// Blank separator line as emitted by both binaries: plain `mvn` writes a
/// truly empty line between a Surefire failure trail and the next section;
/// `mvnd` routes everything through the daemon logger, which prefixes even
/// blank lines with `[INFO] ` (see `mvnd_test_fail_raw.txt`). Both terminate
/// (or bridge, in the re-arm state) a failure trail.
fn is_blank_separator(line: &str) -> bool {
    line.is_empty() || line.trim_end() == "[INFO]"
}

// ── Parallel-reactor lanes ──────────────────────────────────────────────────

/// mvnd parallel reactors prefix per-module log lines with `[module] ` while
/// stack traces stay raw and reactor-level lines stay unprefixed — and lines
/// from different modules interleave freely (see
/// `mvnd_reactor_fail_raw.txt`). Classification must therefore happen on the
/// prefix-stripped view, and Surefire block state must be tracked per module.
///
/// Exhaustive contract, in order:
///
/// - `[tag] [LEVEL] …` (tag non-empty and not itself a level, second bracket
///   a genuine level) → `(Some(tag), core)`, lane-keyed. An *empty* tag
///   (`[] [LEVEL] …`) is excluded and falls to the raw case below — `""` is
///   the root lane's reserved key ([`Lanes::new`] seeds it there), so an
///   empty tag would impersonate root-keyed routing and bypass `raw_owner`
///   the same way the non-level-shaped cases below do; mvnd never emits an
///   empty module tag. Real mvnd module tags are
///   followed by `[INFO]`/`[ERROR]`/etc. That alone doesn't rule out
///   look-alikes: application logging shaped `[main] [INFO] started …` (a
///   thread-name tag followed by a level bracket) is syntactically identical
///   to a real `[module] [INFO] …` mvnd line — no per-line check can tell
///   them apart. [`Lanes::route`] resolves the ambiguity using state this
///   function doesn't have: a **never-seen** tag is only admitted as a new
///   lane when its line is also a genuine module-establishing line
///   ([`is_lane_opener`]); otherwise it falls back to ownership rules like a
///   raw line.
/// - `[LEVEL] …` (single bracket, Maven's own unprefixed output — the tag
///   itself is the level, with more content after it) → `(Some(""), line)`,
///   root-keyed.
/// - bare `[LEVEL]` (trailing whitespace tolerated; no `"] "` substring at
///   all — the daemon-prefixed trail terminator, e.g. `[INFO] ` or, once
///   `"] "` is absent, just `[INFO]`) → `(Some(""), line)`, root-keyed.
///   Keeps the root lane's blank-terminator contract (`is_blank_separator`)
///   intact.
/// - any other `[`-leading line (bracket-shaped but neither of the above —
///   e.g. `[boom] weird bracket assertion line`, `[1, 2] != [1, 3]`, or a
///   bare `[1, 2]`/`[boom]` with no `"] "` substring at all — a
///   bracket-leading fragment of a multi-line assertion message, not
///   Maven's own output) → `(None, line)`, raw. `Lanes::raw_owner` decides
///   ownership like any other raw line, instead of unconditionally landing
///   on the root keep-list (root-keyed bypasses `raw_owner` entirely, so
///   such a line inside a *tagged* lane's active failure trail/block was
///   dropped: it never reached the trail-keeping logic).
/// - non-bracket-leading line → `(None, line)`, raw (checked first, below).
///
/// `daemon` gates this whole lane layer (cold-preclear finding, upstream PR
/// #3199, third review round): plain `mvn` never interleaves module output —
/// there is nothing for lane tracking to protect against, and the bracket
/// heuristic above is exactly what let an SLF4J/Logback `[%thread] [%level]
/// %msg` line (or a `[timestamp] [LEVEL] …` layout) masquerade as a real
/// mvnd module tag, minting a phantom lane, opening a block that never
/// closes, and resurrecting or reordering content base would have dropped
/// or kept in place. `daemon == false` short-circuits to `(Some(""), line)`
/// unconditionally — every line, bracket-leading or not, root-keyed with
/// `core == line` — which is exactly base's (pre-lane) flat model: a single
/// always-keyed state machine, `is_lane_opener`/`Lanes::raw_owner`/
/// `MAX_LANES` never consulted at all (`Lanes::route` finds the pre-seeded
/// root lane on the first lookup). This makes *routing* byte-identical to
/// base *by construction*, not by case analysis.
///
/// Scope of that claim: it covers `split_lane`/`Lanes::route` only, not
/// every byte `filter_surefire`/`filter_package` can ever emit for plain
/// `mvn`. `SurefireBlock::step`'s in-block summary-header recovery (flushing
/// a stale open block when an `[ERROR] Failures:`/`Errors:` header arrives
/// mid-block — see its own doc comment) is a single shared fix that also
/// improves the root lane's handling of a malformed/truncated stream over
/// base's; that divergence is deliberate and orthogonal to routing, not a
/// gap in this function.
fn split_lane(line: &str, daemon: bool) -> (Option<&str>, &str) {
    if !daemon {
        return (Some(""), line);
    }
    if !line.starts_with('[') {
        return (None, line);
    }
    if let Some(end) = line.find("] ") {
        let tag = &line[1..end];
        let rest = &line[end + 2..];
        if tag.is_empty() {
            // `""` is the root lane's reserved key (`Lanes::new` seeds it
            // there) — an empty tag would impersonate root-keyed routing
            // and bypass `raw_owner` entirely. mvnd never emits an empty
            // module tag, so `[] [LEVEL] …` is raw, not root-keyed.
            return (None, line);
        }
        if !is_log_level(tag) {
            if lane_rest_level(rest).is_some() {
                return (Some(tag), rest);
            }
            return (None, line);
        }
        return (Some(""), line);
    }
    // No "] " substring: root-key only Maven's own bare `[LEVEL]` blank
    // (the daemon-prefixed trail terminator, trailing whitespace tolerated)
    // — everything else `[`-leading with no "] " (e.g. `[1, 2]`, `[boom]`,
    // a bracketed fragment of a multi-line assertion diff) is raw, not
    // Maven's own output, so `Lanes::raw_owner` decides ownership.
    let bare = line.trim_end();
    if bare
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .is_some_and(is_log_level)
    {
        return (Some(""), line);
    }
    (None, line)
}

fn is_log_level(tag: &str) -> bool {
    matches!(tag, "INFO" | "ERROR" | "WARNING" | "DEBUG" | "FATAL")
}

/// The bracketed tag at the start of `rest`, if it's a real log level —
/// i.e. `rest` looks like `[LEVEL] …` or (the no-trailing-space blank
/// spelling, `[tag] [INFO]` with nothing after) exactly `[LEVEL]`. Used to
/// confirm a candidate lane tag is genuinely followed by a level bracket,
/// not just any `[...]` — both spellings must be recognized so
/// `is_blank_separator`'s `[INFO]`-blank contract holds for a tagged lane
/// regardless of whether the daemon's trailing space survived.
fn lane_rest_level(rest: &str) -> Option<&str> {
    let rest = rest.strip_prefix('[')?;
    let level = match rest.find("] ") {
        Some(end) => &rest[..end],
        None => rest.strip_suffix(']')?,
    };
    is_log_level(level).then_some(level)
}

/// Whether `core` (the tag-stripped remainder of a candidate lane-keyed
/// line) is a genuine Maven/mvnd module-establishing line: a test class
/// starting ([`RUNNING_MODULE_HEADER`]), a plugin or module banner, a
/// `Building` line, a
/// per-class result (`CLOSE`), or a genuine compiler diagnostic — a
/// `file.java:[line,col]`-coordinate `[ERROR]` line ([`FILE_COORD`]; compiler
/// errors are always the first line mvnd emits for a module that fails to
/// compile — there is no `RUNNING`/`Building` line first). `[tag] [LEVEL] …`
/// is syntactically identical whether `tag` is a real mvnd module or an
/// application thread/timestamp caught in test-captured stdout (see
/// `split_lane`); this heuristic only narrows the `[INFO]`-shaped case
/// (e.g. `[main] [INFO] …`), which is the common one in practice — real
/// module tags are always *introduced* by one of these lines before any
/// other content appears under them, so this is the signal
/// [`Lanes::route`] uses to admit a never-seen tag as a new lane rather
/// than falling back to raw-line ownership.
///
/// Enforced contract: every arm requires the tool's own emission shape for
/// that line kind, not just a matching prefix — `RUNNING`:
/// [`RUNNING_MODULE_HEADER`]'s single whitespace-free dotted FQCN (Surefire
/// emits `"Running " + clazz.getName()`; a Java class name cannot contain a
/// space — confirmed against all 22 `Running` lines across all 16 real
/// fixtures); `Building`: [`BUILDING_MODULE_HEADER`]'s trailing `[n/m]`
/// reactor-position counter; `[ERROR]`: [`FILE_COORD`]'s `file.java:[l,c]`
/// coordinate; `CLOSE`/`MODULE_BANNER`/`PLUGIN_BANNER` are already
/// structurally specific. An app log merely shaped like one of these (a
/// multi-word `[main] [INFO] Running the widget pipeline`, a `[pool-1]
/// [INFO] Building segment N of the data pipeline`, a `[Server] [ERROR]
/// connection refused`) never qualifies — it falls back to
/// [`Lanes::raw_owner`] like any other raw line instead of minting a
/// phantom lane.
///
/// Reviewer finding #1 (upstream PR #3199, second review round) established
/// this pattern for the `[ERROR]` arm: a blanket `[ERROR]`-prefix rule (no
/// `FILE_COORD` requirement) let *any* `[ERROR]`-shaped app log on a
/// never-seen tag (e.g. `[Server] [ERROR] connection refused`) mint a lane.
/// The claim it opens stays inert either way (a continuation only ever arms
/// on a genuine compiler diagnostic — see the `FILE_COORD` guard at each
/// `keep_continuation` arm site), but *minting the lane at all* was itself
/// observable: routed to its own fresh (empty, `in_block == false`) lane
/// instead of falling back to [`Lanes::raw_owner`], such a line bypassed
/// whichever other lane's buffered block was genuinely open around it —
/// written to `out` immediately via the outside-block keep-list while that
/// other block was still buffering, re-ordering it ahead of the `Running`
/// line it followed in the input, and on a green close (block discarded, not
/// written at all) letting it survive as a stray line the whole rest of the
/// passing block was correctly dropped for. Requiring `FILE_COORD` here
/// fixes both: a non-diagnostic `[ERROR]` line on a never-seen tag is no
/// longer an opener, so `route` falls back to `raw_owner`, which (with
/// exactly one lane's block open) buffers it into *that* lane at its correct
/// input position — preserved in order on a failing close, discarded with
/// the rest on a green one. A genuine `file.java:[line,col]` compiler
/// diagnostic is unaffected (mvnd's compile-error-first-sight case: no
/// `Running`/`Building` line ever precedes it). A Surefire close whose
/// module is never otherwise seen (`[ERROR] Tests run: … <<< FAILURE!`) is
/// unaffected too — it's admitted by the separate `CLOSE.is_match` arm
/// above, not this one.
///
/// Lane growth itself is bounded by [`MAX_LANES`] regardless, so an
/// adversarial run of uniquely-tagged opener-shaped lines can mint at most
/// that many lanes, keeping `route`'s scan O(`MAX_LANES`) regardless of how
/// many distinct tags the input contains.
fn is_lane_opener(core: &str) -> bool {
    RUNNING_MODULE_HEADER.is_match(core)
        || CLOSE.is_match(core)
        || MODULE_BANNER.is_match(core)
        || PLUGIN_BANNER.is_match(core)
        || BUILDING_MODULE_HEADER.is_match(core)
        || (core.starts_with("[ERROR]") && FILE_COORD.is_match(core))
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

/// `gate_building` narrows the `[INFO] Building ` keeper to
/// [`BUILDING_MODULE_HEADER`]'s reactor-numbered `[n/m]` shape. Callers pass
/// `daemon && is_tag_prefixed(line, core)`: a line that arrived under a
/// module tag can never be plain `mvn`'s own single-module `Building` header,
/// so requiring the real shape there rejects `[pool-1] [INFO] Building
/// segment N of the data pipeline` app logs while leaving untagged headers
/// (plain `mvn`, and mvnd's own unprefixed output) on the unconditional
/// check. `Building war:`/`jar:`/`ear:` are artifact-packaging lines, a
/// different shape entirely, and are never gated.
fn is_tag_prefixed(line: &str, core: &str) -> bool {
    core.len() != line.len()
}

fn keep_outside_block(line: &str, gate_building: bool) -> bool {
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
        || (line.starts_with("[INFO] Building ")
            && (!gate_building || BUILDING_MODULE_HEADER.is_match(line)))
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
    /// per-test blocks, not just the first). Cleared by the lane's own keyed
    /// non-blank non-subline lines, by `RUNNING`, and by
    /// `commit_failing`/`drop_failing` — never by raw lines, which reach the
    /// lane only via routing fallback and can't speak for the trail.
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

    /// Matching is done on `core` (the module-prefix-stripped view of the
    /// line — identical to `line` outside mvnd parallel reactors); `line` is
    /// the original, which is what gets buffered and emitted so module
    /// identity survives in the output. `keyed` is whether this line was
    /// routed by its own module tag (see [`Lanes::route`]) rather than
    /// falling back to ownership rules; `is_root` is whether this lane is
    /// the root (untagged) lane, the only one plain `mvn` ever uses.
    fn step(
        &mut self,
        line: &'a str,
        core: &str,
        keyed: bool,
        is_root: bool,
        out: &mut String,
    ) -> SurefireStep<'a> {
        // PLUGIN_BANNER/RUNNING/CLOSE only mean anything as *this lane's own*
        // state transitions. A fallback-routed line (keyed == false) landed
        // here via raw-line ownership rules, not because it's genuinely
        // this lane's banner/class-start/class-end — treating its core as
        // one anyway (e.g. a lane-cap-overflow module's own `Running`/close
        // line, misrouted to an unrelated lane) would flush or discard that
        // lane's real open block, or open a block that never closes. Not
        // keyed: fall through and treat it as ordinary content instead.
        if keyed && PLUGIN_BANNER.is_match(core) {
            return SurefireStep::Consumed;
        }

        if keyed && RUNNING.is_match(core) {
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

        // A `[ERROR] Failures:` / `[ERROR] Errors:` summary header can never
        // legitimately appear while a per-class block is genuinely still
        // open — Maven only emits it after every class in the module has
        // already closed (see `mvnd_reactor_fail_raw.txt`: the last class's
        // own close line always precedes it). If one reaches here anyway —
        // nothing closed the last class explicitly — swallowing it (and the
        // entries/AGG that follow) into `block_lines` would dump the whole
        // summary uncapped at end-of-stream, bypassing `FailuresSummaryCap`
        // entirely. Flush the stale block as keep (the same recovery
        // `RUNNING` already gets, right above) and fall through to
        // `Passthrough` so the header actually reaches the summary-cap
        // machinery. Invariant: the reactor-wide budget is keyed to summary
        // blocks (`[ERROR] Failures:`/`Errors:` … the `AGG` aggregate),
        // never to whether the carrying lane's last per-class block
        // happened to close first.
        if keyed
            && self.in_block
            && (core.starts_with("[ERROR] Failures:") || core.starts_with("[ERROR] Errors:"))
        {
            self.flush_open_block_as_keep(out);
            return SurefireStep::Passthrough;
        }

        if self.in_block {
            if keyed {
                if let Some(caps) = CLOSE.captures(core) {
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
            }
            self.block_lines.push(line);
            return SurefireStep::Consumed;
        }

        if self.failure_trail {
            // Invariant: a trail terminates on a blank line only when that
            // blank is genuinely this lane's own — keyed (a tagged lane's
            // `[tag] [INFO] ` blank, or any other keyed line reaching here)
            // or the root lane (plain `mvn`'s terminator is *always* a raw,
            // unkeyed empty line — mvnd is the only binary that prefixes
            // blanks, so a tagged lane's real terminator is always keyed).
            // A raw blank landing in a *tagged* lane's trail is foreign
            // (another module's stray blank println, or a blank line inside
            // a multi-line assertion message) and must not end the trail —
            // it's just more trail content.
            if is_blank_separator(core) && (keyed || is_root) {
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
            let t = core.trim_start();
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
            if is_blank_separator(core) {
                // Tolerate extra blanks between per-test blocks: stay armed,
                // let the blank fall through (outer keep-lists drop it).
                return SurefireStep::Passthrough;
            }
            if keyed {
                // Only the lane's own keyed lines disarm (load-bearing for
                // the sequential stale-rearm case: `[INFO] Results:` etc.).
                self.trail_rearm = None;
                if is_per_test_subline(core) {
                    self.failure_trail = true;
                    self.drop_trail = dropped;
                    if !dropped {
                        out.push_str(line);
                        out.push('\n');
                    }
                    return SurefireStep::Consumed;
                }
                // Keyed non-subline: trail is over; already disarmed — fall
                // through.
            }
            // Raw line: leave the rearm armed. A raw line reached this lane
            // only via the routing fallback; per-test sublines are always
            // keyed, so a raw stray can neither be the re-entry line nor
            // prove the trail is over — letting it disarm would drop (kept
            // class) or leak (capped class) every detail block after it.
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
///
/// The *budget* (`emitted`, how many entries may still be kept) behaves
/// differently by mode ([`FailuresSummaryCap::daemon`]):
///
/// - **Plain `mvn`** (`daemon == false`): matches base (pre-lane) semantics
///   exactly — every `[ERROR] Failures:`/`Errors:` header, however many a
///   sequential multi-module build shows, gets its own unconditionally
///   fresh budget. Plain `mvn` has no reactor-wide sharing concept; there is
///   only ever one (root) lane, so there is nothing to share *between*.
/// - **mvnd** (`daemon == true`): **reactor-wide within one generation**, not
///   per module — a parallel reactor emits one summary block per failing
///   module, and each module's *first* summary in a generation shares that
///   generation's one running budget: a 20-module reactor still keeps at
///   most `cap` entries total for that pass, never `modules × cap`, whether
///   the modules' summaries interleave or run back-to-back. But `mvn
///   verify`/`install` runs Surefire's summary then Failsafe's as two
///   independent generations — a new generation gets a fresh `cap`. The
///   boundary between generations is an explicit phase marker
///   ([`FailuresSummaryCap::observe_plugin_banner`]), not lane-repeat
///   inference (cold-preclear finding, upstream PR #3199, third review
///   round): inferring "new generation" from a lane's header *repeating*
///   silently failed to reset the budget when a module's only failures were
///   integration-test ones — its Failsafe header was its first-ever
///   sighting, never a repeat, so it shared whatever Surefire's phase had
///   already spent (potentially zero) instead of a fresh `cap`.
///
/// The *dropped-entry count* backing each `… +N more failures` tail is
/// **per lane** ([`SurefireLane::dropped`]) in both modes — the budget says
/// how many entries total may survive, but which module's entries got
/// dropped while spending that budget is per-module information, and each
/// module's own tail must report only its own drops, flushed at its own AGG
/// line. Reactor-wide `dropped` (an earlier design) let whichever lane's AGG
/// happened to arrive first flush *everyone's* outstanding drops under its
/// own header and zero the counter for every lane after it — dropping a
/// module's only failure with no tail to show for it, or crediting one
/// module's drops to another's summary. Attribution invariant: **a lane's
/// `… +N more failures` tail reports exactly the entries dropped while that
/// lane's own summary block was open, no more and no less** — even when
/// several lanes' summaries interleave and share one budget.
struct FailuresSummaryCap {
    cap: usize,
    emitted: usize,
    /// `false` for plain `mvn`: every header resets unconditionally (base
    /// parity — see the struct doc). `true` for mvnd: resets only at a
    /// plugin-banner phase transition ([`FailuresSummaryCap::
    /// observe_plugin_banner`]).
    daemon: bool,
    /// mvnd only: the plugin family (`surefire` / `failsafe`) captured from
    /// the most recent test-plugin banner. `None` until the first banner is
    /// seen.
    phase: Option<String>,
}

impl FailuresSummaryCap {
    fn new(cap: usize, daemon: bool) -> Self {
        Self {
            cap,
            emitted: 0,
            daemon,
            phase: None,
        }
    }

    /// If `core` is an `[ERROR]   ` entry inside the calling lane's failures
    /// summary, write `line` (the original, module prefix included) — or
    /// increment `dropped` (the calling lane's own pending-drop count) —
    /// and return `true` so the caller skips its own keep-list. Returns
    /// `false` otherwise. `emitted` is the only thing that decides
    /// keep-vs-drop; `dropped` only tracks *whose* tail reports the drop.
    fn handle_entry(
        &mut self,
        in_summary: bool,
        dropped: &mut usize,
        core: &str,
        line: &str,
        out: &mut String,
    ) -> bool {
        if !in_summary || !core.starts_with("[ERROR]   ") {
            return false;
        }
        // Per core cap policy, `0` means summary-only: no entries, tail still counts.
        if self.emitted < self.cap {
            out.push_str(line);
            out.push('\n');
            self.emitted += 1;
        } else {
            *dropped += 1;
        }
        true
    }

    /// Detect the `[ERROR] Failures:` or `[ERROR] Errors:` header so
    /// subsequent `[ERROR]   ` lines get capped — a run whose failures are
    /// all thrown exceptions (no assertion failures) gets an `Errors:`-only
    /// summary with no `Failures:` header at all, and must be capped the
    /// same way. Caller is responsible for writing the header to `out`.
    ///
    /// Plain `mvn` (`!self.daemon`): every header resets `emitted` and the
    /// caller's `dropped` unconditionally — base parity, see the struct doc.
    /// mvnd: no reset here at all — generation boundaries are driven
    /// exclusively by [`FailuresSummaryCap::observe_plugin_banner`]. The
    /// `*in_summary` guard above already prevents a mid-block `Errors:`
    /// header (Maven can emit `Failures:` then `Errors:` as two sections of
    /// one summary) from being treated as a new generation at all.
    fn handle_header(&mut self, line: &str, in_summary: &mut bool, dropped: &mut usize) {
        let is_header =
            line.starts_with("[ERROR] Failures:") || line.starts_with("[ERROR] Errors:");
        if !is_header || *in_summary {
            return;
        }
        *in_summary = true;
        if !self.daemon {
            self.emitted = 0;
            *dropped = 0;
        }
    }

    /// mvnd only (no-op for plain `mvn`): `family` is the plugin family
    /// captured from a Surefire/Failsafe banner (see
    /// [`TEST_PLUGIN_BANNER`]). Entering a family other than the one
    /// currently tracked is a genuine unit → integration phase boundary and
    /// starts a fresh reactor-wide budget; every module's banner within one
    /// phase carries the same family, so repeated banners across modules
    /// never reset, regardless of the versions or execution ids those
    /// modules configure.
    fn observe_plugin_banner(&mut self, family: &str) {
        if !self.daemon {
            return;
        }
        if self.phase.as_deref() != Some(family) {
            self.phase = Some(family.to_string());
            self.emitted = 0;
        }
    }

    /// Pre-emit the calling lane's own `… +N more failures` tail (from
    /// `dropped`, that lane's own pending-drop count — never another lane's)
    /// when the aggregate `[ERROR] Tests run:` line is about to be written,
    /// then close this lane's summary and zero its `dropped`. Caller writes
    /// the AGG line itself afterwards.
    fn handle_aggregate(
        &mut self,
        line: &str,
        dropped: &mut usize,
        out: &mut String,
        in_summary: &mut bool,
    ) {
        if !*in_summary || !AGG.is_match(line) {
            return;
        }
        if *dropped > 0 {
            out.push_str(&format!("\n… +{} more failures\n", dropped));
            *dropped = 0;
        }
        *in_summary = false;
    }
}

/// Per-module filter state for parallel reactors: each module gets its own
/// Surefire block machine, continuation flag, and summary-open flag, because
/// mvnd interleaves module output line-by-line (a `[child-b]` close can land
/// between a `[child-a]` `Running` and its close). The failures-summary
/// *shared budget* (`emitted`/`cap`) is deliberately not here — see
/// [`FailuresSummaryCap`] — but each lane's own *pending-drop count*
/// (`dropped`) is: it must be flushed as that lane's own `… +N more
/// failures` tail at that lane's own AGG line, never folded into or flushed
/// by another lane's.
struct SurefireLane<'a> {
    block: SurefireBlock<'a>,
    keep_continuation: bool,
    in_summary: bool,
    /// Entries dropped (cap exceeded) while *this* lane's failures summary
    /// was open, not yet reported in a `… +N more failures` tail. Flushed
    /// and reset to `0` by [`FailuresSummaryCap::handle_aggregate`] at this
    /// lane's own AGG line — see the attribution invariant on
    /// [`FailuresSummaryCap`].
    dropped: usize,
}

impl<'a> SurefireLane<'a> {
    fn new() -> Self {
        Self {
            block: SurefireBlock::new(),
            keep_continuation: false,
            in_summary: false,
            dropped: 0,
        }
    }
}

/// The set of per-module lanes plus the "hot" lane raw lines fall back to
/// when no block, trail, or armed continuation exists anywhere. `hot` has a
/// single writer — a failing close (stray raw lines after its trail ends
/// attribute to the failing lane's keep-list); every other ownership claim
/// (trails, open blocks, armed continuations) is resolved by
/// [`Lanes::raw_owner`] scanning per-lane state for a unique owner. Lane 0
/// (`""`) is the root lane, the only one a plain-`mvn` (or single-module
/// mvnd) run ever uses. Insertion order is preserved so end-of-stream
/// flushes are deterministic; lookups are a linear scan (a reactor has a
/// handful of modules, not thousands).
struct Lanes<'a> {
    lanes: Vec<(&'a str, SurefireLane<'a>)>,
    hot: usize,
}

/// Index of the root (untagged, `""`) lane — always 0, by construction of
/// [`Lanes::new`]; never reassigned since lanes are only ever appended.
const ROOT_LANE: usize = 0;

impl<'a> Lanes<'a> {
    fn new() -> Self {
        Self {
            lanes: vec![("", SurefireLane::new())],
            hot: 0,
        }
    }

    fn get(&mut self, idx: usize) -> &mut SurefireLane<'a> {
        &mut self.lanes[idx].1
    }

    /// Lane index for a line, plus whether it was routed *by its own key*
    /// (`true`) or fell back to ownership rules (`false`) — the latter must
    /// be treated exactly like a raw line by callers (it may disarm a claim
    /// or re-enter a trail only a genuinely keyed line for that lane may
    /// touch). Keyed routing creates the lane on first sight, but only for a
    /// tag whose line is a genuine module-establishing line per
    /// [`is_lane_opener`] *and* only while under [`MAX_LANES`]; a never-seen
    /// tag on an ordinary line falls back to ownership rules, since it may
    /// be an application log line whose bracketed thread/timestamp only
    /// looks like a module tag. `None` means the line's ownership is
    /// genuinely ambiguous (a raw line with no unique claimant, or a
    /// confirmed new module that can't be routed because the lane cap is
    /// full) and the caller must preserve it verbatim rather than guess a
    /// lane that may drop or corrupt it.
    fn route(&mut self, key: Option<&'a str>, core: &str) -> Option<(usize, bool)> {
        match key {
            Some(k) => {
                if let Some(i) = self.lanes.iter().position(|(t, _)| *t == k) {
                    return Some((i, true));
                }
                if !is_lane_opener(core) {
                    // Never-seen tag on an ordinary line: not a confirmed
                    // module — fall back to ownership rules instead of
                    // minting a phantom lane that would divert this line
                    // away from whichever lane's block/trail it actually
                    // belongs to.
                    return self.raw_owner().map(|i| (i, false));
                }
                if self.lanes.len() < MAX_LANES {
                    self.lanes.push((k, SurefireLane::new()));
                    return Some((self.lanes.len() - 1, true));
                }
                // A confirmed new module, but the lane cap is already full:
                // preserve verbatim rather than guess an owner via
                // raw_owner. Unlike the phantom-tag case above, this line's
                // core genuinely looks like a state transition (`Running`,
                // a close, a banner) — routing it onto an unrelated
                // existing lane would corrupt that lane's block/trail, not
                // just misattribute a bystander line. Ceiling cost: a 257th
                // genuine module's own lines are preserved but ungrouped
                // (over-keep), never dropped or misattributed onto another
                // module's block (never data loss).
                None
            }
            None => self.raw_owner().map(|i| (i, false)),
        }
    }

    /// Lane owning an unprefixed raw line (stack trace, stray stdout — mvnd
    /// emits these without a module tag even in parallel builds).
    ///
    /// One rule, applied literally: a raw line is routed only when its owner
    /// is **unique** — exactly one lane in a failure trail (trails outrank
    /// open blocks: that's where actionable diagnostics land), else exactly
    /// one lane with an open block. Any tie is genuine ambiguity → `None`,
    /// and the caller preserves the line verbatim rather than guessing a lane
    /// that may drop it. With nothing open at all there is no block or trail
    /// to misroute into, so the hot lane's outside-block keep-list decides.
    ///
    /// An armed compile continuation is itself a competing claim, wherever
    /// its lane sits — the arming lane need not be `hot`, since a failing
    /// close elsewhere steals `hot` unconditionally. Its raw `symbol:` /
    /// `location:` lines must be neither buffered into another lane's open
    /// block (destroyed on a green close) nor consumed by a trail (silently,
    /// if that trail is dropping), so any block or trail open alongside an
    /// armed lane is a tie too, and several armed lanes are a tie on their
    /// own. With nothing open, a unique armed lane outranks the hot lane —
    /// its continuations are the only raw lines anyone is expecting.
    ///
    /// Deliberate over-keep, never loss: when a tie preserves verbatim, a
    /// capped class's framework frames (or a passing block's stack chatter)
    /// can leak into the output for the duration of the tie — and an armed
    /// claim persists until its lane's next keyed line, so the tie window
    /// can outlive the continuations themselves. That trades a few noise
    /// lines for the guarantee that actionable diagnostics are never routed
    /// into a lane that discards them.
    ///
    /// A unique *dropping* trail consuming raw lines silently is loss-free
    /// even with blocks open concurrently: a dropping trail exists only once
    /// the class cap is exhausted ([`FailingClassCap::admit`] is monotonic),
    /// so a concurrent block's failing close would be capped and dropped
    /// too, and a green close discards its buffer by definition.
    fn raw_owner(&self) -> Option<usize> {
        let mut armed_lanes = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, (_, l))| l.keep_continuation);
        let armed = match (armed_lanes.next(), armed_lanes.next()) {
            (Some((i, _)), None) => Some(i),
            (Some(_), Some(_)) => return None,
            _ => None,
        };
        let mut trails = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, (_, l))| l.block.failure_trail);
        match (trails.next(), trails.next()) {
            (Some((i, _)), None) if armed.is_none() => return Some(i),
            (Some(_), _) => return None,
            _ => {}
        }
        let mut open = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, (_, l))| l.block.in_block);
        match (open.next(), open.next()) {
            (Some((i, _)), None) if armed.is_none() => Some(i),
            (Some(_), _) => None,
            // Nothing open: the unique armed lane's continuation handling,
            // else the hot lane's outside-block keep-list, decides.
            _ => Some(armed.unwrap_or(self.hot)),
        }
    }

    /// End-of-stream flush of every lane's block machine, in lane order.
    fn finish(&mut self, out: &mut String) {
        for (_, lane) in &mut self.lanes {
            lane.block.finish(out);
        }
    }

    /// End-of-stream flush of every lane's own pending failures-summary
    /// `dropped` count — covers truncated output where a lane's `[ERROR]
    /// Failures:` block opened, entries were capped, but its own AGG line
    /// never arrived. Per-lane, in lane order — see the attribution
    /// invariant on [`FailuresSummaryCap`].
    fn finish_summaries(&mut self, out: &mut String) {
        for (_, lane) in &mut self.lanes {
            if lane.dropped > 0 {
                out.push_str(&format!("\n… +{} more failures\n", lane.dropped));
                lane.dropped = 0;
            }
        }
    }
}

/// Reactor-wide cap on emitted failing test classes, with the
/// `… +N more failing test classes` tail. Shared by
/// `filter_surefire_with_cap` and `filter_package_with_cap`.
struct FailingClassCap {
    cap: usize,
    emitted: usize,
    dropped: usize,
}

impl FailingClassCap {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            emitted: 0,
            dropped: 0,
        }
    }

    /// `true` when the next failing class still fits under the cap.
    fn admit(&mut self) -> bool {
        if self.emitted < self.cap {
            self.emitted += 1;
            true
        } else {
            self.dropped += 1;
            false
        }
    }

    fn finish(&self, out: &mut String) {
        if self.dropped > 0 {
            out.push_str(&format!(
                "\n… +{} more failing test classes\n",
                self.dropped
            ));
        }
    }
}

/// Shared per-line front half of `filter_surefire_with_cap` and
/// `filter_package_with_cap`: route the line to its lane, drive the lane's
/// Surefire block machine, and commit/drop failing closes against the
/// reactor-wide class cap. Returns `Some((lane index, core, keyed))` when
/// the line fell through to the caller's outside-block keep-list — `keyed`
/// is whether the line was routed *by its own module tag* (see
/// [`Lanes::route`]) or fell back to ownership rules, whether raw or a
/// never-established tag; callers must only disarm a lane's armed
/// continuation on that lane's own keyed lines, since a raw line reached
/// the lane *because of* the claim. `None` when the line was consumed — or
/// preserved verbatim on ambiguous raw-line ownership.
fn drive_surefire_line<'a>(
    lanes: &mut Lanes<'a>,
    line: &'a str,
    classes: &mut FailingClassCap,
    summary: &mut FailuresSummaryCap,
    daemon: bool,
    out: &mut String,
) -> Option<(usize, &'a str, bool)> {
    let (key, core) = split_lane(line, daemon);
    let (idx, keyed) = match lanes.route(key, core) {
        Some(v) => v,
        None => {
            // Ambiguous ownership: preserve rather than risk dropping a
            // failing module's diagnostics into a passing block.
            out.push_str(line);
            out.push('\n');
            return None;
        }
    };

    // Surefire/Failsafe plugin banners are swallowed silently by
    // `block.step()` below (never surface to the outer loop's keep-list),
    // so this is the only point that can observe a phase transition —
    // needed before the swallow so `FailuresSummaryCap` can reset the
    // budget at an unambiguous boundary instead of inferring one from lane
    // repetition (cold-preclear finding, upstream PR #3199, third review
    // round: that inference silently shared Surefire's leftover budget with
    // a module whose only failures were integration-test ones).
    if keyed {
        if let Some(caps) = TEST_PLUGIN_BANNER.captures(core) {
            summary.observe_plugin_banner(caps.get(1).map_or("", |m| m.as_str()));
        }
    }

    let step = lanes.get(idx).block.step(line, core, keyed, idx == ROOT_LANE, out);
    // A lane inside a Surefire block has no pending javac continuations:
    // entering a block retires any stale armed claim, so a single lane can't
    // hold a permanent armed-vs-block tie against raw-line routing.
    if lanes.get(idx).block.in_block {
        lanes.get(idx).keep_continuation = false;
    }
    match step {
        SurefireStep::Consumed => None,
        SurefireStep::FailingClose {
            running,
            lines,
            close,
        } => {
            if classes.admit() {
                lanes.get(idx).block.commit_failing(out, running, &lines, close);
            } else {
                lanes.get(idx).block.drop_failing();
            }
            // While the trail is active, raw_owner routes by trail uniqueness;
            // `hot` claims only the nothing-open fallback for stray raw lines
            // after the trail ends.
            lanes.hot = idx;
            lanes.get(idx).keep_continuation = false;
            None
        }
        SurefireStep::Passthrough => Some((idx, core, keyed)),
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
pub fn filter_surefire(raw: &str, daemon: bool) -> String {
    filter_surefire_with_cap(raw, MAX_MVN_FAILING_CLASSES, daemon)
}

fn filter_surefire_with_cap(raw: &str, cap: usize, daemon: bool) -> String {
    let stripped = strip_ansi(raw);
    if !has_english_footer(&stripped) {
        return stripped;
    }

    let mut out = String::new();
    let mut lanes = Lanes::new();
    let mut classes = FailingClassCap::new(cap);
    let mut summary = FailuresSummaryCap::new(cap, daemon);
    let mut in_reactor_summary = false;

    for line in stripped.lines() {
        let (idx, core, keyed) = match drive_surefire_line(
            &mut lanes, line, &mut classes, &mut summary, daemon, &mut out,
        ) {
            Some(v) => v,
            None => continue,
        };
        if lanes.get(idx).keep_continuation && (core.starts_with(' ') || core.starts_with('\t')) {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Failures-summary cap: gate `[ERROR]   ` entries, emit `+N more` tail
        // before AGG. The helper consumes only summary entries — other lines
        // (header, AGG) fall through to the keep-list below.
        {
            let lane = lanes.get(idx);
            if summary.handle_entry(lane.in_summary, &mut lane.dropped, core, line, &mut out) {
                continue;
            }
        }

        // Order matters: call reactor_summary_keep first so its BUILD_FOOT
        // clears-flag side effect always runs regardless of `||` short-circuit.
        let reactor_keep = reactor_summary_keep(core, &mut in_reactor_summary);
        if reactor_keep || keep_outside_block(core, daemon && is_tag_prefixed(line, core)) {
            let lane = lanes.get(idx);
            // Pre-emit this lane's own summary tail when we're about to
            // write its own AGG (never another lane's — see the attribution
            // invariant on `FailuresSummaryCap`).
            summary.handle_aggregate(core, &mut lane.dropped, &mut out, &mut lane.in_summary);
            // Detect summary header so subsequent `[ERROR]   ` entries get capped.
            summary.handle_header(core, &mut lane.in_summary, &mut lane.dropped);
            out.push_str(line);
            out.push('\n');
            // The armed per-lane flag is an owner claim in its own right:
            // raw_owner routes (or verbatim-preserves) the raw indented
            // continuations by scanning all lanes for it. Only this lane's
            // own keyed lines may rewrite the claim (a kept raw line can't
            // arm anyway — starting with `[ERROR]` would have keyed it).
            if keyed {
                // Invariant: on the root (untagged) lane, any `[ERROR]` line
                // arms the claim — plain `mvn` never mints tagged lanes, so
                // there is no phantom-tag risk there, and this preserves the
                // pre-PR arming behavior exactly. On a tagged (mvnd module)
                // lane, only a genuine compiler diagnostic
                // (`file.java:[line,col]`) arms it — an `[ERROR]`-shaped app
                // log (or the Surefire summary/close lines excluded below)
                // can open a lane but can never arm it, so a one-off stray
                // line can't hold a claim open.
                lanes.get(idx).keep_continuation = core.starts_with("[ERROR]")
                    && !core.starts_with("[ERROR] Tests run:")
                    && !core.starts_with("[ERROR] Failures:")
                    && !core.starts_with("[ERROR] Errors:")
                    && (idx == ROOT_LANE || FILE_COORD.is_match(core));
            }
            continue;
        }
        // Dropped keyed line (e.g. help boilerplate): reset so a stale flag
        // can't keep an indented line that follows a dropped `[ERROR]` line.
        // Parity with filter_package's fall-through reset. On a *tagged*
        // (mvnd module) lane, raw fall-through lines never disarm: a raw
        // line only routed here *because of* the armed claim, so letting it
        // clear that claim would drop the `symbol:` / `location:`
        // continuations whenever another module's stray stdout lands in the
        // arm window. The root (untagged) lane has no such ambiguity — it's
        // the only lane a plain `mvn` run ever uses, so every raw line is
        // genuinely this lane's own content — and disarms on any
        // fall-through line, keyed or not: this is pre-d602a3b behavior,
        // restored so a stale root-lane claim can't survive an intervening
        // raw line (e.g. an exception message) and wrongly keep unrelated
        // indented lines (e.g. stack frames) that follow it.
        if keyed || idx == ROOT_LANE {
            lanes.get(idx).keep_continuation = false;
        }
    }

    lanes.finish(&mut out);
    lanes.finish_summaries(&mut out);
    classes.finish(&mut out);
    out
}

// ── Compile filter ──────────────────────────────────────────────────────────

/// Buffered single-pass filter for `mvn compile` / `test-compile`.
///
/// Keeps module banners, `[INFO] Building …`, `[INFO] BUILD …`, totals, finish
/// time, scanning line, install lines, and `[ERROR]` blocks with indented
/// continuation (`  symbol:`, `  ^`, `  required:`). Deduplicates `[WARNING]`
/// lines by normalised message (strip file coordinates).
pub fn filter_compile(raw: &str, daemon: bool) -> String {
    let stripped = strip_ansi(raw);
    if !has_english_footer(&stripped) {
        return stripped;
    }

    let mut out = String::new();
    // Continuation ownership is per module: javac emits `symbol:` / `location:`
    // as raw indented lines *after* the `[ERROR] … cannot find symbol` line, and
    // mvnd interleaves reactor modules, so a `[child-b] [INFO]` line landing in
    // between must not clear the flag armed by `[child-a] [ERROR]`. Compile
    // never opens Surefire blocks, so `route` resolves raw lines to the unique
    // armed lane (or preserves them verbatim when several lanes are armed).
    // `daemon` gates the whole lane layer — see `split_lane`.
    let mut lanes = Lanes::new();
    let mut seen_warnings: HashSet<String> = HashSet::new();

    for line in stripped.lines() {
        // Classify on the module-prefix-stripped view; emit the original so
        // module identity survives in mvnd parallel reactors.
        let (key, core) = split_lane(line, daemon);
        let (idx, keyed) = match lanes.route(key, core) {
            Some(v) => v,
            // Reachable when two modules are armed concurrently (a tie):
            // preserve the raw line verbatim rather than guess an owner.
            None => {
                out.push_str(line);
                out.push('\n');
                continue;
            }
        };
        if MODULE_BANNER.is_match(core) {
            out.push_str(line);
            out.push('\n');
            // Uniform with every other disarm site: only this lane's own
            // keyed line may rewrite its claim. Provably unreachable with
            // `keyed == false` today (`is_lane_opener` already requires a
            // `MODULE_BANNER` match to establish a lane), but guarded here
            // too so that invariant doesn't have to hold forever for safety.
            if keyed {
                lanes.get(idx).keep_continuation = false;
            }
            continue;
        }
        // `[INFO] Building ` gated the same way as `keep_outside_block` —
        // see the cold-preclear finding on that function's doc comment.
        if BUILD_FOOT.is_match(core)
            || (core.starts_with("[INFO] Building ")
                && (!(daemon && is_tag_prefixed(line, core))
                    || BUILDING_MODULE_HEADER.is_match(core)))
            || core.starts_with("[INFO] Total time:")
            || core.starts_with("[INFO] Finished at:")
            || core.starts_with("[INFO] Scanning ")
        {
            out.push_str(line);
            out.push('\n');
            if keyed {
                lanes.get(idx).keep_continuation = false;
            }
            continue;
        }
        // Help boilerplate: drop before the `[ERROR]` catch-all (parity with
        // keep_outside_block / filter_quiet). Raw boilerplate must not
        // disarm the claim that routed it here — see the fall-through reset.
        if is_boilerplate(core) {
            if keyed {
                lanes.get(idx).keep_continuation = false;
            }
            continue;
        }
        if core.starts_with("[ERROR]") {
            out.push_str(line);
            out.push('\n');
            // Armed flag is an owner claim scanned by raw_owner — no `hot`
            // bookkeeping needed. Root lane: any `[ERROR]` arms (pre-PR
            // behavior, no phantom-tag risk on plain `mvn`). Tagged lane:
            // only a genuine compiler diagnostic (`file.java:[line,col]`)
            // arms — see the matching invariant on the Surefire/package arm
            // sites. Only this lane's own keyed line may rewrite its claim
            // (parity with every other disarm/arm site in this function): a
            // never-established tag's `[ERROR]` app log (e.g. `[main]
            // [ERROR] connection pool exhausted`) reaches here via
            // `raw_owner`'s fallback with `keyed == false` whenever this
            // lane is the uniquely armed one, and rewriting the claim on
            // that raw-routed line would silently disarm it, dropping the
            // `symbol:`/`location:` continuations that follow.
            if keyed {
                lanes.get(idx).keep_continuation = idx == ROOT_LANE || FILE_COORD.is_match(core);
            }
            continue;
        }
        if lanes.get(idx).keep_continuation && (core.starts_with(' ') || core.starts_with('\t')) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if core.starts_with("[WARNING]") {
            let payload = core.strip_prefix("[WARNING] ").unwrap_or(core);
            let norm = FILE_COORD.replace_all(payload, "").to_string();
            if seen_warnings.insert(norm) {
                out.push_str(line);
                out.push('\n');
            }
            // Only this lane's own keyed line may rewrite its claim — a
            // `[tag] [WARNING] …` line for a never-established tag routes
            // here (via raw_owner's fallback) with `keyed == false` when
            // this lane happens to be armed, and clearing the claim on that
            // raw-routed line would drop the `symbol:` / `location:`
            // continuations that follow. See the fall-through reset below.
            if keyed {
                lanes.get(idx).keep_continuation = false;
            }
            continue;
        }
        // Drop everything else. On a *tagged* lane, only keyed lines disarm:
        // a raw stray only routed to this lane because of its armed claim,
        // and clearing it would drop the `symbol:` / `location:`
        // continuations that follow. The root lane disarms on any
        // fall-through line — see the matching comment in
        // `filter_surefire_with_cap`.
        if keyed || idx == ROOT_LANE {
            lanes.get(idx).keep_continuation = false;
        }
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
pub fn filter_package(raw: &str, daemon: bool) -> String {
    filter_package_with_cap(raw, MAX_MVN_FAILING_CLASSES, daemon)
}

fn filter_package_with_cap(raw: &str, cap: usize, daemon: bool) -> String {
    let stripped = strip_ansi(raw);
    if !has_english_footer(&stripped) {
        return stripped;
    }

    let mut out = String::new();
    // Per-module lanes + raw-line routing: see drive_surefire_line.
    let mut lanes = Lanes::new();
    let mut classes = FailingClassCap::new(cap);
    let mut summary = FailuresSummaryCap::new(cap, daemon);
    let mut in_reactor_summary = false;
    // Warning dedup is deliberately global: the same warning surfacing from
    // several reactor modules is still the same warning.
    let mut seen_warnings: HashSet<String> = HashSet::new();

    for line in stripped.lines() {
        let (idx, core, keyed) = match drive_surefire_line(
            &mut lanes, line, &mut classes, &mut summary, daemon, &mut out,
        ) {
            Some(v) => v,
            None => continue,
        };
        // Failures-summary cap (see filter_surefire_with_cap for details).
        {
            let lane = lanes.get(idx);
            if summary.handle_entry(lane.in_summary, &mut lane.dropped, core, line, &mut out) {
                continue;
            }
        }

        // Order matters: call reactor_summary_keep first so its BUILD_FOOT
        // clears-flag side effect always runs regardless of `||` short-circuit.
        let reactor_keep = reactor_summary_keep(core, &mut in_reactor_summary);
        // Outside any Surefire block: compile-keep AND surefire-outside-keep merge.
        if reactor_keep
            || MODULE_BANNER.is_match(core)
            || keep_outside_block(core, daemon && is_tag_prefixed(line, core))
        {
            let lane = lanes.get(idx);
            summary.handle_aggregate(core, &mut lane.dropped, &mut out, &mut lane.in_summary);
            summary.handle_header(core, &mut lane.in_summary, &mut lane.dropped);
            out.push_str(line);
            out.push('\n');
            // Armed flag is an owner claim scanned by raw_owner; only keyed
            // lines rewrite it — see filter_surefire_with_cap.
            if keyed {
                // Invariant: on the root (untagged) lane, any `[ERROR]` line
                // arms the claim — plain `mvn` never mints tagged lanes, so
                // there is no phantom-tag risk there, and this preserves the
                // pre-PR arming behavior exactly. On a tagged (mvnd module)
                // lane, only a genuine compiler diagnostic
                // (`file.java:[line,col]`) arms it — an `[ERROR]`-shaped app
                // log (or the Surefire summary/close lines excluded below)
                // can open a lane but can never arm it, so a one-off stray
                // line can't hold a claim open.
                lanes.get(idx).keep_continuation = core.starts_with("[ERROR]")
                    && !core.starts_with("[ERROR] Tests run:")
                    && !core.starts_with("[ERROR] Failures:")
                    && !core.starts_with("[ERROR] Errors:")
                    && (idx == ROOT_LANE || FILE_COORD.is_match(core));
            }
            continue;
        }
        if lanes.get(idx).keep_continuation && (core.starts_with(' ') || core.starts_with('\t')) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if core.starts_with("[WARNING]") {
            let payload = core.strip_prefix("[WARNING] ").unwrap_or(core);
            let norm = FILE_COORD.replace_all(payload, "").to_string();
            if seen_warnings.insert(norm) {
                out.push_str(line);
                out.push('\n');
            }
            // Only this lane's own keyed line may rewrite its claim — a
            // `[tag] [WARNING] …` line for a never-established tag reaches
            // here via raw_owner's fallback (`keyed == false`) whenever this
            // lane is the one currently armed, and clearing the claim on
            // that raw-routed line would drop the `symbol:` / `location:`
            // continuations that follow. See the fall-through reset below.
            if keyed {
                lanes.get(idx).keep_continuation = false;
            }
            continue;
        }
        // Tagged-lane raw fall-through lines never disarm; the root lane
        // disarms on any fall-through line — see filter_surefire_with_cap.
        if keyed || idx == ROOT_LANE {
            lanes.get(idx).keep_continuation = false;
        }
    }

    lanes.finish(&mut out);
    lanes.finish_summaries(&mut out);
    classes.finish(&mut out);
    out
}

// ── Quiet-mode filter ───────────────────────────────────────────────────────

/// Strip an mvnd module prefix (`[module] `) so quiet-mode classification
/// sees the same line shape it does for unprefixed output. Module tags are
/// artifactIds — non-empty and whitespace-free — so a bracketed fragment
/// that holds whitespace, or that is itself a log level, is left alone.
/// Callers always emit the *original* line; this only decides how the line
/// is classified.
///
/// Gated on `daemon` for the same reason [`split_lane`] is: `[tag] [LEVEL] …`
/// is byte-identical whether `tag` is an mvnd module or an application thread
/// name from `[%thread] [%level]` logging, and only the daemon emits the
/// former. Applying the heuristic to plain `mvn` would let ordinary test
/// stdout be classified — and therefore dropped — as Maven's own boilerplate.
fn strip_module_tag(line: &str, daemon: bool) -> &str {
    if !daemon {
        return line;
    }
    let Some(rest) = line.strip_prefix('[') else {
        return line;
    };
    let Some(end) = rest.find("] ") else {
        return line;
    };
    let tag = &rest[..end];
    if tag.is_empty() || tag.chars().any(char::is_whitespace) || is_log_level(tag) {
        return line;
    }
    &rest[end + 2..]
}

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
pub fn filter_quiet(raw: &str, daemon: bool) -> String {
    let stripped = strip_ansi(raw);
    if stripped.trim().is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let mut failure_trail = false;

    for line in stripped.lines() {
        // mvnd prefixes each parallel module's own log lines with `[module] `;
        // classify on the unprefixed shape, emit the line as it arrived.
        let core = strip_module_tag(line, daemon);

        // Surefire close-line for a failed class — keep + enter failure trail.
        if CLOSE.is_match(core) {
            out.push_str(line);
            out.push('\n');
            failure_trail =
                core.contains("<<< FAILURE!") || core.contains("<<< ERROR!");
            continue;
        }

        // Per-test failure subline: `[ERROR] FQN.method -- Time elapsed: … <<< FAILURE!`
        // (or `<<< ERROR!` for thrown exceptions).
        if is_per_test_subline(core) {
            out.push_str(line);
            out.push('\n');
            failure_trail = true;
            continue;
        }

        // Failure-trail body: exception class, user-code frames; drop framework frames.
        if failure_trail {
            // A module's own blank line terminates its trail. mvnd's
            // terminator carries the module prefix and is emitted as it
            // arrived; plain `mvn`'s is a bare blank line, and base parity
            // means emitting exactly that — including when the terminator is
            // whitespace-only rather than empty.
            if core.trim().is_empty() {
                if daemon {
                    out.push_str(line);
                }
                out.push('\n');
                failure_trail = false;
                continue;
            }
            let t = core.trim_start();
            if t.starts_with("at ") && is_framework_frame(t) {
                continue;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Failure summary keepers.
        if core.starts_with("[ERROR] Tests run:")
            || core.starts_with("[ERROR] Failures:")
            || core.starts_with("[ERROR] Errors:")
            || core.starts_with("[ERROR]   ")
            || core.starts_with("[ERROR] Failed to execute goal")
        {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Drop post-failure help boilerplate and bare `[ERROR]` dividers
        // (shared with the non-quiet filters — see BOILER_PREFIXES).
        if is_boilerplate(core) {
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

/// Maven Daemon (`mvnd`) has no project-local wrapper of its own, so it is
/// never substituted by `./mvnw`: the user asked for the daemon explicitly.
fn mvn_binary(daemon: bool) -> &'static str {
    if daemon {
        "mvnd"
    } else if cfg!(windows) {
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

fn new_mvn_command(args: &[String], daemon: bool) -> Command {
    let mut cmd = if daemon {
        resolved_command("mvnd")
    } else if cfg!(windows) {
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
    run_tool(args, false, verbose)
}

/// `rtk mvnd` — Maven Daemon. Non-interactive `mvnd` output is plain Maven
/// output (the rolling console UI only engages on a TTY), so the same phase
/// detection and filters apply; only the executed binary differs.
pub fn run_daemon(args: &[String], verbose: u8) -> Result<i32> {
    run_tool(args, true, verbose)
}

fn run_tool(args: &[String], daemon: bool, verbose: u8) -> Result<i32> {
    // Verbose flags bypass filtering — user wants full output.
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "-X" | "--debug" | "-e" | "--errors"))
    {
        let osargs: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough(mvn_binary(daemon), &osargs, verbose);
    }

    let tool = mvn_binary(daemon);
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
            new_mvn_command(args, daemon),
            tool,
            &args_display,
            |raw: &str| filter_quiet(raw, daemon),
            RunOptions::with_tee("mvn_quiet"),
        );
    }

    let phase = detect_phase(args);

    match phase {
        MvnPhase::Test => runner::run_filtered(
            new_mvn_command(args, daemon),
            tool,
            &args_display,
            move |raw: &str| filter_surefire(raw, daemon),
            RunOptions::with_tee("mvn_test"),
        ),
        MvnPhase::Compile => runner::run_filtered(
            new_mvn_command(args, daemon),
            tool,
            &args_display,
            move |raw: &str| filter_compile(raw, daemon),
            RunOptions::with_tee("mvn_compile"),
        ),
        MvnPhase::Package => runner::run_filtered(
            new_mvn_command(args, daemon),
            tool,
            &args_display,
            move |raw: &str| filter_package(raw, daemon),
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

    /// Cold-preclear finding (upstream PR #3199, fourth review round): the
    /// `[INFO] Building ` keeper was a bare `starts_with`, so on a *tagged*
    /// lane it kept any app log merely shaped like one — mvnd tags a
    /// module's entire output stream with its own module tag regardless of
    /// which thread emitted a line, so background/pool logging from a
    /// module's own code (`[child-a] [INFO] Building segment N of the data
    /// pipeline`) is exactly as opener/keeper-shaped as mvnd's own genuine
    /// `Building <name> <version> [n/m]` header. Probe: 50 such lines in an
    /// otherwise-green single-module run kept wholesale pre-fix (bloating
    /// output on a real reactor's 2000-line equivalent to ~0.1% savings);
    /// post-fix, dropped, same as plain `mvn` already drops any `[INFO]`
    /// line outside its keep-list.
    #[test]
    fn daemon_mode_building_keeper_drops_app_log_spam_on_tagged_lane() {
        let mut i = String::from(
            "[INFO] Scanning for projects...\n\
             [child-a] [INFO] ----------------------< com.example.rtk:child-a >-----------------------\n\
             [child-a] [INFO] Building child-a 1.0.0-SNAPSHOT                                    [1/1]\n\
             [child-a] [INFO] Running com.example.rtk.APassTest\n\
             [child-a] [INFO] Tests run: 1, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.01 s -- in com.example.rtk.APassTest\n",
        );
        for n in 0..50 {
            i.push_str(&format!(
                "[child-a] [INFO] Building segment {n} of the data pipeline\n"
            ));
        }
        i.push_str("[INFO] BUILD SUCCESS\n");
        let o = filter_surefire(&i, true);
        assert!(
            !o.contains("Building segment"),
            "app-log spam shaped like a Building header is dropped on a tagged lane; got:\n{o}"
        );
        assert!(
            o.contains("< com.example.rtk:child-a >") && o.contains("BUILD SUCCESS"),
            "genuine module banner and footer survive; got:\n{o}"
        );
        assert!(
            o.len() < 400,
            "savings recover once the spam is dropped (input was {} bytes); got {} bytes:\n{o}",
            i.len(),
            o.len()
        );
    }

    /// Real-fixture control for the same gate: mvnd's own genuine
    /// `Building <name> <version> [n/m]` headers (the shape
    /// [`BUILDING_MODULE_HEADER`] requires) must still survive on tagged
    /// lanes — the gate narrows, it doesn't drop real content. Already
    /// covered end-to-end by the `mvnd_reactor_pass_full_output` /
    /// `mvnd_parallel_reactor_fail_full_output` fixture-diff tests staying
    /// green (both fixtures' `Building … [n/3]` lines are asserted present
    /// via `include_str!`-fixture byte equality); this test pins the
    /// specific line directly, independent of the fuller snapshot.
    #[test]
    fn daemon_mode_building_keeper_keeps_real_module_header() {
        let i = include_str!("../../../tests/fixtures/mvnd_reactor_pass_raw.txt");
        let o = filter_package(i, true);
        assert!(
            o.contains("Building child-a 1.0.0-SNAPSHOT") && o.contains("Building child-b 1.0.0-SNAPSHOT"),
            "genuine `[n/m]`-numbered module headers survive on tagged lanes; got:\n{o}"
        );
    }

    /// KuSh's exact probe (resolving the round-5 RUNNING residual, upstream
    /// PR #3199, fifth review round): 200 multi-word `[main] [INFO] Running
    /// …` app-log lines inside an otherwise-green module. Pre-fix (`RUNNING`
    /// itself as the opener test), each one minted its own phantom lane and
    /// flushed at end-of-stream — 3.1% savings on the equivalent full-size
    /// probe. Post-fix (`RUNNING_MODULE_HEADER`'s whitespace-free-FQCN
    /// requirement), none of them are lane-openers, so they fall back to
    /// `raw_owner` and are dropped like any other unkeyed `[INFO]` line
    /// outside a keep-list — 98%+ savings.
    #[test]
    fn app_log_running_lines_dropped_in_green_daemon_run() {
        let mut i = String::from(
            "[INFO] Scanning for projects...\n\
             [child-a] [INFO] Running com.example.rtk.APassTest\n\
             [child-a] [INFO] Tests run: 1, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.01 s -- in com.example.rtk.APassTest\n",
        );
        for n in 0..200 {
            i.push_str(&format!(
                "[main] [INFO] Running the widget pipeline for tenant acme {n}\n"
            ));
        }
        i.push_str("[INFO] BUILD SUCCESS\n");
        let o = filter_surefire(&i, true);
        assert!(
            !o.contains("widget pipeline"),
            "multi-word Running-shaped app log lines are dropped, not kept via a \
             phantom lane; got:\n{o}"
        );
        assert!(
            o.len() < 500,
            "savings recover once the phantom lanes stop minting (input was {} bytes); \
             got {} bytes:\n{o}",
            i.len(),
            o.len()
        );
    }

    /// Same multi-word Running-shaped app-log line, but landing inside a
    /// module's genuinely open (failing) block instead of a green run: with
    /// no fixture-grounded shape to open its own lane on, it falls back to
    /// `Lanes::raw_owner`, which routes it to the one uniquely open block —
    /// same rule any other raw app-log line follows — so it's buffered into
    /// that block and survives at its own input position (between the
    /// `Running` line it followed and the failing close), not lost and not
    /// reordered to end-of-stream.
    #[test]
    fn app_log_running_line_rides_raw_owner_inside_failing_block() {
        let i = "[INFO] Scanning for projects...\n\
                  [child-a] [INFO] Running com.example.rtk.AFailTest\n\
                  [main] [INFO] Running the widget pipeline for tenant acme\n\
                  [child-a] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.AFailTest\n\
                  [child-a] [ERROR] com.example.rtk.AFailTest.foo -- Time elapsed: 0.02 s <<< FAILURE!\n\
                  java.lang.AssertionError: boom\n\
                  \tat com.example.rtk.AFailTest.foo(AFailTest.java:10)\n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i, true);
        let running = o.find("Running com.example.rtk.AFailTest").expect("Running kept");
        let pipeline = o
            .find("Running the widget pipeline for tenant acme")
            .expect("app-log line kept, buffered inside the block");
        let close = o
            .find("<<< FAILURE! -- in com.example.rtk.AFailTest")
            .expect("close line kept");
        assert!(
            running < pipeline && pipeline < close,
            "app-log line stays at its own input position inside the block \
             (Running, then the app log, then the close), not lost or moved \
             to end-of-stream; got:\n{o}"
        );
    }

    /// A real Surefire class-start line still opens its module's lane, so the
    /// narrowing above cannot cost a genuine reactor module its routing.
    #[test]
    fn real_running_line_still_mints_a_lane() {
        let i = "[INFO] Scanning for projects...\n\
                  [child-a] [INFO] Running com.example.rtk.AFailTest\n\
                  [child-a] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.02 s <<< FAILURE! -- in com.example.rtk.AFailTest\n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i, true);
        assert!(
            o.contains("Running com.example.rtk.AFailTest"),
            "a genuine class-start line opens the lane and is kept with its failing \
             block; got:\n{o}"
        );
    }

    /// An app log shaped like a module header is gated on how the line
    /// *arrived* (tag-prefixed), not on which lane it was routed to: an
    /// unrecognized tag falls back to `raw_owner`, which can land on the root
    /// lane, and a gate keyed on the destination would switch itself off
    /// exactly there.
    #[test]
    fn daemon_building_applog_dropped_with_no_block_open() {
        let mut i = String::from("[INFO] Scanning for projects...\n[INFO] Building web 2.1.0 [1/1]\n");
        for n in 0..10 {
            i.push_str(&format!(
                "[pool-1] [INFO] Building segment {n} of the data pipeline\n"
            ));
        }
        i.push_str("[INFO] BUILD SUCCESS\n");
        let o = filter_surefire(&i, true);
        assert_eq!(
            o.matches("of the data pipeline").count(),
            0,
            "tag-prefixed `Building` app logs never carry the reactor counter and \
             are dropped even when no lane's block is open; got:\n{o}"
        );
        assert!(
            o.contains("[INFO] Building web 2.1.0 [1/1]"),
            "the real reactor-numbered module header survives; got:\n{o}"
        );
    }

    /// The failures budget is keyed to the plugin *family*, so version and
    /// execution-id differences between modules — both per-module
    /// configuration — stay inside one phase and share one budget.
    #[test]
    fn summary_cap_survives_per_module_plugin_coordinates() {
        let block = |tag: &str, banner: &str, p: &str| {
            let mut s = format!("[{tag}] [INFO] --- {banner} @ {tag} ---\n");
            s.push_str(&format!("[{tag}] [ERROR] Failures: \n"));
            for n in 0..3 {
                s.push_str(&format!("[{tag}] [ERROR]   {p}{n}:1{n} boom {p}{n}\n"));
            }
            s.push_str(&format!(
                "[{tag}] [ERROR] Tests run: 3, Failures: 3, Errors: 0, Skipped: 0\n"
            ));
            s
        };
        for (label, b_banner) in [
            ("same coordinates", "surefire:3.5.5:test (default-test)"),
            ("different version", "surefire:3.2.2:test (default-test)"),
            ("different execution id", "surefire:3.5.5:test (unit-tests)"),
            ("legacy spelling", "maven-surefire-plugin:2.22.2:test (default-test)"),
        ] {
            let i = format!(
                "[INFO] Scanning for projects...\n{}{}[INFO] BUILD FAILURE\n",
                block("a", "surefire:3.5.5:test (default-test)", "A"),
                block("b", b_banner, "B"),
            );
            let o = filter_package_with_cap(&i, 2, true);
            assert_eq!(
                o.matches("boom ").count(),
                2,
                "{label}: one phase, one reactor-wide budget; got:\n{o}"
            );
        }
    }

    /// A genuine Surefire -> Failsafe transition is still a phase boundary and
    /// still earns its own budget.
    #[test]
    fn summary_cap_resets_on_surefire_to_failsafe() {
        let i = "[INFO] Scanning for projects...\n\
                  [a] [INFO] --- surefire:3.5.5:test (default-test) @ a ---\n\
                  [a] [ERROR] Failures: \n\
                  [a] [ERROR]   A0:10 boom A0\n\
                  [a] [ERROR]   A1:11 boom A1\n\
                  [a] [ERROR]   A2:12 boom A2\n\
                  [a] [ERROR] Tests run: 3, Failures: 3, Errors: 0, Skipped: 0\n\
                  [a] [INFO] --- failsafe:3.5.5:integration-test (default-integration-test) @ a ---\n\
                  [a] [ERROR] Failures: \n\
                  [a] [ERROR]   C0:20 boom C0\n\
                  [a] [ERROR]   C1:21 boom C1\n\
                  [a] [ERROR]   C2:22 boom C2\n\
                  [a] [ERROR] Tests run: 3, Failures: 3, Errors: 0, Skipped: 0\n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_package_with_cap(i, 2, true);
        assert_eq!(
            o.matches("boom ").count(),
            4,
            "unit and integration phases each get their own budget; got:\n{o}"
        );
    }

    /// Quiet mode classifies a module-prefixed reactor line exactly as it
    /// classifies the same line unprefixed — mvnd's `[module] ` prefix is
    /// presentation, not signal.
    #[test]
    fn quiet_mode_classifies_module_prefixed_lines() {
        let plain = "[ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.1 s <<< FAILURE! -- in com.example.a.T\n\
                      [ERROR] com.example.a.T.x -- Time elapsed: 0.02 s <<< FAILURE!\n\
                      org.opentest4j.AssertionFailedError: boom\n\
                      \tat org.junit.jupiter.api.AssertionUtils.fail(AssertionUtils.java:38)\n\
                      \tat com.example.a.T.x(T.java:42)\n\
                      \n\
                      [ERROR] -> [Help 1]\n";
        let tagged: String = plain
            .lines()
            .map(|l| format!("[child-a] {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        let o = filter_quiet(&tagged, true);
        assert!(
            o.contains("com.example.a.T.x(T.java:42)"),
            "user-code frame kept; got:\n{o}"
        );
        assert!(
            !o.contains("AssertionUtils.fail"),
            "framework frame dropped; got:\n{o}"
        );
        assert!(!o.contains("[Help 1]"), "boilerplate dropped; got:\n{o}");
        // Strongest form of the claim: strip the prefix back off and the two
        // runs must agree line for line, not merely in line count.
        let unprefixed: String = o
            .lines()
            .map(|l| l.strip_prefix("[child-a] ").unwrap_or(l).trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        let expected: String = filter_quiet(plain, false)
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            unprefixed, expected,
            "prefixed and unprefixed runs keep the same lines"
        );
    }

    /// Plain `mvn` never emits a module prefix, so the tag heuristic must not
    /// run there: `[%thread] [%level]` application logging is byte-identical
    /// to a tagged Maven line, and classifying it would let ordinary test
    /// diagnostics be dropped as Maven's own boilerplate.
    #[test]
    fn quiet_mode_keeps_thread_tagged_app_logs_for_plain_mvn() {
        let i = "[ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.1 s <<< FAILURE! -- in com.example.T\n\
                  [ERROR] com.example.T.x -- Time elapsed: 0.02 s <<< FAILURE!\n\
                  java.lang.AssertionError: boom\n\
                  \tat com.example.T.x(T.java:42)\n\
                  \n\
                  [main] [ERROR] See https://docs.example.com/troubleshoot for details\n\
                  [main] [ERROR] Re-run Maven with the flag we documented\n\
                  [worker-1] [ERROR] For more information about the widget cache\n\
                  [ERROR] Failed to execute goal org.apache.maven.plugins:maven-surefire-plugin:3.5.5:test\n";
        let o = filter_quiet(i, false);
        for want in [
            "See https://docs.example.com/troubleshoot",
            "Re-run Maven with the flag we documented",
            "For more information about the widget cache",
        ] {
            assert!(
                o.contains(want),
                "application diagnostics survive plain `mvn -q`; missing {want:?} in:\n{o}"
            );
        }
    }

    /// Maven prints a plugin's banner before any of that plugin's output, so
    /// a module's lane is always minted by its `--- surefire:… @ mod ---`
    /// banner before the first `Running` line arrives. That is what keeps the
    /// [`RUNNING_MODULE_HEADER`] narrowing from costing a class its block: by
    /// the time a `Running` line of any shape shows up, its tag is already a
    /// known lane and `is_lane_opener` is no longer consulted for it.
    #[test]
    fn plugin_banner_mints_the_lane_before_any_running_line() {
        let i = "[INFO] Scanning for projects...\n\
                  [child-a] [INFO] --- surefire:3.5.5:test (default-test) @ child-a ---\n\
                  [child-a] [INFO] Running Regression Tests\n\
                  [child-a] [ERROR] Tests run: 3, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.4 s <<< FAILURE! -- in Regression Tests\n\
                  [child-a] [ERROR] com.example.T.x -- Time elapsed: 0.02 s <<< FAILURE!\n\
                  [child-a] java.lang.AssertionError: boom\n\
                  [child-a] \tat com.example.T.x(T.java:42)\n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i, true);
        for want in [
            "Running Regression Tests",
            "Tests run: 3, Failures: 1",
            "java.lang.AssertionError: boom",
            "T.java:42",
        ] {
            assert!(
                o.contains(want),
                "a banner-minted lane keeps its failing class whole, whatever \
                 shape the `Running` line takes; missing {want:?} in:\n{o}"
            );
        }
    }

    // Thin `daemon`-fixed wrappers so the bulk of the test suite (written
    // before the `daemon` gate — cold-preclear finding, upstream PR #3199,
    // third review round) keeps its original `fn(&str) -> String` /
    // `fn(&str, usize) -> String` call shape. `_plain` == real `mvn`
    // (`daemon == false`, base parity by construction); `_daemon` == `mvnd`.
    fn filter_surefire_plain(raw: &str) -> String {
        filter_surefire(raw, false)
    }
    fn filter_surefire_daemon(raw: &str) -> String {
        filter_surefire(raw, true)
    }
    fn filter_compile_plain(raw: &str) -> String {
        filter_compile(raw, false)
    }
    fn filter_compile_daemon(raw: &str) -> String {
        filter_compile(raw, true)
    }
    fn filter_package_plain(raw: &str) -> String {
        filter_package(raw, false)
    }
    fn filter_package_daemon(raw: &str) -> String {
        filter_package(raw, true)
    }
    fn filter_surefire_with_cap_plain(raw: &str, cap: usize) -> String {
        filter_surefire_with_cap(raw, cap, false)
    }
    fn filter_surefire_with_cap_daemon(raw: &str, cap: usize) -> String {
        filter_surefire_with_cap(raw, cap, true)
    }
    fn filter_package_with_cap_plain(raw: &str, cap: usize) -> String {
        filter_package_with_cap(raw, cap, false)
    }
    fn filter_package_with_cap_daemon(raw: &str, cap: usize) -> String {
        filter_package_with_cap(raw, cap, true)
    }

    /// Cold-preclear finding #2 (upstream PR #3199, third review round),
    /// KuSh's "fuller two-module probe": module banners, `Running`, genuine
    /// failing closes (Surefire 3.x multi-failure shape — one class per
    /// module, two failing methods each), per-module `Results:`/`Failures:`,
    /// and interleaved `AGG` lines — modeled on `mvnd_reactor_fail_raw.txt`'s
    /// real shape, extended with enough failures to exceed the cap. Asserts
    /// all three things the finding named: the reactor-wide budget still
    /// engages on real (non-root) lanes (2 kept, not 4), each lane's tail is
    /// attributed to its own AGG (never the other lane's), and output order
    /// stays sane (banners before Running before closes before the capped
    /// summary before the reactor footer).
    fn assert_fuller_two_module_probe_engages_cap(filter: fn(&str, usize) -> String) {
        let i = "[INFO] Scanning for projects...\n\
             [child-b] [INFO] ----------------------< com.example.rtk:child-b >-----------------------\n\
             [child-b] [INFO] Building child-b 1.0.0-SNAPSHOT\n\
             [child-a] [INFO] ----------------------< com.example.rtk:child-a >-----------------------\n\
             [child-a] [INFO] Building child-a 1.0.0-SNAPSHOT\n\
             [child-a] [INFO] Running com.example.rtk.AMultiFailTest\n\
             [child-b] [INFO] Running com.example.rtk.BMultiFailTest\n\
             [child-a] [ERROR] Tests run: 2, Failures: 2, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.AMultiFailTest\n\
             [child-a] [ERROR] com.example.rtk.AMultiFailTest.first -- Time elapsed: 0.02 s <<< FAILURE!\n\
             java.lang.AssertionError: a1 boom\n\
             \tat com.example.rtk.AMultiFailTest.first(AMultiFailTest.java:10)\n\
             [child-a] [INFO] \n\
             [child-b] [ERROR] Tests run: 2, Failures: 2, Errors: 0, Skipped: 0, Time elapsed: 0.04 s <<< FAILURE! -- in com.example.rtk.BMultiFailTest\n\
             [child-b] [ERROR] com.example.rtk.BMultiFailTest.first -- Time elapsed: 0.02 s <<< FAILURE!\n\
             java.lang.AssertionError: b1 boom\n\
             \tat com.example.rtk.BMultiFailTest.first(BMultiFailTest.java:10)\n\
             [child-b] [INFO] \n\
             [child-a] [ERROR] com.example.rtk.AMultiFailTest.second -- Time elapsed: 0.02 s <<< FAILURE!\n\
             java.lang.AssertionError: a2 boom\n\
             \tat com.example.rtk.AMultiFailTest.second(AMultiFailTest.java:20)\n\
             [child-a] [INFO] \n\
             [child-b] [ERROR] com.example.rtk.BMultiFailTest.second -- Time elapsed: 0.02 s <<< FAILURE!\n\
             java.lang.AssertionError: b2 boom\n\
             \tat com.example.rtk.BMultiFailTest.second(BMultiFailTest.java:20)\n\
             [child-b] [INFO] \n\
             [child-a] [INFO] Results:\n\
             [child-b] [INFO] Results:\n\
             [child-a] [ERROR] Failures: \n\
             [child-b] [ERROR] Failures: \n\
             [child-a] [ERROR]   AMultiFailTest.first:10 a1 boom\n\
             [child-b] [ERROR]   BMultiFailTest.first:10 b1 boom\n\
             [child-a] [ERROR]   AMultiFailTest.second:20 a2 boom\n\
             [child-b] [ERROR]   BMultiFailTest.second:20 b2 boom\n\
             [child-a] [ERROR] Tests run: 2, Failures: 2, Errors: 0, Skipped: 0\n\
             [child-b] [ERROR] Tests run: 2, Failures: 2, Errors: 0, Skipped: 0\n\
             [INFO] Reactor Summary for multi-module-fail-skeleton 1.0.0-SNAPSHOT:\n\
             [INFO] \n\
             [INFO] child-a ............................................ FAILURE [  1.234 s]\n\
             [INFO] child-b ............................................ FAILURE [  1.234 s]\n\
             [INFO] BUILD FAILURE\n";
        let o = filter(i, 2);

        // Cap engagement: only the *first* summary entry per module is
        // kept — pre-fix (stale open block swallowing the header/entries)
        // all 4 would survive.
        assert!(
            o.contains("AMultiFailTest.first:10 a1 boom") && o.contains("BMultiFailTest.first:10 b1 boom"),
            "the first entry of each module's summary is kept; got:\n{o}"
        );
        assert!(
            !o.contains("AMultiFailTest.second:20 a2 boom") && !o.contains("BMultiFailTest.second:20 b2 boom"),
            "the second entry of each module's summary is capped, not kept; got:\n{o}"
        );

        // Per-lane tail attribution: each module's own drop is reported
        // under its own AGG, not the other module's.
        assert_eq!(
            o.matches("… +1 more failures").count(),
            2,
            "each module reports exactly its own one dropped entry; got:\n{o}"
        );
        assert!(
            !o.contains("… +2 more failures"),
            "no module's tail should absorb the other's drop; got:\n{o}"
        );
        let a_header = o.find("[child-a] [ERROR] Failures:").expect("child-a header kept");
        let a_agg = o
            .find("[child-a] [ERROR] Tests run: 2, Failures: 2, Errors: 0, Skipped: 0\n")
            .expect("child-a AGG kept");
        let b_header = o.find("[child-b] [ERROR] Failures:").expect("child-b header kept");
        let b_agg = o
            .rfind("[child-b] [ERROR] Tests run: 2, Failures: 2, Errors: 0, Skipped: 0\n")
            .expect("child-b AGG kept");
        assert!(
            o[a_header..a_agg].contains("… +1 more failures"),
            "child-a's tail sits between its own header and its own AGG; got:\n{o}"
        );
        assert!(
            o[b_header..b_agg].contains("… +1 more failures"),
            "child-b's tail sits between its own header and its own AGG; got:\n{o}"
        );

        // Ordering: banners, Running, and the capped summary all survive in
        // a sane relative order, ending at the reactor footer.
        let banner_a = o.find("< com.example.rtk:child-a >").expect("child-a banner kept");
        let running_a = o
            .find("Running com.example.rtk.AMultiFailTest")
            .expect("child-a Running kept");
        let build_failure = o.rfind("BUILD FAILURE").expect("footer kept");
        assert!(
            banner_a < running_a && running_a < a_header && a_header < a_agg && a_agg < build_failure,
            "banner < Running < header < AGG < footer; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_fuller_two_module_probe_engages_cap() {
        assert_fuller_two_module_probe_engages_cap(filter_surefire_with_cap_daemon);
    }

    #[test]
    fn mvnd_package_fuller_two_module_probe_engages_cap() {
        assert_fuller_two_module_probe_engages_cap(filter_package_with_cap_daemon);
    }

    /// Reviewer finding #3 (upstream PR #3199, second review round): d602a3b
    /// gated every fall-through `keep_continuation` disarm on `keyed`,
    /// protecting mvnd's tagged lanes — but the root (untagged) lane, the
    /// only one plain `mvn` ever uses, has no raw-line ambiguity to protect:
    /// every raw line genuinely belongs to it. Gating its disarm too let a
    /// stale claim (armed by any `[ERROR]` line, root lane arms broadly —
    /// see `assert_root_lane_arms_broadly_without_file_coord`) survive an
    /// intervening raw, non-indented line (here, a JUnit assertion message)
    /// and then wrongly keep unrelated indented lines that follow it (here,
    /// stack frames misread as compile-error `symbol:`/`location:`
    /// continuations). Probed through `filter_compile` — it shares the same
    /// `Lanes`/`keep_continuation` machinery as `filter_surefire`/
    /// `filter_package` and has no Surefire block state to obscure the
    /// effect. Byte/frame counts pinned to the exact numbers measured
    /// against merge-base ba7a9ce (pre-d602a3b: 0 frames kept, byte-identical
    /// output) and against the d602a3b-era regression (10 / 17 frames leaked,
    /// output grown to 1588 / 2391 bytes) — a regression on either axis fails
    /// this test.
    fn assert_root_lane_disarms_on_raw_trail_line(fixture: &str, expected_bytes: usize) {
        let o = filter_compile(fixture, true);
        let frames = o
            .lines()
            .filter(|l| l.trim_start().starts_with("at "))
            .count();
        assert_eq!(
            frames, 0,
            "stray stack frames leaked through as compile continuations;\noutput:\n{o}"
        );
        assert_eq!(
            o.len(),
            expected_bytes,
            "output size drifted from the known-good (pre-d602a3b) baseline;\noutput:\n{o}"
        );
    }

    #[test]
    fn root_lane_disarms_on_raw_trail_line_single_failure() {
        assert_root_lane_disarms_on_raw_trail_line(
            include_str!("../../../tests/fixtures/mvn_test_fail_slice_raw.txt"),
            822,
        );
    }

    #[test]
    fn root_lane_disarms_on_raw_trail_line_multi_failure() {
        assert_root_lane_disarms_on_raw_trail_line(
            include_str!("../../../tests/fixtures/mvn_test_multifail_slice_raw.txt"),
            1254,
        );
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

    // ── Binary selection ─────────────────────────────────────────────────────

    /// rtk-ai/rtk#3184 — the daemon is never swapped for `mvn`/`./mvnw`,
    /// whatever wrapper happens to sit in the working directory.
    #[test]
    fn mvnd_binary_is_never_the_wrapper() {
        assert_eq!(mvn_binary(true), "mvnd");
    }

    // ── Maven Daemon fixtures ────────────────────────────────────────────────
    //
    // Real output captured with Apache Maven Daemon 1.0.6 (Maven 3.9.16,
    // non-TTY) on the skeleton projects under `tests/fixtures/`. Non-TTY mvnd
    // output is plain Maven output plus daemon-specific lines with no
    // `[INFO]`-only shape the keep-lists would retain: `Processing build on
    // daemon <id>`, `BuildTimeEventSpy is registered.`, the SmartBuilder
    // thread-count line, and the concurrency stats block. Parallel reactor
    // builds additionally prefix per-module log lines with `[module] `.

    /// Warmed `mvnd clean install` on the multi-module skeleton: the parallel
    /// reactor case. Per-module `[module] [INFO] …` lines and daemon chatter
    /// are noise; the (unprefixed) reactor summary and footer are the signal.
    #[test]
    fn mvnd_reactor_pass_keeps_summary_drops_daemon_noise() {
        let i = include_str!("../../../tests/fixtures/mvnd_reactor_pass_raw.txt");
        let o = filter_package(i, true);
        assert!(o.contains("[INFO] Reactor Summary for multi-module-skeleton 1.0.0-SNAPSHOT:"));
        assert!(o.contains("child-a ............................................ SUCCESS"));
        assert!(o.contains("child-b ............................................ SUCCESS"));
        assert!(o.contains("[INFO] BUILD SUCCESS"));
        assert!(o.contains("[INFO] Total time:"));
        // Module identity survives on keeper lines (parity with plain-mvn
        // reactors, whose banners/artifact lines are kept).
        assert!(o.contains("[child-b] [INFO] Building jar:"));
        // Daemon chatter and per-module noise are dropped.
        assert!(!o.contains("Processing build on daemon"));
        assert!(!o.contains("BuildTimeEventSpy"));
        assert!(!o.contains("SmartBuilder"));
        assert!(!o.contains("Bottleneck projects"));
        assert!(!o.contains("skip non existing resourceDirectory"));
        assert!(!o.contains("[INFO] Deleting"));
    }

    #[test]
    fn mvnd_reactor_pass_savings() {
        let i = include_str!("../../../tests/fixtures/mvnd_reactor_pass_raw.txt");
        let o = filter_package(i, true);
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(i) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60% savings, got {savings:.1}%");
    }

    /// Warmed `mvnd test` on the failing skeleton (exit code 1). Same Surefire
    /// shape as `mvn`: failure names, messages, and user stack frames survive;
    /// daemon chatter, framework frames, and help boilerplate do not.
    #[test]
    fn mvnd_test_fail_preserves_failures() {
        let i = include_str!("../../../tests/fixtures/mvnd_test_fail_raw.txt");
        let o = filter_surefire(i, true);
        assert!(o.contains("[INFO] Running com.example.rtk.BoomTest"));
        assert!(o.contains("[INFO] Running com.example.rtk.CalcTest"));
        assert!(o.contains("failOne: addition should equal five ==> expected: <5> but was: <4>"));
        assert!(o.contains("at com.example.rtk.CalcTest.failOne(CalcTest.java:12)"));
        assert!(o.contains("[ERROR]   CalcTest.failOne:12"));
        assert!(o.contains("[ERROR] Tests run: 16, Failures: 1, Errors: 2, Skipped: 0"));
        // Passing classes are collapsed entirely.
        assert!(!o.contains("PassOneTest"));
        assert!(o.contains("[INFO] BUILD FAILURE"));
        assert!(o.contains("[ERROR] Failed to execute goal"));
        // Daemon chatter, framework frames, and boilerplate are dropped.
        assert!(!o.contains("Processing build on daemon"));
        assert!(!o.contains("BuildTimeEventSpy"));
        assert!(!o.contains("SmartBuilder"));
        assert!(!o.contains("at java.base/"));
        assert!(!o.contains("Re-run Maven"));
    }

    #[test]
    fn mvnd_test_fail_savings() {
        let i = include_str!("../../../tests/fixtures/mvnd_test_fail_raw.txt");
        let o = filter_surefire(i, true);
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(i) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60% savings, got {savings:.1}%");
    }

    /// Failing test inside a parallel reactor (`mvnd clean test` on the
    /// multi-module-fail-skeleton, exit code 1): module output interleaves
    /// line-by-line and per-module lines carry a `[module] ` prefix, while
    /// the assertion message and stack frames arrive raw (unprefixed). The
    /// failing class, its message, user frame, and summary entry must all
    /// survive — with module identity — and the interleaved passing classes
    /// from the other module must not bleed into the failing block.
    #[test]
    fn mvnd_parallel_reactor_fail_preserves_diagnostics() {
        let i = include_str!("../../../tests/fixtures/mvnd_reactor_fail_raw.txt");
        let o = filter_surefire(i, true);
        assert!(o.contains("[child-a] [INFO] Running com.example.rtk.ParallelFailTest"));
        assert!(o.contains(
            "[child-a] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed:"
        ));
        assert!(o.contains("parallel reactor diagnostic ==> expected: <1> but was: <2>"));
        assert!(o.contains("at com.example.rtk.ParallelFailTest.reactorDiagnostic(ParallelFailTest.java:10)"));
        assert!(o.contains("[child-a] [ERROR]   ParallelFailTest.reactorDiagnostic:10"));
        assert!(o.contains("[child-a] [ERROR] Tests run: 3, Failures: 1, Errors: 0, Skipped: 0"));
        // Reactor summary keeps the per-module verdicts.
        assert!(o.contains("child-a ............................................ FAILURE"));
        assert!(o.contains("[INFO] BUILD FAILURE"));
        assert!(o.contains("[ERROR] Failed to execute goal"));
        // Interleaved passing classes are collapsed; a passing close from the
        // other module must never be attributed to the failing block.
        assert!(!o.contains("PassBetaTest"));
        assert!(!o.contains("PassGammaTest"));
        assert!(!o.contains("PassAlphaTest"));
        // Framework frames, daemon chatter, and boilerplate are dropped.
        assert!(!o.contains("at org.junit.jupiter"));
        assert!(!o.contains("at java.base/"));
        assert!(!o.contains("Processing build on daemon"));
        assert!(!o.contains("Re-run Maven"));
    }

    #[test]
    fn mvnd_parallel_reactor_fail_savings() {
        let i = include_str!("../../../tests/fixtures/mvnd_reactor_fail_raw.txt");
        let o = filter_surefire(i, true);
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(i) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60% savings, got {savings:.1}%");
    }

    // The tests below use inline strings rather than captures: they pin
    // *interleavings* (which module's line lands between which two others),
    // and a real `mvnd` run can't be made to emit a chosen interleaving on
    // demand — the captured fixtures above happen to keep each failure trail
    // contiguous. Line shapes are copied verbatim from
    // `mvnd_reactor_fail_raw.txt` / `mvnd_compile_error_raw.txt`, only the
    // ordering is arranged.

    /// Another module opening a Surefire block *between* a failing close and
    /// its raw (unprefixed) exception trail must not steal the trail: routing
    /// those lines into the passing block discards them when it closes green.
    /// An active failure trail outranks an ordinary open block. Asserted on
    /// both entry points that drive the lane machinery.
    fn assert_interleaved_trail_survives(filter: fn(&str) -> String) {
        let i = "[INFO] Scanning for projects...\n\
             [child-a] [INFO] Running com.example.rtk.ParallelFailTest\n\
             [child-b] [INFO] Running com.example.rtk.PassBetaTest\n\
             [child-b] [INFO] Tests run: 2, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.157 s -- in com.example.rtk.PassBetaTest\n\
             [child-a] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.153 s <<< FAILURE! -- in com.example.rtk.ParallelFailTest\n\
             [child-a] [ERROR] com.example.rtk.ParallelFailTest.reactorDiagnostic -- Time elapsed: 0.098 s <<< FAILURE!\n\
             [child-b] [INFO] Running com.example.rtk.PassGammaTest\n\
             org.opentest4j.AssertionFailedError: parallel reactor diagnostic ==> expected: <1> but was: <2>\n\
             \tat com.example.rtk.ParallelFailTest.reactorDiagnostic(ParallelFailTest.java:10)\n\
             [child-b] [INFO] Tests run: 1, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.009 s -- in com.example.rtk.PassGammaTest\n\
             [child-a] [INFO] \n\
             [INFO] BUILD FAILURE\n";
        let o = filter(i);
        assert!(
            o.contains("parallel reactor diagnostic ==> expected: <1> but was: <2>"),
            "assertion message survives the interleave; got:\n{o}"
        );
        assert!(
            o.contains(
                "at com.example.rtk.ParallelFailTest.reactorDiagnostic(ParallelFailTest.java:10)"
            ),
            "user frame survives the interleave; got:\n{o}"
        );
        // The interleaving module's passing classes are still collapsed.
        assert!(!o.contains("PassBetaTest"), "got:\n{o}");
        assert!(!o.contains("PassGammaTest"), "got:\n{o}");
    }

    #[test]
    fn mvnd_interleaved_block_does_not_steal_failure_trail() {
        assert_interleaved_trail_survives(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_package_interleaved_block_does_not_steal_failure_trail() {
        assert_interleaved_trail_survives(filter_package_daemon);
    }

    /// Raw lines with no unambiguous owner — several plain blocks open, no
    /// failure trail — are preserved verbatim rather than buffered into a
    /// guessed lane that may drop them.
    fn assert_ambiguous_raw_line_preserved(filter: fn(&str) -> String) {
        let i = "[INFO] Scanning for projects...\n\
             [child-a] [INFO] Running com.example.rtk.PassAlphaTest\n\
             [child-b] [INFO] Running com.example.rtk.PassBetaTest\n\
             java.lang.IllegalStateException: stray reactor stdout\n\
             [child-a] [INFO] Tests run: 2, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.026 s -- in com.example.rtk.PassAlphaTest\n\
             [child-b] [INFO] Tests run: 2, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.157 s -- in com.example.rtk.PassBetaTest\n\
             [INFO] BUILD SUCCESS\n";
        let o = filter(i);
        assert!(
            o.contains("stray reactor stdout"),
            "ambiguous raw line preserved; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_ambiguous_raw_line_is_preserved() {
        assert_ambiguous_raw_line_preserved(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_package_ambiguous_raw_line_is_preserved() {
        assert_ambiguous_raw_line_preserved(filter_package_daemon);
    }

    /// javac emits `symbol:` / `location:` as raw indented lines *after* the
    /// `[ERROR] … cannot find symbol` line. A line from another reactor module
    /// landing in between must not clear the continuation flag — ownership is
    /// per lane.
    #[test]
    fn mvnd_parallel_compile_keeps_interleaved_continuations() {
        let i = "[INFO] Scanning for projects...\n\
             [child-a] [ERROR] /C:/work/child-a/src/main/java/com/example/rtk/A.java:[7,9] cannot find symbol\n\
             [child-b] [INFO] Compiling 1 source file with javac [debug target 21] to target\\classes\n\
             \x20 symbol:   variable bar\n\
             \x20 location: class com.example.rtk.A\n\
             [INFO] BUILD FAILURE\n";
        let o = filter_compile(i, true);
        assert!(
            o.contains("symbol:   variable bar"),
            "continuation survives the interleave; got:\n{o}"
        );
        assert!(
            o.contains("location: class com.example.rtk.A"),
            "continuation survives the interleave; got:\n{o}"
        );
    }

    /// The `[ERROR] Failures:` *budget* is reactor-wide: two modules'
    /// summaries interleaving must share one budget, not get `modules × cap`
    /// entries. But the `… +N more failures` *tail* is attributed per lane
    /// (reviewer finding #3199, second round): each module's own tail
    /// reports only its own drops, flushed at its own AGG line — here
    /// child-a and child-b each drop exactly one entry while sharing the
    /// cap=2 budget, so each gets its own `… +1 more failures`, not one
    /// combined `… +2 more failures` sitting under whichever AGG happened
    /// to arrive first.
    ///
    /// Cold-preclear finding (upstream PR #3199, third review round): a bare
    /// `Running` line — no explicit close for it — is exactly what real
    /// mvnd always emits for a module before its failures summary
    /// (establishing the lane), and is deliberately used here rather than a
    /// `Running` + passing-close pair: the pair alone doesn't reach the
    /// stale-open-block path this test guards, so it wouldn't have caught
    /// the regression.
    fn assert_summary_cap_shared_across_lanes(filter: fn(&str, usize) -> String) {
        let i = "[INFO] Scanning for projects...\n\
             [child-a] [INFO] Running com.example.rtk.ChildAPass\n\
             [child-b] [INFO] Running com.example.rtk.ChildBPass\n\
             [child-a] [ERROR] Failures: \n\
             [child-a] [ERROR]   ChildATest.one:11 boom a1\n\
             [child-b] [ERROR] Failures: \n\
             [child-b] [ERROR]   ChildBTest.one:11 boom b1\n\
             [child-a] [ERROR]   ChildATest.two:12 boom a2\n\
             [child-b] [ERROR]   ChildBTest.two:12 boom b2\n\
             [child-a] [ERROR] Tests run: 4, Failures: 2, Errors: 0, Skipped: 0\n\
             [child-b] [ERROR] Tests run: 4, Failures: 2, Errors: 0, Skipped: 0\n\
             [INFO] BUILD FAILURE\n";
        let o = filter(i, 2);
        assert_eq!(
            o.matches("boom ").count(),
            2,
            "cap=2 bounds the whole reactor, not each module; got:\n{o}"
        );
        assert_eq!(
            o.matches("… +1 more failures").count(),
            2,
            "each lane reports only its own drop, not a combined tail on whichever lane's AGG arrived first; got:\n{o}"
        );
        assert!(
            !o.contains("… +2 more failures"),
            "no lane's tail should absorb another lane's drop; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_failures_summary_cap_is_shared_across_lanes() {
        assert_summary_cap_shared_across_lanes(filter_surefire_with_cap_daemon);
    }

    #[test]
    fn mvnd_package_failures_summary_cap_is_shared_across_lanes() {
        assert_summary_cap_shared_across_lanes(filter_package_with_cap_daemon);
    }

    /// Reviewer finding #2 (upstream PR #3199, second review round), exact
    /// probe: cap=2, child-a has 3 failures, child-b has 1 — and by the time
    /// child-b's *own* single entry (B1) arrives, child-a's first two
    /// entries have already spent the whole shared budget, so B1 is
    /// dropped. child-b's AGG line then arrives before child-a's.
    ///
    /// Attribution invariant (see the doc comment on
    /// [`FailuresSummaryCap`]): each lane's `… +N more failures` tail
    /// reports exactly the entries dropped while *that* lane's own summary
    /// block was open, flushed at *that* lane's own AGG — never another
    /// lane's. Pre-fix (reactor-wide `dropped`, flushed once at whichever
    /// AGG arrived first): B1 vanished with no tail under child-b's
    /// zero-entry header, the flush under child-b's AGG double-counted
    /// child-a's still-pending drop, and child-a's own AGG then found
    /// `dropped` already zeroed and emitted no tail at all — a module's
    /// only failure silently disappearing is the core wrong this fixes.
    /// Post-fix: child-b's header is legitimately empty (its one entry lost
    /// the shared-budget race) but is followed by its own `… +1 more
    /// failures`, and child-a's block keeps its two entries and reports its
    /// own `… +1 more failures` for the third — every drop is accounted for
    /// under the lane that actually dropped it, and no header is left
    /// looking complete when it silently isn't.
    fn assert_shared_budget_race_attributes_tails_per_lane(filter: fn(&str, usize) -> String) {
        let i = "[INFO] Scanning for projects...\n\
             [child-a] [INFO] Running com.example.rtk.ChildAPass\n\
             [child-b] [INFO] Running com.example.rtk.ChildBPass\n\
             [child-a] [ERROR] Failures: \n\
             [child-a] [ERROR]   ChildATest.one:11 boom a1\n\
             [child-a] [ERROR]   ChildATest.two:12 boom a2\n\
             [child-b] [ERROR] Failures: \n\
             [child-b] [ERROR]   ChildBTest.one:11 boom b1\n\
             [child-a] [ERROR]   ChildATest.three:13 boom a3\n\
             [child-b] [INFO] \n\
             [child-b] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0\n\
             [child-a] [INFO] \n\
             [child-a] [ERROR] Tests run: 3, Failures: 3, Errors: 0, Skipped: 0\n\
             [INFO] BUILD FAILURE\n";
        let o = filter(i, 2);
        assert_eq!(
            o.matches("boom ").count(),
            2,
            "shared budget still bounds the whole reactor at 2 kept entries; got:\n{o}"
        );
        assert!(
            !o.contains("boom b1"),
            "B1 lost the shared-budget race, same as pre-fix; got:\n{o}"
        );
        assert_eq!(
            o.matches("… +1 more failures").count(),
            2,
            "child-a and child-b each report exactly their own one drop, not a combined or misattributed tail; got:\n{o}"
        );
        assert!(
            !o.contains("… +2 more failures"),
            "no lane's tail should double-count another lane's drop; got:\n{o}"
        );
        // child-b's header is legitimately empty (its only entry lost the
        // race) — but must never look *complete*: its own tail must follow
        // directly, not vanish silently.
        let b_header = o.find("[child-b] [ERROR] Failures:").expect("child-b header kept");
        let b_agg = o
            .find("[child-b] [ERROR] Tests run:")
            .expect("child-b AGG kept");
        assert!(
            o[b_header..b_agg].contains("… +1 more failures"),
            "child-b's zero-entry header is followed by its own tail before its AGG; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_shared_budget_race_attributes_tails_per_lane() {
        assert_shared_budget_race_attributes_tails_per_lane(filter_surefire_with_cap_daemon);
    }

    #[test]
    fn mvnd_package_shared_budget_race_attributes_tails_per_lane() {
        assert_shared_budget_race_attributes_tails_per_lane(filter_package_with_cap_daemon);
    }

    /// The failures-summary budget spans the whole invocation: module
    /// summaries that run back-to-back (child A opens and closes its summary
    /// before child B opens) share one budget too — sequential lanes must not
    /// each get a fresh `cap`.
    fn assert_summary_cap_spans_sequential_lanes(filter: fn(&str, usize) -> String) {
        let i = "[INFO] Scanning for projects...\n\
             [child-a] [INFO] Running com.example.rtk.ChildAPass\n\
             [child-b] [INFO] Running com.example.rtk.ChildBPass\n\
             [child-a] [ERROR] Failures: \n\
             [child-a] [ERROR]   ChildATest.one:11 boom a1\n\
             [child-a] [ERROR]   ChildATest.two:12 boom a2\n\
             [child-a] [ERROR] Tests run: 4, Failures: 2, Errors: 0, Skipped: 0\n\
             [child-b] [ERROR] Failures: \n\
             [child-b] [ERROR]   ChildBTest.one:11 boom b1\n\
             [child-b] [ERROR]   ChildBTest.two:12 boom b2\n\
             [child-b] [ERROR] Tests run: 4, Failures: 2, Errors: 0, Skipped: 0\n\
             [INFO] BUILD FAILURE\n";
        let o = filter(i, 2);
        assert_eq!(
            o.matches("boom ").count(),
            2,
            "cap=2 bounds the whole run, sequential summaries included; got:\n{o}"
        );
        assert!(
            o.contains("… +2 more failures"),
            "tail reports the dropped second-module entries; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_failures_summary_cap_spans_sequential_lanes() {
        assert_summary_cap_spans_sequential_lanes(filter_surefire_with_cap_daemon);
    }

    #[test]
    fn mvnd_package_failures_summary_cap_spans_sequential_lanes() {
        assert_summary_cap_spans_sequential_lanes(filter_package_with_cap_daemon);
    }

    /// Reviewer probe (upstream PR #3199 finding 1): `mvn verify`/`install`
    /// emits two independent failures summaries in the same (unprefixed)
    /// lane — Surefire's for unit-test failures, then Failsafe's for
    /// integration-test failures. Each must get its own budget of `cap`: the
    /// second summary must not start at zero remaining because the first
    /// one already spent the whole invocation-wide budget.
    #[test]
    fn failures_summary_cap_resets_per_phase_on_same_lane() {
        let mut i = String::from("[INFO] Scanning for projects...\n[ERROR] Failures:\n");
        for n in 1..=12 {
            i.push_str(&format!("[ERROR]   UnitTest.case{n}:{n} boom u{n}\n"));
        }
        i.push_str("[ERROR] Tests run: 12, Failures: 12, Errors: 0, Skipped: 0\n");
        i.push_str("[ERROR] Failures:\n");
        for n in 1..=12 {
            i.push_str(&format!("[ERROR]   ITTest.case{n}:{n} boom it{n}\n"));
        }
        i.push_str("[ERROR] Tests run: 12, Failures: 12, Errors: 0, Skipped: 0\n[INFO] BUILD FAILURE\n");

        let o = filter_package(&i, false);
        assert_eq!(
            o.matches("boom u").count(),
            10,
            "Surefire summary keeps its own 10 entries; got:\n{o}"
        );
        assert_eq!(
            o.matches("boom it").count(),
            10,
            "Failsafe summary gets a fresh budget of 10, not zero; got:\n{o}"
        );
        assert_eq!(
            o.matches("… +2 more failures").count(),
            2,
            "each phase reports its own 2 dropped entries; got:\n{o}"
        );
    }

    /// Reviewer follow-up (upstream PR #3199, follow-up cold review): a run
    /// whose failures are all thrown exceptions gets an `[ERROR] Errors:`
    /// summary with no `Failures:` header at all — pre-fix, `handle_header`
    /// only recognized `Failures:`, so this summary shape bypassed the
    /// budget entirely (probe: 5/5 kept at cap=2).
    fn assert_errors_only_summary_respects_cap(filter: fn(&str, usize) -> String) {
        let i = "[INFO] Scanning for projects...\n\
                  [ERROR] Errors: \n\
                  [ERROR]   BoomTest.one:11 java.lang.IllegalStateException\n\
                  [ERROR]   BoomTest.two:12 java.lang.IllegalStateException\n\
                  [ERROR]   BoomTest.three:13 java.lang.IllegalStateException\n\
                  [ERROR]   BoomTest.four:14 java.lang.IllegalStateException\n\
                  [ERROR]   BoomTest.five:15 java.lang.IllegalStateException\n\
                  [ERROR] Tests run: 5, Failures: 0, Errors: 5, Skipped: 0\n\
                  [INFO] BUILD FAILURE\n";
        let o = filter(i, 2);
        assert_eq!(
            o.matches("java.lang.IllegalStateException").count(),
            2,
            "cap=2 bounds an Errors-only summary same as a Failures one; got:\n{o}"
        );
        assert!(
            o.contains("… +3 more failures"),
            "tail reports the 3 dropped entries; got:\n{o}"
        );
    }

    #[test]
    fn surefire_errors_only_summary_respects_cap() {
        assert_errors_only_summary_respects_cap(filter_surefire_with_cap_plain);
    }

    #[test]
    fn package_errors_only_summary_respects_cap() {
        assert_errors_only_summary_respects_cap(filter_package_with_cap_plain);
    }

    /// Cold-preclear finding (🟡 2): truncated output — a lane's failures
    /// summary opened, entries got capped, but its own AGG line never
    /// arrives before the stream ends. `Lanes::finish_summaries` must still
    /// flush that lane's own pending `dropped` count as a `… +N more
    /// failures` tail, attributed to the right lane — not silently lost, and
    /// not folded into another lane's count. Two lanes truncate at once here
    /// (`child-a` drops 1, `child-b` drops 2) to prove the per-lane
    /// attribution invariant (see the doc comment on `FailuresSummaryCap`)
    /// survives into the end-of-stream path too, not just the AGG-driven one
    /// `handle_aggregate` covers.
    fn assert_truncated_summary_flushes_per_lane_tail(filter: fn(&str, usize) -> String) {
        let i = "[INFO] BUILD FAILURE\n\
             [child-a] [INFO] Running com.example.rtk.ChildAPass\n\
             [child-b] [INFO] Running com.example.rtk.ChildBPass\n\
             [child-a] [ERROR] Failures: \n\
             [child-a] [ERROR]   ChildATest.one:11 boom a1\n\
             [child-a] [ERROR]   ChildATest.two:12 boom a2\n\
             [child-a] [ERROR]   ChildATest.three:13 boom a3\n\
             [child-b] [ERROR] Failures: \n\
             [child-b] [ERROR]   ChildBTest.one:11 boom b1\n\
             [child-b] [ERROR]   ChildBTest.two:12 boom b2\n";
        let o = filter(i, 2);
        assert_eq!(
            o.matches("boom ").count(),
            2,
            "shared budget still bounds kept entries at 2; got:\n{o}"
        );
        assert!(
            o.contains("… +1 more failures"),
            "child-a's own truncated drop (1 entry) is flushed at end-of-stream; got:\n{o}"
        );
        assert!(
            o.contains("… +2 more failures"),
            "child-b's own truncated drop (2 entries) is flushed at end-of-stream; got:\n{o}"
        );
        assert!(
            !o.contains("… +3 more failures"),
            "the two lanes' truncated drops must not be folded into one combined tail; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_truncated_summary_flushes_per_lane_tail() {
        assert_truncated_summary_flushes_per_lane_tail(filter_surefire_with_cap_daemon);
    }

    #[test]
    fn mvnd_package_truncated_summary_flushes_per_lane_tail() {
        assert_truncated_summary_flushes_per_lane_tail(filter_package_with_cap_daemon);
    }

    /// Same phase-reset invariant as `failures_summary_cap_resets_per_phase_on_same_lane`,
    /// but the second phase is headed by `[ERROR] Errors:` instead of
    /// `[ERROR] Failures:` — the reset must not care which of the two
    /// header spellings opened either phase.
    #[test]
    fn errors_header_resets_summary_cap_per_phase_same_as_failures_header() {
        let mut i = String::from("[INFO] Scanning for projects...\n[ERROR] Failures:\n");
        for n in 1..=12 {
            i.push_str(&format!("[ERROR]   UnitTest.case{n}:{n} boom u{n}\n"));
        }
        i.push_str("[ERROR] Tests run: 12, Failures: 12, Errors: 0, Skipped: 0\n");
        i.push_str("[ERROR] Errors:\n");
        for n in 1..=12 {
            i.push_str(&format!("[ERROR]   ITTest.case{n}:{n} boom it{n}\n"));
        }
        i.push_str("[ERROR] Tests run: 0, Failures: 0, Errors: 12, Skipped: 0\n[INFO] BUILD FAILURE\n");

        let o = filter_package(&i, false);
        assert_eq!(
            o.matches("boom u").count(),
            10,
            "Failures:-headed phase keeps its own 10 entries; got:\n{o}"
        );
        assert_eq!(
            o.matches("boom it").count(),
            10,
            "Errors:-headed phase gets a fresh budget of 10, not zero; got:\n{o}"
        );
        assert_eq!(
            o.matches("… +2 more failures").count(),
            2,
            "each phase reports its own 2 dropped entries; got:\n{o}"
        );
    }

    /// Cold-preclear finding (upstream PR #3199, third review round):
    /// generation resets are now driven exclusively by
    /// [`FailuresSummaryCap::observe_plugin_banner`], not lane-repeat
    /// inference — a design that structurally can't double-reset from two
    /// *different* lanes' headers repeating (there's no per-lane repeat
    /// tracking left to trip over), but must also not spuriously reset
    /// *within* one phase just because two modules each carry their own
    /// Surefire banner. child-a and child-b share the identical
    /// `surefire:3.5.5:test (default-test)` goal string — `observe_plugin_banner`
    /// must treat child-b's own banner as a no-op (same phase), not a fresh
    /// budget, even though it's a different lane's banner.
    #[test]
    fn same_goal_banners_from_different_lanes_do_not_reset_budget() {
        let i = "[INFO] Scanning for projects...\n\
                  [child-a] [INFO] --- surefire:3.5.5:test (default-test) @ child-a ---\n\
                  [child-a] [INFO] Running com.example.rtk.AOneTest\n\
                  [child-a] [INFO] Tests run: 1, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.01 s -- in com.example.rtk.AOneTest\n\
                  [child-b] [INFO] --- surefire:3.5.5:test (default-test) @ child-b ---\n\
                  [child-b] [INFO] Running com.example.rtk.BOneTest\n\
                  [child-b] [INFO] Tests run: 1, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.01 s -- in com.example.rtk.BOneTest\n\
                  [child-a] [ERROR] Failures:\n\
                  [child-a] [ERROR]   entryA1\n\
                  [child-a] [ERROR]   entryA2\n\
                  [child-b] [ERROR] Failures:\n\
                  [child-b] [ERROR]   entryB1\n\
                  [child-a] [ERROR] Tests run: 2, Failures: 2, Errors: 0, Skipped: 0\n\
                  [child-b] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0\n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_package_with_cap(i, 2, true);
        assert!(
            o.contains("entryA1") && o.contains("entryA2"),
            "child-a's two entries fill the shared cap=2 budget; got:\n{o}"
        );
        assert!(
            !o.contains("entryB1"),
            "child-b's own same-goal banner must not grant it a fresh budget; got:\n{o}"
        );
        assert_eq!(
            o.matches("… +1 more failures").count(),
            1,
            "exactly one dropped entry (child-b's), not a doubled or missing tail; got:\n{o}"
        );
    }

    /// A compile error surfacing inside a test/package run: the raw indented
    /// `symbol:` / `location:` continuations must route back to the module
    /// that armed them even when another module's line lands in between —
    /// arming claims raw-line ownership on these paths too, not just in
    /// `filter_compile`.
    fn assert_interleaved_compile_continuation_survives(filter: fn(&str) -> String) {
        let i = "[INFO] Scanning for projects...\n\
             [child-a] [ERROR] /C:/work/child-a/src/main/java/com/example/rtk/A.java:[7,9] cannot find symbol\n\
             [child-b] [INFO] Compiling 1 source file with javac [debug target 21] to target\\classes\n\
             \x20 symbol:   variable bar\n\
             \x20 location: class com.example.rtk.A\n\
             [INFO] BUILD FAILURE\n";
        let o = filter(i);
        assert!(
            o.contains("symbol:   variable bar"),
            "continuation survives the interleave; got:\n{o}"
        );
        assert!(
            o.contains("location: class com.example.rtk.A"),
            "continuation survives the interleave; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_surefire_interleaved_compile_continuation_survives() {
        assert_interleaved_compile_continuation_survives(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_package_interleaved_compile_continuation_survives() {
        assert_interleaved_compile_continuation_survives(filter_package_daemon);
    }

    // ── Exhaustive interleaving sweeps ──────────────────────────────────────
    //
    // mvnd's scheduler controls interleaving, not us: any order-preserving
    // merge of two modules' output is a run that can really happen. Rather
    // than pinning hand-picked orderings one review round at a time, sweep
    // every merge and assert the failure signal survives all of them —
    // routed into its lane or preserved verbatim, never dropped.

    /// All order-preserving merges of `a` and `b` (each module's own lines
    /// keep their order; the interleaving varies).
    fn merges<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Vec<&'a str>> {
        fn rec<'a>(
            a: &[&'a str],
            b: &[&'a str],
            prefix: &mut Vec<&'a str>,
            out: &mut Vec<Vec<&'a str>>,
        ) {
            if a.is_empty() && b.is_empty() {
                out.push(prefix.clone());
                return;
            }
            if let Some((&h, t)) = a.split_first() {
                prefix.push(h);
                rec(t, b, prefix, out);
                prefix.pop();
            }
            if let Some((&h, t)) = b.split_first() {
                prefix.push(h);
                rec(a, t, prefix, out);
                prefix.pop();
            }
        }
        let mut out = Vec::new();
        rec(a, b, &mut Vec::new(), &mut out);
        out
    }

    fn sweep_input(merge: &[&str]) -> String {
        format!(
            "[INFO] Scanning for projects...\n{}\n[INFO] BUILD FAILURE\n",
            merge.join("\n")
        )
    }

    /// child-a: one failing class — Running, failing close, per-test subline,
    /// raw exception message, raw user frame, blank trail terminator. Line
    /// shapes copied from `mvnd_reactor_fail_raw.txt`.
    const SWEEP_FAIL_A: [&str; 6] = [
        "[child-a] [INFO] Running com.example.rtk.ParallelFailTest",
        "[child-a] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.153 s <<< FAILURE! -- in com.example.rtk.ParallelFailTest",
        "[child-a] [ERROR] com.example.rtk.ParallelFailTest.reactorDiagnostic -- Time elapsed: 0.098 s <<< FAILURE!",
        "org.opentest4j.AssertionFailedError: parallel reactor diagnostic ==> expected: <1> but was: <2>",
        "\tat com.example.rtk.ParallelFailTest.reactorDiagnostic(ParallelFailTest.java:10)",
        "[child-a] [INFO] ",
    ];

    /// child-b: two passing classes (open/close, open/close).
    const SWEEP_PASS_B: [&str; 4] = [
        "[child-b] [INFO] Running com.example.rtk.PassBetaTest",
        "[child-b] [INFO] Tests run: 2, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.157 s -- in com.example.rtk.PassBetaTest",
        "[child-b] [INFO] Running com.example.rtk.PassGammaTest",
        "[child-b] [INFO] Tests run: 1, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.009 s -- in com.example.rtk.PassGammaTest",
    ];

    /// child-a variant: a compile error with raw indented continuations —
    /// arming a continuation must survive racing another module's open block.
    const SWEEP_COMPILE_A: [&str; 3] = [
        "[child-a] [ERROR] /C:/work/child-a/src/main/java/com/example/rtk/A.java:[7,9] cannot find symbol",
        "  symbol:   variable bar",
        "  location: class com.example.rtk.A",
    ];

    /// child-b variant: one failing class of its own, for the capped sweep.
    const SWEEP_FAIL_B: [&str; 6] = [
        "[child-b] [INFO] Running com.example.rtk.OtherFailTest",
        "[child-b] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.120 s <<< FAILURE! -- in com.example.rtk.OtherFailTest",
        "[child-b] [ERROR] com.example.rtk.OtherFailTest.otherDiagnostic -- Time elapsed: 0.080 s <<< FAILURE!",
        "org.opentest4j.AssertionFailedError: other reactor diagnostic ==> expected: <3> but was: <4>",
        "\tat com.example.rtk.OtherFailTest.otherDiagnostic(OtherFailTest.java:8)",
        "[child-b] [INFO] ",
    ];

    /// Failing module × passing module, all 210 merges: the failure's close
    /// line, assertion message, and user frame survive every interleaving,
    /// and the passing module's classes stay collapsed in every one.
    fn assert_every_interleaving_keeps_diagnostics(filter: fn(&str) -> String) {
        for (n, m) in merges(&SWEEP_FAIL_A, &SWEEP_PASS_B).iter().enumerate() {
            let i = sweep_input(m);
            let o = filter(&i);
            assert!(
                o.contains("expected: <1> but was: <2>")
                    && o.contains("ParallelFailTest.reactorDiagnostic(ParallelFailTest.java:10)")
                    && o.contains("<<< FAILURE! -- in com.example.rtk.ParallelFailTest"),
                "merge #{n} lost failure signal;\ninput:\n{i}\noutput:\n{o}"
            );
            assert!(
                !o.contains("PassBetaTest") && !o.contains("PassGammaTest"),
                "merge #{n} leaked passing classes;\ninput:\n{i}\noutput:\n{o}"
            );
        }
    }

    #[test]
    fn mvnd_every_interleaving_keeps_diagnostics() {
        assert_every_interleaving_keeps_diagnostics(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_package_every_interleaving_keeps_diagnostics() {
        assert_every_interleaving_keeps_diagnostics(filter_package_daemon);
    }

    /// Two failing modules under `cap = 1`, all 924 merges: whichever class
    /// the cap admits keeps its full diagnostics in every interleaving — a
    /// capped (dropping) trail in one lane must never swallow the admitted
    /// lane's raw exception or frames — and the `+1 more` tail always
    /// reports the capped class.
    fn assert_every_interleaving_keeps_admitted_class(filter: fn(&str, usize) -> String) {
        for (n, m) in merges(&SWEEP_FAIL_A, &SWEEP_FAIL_B).iter().enumerate() {
            let i = sweep_input(m);
            let o = filter(&i, 1);
            let a = o.contains("<<< FAILURE! -- in com.example.rtk.ParallelFailTest");
            let b = o.contains("<<< FAILURE! -- in com.example.rtk.OtherFailTest");
            assert!(
                a != b,
                "merge #{n}: exactly one class admitted under cap=1;\ninput:\n{i}\noutput:\n{o}"
            );
            let (msg, frame) = if a {
                (
                    "expected: <1> but was: <2>",
                    "ParallelFailTest.reactorDiagnostic(ParallelFailTest.java:10)",
                )
            } else {
                (
                    "expected: <3> but was: <4>",
                    "OtherFailTest.otherDiagnostic(OtherFailTest.java:8)",
                )
            };
            assert!(
                o.contains(msg) && o.contains(frame),
                "merge #{n}: admitted class lost its diagnostics;\ninput:\n{i}\noutput:\n{o}"
            );
            assert!(
                o.contains("+1 more failing test classes"),
                "merge #{n}: cap tail missing;\ninput:\n{i}\noutput:\n{o}"
            );
        }
    }

    #[test]
    fn mvnd_every_interleaving_keeps_admitted_class() {
        assert_every_interleaving_keeps_admitted_class(filter_surefire_with_cap_daemon);
    }

    #[test]
    fn mvnd_package_every_interleaving_keeps_admitted_class() {
        assert_every_interleaving_keeps_admitted_class(filter_package_with_cap_daemon);
    }

    /// Compile-error module × passing test module, all 35 merges: the raw
    /// `symbol:` / `location:` continuations survive every interleaving —
    /// in particular when they race another module's open Surefire block,
    /// which must not buffer them into a green close that discards them.
    fn assert_every_interleaving_keeps_compile_continuation(filter: fn(&str) -> String) {
        for (n, m) in merges(&SWEEP_COMPILE_A, &SWEEP_PASS_B).iter().enumerate() {
            let i = sweep_input(m);
            let o = filter(&i);
            assert!(
                o.contains("symbol:   variable bar")
                    && o.contains("location: class com.example.rtk.A"),
                "merge #{n} lost compile continuation;\ninput:\n{i}\noutput:\n{o}"
            );
            assert!(
                !o.contains("PassBetaTest") && !o.contains("PassGammaTest"),
                "merge #{n} leaked passing classes;\ninput:\n{i}\noutput:\n{o}"
            );
        }
    }

    #[test]
    fn mvnd_every_interleaving_keeps_compile_continuation() {
        assert_every_interleaving_keeps_compile_continuation(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_package_every_interleaving_keeps_compile_continuation() {
        assert_every_interleaving_keeps_compile_continuation(filter_package_daemon);
    }

    /// Compile-error module × *failing* test module, all 84 merges: the armed
    /// continuation claim must survive a failing close stealing `hot` — the
    /// admitted class's diagnostics and the compile continuations both
    /// survive every interleaving.
    fn assert_every_interleaving_keeps_continuation_and_failure(filter: fn(&str) -> String) {
        for (n, m) in merges(&SWEEP_COMPILE_A, &SWEEP_FAIL_B).iter().enumerate() {
            let i = sweep_input(m);
            let o = filter(&i);
            assert!(
                o.contains("symbol:   variable bar")
                    && o.contains("location: class com.example.rtk.A"),
                "merge #{n} lost compile continuation;\ninput:\n{i}\noutput:\n{o}"
            );
            assert!(
                o.contains("expected: <3> but was: <4>")
                    && o.contains("OtherFailTest.otherDiagnostic(OtherFailTest.java:8)")
                    && o.contains("<<< FAILURE! -- in com.example.rtk.OtherFailTest"),
                "merge #{n} lost failure signal;\ninput:\n{i}\noutput:\n{o}"
            );
        }
    }

    #[test]
    fn mvnd_every_interleaving_keeps_continuation_and_failure() {
        assert_every_interleaving_keeps_continuation_and_failure(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_package_every_interleaving_keeps_continuation_and_failure() {
        assert_every_interleaving_keeps_continuation_and_failure(filter_package_daemon);
    }

    /// Dropping-trail variant of the orphaned-continuation case, cap=1: A's
    /// class is admitted, B's is capped (its trail is consuming raw lines
    /// silently), and C arms a compile continuation. The continuations land
    /// while B's dropping trail is active — an armed lane alongside a trail
    /// is a tie, so they must be preserved verbatim, not swallowed by the
    /// dropping trail.
    fn assert_continuation_survives_dropping_trail(filter: fn(&str, usize) -> String) {
        let i = "[INFO] Scanning for projects...\n\
             [child-a] [INFO] Running com.example.rtk.ParallelFailTest\n\
             [child-a] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.153 s <<< FAILURE! -- in com.example.rtk.ParallelFailTest\n\
             org.opentest4j.AssertionFailedError: parallel reactor diagnostic ==> expected: <1> but was: <2>\n\
             [child-a] [INFO] \n\
             [child-b] [INFO] Running com.example.rtk.OtherFailTest\n\
             [child-b] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.120 s <<< FAILURE! -- in com.example.rtk.OtherFailTest\n\
             [child-c] [ERROR] /C:/work/child-c/src/main/java/com/example/rtk/C.java:[7,9] cannot find symbol\n\
             \x20 symbol:   variable bar\n\
             \x20 location: class com.example.rtk.C\n\
             [child-b] [INFO] \n\
             [INFO] BUILD FAILURE\n";
        let o = filter(i, 1);
        assert!(
            o.contains("symbol:   variable bar") && o.contains("location: class com.example.rtk.C"),
            "continuations survive a concurrent dropping trail; got:\n{o}"
        );
        assert!(
            o.contains("expected: <1> but was: <2>"),
            "admitted class keeps its diagnostics; got:\n{o}"
        );
        assert!(
            o.contains("+1 more failing test classes"),
            "capped class reported in the tail; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_continuation_survives_dropping_trail() {
        assert_continuation_survives_dropping_trail(filter_surefire_with_cap_daemon);
    }

    #[test]
    fn mvnd_package_continuation_survives_dropping_trail() {
        assert_continuation_survives_dropping_trail(filter_package_with_cap_daemon);
    }

    /// Entering a Surefire block retires a lane's stale armed claim: a lane
    /// that armed a continuation and then opened its own block must not hold
    /// a permanent armed-vs-block tie that leaks its in-block stdout
    /// verbatim past a green close.
    fn assert_block_entry_retires_armed_claim(filter: fn(&str) -> String) {
        let i = "[INFO] Scanning for projects...\n\
             [child-a] [ERROR] /C:/work/child-a/src/main/java/com/example/rtk/A.java:[7,9] cannot find symbol\n\
             [child-a] [INFO] Running com.example.rtk.PassAlphaTest\n\
             stray in-block stdout line\n\
             [child-a] [INFO] Tests run: 2, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.026 s -- in com.example.rtk.PassAlphaTest\n\
             [INFO] BUILD FAILURE\n";
        let o = filter(i);
        assert!(
            !o.contains("stray in-block stdout line"),
            "green-closing block's stdout stays collapsed after arm retires; got:\n{o}"
        );
        assert!(
            o.contains("cannot find symbol"),
            "the [ERROR] diagnostic line itself survives; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_block_entry_retires_armed_claim() {
        assert_block_entry_retires_armed_claim(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_package_block_entry_retires_armed_claim() {
        assert_block_entry_retires_armed_claim(filter_package_daemon);
    }

    /// An unrelated raw stdout line (another module's, unprefixed) must not
    /// disarm a pending continuation claim: raw fall-through lines never
    /// reset `keep_continuation` — only a lane's own keyed lines do. Swept
    /// through every position of the compile-error sequence, on all three
    /// filter paths.
    fn assert_raw_stray_does_not_disarm_continuation(filter: fn(&str) -> String) {
        const STRAY: [&str; 1] = ["stray stdout from another reactor module"];
        for (n, m) in merges(&SWEEP_COMPILE_A, &STRAY).iter().enumerate() {
            let i = sweep_input(m);
            let o = filter(&i);
            assert!(
                o.contains("symbol:   variable bar")
                    && o.contains("location: class com.example.rtk.A"),
                "stray at position #{n} disarmed the continuation;\ninput:\n{i}\noutput:\n{o}"
            );
        }
    }

    #[test]
    fn mvnd_raw_stray_does_not_disarm_continuation() {
        assert_raw_stray_does_not_disarm_continuation(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_package_raw_stray_does_not_disarm_continuation() {
        assert_raw_stray_does_not_disarm_continuation(filter_package_daemon);
    }

    /// Reviewer blocker (upstream PR #3199, final review round): a
    /// `[tag] [WARNING] …` line for a never-established tag (e.g. an app log
    /// line, not a real module) reaches an armed lane via `raw_owner`'s
    /// fallback with `keyed == false` — the `[WARNING]` dedup branch in
    /// `filter_compile` and `filter_package` must not disarm on it, exactly
    /// like every other disarm site. Swept through every position of the
    /// compile-error sequence, on all three filter paths (`filter_surefire`
    /// has no `[WARNING]` branch but shares the same fall-through reset).
    fn assert_warning_interloper_does_not_disarm_continuation(filter: fn(&str) -> String) {
        const WARNING_INTERLOPER: [&str; 1] = ["[main] [WARNING] connection pool exhausted"];
        for (n, m) in merges(&SWEEP_COMPILE_A, &WARNING_INTERLOPER).iter().enumerate() {
            let i = sweep_input(m);
            let o = filter(&i);
            assert!(
                o.contains("symbol:   variable bar")
                    && o.contains("location: class com.example.rtk.A"),
                "WARNING interloper at position #{n} disarmed the continuation;\ninput:\n{i}\noutput:\n{o}"
            );
        }
    }

    #[test]
    fn mvnd_warning_interloper_does_not_disarm_continuation() {
        assert_warning_interloper_does_not_disarm_continuation(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_compile_warning_interloper_does_not_disarm_continuation() {
        assert_warning_interloper_does_not_disarm_continuation(filter_compile_daemon);
    }

    #[test]
    fn mvnd_package_warning_interloper_does_not_disarm_continuation() {
        assert_warning_interloper_does_not_disarm_continuation(filter_package_daemon);
    }

    #[test]
    fn mvnd_compile_raw_stray_does_not_disarm_continuation() {
        assert_raw_stray_does_not_disarm_continuation(filter_compile_daemon);
    }

    /// Cold-preclear finding: a `[tag] [ERROR] …` app-log line for a
    /// never-established tag (e.g. `[main] [ERROR] connection pool
    /// exhausted`, no `FILE_COORD` coordinate — not an opener) reaches an
    /// armed lane via `raw_owner`'s fallback with `keyed == false`, exactly
    /// like the `[WARNING]` interloper above. `filter_compile`'s `[ERROR]`
    /// arm/rewrite site was missing the `keyed` guard every sibling site has
    /// (surefire, package, and `filter_compile`'s own `[WARNING]`/banner/
    /// fall-through sites) and silently disarmed on it, dropping the
    /// `symbol:`/`location:` continuations that follow. Swept through every
    /// position of the compile-error sequence, on all three filter paths.
    fn assert_error_interloper_does_not_disarm_continuation(filter: fn(&str) -> String) {
        const ERROR_INTERLOPER: [&str; 1] = ["[main] [ERROR] connection pool exhausted"];
        for (n, m) in merges(&SWEEP_COMPILE_A, &ERROR_INTERLOPER).iter().enumerate() {
            let i = sweep_input(m);
            let o = filter(&i);
            assert!(
                o.contains("symbol:   variable bar")
                    && o.contains("location: class com.example.rtk.A"),
                "ERROR interloper at position #{n} disarmed the continuation;\ninput:\n{i}\noutput:\n{o}"
            );
        }
    }

    #[test]
    fn mvnd_error_interloper_does_not_disarm_continuation() {
        assert_error_interloper_does_not_disarm_continuation(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_compile_error_interloper_does_not_disarm_continuation() {
        assert_error_interloper_does_not_disarm_continuation(filter_compile_daemon);
    }

    #[test]
    fn mvnd_package_error_interloper_does_not_disarm_continuation() {
        assert_error_interloper_does_not_disarm_continuation(filter_package_daemon);
    }

    /// child-a variant: one class with TWO blank-separated per-test detail
    /// blocks (Surefire's multi-failure shape) — `trail_rearm` carries the
    /// keep/drop decision across the blanks.
    const SWEEP_MULTIFAIL_A: [&str; 9] = [
        "[child-a] [INFO] Running com.example.rtk.MultiFailTest",
        "[child-a] [ERROR] Tests run: 2, Failures: 2, Errors: 0, Skipped: 0, Time elapsed: 0.051 s <<< FAILURE! -- in com.example.rtk.MultiFailTest",
        "[child-a] [ERROR] com.example.rtk.MultiFailTest.first -- Time elapsed: 0.020 s <<< FAILURE!",
        "org.opentest4j.AssertionFailedError: boomFirst ==> expected: <1> but was: <2>",
        "[child-a] [INFO] ",
        "[child-a] [ERROR] com.example.rtk.MultiFailTest.second -- Time elapsed: 0.030 s <<< FAILURE!",
        "java.lang.IllegalStateException: boomSecond",
        "\tat com.example.rtk.MultiFailTest.second(MultiFailTest.java:30)",
        "[child-a] [INFO] ",
    ];

    const RAW_STRAY: [&str; 1] = ["stray stdout from another reactor module"];

    /// Multi-failure class × raw stray, all 10 positions: a raw stray must
    /// not disarm `trail_rearm` between detail blocks — per-test sublines
    /// are always keyed, so a raw line can neither be the re-entry line nor
    /// prove the trail is over. Pre-fix, a stray in the rearm window dropped
    /// the second block's exception message.
    fn assert_raw_stray_does_not_disarm_trail_rearm(filter: fn(&str) -> String) {
        for (n, m) in merges(&SWEEP_MULTIFAIL_A, &RAW_STRAY).iter().enumerate() {
            let i = sweep_input(m);
            let o = filter(&i);
            assert!(
                o.contains("boomFirst")
                    && o.contains("boomSecond")
                    && o.contains("MultiFailTest.first")
                    && o.contains("MultiFailTest.second")
                    && o.contains("at com.example.rtk.MultiFailTest.second(MultiFailTest.java:30)"),
                "stray at position #{n} broke the multi-failure trail;\ninput:\n{i}\noutput:\n{o}"
            );
        }
    }

    #[test]
    fn mvnd_raw_stray_does_not_disarm_trail_rearm() {
        assert_raw_stray_does_not_disarm_trail_rearm(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_package_raw_stray_does_not_disarm_trail_rearm() {
        assert_raw_stray_does_not_disarm_trail_rearm(filter_package_daemon);
    }

    /// Drop side of the same claim: with the class capped, the stray must
    /// not disarm the *dropping* rearm either — a capped class drops ALL its
    /// detail blocks, and a disarm here leaks the second subline through the
    /// `[ERROR]` catch-all.
    fn assert_raw_stray_does_not_leak_capped_rearm(filter: fn(&str, usize) -> String) {
        let mut m: Vec<&str> = SWEEP_MULTIFAIL_A[..5].to_vec();
        m.push(RAW_STRAY[0]);
        m.extend_from_slice(&SWEEP_MULTIFAIL_A[5..]);
        let i = sweep_input(&m);
        let o = filter(&i, 0);
        assert!(
            !o.contains("MultiFailTest") && !o.contains("boom"),
            "capped class stays fully dropped across the stray; got:\n{o}"
        );
        assert!(
            o.contains("+1 more failing test classes"),
            "capped class still reported in the tail; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_raw_stray_does_not_leak_capped_rearm() {
        assert_raw_stray_does_not_leak_capped_rearm(filter_surefire_with_cap_daemon);
    }

    #[test]
    fn mvnd_package_raw_stray_does_not_leak_capped_rearm() {
        assert_raw_stray_does_not_leak_capped_rearm(filter_package_with_cap_daemon);
    }

    const RAW_BLANK_STRAY: [&str; 1] = [""];

    /// Reviewer blocker (upstream PR #3199, cold review round): a raw BLANK
    /// line landing inside a *tagged* lane's failure trail must not be
    /// mistaken for that lane's own `[tag] [INFO] ` terminator. mvnd always
    /// prefixes a tagged lane's real blank terminator, so an unprefixed
    /// blank reaching a tagged trail is foreign (another module's stray
    /// blank println, or a blank line inside a multi-line assertion
    /// message) — the trail must stay open until the lane's own keyed
    /// terminator arrives. Pre-fix, the blank closed the trail early and
    /// the remaining assertion message / user frame found no claim and were
    /// dropped by the keep-lists.
    fn assert_raw_blank_does_not_terminate_trail(filter: fn(&str) -> String) {
        for (n, m) in merges(&SWEEP_FAIL_A, &RAW_BLANK_STRAY).iter().enumerate() {
            let i = sweep_input(m);
            let o = filter(&i);
            assert!(
                o.contains("expected: <1> but was: <2>")
                    && o.contains("ParallelFailTest.reactorDiagnostic(ParallelFailTest.java:10)"),
                "raw blank at position #{n} truncated the trail;\ninput:\n{i}\noutput:\n{o}"
            );
        }
    }

    #[test]
    fn mvnd_raw_blank_does_not_terminate_trail() {
        assert_raw_blank_does_not_terminate_trail(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_package_raw_blank_does_not_terminate_trail() {
        assert_raw_blank_does_not_terminate_trail(filter_package_daemon);
    }

    /// Drop side of the same claim: with the class capped (fully dropped), a
    /// raw blank inside the trail must not end `drop_trail` mode early —
    /// otherwise the remaining (still-supposed-to-be-suppressed) trail
    /// content falls through unclaimed and can leak past the outer
    /// keep-list instead of staying dropped.
    fn assert_raw_blank_does_not_leak_capped_trail(filter: fn(&str, usize) -> String) {
        // Insert the blank between the assertion message and the user frame
        // — squarely inside the (dropped) trail.
        let mut m: Vec<&str> = SWEEP_FAIL_A[..4].to_vec();
        m.push(RAW_BLANK_STRAY[0]);
        m.extend_from_slice(&SWEEP_FAIL_A[4..]);
        let i = sweep_input(&m);
        let o = filter(&i, 0);
        assert!(
            !o.contains("ParallelFailTest") && !o.contains("expected: <1> but was: <2>"),
            "capped class stays fully dropped across the raw blank; got:\n{o}"
        );
        assert!(
            o.contains("+1 more failing test classes"),
            "capped class still reported in the tail; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_raw_blank_does_not_leak_capped_trail() {
        assert_raw_blank_does_not_leak_capped_trail(filter_surefire_with_cap_daemon);
    }

    #[test]
    fn mvnd_package_raw_blank_does_not_leak_capped_trail() {
        assert_raw_blank_does_not_leak_capped_trail(filter_package_with_cap_daemon);
    }

    /// Two modules armed concurrently: raw continuation lines have no unique
    /// owner (two armed lanes are a tie on their own) and must be preserved
    /// verbatim rather than routed to a guess. Completes the claimant
    /// matrix; in `filter_compile` this is the `route → None` arm.
    fn assert_two_armed_lanes_preserve_continuations(filter: fn(&str) -> String) {
        let i = "[INFO] Scanning for projects...\n\
             [child-a] [ERROR] /C:/work/child-a/src/main/java/com/example/rtk/A.java:[7,9] cannot find symbol\n\
             [child-b] [ERROR] /C:/work/child-b/src/main/java/com/example/rtk/B.java:[3,5] cannot find symbol\n\
             \x20 symbol:   variable bar\n\
             \x20 location: class com.example.rtk.A\n\
             [INFO] BUILD FAILURE\n";
        let o = filter(i);
        assert!(
            o.contains("symbol:   variable bar")
                && o.contains("location: class com.example.rtk.A"),
            "ambiguous continuations preserved verbatim; got:\n{o}"
        );
    }

    #[test]
    fn mvnd_two_armed_lanes_preserve_continuations() {
        assert_two_armed_lanes_preserve_continuations(filter_surefire_daemon);
    }

    #[test]
    fn mvnd_package_two_armed_lanes_preserve_continuations() {
        assert_two_armed_lanes_preserve_continuations(filter_package_daemon);
    }

    #[test]
    fn mvnd_compile_two_armed_lanes_preserve_continuations() {
        assert_two_armed_lanes_preserve_continuations(filter_compile_daemon);
    }

    // Full-output regression tests locking the complete filtered output of
    // every mvnd fixture, per the repo-prescribed fixture pattern
    // (`.claude/rules/cli-testing.md`: expected-output files in
    // `tests/fixtures/`, compared via `include_str!` + `assert_eq!` — no
    // snapshot-testing crate) — the substring assertions above document
    // intent; these catch everything else.

    #[test]
    fn mvnd_reactor_pass_full_output() {
        let i = include_str!("../../../tests/fixtures/mvnd_reactor_pass_raw.txt");
        let expected = include_str!("../../../tests/fixtures/mvnd_reactor_pass_expected.txt");
        assert_eq!(filter_package(i, true), expected);
    }

    #[test]
    fn mvnd_test_fail_full_output() {
        let i = include_str!("../../../tests/fixtures/mvnd_test_fail_raw.txt");
        let expected = include_str!("../../../tests/fixtures/mvnd_test_fail_expected.txt");
        assert_eq!(filter_surefire(i, true), expected);
    }

    #[test]
    fn mvnd_parallel_reactor_fail_full_output() {
        let i = include_str!("../../../tests/fixtures/mvnd_reactor_fail_raw.txt");
        let expected = include_str!("../../../tests/fixtures/mvnd_reactor_fail_expected.txt");
        assert_eq!(filter_surefire(i, true), expected);
    }

    #[test]
    fn mvnd_compile_error_full_output() {
        let i = include_str!("../../../tests/fixtures/mvnd_compile_error_raw.txt");
        let expected = include_str!("../../../tests/fixtures/mvnd_compile_error_expected.txt");
        assert_eq!(filter_compile(i, true), expected);
    }

    /// `mvnd compile` on a syntax error (exit code 1): compile diagnostics
    /// (file, coordinates, message) survive the compile filter.
    #[test]
    fn mvnd_compile_error_preserves_diagnostics() {
        let i = include_str!("../../../tests/fixtures/mvnd_compile_error_raw.txt");
        let o = filter_compile(i, true);
        assert!(o.contains("Calc.java:[5,21] ';' expected"));
        assert!(o.contains("[INFO] BUILD FAILURE"));
        assert!(o.contains("[ERROR] Failed to execute goal"));
        assert!(!o.contains("Processing build on daemon"));
        assert!(!o.contains("BuildTimeEventSpy"));
    }

    /// Parity with the other three mvnd fixtures' dedicated savings tests
    /// (`mvnd_reactor_pass_savings`, `mvnd_test_fail_savings`,
    /// `mvnd_parallel_reactor_fail_savings`) — same ≥60% floor.
    #[test]
    fn mvnd_compile_error_savings() {
        let i = include_str!("../../../tests/fixtures/mvnd_compile_error_raw.txt");
        let o = filter_compile(i, true);
        let savings = 100.0 - (count_tokens(&o) as f64 / count_tokens(i) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60% savings, got {savings:.1}%");
    }

    // ── Surefire filter ──────────────────────────────────────────────────────

    #[test]
    fn filter_surefire_pass_output_compact() {
        let i = include_str!("../../../tests/fixtures/mvn_test_pass_slice_raw.txt");
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
        assert!(o.contains("BUILD FAILURE"));
        assert!(o.contains("Failures: 1"));
    }

    #[test]
    fn surefire_drops_passing_block() {
        let i = include_str!("../../../tests/fixtures/mvn_test_pass_slice_raw.txt");
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_package(i, false);
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
        let o = filter_surefire_with_cap(i, 1, false);

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
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_package(i, false);
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

    /// Reviewer follow-up (upstream PR #3199, final review round): the
    /// `FILE_COORD` arming guard is scoped to *tagged* (mvnd module) lanes
    /// only. On the root (untagged) lane — the only lane a plain `mvn` run
    /// ever uses — any `[ERROR]` line still arms broadly, exactly as it did
    /// pre-PR, so a non-compiler `[ERROR]` (no `file.java:[line,col]`
    /// coordinate) with an indented raw continuation is still kept.
    fn assert_root_lane_arms_broadly_without_file_coord(filter: fn(&str) -> String) {
        let i = "[INFO] Scanning for projects...\n\
                  [ERROR] Internal error: java.lang.IllegalStateException: broken plugin\n\
                  \x20 at com.example.PluginImpl.run(PluginImpl.java:42)\n\
                  [INFO] BUILD FAILURE\n";
        let o = filter(i);
        assert!(
            o.contains("Internal error: java.lang.IllegalStateException"),
            "the ERROR line itself survives; got:\n{o}"
        );
        assert!(
            o.contains("at com.example.PluginImpl.run(PluginImpl.java:42)"),
            "root lane arms broadly (pre-PR behavior), so the raw continuation survives; got:\n{o}"
        );
    }

    #[test]
    fn surefire_root_lane_arms_broadly_without_file_coord() {
        assert_root_lane_arms_broadly_without_file_coord(filter_surefire_plain);
    }

    #[test]
    fn compile_root_lane_arms_broadly_without_file_coord() {
        assert_root_lane_arms_broadly_without_file_coord(filter_compile_plain);
    }

    #[test]
    fn package_root_lane_arms_broadly_without_file_coord() {
        assert_root_lane_arms_broadly_without_file_coord(filter_package_plain);
    }

    #[test]
    fn surefire_keeps_module_banner() {
        let i = "[INFO] Scanning for projects...\n[INFO] -----< com.example:myapp >-----\n[INFO] BUILD SUCCESS\n";
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o = filter_surefire(i, false);
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
        let o_lf = filter_surefire(&i_lf, false);
        let i_crlf = i_lf.replace('\n', "\r\n");
        let o_crlf = filter_surefire(&i_crlf, false);
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
        let o_lf = filter_package(&i_lf, false);
        let i_crlf = i_lf.replace('\n', "\r\n");
        let o_crlf = filter_package(&i_crlf, false);
        assert_eq!(
            o_lf,
            o_crlf.replace("\r\n", "\n"),
            "CRLF filtered output must match LF (modulo line endings)"
        );
    }

    #[test]
    fn compile_handles_crlf_line_endings() {
        let i_lf = include_str!("../../../tests/fixtures/mvnd_compile_error_raw.txt")
            .replace("\r\n", "\n");
        let o_lf = filter_compile(&i_lf, false);
        let i_crlf = i_lf.replace('\n', "\r\n");
        let o_crlf = filter_compile(&i_crlf, false);
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

        let o = filter_surefire_with_cap(&i, 3, false);

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
        let o = filter_surefire_with_cap(&i, 0, false);
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
        let o = filter_surefire_with_cap(&i, 3, false);

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
        let o = filter_package(i, false);
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
        let o = filter_package(i, false);
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
        let o = filter_compile(i, false);
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
        let o = filter_compile(i, false);
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
        let o = filter_compile(i, false);
        let warns = o.matches("[WARNING]").count();
        assert_eq!(warns, 2, "dedup by normalised message; got:\n{}", o);
    }

    // ── Package filter ───────────────────────────────────────────────────────

    #[test]
    fn filter_package_install_compact() {
        let i = include_str!("../../../tests/fixtures/mvn_install_slice_raw.txt");
        let o = filter_package(i, false);
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
        let o = filter_package(i, false);
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
        let pf_out = filter_surefire(&pf, false);
        let pf_in_tok = count_tokens(&pf);
        let pf_out_tok = count_tokens(&pf_out);
        let pf_s = 100.0 - (pf_out_tok as f64 / pf_in_tok as f64 * 100.0);
        println!(
            "mvn_test_pass_full: {} -> {} tokens ({:.1}% savings)",
            pf_in_tok, pf_out_tok, pf_s
        );

        let inst = gunzip(include_bytes!("../../../tests/fixtures/mvn_install_full_raw.txt.gz"));
        let inst_out = filter_package(&inst, false);
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
        let o = filter_surefire(&i, false);
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
        let o = filter_package(&i, false);
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
        assert_eq!(filter_quiet("", false), "");
        assert_eq!(filter_quiet("   \n\n  \n", false), "");
    }

    /// Failure under `-q`: keep close-line, exception, user frame, summary,
    /// goal terminator. Drop framework frames + help boilerplate block.
    #[test]
    fn quiet_fail_strips_framework_and_boilerplate() {
        let i = include_str!("../../../tests/fixtures/mvn_quiet_fail_raw.txt");
        let o = filter_quiet(i, false);

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
        let o = filter_quiet(i, false);
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
        let o = filter_quiet(i, false);
        assert!(
            o.contains("Some unexpected error output"),
            "unclassified ERROR line preserved; got:\n{}",
            o
        );
    }

    // ── split_lane / app-log false-positive lane guard ──────────────────────

    /// `[tag] [LEVEL] …` only counts as lane-shaped when the second bracket
    /// is a genuine level — a non-level second bracket (or none at all)
    /// falls through to the root lane exactly like a plain unprefixed line.
    #[test]
    fn split_lane_requires_real_level_in_second_bracket() {
        assert_eq!(
            split_lane("[child-a] [INFO] Running com.example.rtk.FooTest", true),
            (Some("child-a"), "[INFO] Running com.example.rtk.FooTest")
        );
        assert_eq!(
            split_lane("[main] [status] app started", true),
            (None, "[main] [status] app started")
        );
        assert_eq!(
            split_lane("[INFO] plain single-bracket line", true),
            (Some(""), "[INFO] plain single-bracket line")
        );
        assert_eq!(
            split_lane("at com.example.rtk.FooTest.bar(FooTest.java:1)", true),
            (None, "at com.example.rtk.FooTest.bar(FooTest.java:1)")
        );
        // Bracket-shaped but not Maven's own `[LEVEL] ...` output and not a
        // real `[tag] [LEVEL]` module line either: raw, not root-keyed.
        assert_eq!(
            split_lane("[boom] weird bracket assertion line", true),
            (None, "[boom] weird bracket assertion line")
        );
        assert_eq!(
            split_lane("[1, 2] != [1, 3]", true),
            (None, "[1, 2] != [1, 3]")
        );
        // No `"] "` substring at all: only Maven's own bare `[LEVEL]` blank
        // root-keys — a bracketed assertion-diff fragment with no `"] "`
        // anywhere is raw, same as the `"] "`-present case above.
        assert_eq!(split_lane("[INFO]", true), (Some(""), "[INFO]"));
        assert_eq!(split_lane("[1, 2]", true), (None, "[1, 2]"));
        assert_eq!(split_lane("[boom]", true), (None, "[boom]"));
        // Empty tag: `""` is the root lane's reserved key, so `[] [LEVEL] …`
        // must not impersonate root-keyed routing — raw, not `(Some(""), …)`.
        assert_eq!(
            split_lane("[] [INFO] started", true),
            (None, "[] [INFO] started")
        );
        // Tagged blank, no-trailing-space spelling (`[tag] [LEVEL]`, nothing
        // after — `lane_rest_level`'s documented other spelling of the
        // daemon-prefixed trail terminator, alongside `[tag] [LEVEL] `).
        // Pinned so the "both spellings" contract can't rot.
        assert_eq!(split_lane("[child-a] [INFO]", true), (Some("child-a"), "[INFO]"));
    }

    // ── daemon gate: plain `mvn` never routes through the lane layer ────────

    /// An SLF4J/Logback `[%thread] [%level] %msg` layout produces lines
    /// syntactically identical to a real mvnd module line, so plain `mvn` must
    /// never reach the lane layer at all: `daemon == false` short-circuits
    /// `split_lane` before any bracket parsing happens, which makes the whole
    /// phantom-lane class structurally impossible there. A failure's context
    /// lines stay exactly where base keeps them — inside the failing block,
    /// before the footer.
    #[test]
    fn slf4j_thread_tag_does_not_mint_phantom_lane_in_plain_mvn() {
        let i = "[INFO] Scanning for projects...\n\
                  [INFO] Running com.example.rtk.MyTest\n\
                  [main] [INFO] Running the widget pipeline\n\
                  [main] [INFO] connecting to db\n\
                  [main] [INFO] connection refused\n\
                  [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.MyTest\n\
                  [ERROR] com.example.rtk.MyTest.testFoo -- Time elapsed: 0.05 s <<< FAILURE!\n\
                  java.lang.AssertionError: boom\n\
                  \tat com.example.rtk.MyTest.testFoo(MyTest.java:10)\n\
                  \n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i, false);
        let pipeline = o
            .find("Running the widget pipeline")
            .expect("app-log Running-shaped line kept");
        let db = o.find("connecting to db").expect("first context line kept");
        let refused = o.find("connection refused").expect("second context line kept");
        let footer = o.rfind("BUILD FAILURE").expect("footer kept");
        assert!(
            pipeline < db && db < refused && refused < footer,
            "context lines stay attached to their failing block, in order, \
             before the footer — never detached and reordered after it; got:\n{o}"
        );
    }

    /// Cold-preclear finding (upstream PR #3199, third review round),
    /// timestamp probe: a `[timestamp] [LEVEL] …` layout is exactly as
    /// lane-shaped as an SLF4J thread tag. Pre-fix, 300 uniquely-timestamped
    /// lines each minted their own phantom lane on plain `mvn` output —
    /// base drops every one of them (none match any keep-list pattern), but
    /// the lane bug resurrected all 300 at end-of-stream. Post-fix, `daemon
    /// == false` means none of these lines are ever bracket-parsed at all,
    /// so they fall through the same keep-list check as base and are
    /// dropped identically.
    #[test]
    fn timestamp_tag_does_not_mint_phantom_lanes_in_plain_mvn() {
        let mut i = String::from("[INFO] Scanning for projects...\n");
        for n in 0..300 {
            i.push_str(&format!("[2026-08-29 10:00:00] [INFO] Building segment {n}\n"));
        }
        i.push_str("[INFO] BUILD SUCCESS\n");
        let o = filter_surefire(&i, false);
        assert!(
            !o.contains("Building segment"),
            "timestamp-tagged app log lines are dropped, exactly like base \
             (none match any keep-list pattern); got:\n{o}"
        );
    }

    /// Cold-preclear finding #2 (upstream PR #3199, third review round): a
    /// module whose *only* failures are integration-test (Failsafe) ones —
    /// never Surefire — must still get a fresh budget for its own summary.
    /// child-a exhausts cap=2 in the Surefire phase (3 failures, 1 dropped);
    /// child-b's failsafe banner (a *different* goal string from child-a's
    /// surefire banner) is the phase-marker boundary that resets the budget
    /// before child-b's own (first-ever-sighting, never-repeating) header is
    /// processed — so its one integration-test failure survives instead of
    /// sharing Surefire's already-spent budget.
    #[test]
    fn failsafe_only_module_gets_fresh_budget_via_phase_marker() {
        let i = "[INFO] Scanning for projects...\n\
                  [child-a] [INFO] --- surefire:3.5.5:test (default-test) @ child-a ---\n\
                  [child-a] [INFO] Running com.example.rtk.AMultiFailTest\n\
                  [child-a] [ERROR] Tests run: 3, Failures: 3, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.AMultiFailTest\n\
                  [child-a] [ERROR] com.example.rtk.AMultiFailTest.one -- Time elapsed: 0.02 s <<< FAILURE!\n\
                  java.lang.AssertionError: a1 boom\n\
                  \tat com.example.rtk.AMultiFailTest.one(AMultiFailTest.java:10)\n\
                  [child-a] [INFO] \n\
                  [child-a] [ERROR] com.example.rtk.AMultiFailTest.two -- Time elapsed: 0.02 s <<< FAILURE!\n\
                  java.lang.AssertionError: a2 boom\n\
                  \tat com.example.rtk.AMultiFailTest.two(AMultiFailTest.java:20)\n\
                  [child-a] [INFO] \n\
                  [child-a] [ERROR] com.example.rtk.AMultiFailTest.three -- Time elapsed: 0.02 s <<< FAILURE!\n\
                  java.lang.AssertionError: a3 boom\n\
                  \tat com.example.rtk.AMultiFailTest.three(AMultiFailTest.java:30)\n\
                  [child-a] [INFO] \n\
                  [child-a] [INFO] Results:\n\
                  [child-a] [ERROR] Failures: \n\
                  [child-a] [ERROR]   AMultiFailTest.one:10 a1 boom\n\
                  [child-a] [ERROR]   AMultiFailTest.two:20 a2 boom\n\
                  [child-a] [ERROR]   AMultiFailTest.three:30 a3 boom\n\
                  [child-a] [ERROR] Tests run: 3, Failures: 3, Errors: 0, Skipped: 0\n\
                  [child-b] [INFO] --- failsafe:3.5.5:integration-test (default-integration-test) @ child-b ---\n\
                  [child-b] [INFO] Running com.example.rtk.BIT\n\
                  [child-b] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.BIT\n\
                  [child-b] [ERROR] com.example.rtk.BIT.only -- Time elapsed: 0.02 s <<< FAILURE!\n\
                  java.lang.AssertionError: b1 boom\n\
                  \tat com.example.rtk.BIT.only(BIT.java:5)\n\
                  [child-b] [INFO] \n\
                  [child-b] [INFO] Results:\n\
                  [child-b] [ERROR] Failures: \n\
                  [child-b] [ERROR]   BIT.only:5 b1 boom\n\
                  [child-b] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0\n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_package_with_cap(i, 2, true);
        assert!(
            o.contains("AMultiFailTest.one:10 a1 boom") && o.contains("AMultiFailTest.two:20 a2 boom"),
            "child-a's first two entries fill its Surefire-phase budget; got:\n{o}"
        );
        assert!(
            !o.contains("AMultiFailTest.three:30 a3 boom"),
            "child-a's third entry is capped; got:\n{o}"
        );
        assert!(
            o.contains("BIT.only:5 b1 boom"),
            "child-b's Failsafe-only entry gets a fresh budget from the phase-marker \
             boundary, not zero from Surefire's already-spent one; got:\n{o}"
        );
        assert_eq!(
            o.matches("… +1 more failures").count(),
            1,
            "only child-a reports a drop (its own third entry); child-b's fresh \
             budget covers its one entry with no tail; got:\n{o}"
        );
    }

    /// Cold-preclear finding (upstream PR #3199, fourth review round): same
    /// scenario as [`failsafe_only_module_gets_fresh_budget_via_phase_marker`],
    /// but with Maven ≤3.8 / mvnd 0.x's banner spelling
    /// (`maven-surefire-plugin:2.22.2:test` / `maven-failsafe-plugin:2.22.2:
    /// integration-test`, not the bare `surefire:`/`failsafe:` shorthand).
    /// Pre-fix, `TEST_PLUGIN_BANNER` didn't match either banner at all, so
    /// `observe_plugin_banner` never fired and child-b's Failsafe-only entry
    /// silently inherited child-a's already-spent budget (kept 0, not 1) —
    /// the exact failure the phase marker exists to prevent.
    #[test]
    fn failsafe_only_module_gets_fresh_budget_with_old_plugin_banner_spelling() {
        let i = "[INFO] Scanning for projects...\n\
                  [child-a] [INFO] --- maven-surefire-plugin:2.22.2:test (default-test) @ child-a ---\n\
                  [child-a] [INFO] Running com.example.rtk.AMultiFailTest\n\
                  [child-a] [ERROR] Tests run: 3, Failures: 3, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.AMultiFailTest\n\
                  [child-a] [ERROR] com.example.rtk.AMultiFailTest.one -- Time elapsed: 0.02 s <<< FAILURE!\n\
                  java.lang.AssertionError: a1 boom\n\
                  \tat com.example.rtk.AMultiFailTest.one(AMultiFailTest.java:10)\n\
                  [child-a] [INFO] \n\
                  [child-a] [ERROR] com.example.rtk.AMultiFailTest.two -- Time elapsed: 0.02 s <<< FAILURE!\n\
                  java.lang.AssertionError: a2 boom\n\
                  \tat com.example.rtk.AMultiFailTest.two(AMultiFailTest.java:20)\n\
                  [child-a] [INFO] \n\
                  [child-a] [ERROR] com.example.rtk.AMultiFailTest.three -- Time elapsed: 0.02 s <<< FAILURE!\n\
                  java.lang.AssertionError: a3 boom\n\
                  \tat com.example.rtk.AMultiFailTest.three(AMultiFailTest.java:30)\n\
                  [child-a] [INFO] \n\
                  [child-a] [INFO] Results:\n\
                  [child-a] [ERROR] Failures: \n\
                  [child-a] [ERROR]   AMultiFailTest.one:10 a1 boom\n\
                  [child-a] [ERROR]   AMultiFailTest.two:20 a2 boom\n\
                  [child-a] [ERROR]   AMultiFailTest.three:30 a3 boom\n\
                  [child-a] [ERROR] Tests run: 3, Failures: 3, Errors: 0, Skipped: 0\n\
                  [child-b] [INFO] --- maven-failsafe-plugin:2.22.2:integration-test (default-integration-test) @ child-b ---\n\
                  [child-b] [INFO] Running com.example.rtk.BIT\n\
                  [child-b] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.BIT\n\
                  [child-b] [ERROR] com.example.rtk.BIT.only -- Time elapsed: 0.02 s <<< FAILURE!\n\
                  java.lang.AssertionError: b1 boom\n\
                  \tat com.example.rtk.BIT.only(BIT.java:5)\n\
                  [child-b] [INFO] \n\
                  [child-b] [INFO] Results:\n\
                  [child-b] [ERROR] Failures: \n\
                  [child-b] [ERROR]   BIT.only:5 b1 boom\n\
                  [child-b] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0\n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_package_with_cap(i, 2, true);
        assert!(
            o.contains("AMultiFailTest.one:10 a1 boom") && o.contains("AMultiFailTest.two:20 a2 boom"),
            "child-a's first two entries fill its Surefire-phase budget; got:\n{o}"
        );
        assert!(
            !o.contains("AMultiFailTest.three:30 a3 boom"),
            "child-a's third entry is capped; got:\n{o}"
        );
        assert!(
            o.contains("BIT.only:5 b1 boom"),
            "old-spelling `maven-failsafe-plugin:...` banner still fires the phase-marker \
             reset, so child-b's Failsafe-only entry gets a fresh budget; got:\n{o}"
        );
    }

    /// Reviewer probe (upstream PR #3199 finding 2): an application log line
    /// shaped `[thread] [LEVEL] …` inside a failing test's captured stdout
    /// must not mint a phantom lane and vanish from the committed block —
    /// `main` was never established as a module (no `Running`/`Building`/
    /// banner/`[ERROR]` line for it), so it falls back to raw-line ownership
    /// and rides along with the rest of the failing block's content.
    #[test]
    fn app_log_line_survives_inside_failing_block() {
        let i = "[INFO] Running com.example.rtk.MyTest\n\
                  [main] [INFO] app started with config=/etc/foo.yml\n\
                  plain stdout line\n\
                  [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.MyTest\n\
                  [ERROR] com.example.rtk.MyTest.testFoo -- Time elapsed: 0.05 s <<< FAILURE!\n\
                  java.lang.AssertionError: expected <1> but was <2>\n\
                  \tat com.example.rtk.MyTest.testFoo(MyTest.java:10)\n\
                  \n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i, true);
        assert!(
            o.contains("app started with config=/etc/foo.yml"),
            "app log line kept in the committed block; got:\n{o}"
        );
        assert!(
            o.contains("plain stdout line"),
            "adjacent unbracketed stdout still kept; got:\n{o}"
        );
    }

    /// A genuine mvnd module tag (established via its `Running` line) must
    /// still lane-split correctly: interleaved output from two modules is
    /// kept separate, so a passing class from one module never leaks under
    /// the other's failing block.
    #[test]
    fn genuine_mvnd_module_tags_still_lane_split() {
        let i = "[INFO] Scanning for projects...\n\
                  [child-a] [INFO] Running com.example.rtk.FailTest\n\
                  [child-b] [INFO] Running com.example.rtk.PassTest\n\
                  [child-b] [INFO] Tests run: 1, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.010 s -- in com.example.rtk.PassTest\n\
                  [child-a] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.020 s <<< FAILURE! -- in com.example.rtk.FailTest\n\
                  [child-a] [ERROR] com.example.rtk.FailTest.bar -- Time elapsed: 0.020 s <<< FAILURE!\n\
                  org.opentest4j.AssertionFailedError: expected: <1> but was: <2>\n\
                  \tat com.example.rtk.FailTest.bar(FailTest.java:5)\n\
                  \n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i, true);
        assert!(
            o.contains("[child-a] [ERROR] Tests run: 1, Failures: 1"),
            "failing module's close kept; got:\n{o}"
        );
        assert!(
            o.contains("expected: <1> but was: <2>"),
            "failing module's assertion kept; got:\n{o}"
        );
        assert!(
            !o.contains("PassTest"),
            "passing module's class collapsed, not leaked under child-a's block; got:\n{o}"
        );
    }

    /// Reviewer blocker (upstream PR #3199, follow-up cold review): a
    /// multi-line assertion message inside a *tagged* lane's failure trail
    /// can contain bracket-leading continuation lines (`[1, 2] != [1, 3]`,
    /// a collection diff; `[boom] weird bracket assertion line`) that are
    /// bracket-shaped but not Maven's own `[LEVEL] ...` output. Pre-fix,
    /// `split_lane` root-keyed these (bypassing `raw_owner` entirely), so
    /// they landed on the root keep-list and were dropped even though
    /// child-a's trail was active. They must now route as raw content and
    /// survive as part of the trail, same as any other continuation line.
    fn assert_bracket_leading_trail_line_survives(filter: fn(&str) -> String) {
        let i = "[INFO] Scanning for projects...\n\
                  [child-a] [INFO] Running com.example.rtk.FooTest\n\
                  [child-a] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.FooTest\n\
                  [child-a] [ERROR] com.example.rtk.FooTest.bar -- Time elapsed: 0.05 s <<< FAILURE!\n\
                  org.opentest4j.AssertionFailedError: collections differ\n\
                  [1, 2] != [1, 3]\n\
                  [boom] weird bracket assertion line\n\
                  [1, 2]\n\
                  [boom]\n\
                  [] [INFO] started\n\
                  \tat com.example.rtk.FooTest.bar(FooTest.java:5)\n\
                  \n\
                  [INFO] BUILD FAILURE\n";
        let o = filter(i);
        assert!(
            o.contains("[1, 2] != [1, 3]"),
            "bracket-leading collection diff survives; got:\n{o}"
        );
        assert!(
            o.contains("[boom] weird bracket assertion line"),
            "bracket-leading assertion fragment survives; got:\n{o}"
        );
        // No `"] "` substring at all — the sibling shape split_lane must
        // also not root-key (only a bare `[LEVEL]` blank may).
        assert!(
            o.lines().any(|l| l.trim_end() == "[1, 2]"),
            "bare bracket diff line (no \"] \") survives; got:\n{o}"
        );
        assert!(
            o.lines().any(|l| l.trim_end() == "[boom]"),
            "bare bracket fragment (no \"] \") survives; got:\n{o}"
        );
        // Empty tag: `[] [INFO] started` must not impersonate the root
        // lane and get swallowed by the root keep-list.
        assert!(
            o.contains("[] [INFO] started"),
            "empty-tag line survives, not swallowed as root-keyed; got:\n{o}"
        );
        assert!(
            o.contains("at com.example.rtk.FooTest.bar(FooTest.java:5)"),
            "user frame still survives too; got:\n{o}"
        );
    }

    #[test]
    fn surefire_bracket_leading_trail_line_survives() {
        assert_bracket_leading_trail_line_survives(filter_surefire_daemon);
    }

    #[test]
    fn package_bracket_leading_trail_line_survives() {
        assert_bracket_leading_trail_line_survives(filter_package_daemon);
    }

    /// Reviewer probe (upstream PR #3199 finding 3): two `[tag] [ERROR] …`
    /// app-log lines earlier in the output each mint a lane, but neither
    /// looks like a compiler diagnostic, so neither ever arms a
    /// continuation claim. A later real failing class's trail must then be
    /// the sole, unambiguous claimant of its own raw frames: the single
    /// user frame survives, and the framework frames (`org.junit.jupiter`,
    /// `java.base/`) are stripped exactly as they would be with no phantom
    /// lanes in play at all.
    #[test]
    fn phantom_lane_claim_never_arms_and_never_steals_a_real_trail() {
        let i = "[INFO] Scanning for projects...\n\
                  [Server] [ERROR] connection refused to db\n\
                  [Cache] [ERROR] connection refused to cache\n\
                  [INFO] Running com.example.rtk.RealTest\n\
                  [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.RealTest\n\
                  [ERROR] com.example.rtk.RealTest.foo -- Time elapsed: 0.05 s <<< FAILURE!\n\
                  java.lang.AssertionError: boom\n\
                  \tat com.example.rtk.RealTest.foo(RealTest.java:5)\n\
                  \tat org.junit.jupiter.engine.execution.SomeThing.invoke(SomeThing.java:1)\n\
                  \tat java.base/jdk.internal.reflect.NativeMethodAccessorImpl.invoke0(Native Method)\n\
                  \n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i, true);
        assert!(
            o.contains("at com.example.rtk.RealTest.foo(RealTest.java:5)"),
            "the single user frame survives; got:\n{o}"
        );
        assert!(
            !o.contains("org.junit.jupiter"),
            "junit framework frame must not leak; got:\n{o}"
        );
        assert!(
            !o.contains("java.base/"),
            "java.base framework frame must not leak; got:\n{o}"
        );
    }

    /// Reviewer finding #1 (upstream PR #3199, second review round), failing
    /// side: a `[tag] [ERROR]` app-log line for a never-seen tag (e.g.
    /// `[Server] [ERROR] connection refused`, no `file.java:[line,col]`
    /// coordinate) landing *inside* another lane's still-open buffered block
    /// must stay inside that block, at its input position — not mint its own
    /// fresh lane and get written to `out` immediately, ahead of the
    /// `Running` line (and the rest of the block) it followed in the input.
    /// Pre-fix, `is_lane_opener`'s blanket `[ERROR]`-prefix rule minted
    /// `[Server]` its own lane; post-fix it has no `FILE_COORD` coordinate,
    /// so it's not an opener and `Lanes::route` falls back to `raw_owner`,
    /// which buffers it into `child-a`'s uniquely-open block instead.
    #[test]
    fn app_log_error_stays_in_order_inside_failing_block() {
        let i = "[INFO] Scanning for projects...\n\
                  [child-a] [INFO] Running com.example.rtk.SlowTest\n\
                  [Server] [ERROR] connection refused\n\
                  [child-a] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.SlowTest\n\
                  [child-a] [ERROR] com.example.rtk.SlowTest.foo -- Time elapsed: 0.05 s <<< FAILURE!\n\
                  java.lang.AssertionError: boom\n\
                  \tat com.example.rtk.SlowTest.foo(SlowTest.java:5)\n\
                  [child-a] [INFO] \n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i, true);
        let running = o.find("Running com.example.rtk.SlowTest").expect("Running kept");
        let server = o
            .find("[Server] [ERROR] connection refused")
            .expect("app log line kept, buffered inside the block");
        let close = o
            .find("<<< FAILURE! -- in com.example.rtk.SlowTest")
            .expect("close line kept");
        assert!(
            running < server && server < close,
            "app-log line must stay in its original input order inside the block \
             (Running, then the app log, then the close), not jump ahead of Running; got:\n{o}"
        );
    }

    /// Cold-preclear finding (🟡 1): `lane_rest_level` documents two blank
    /// terminator spellings for a tagged lane's trail — `[tag] [LEVEL] `
    /// (trailing space, mvnd's usual daemon-prefixed blank) and `[tag]
    /// [LEVEL]` (no trailing space, nothing after). Every other
    /// trail-termination test in this file uses the trailing-space spelling
    /// (see `split_lane`'s own doc comment); this one pins the no-space
    /// spelling so that half of the documented contract can't silently rot.
    /// If termination on the no-space spelling ever breaks, the bare
    /// terminator line survives as literal trail content instead of being
    /// consumed, so it would appear verbatim right before the next
    /// `Running` line.
    #[test]
    fn tagged_trail_terminates_on_no_trailing_space_blank() {
        let i = "[INFO] Scanning for projects...\n\
                  [child-a] [INFO] Running com.example.rtk.SlowTest\n\
                  [child-a] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in com.example.rtk.SlowTest\n\
                  [child-a] [ERROR] com.example.rtk.SlowTest.foo -- Time elapsed: 0.05 s <<< FAILURE!\n\
                  java.lang.AssertionError: boom\n\
                  \tat com.example.rtk.SlowTest.foo(SlowTest.java:5)\n\
                  \tat org.junit.jupiter.api.Assertions.fail(Assertions.java:1)\n\
                  [child-a] [INFO]\n\
                  [child-a] [INFO] Running com.example.rtk.NextTest\n\
                  [child-a] [INFO] Tests run: 1, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.01 s -- in com.example.rtk.NextTest\n\
                  [INFO] BUILD FAILURE\n";
        let o = filter_surefire(i, true);
        assert!(
            o.contains("at com.example.rtk.SlowTest.foo(SlowTest.java:5)"),
            "user frame survives; got:\n{o}"
        );
        assert!(
            !o.contains("org.junit.jupiter"),
            "framework frame stripped; got:\n{o}"
        );
        assert!(
            !o.contains("[child-a] [INFO]\n[child-a] [INFO] Running"),
            "the no-space blank must be consumed as the trail terminator, not survive \
             as a stray literal line before the next Running; got:\n{o}"
        );
        assert!(o.contains("BUILD FAILURE"), "footer survives; got:\n{o}");
    }

    /// Reviewer finding #1 (upstream PR #3199, second review round), green
    /// side: the same `[tag] [ERROR]` app-log line, but the block it lands
    /// inside closes clean (0 failures/errors) — the whole block, app-log
    /// line included, must be discarded with the rest, not survive as a
    /// stray leftover from a lane it should never have minted.
    #[test]
    fn app_log_error_dropped_with_green_close_block() {
        let i = "[INFO] Scanning for projects...\n\
                  [child-a] [INFO] Running com.example.rtk.SlowTest\n\
                  [Server] [ERROR] connection refused\n\
                  [child-a] [INFO] Tests run: 1, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.05 s -- in com.example.rtk.SlowTest\n\
                  [INFO] BUILD SUCCESS\n";
        let o = filter_surefire(i, true);
        assert!(
            !o.contains("[Server]") && !o.contains("connection refused"),
            "app-log line inside a green-closed block must be discarded with the rest \
             of the block, not survive via a phantom lane; got:\n{o}"
        );
    }

    /// Finding 4: an adversarial run of uniquely-tagged opener-shaped lines
    /// (each individually opener-shaped, per `is_lane_opener`) must not mint
    /// unbounded lanes — `Lanes::route`'s scan stays `O(MAX_LANES)`, not
    /// `O(distinct tags)`. Also exercises the public entry point end to end
    /// to confirm the cap doesn't panic or drop output.
    #[test]
    fn lane_count_is_capped_on_pathological_input() {
        let mut i = String::from("[INFO] Scanning for projects...\n");
        // `Running` lines deliberately (not bare `[ERROR]` text): since
        // finding #1 (upstream PR #3199, second round) narrowed
        // `is_lane_opener`'s `[ERROR]` arm to genuine `FILE_COORD` compiler
        // diagnostics, a non-coordinate `[ERROR]` line no longer mints a
        // lane at all. `Running` is unaffected by that fix and, unlike a
        // `FILE_COORD` line, never arms `keep_continuation` either — so the
        // overflow-module assertion below isn't accidentally satisfied by an
        // unrelated 255-way armed tie in `raw_owner` instead of exercising
        // the fix it targets.
        for n in 0..(MAX_LANES + 50) {
            i.push_str(&format!("[tag{n}] [INFO] Running com.example.rtk.Tag{n}Test\n"));
        }
        // One more never-seen tag, past the cap: a genuine failing test
        // class (`Running` + failing close), not just a bare `[ERROR]`
        // line — the shape the cold-review fix specifically targets
        // (`Lanes::route` preferring `None` over `raw_owner` guesswork once
        // the cap is exceeded, so this doesn't get misrouted onto an
        // unrelated lane and corrupt or drop it).
        i.push_str(
            "[overflow-mod] [INFO] Running com.example.rtk.OverflowTest\n\
             [overflow-mod] [ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.01 s <<< FAILURE! -- in com.example.rtk.OverflowTest\n",
        );
        i.push_str("[INFO] BUILD FAILURE\n");

        let mut lanes = Lanes::new();
        let mut classes = FailingClassCap::new(MAX_MVN_FAILING_CLASSES);
        let mut summary = FailuresSummaryCap::new(MAX_MVN_FAILING_CLASSES, true);
        let mut out = String::new();
        for line in i.lines() {
            drive_surefire_line(&mut lanes, line, &mut classes, &mut summary, true, &mut out);
        }
        assert!(
            lanes.lanes.len() <= MAX_LANES,
            "lane count must stay capped at {MAX_LANES}, got {}",
            lanes.lanes.len()
        );

        let o = filter_surefire(&i, true);
        assert!(!o.is_empty(), "capped run still produces output");
        assert!(
            o.contains("Running com.example.rtk.OverflowTest")
                && o.contains("<<< FAILURE! -- in com.example.rtk.OverflowTest"),
            "an overflow (past-cap) module's own failing diagnostics must survive \
             (preserved verbatim), not be lost to a misrouted guess; got:\n{o}"
        );
    }
}





