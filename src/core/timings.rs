//! Opt-in wall-clock stage breakdown (`RTK_TIMINGS=1`), printed to stderr.
//!
//! The recorded `exec_time_ms` starts inside each command handler, so it can
//! never say how much of a run was the wrapped command versus rtk itself.
//! With `RTK_TIMINGS=1`, every tracked command prints one stderr line after
//! it finishes:
//!
//! ```text
//! rtk timings: total=41.2ms startup=2.9ms handler=37.8ms child=30.1ms spawns=2 filter=7.7ms track=0.5ms
//! ```
//!
//! - `total`   — process start to this line
//! - `startup` — process start to the handler's timer: CLI parse, config, hook checks
//! - `handler` — the span recorded as `exec_time_ms`
//! - `child`   — wall time spent on spawned processes (`spawns` counts them).
//!   For streaming commands this runs from spawn until output is drained, so
//!   it includes rtk's concurrent reading and printing.
//! - `filter`  — derived as `handler - child`: rtk's own parsing, filtering,
//!   and printing. A floor, not a measurement — on streaming commands the
//!   overlapped work sits in `child`.
//! - `track`   — the history-DB write, which runs after `exec_time_ms` is read
//!
//! Stderr-only, so filtered stdout stays a valid subset of the wrapped tool's
//! output (Transparency). Disabled, every hook is a single boolean check
//! (Zero Overhead). Commands that never call [`super::tracking::TimedExecution`]
//! (`--version`, `gain`, `run`, hook mode) print nothing.
//!
//! Handlers that spawn a child without going through `stream.rs` must wrap
//! the spawn in [`time_child`] (or a [`child_start`]/[`child_end`] pair), or
//! that child's runtime is misreported as `filter`.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

static ENABLED: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RTK_TIMINGS").as_deref() == Ok("1"));

static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);

static CHILD_MICROS: AtomicU64 = AtomicU64::new(0);
static CHILD_SPAWNS: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool {
    *ENABLED
}

/// Pin the process-start instant. Call first thing in `main()`; without it,
/// `total` and `startup` in [`report`] are meaningless (near-zero).
pub fn mark_process_start() {
    LazyLock::force(&PROCESS_START);
}

/// Start timing a child process. `None` when timings are disabled.
pub fn child_start() -> Option<Instant> {
    enabled().then(Instant::now)
}

/// Accumulate the wall time since [`child_start`]. No-op on `None`.
pub fn child_end(started: Option<Instant>) {
    if let Some(t) = started {
        CHILD_MICROS.fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
        CHILD_SPAWNS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Run `f` (a child spawn-and-wait) with its wall time attributed to `child`.
pub fn time_child<T>(f: impl FnOnce() -> T) -> T {
    let started = child_start();
    let result = f();
    child_end(started);
    result
}

/// Print the breakdown line. Called by `TimedExecution` after the history-DB
/// write; `handler_started` is its start instant, `handler` the span recorded
/// as `exec_time_ms`, `track` the DB-write duration. Drains the child
/// accumulators so each report covers one command.
pub fn report(handler_started: Instant, handler: Duration, track: Duration) {
    if !enabled() {
        return;
    }
    let child = Duration::from_micros(CHILD_MICROS.swap(0, Ordering::Relaxed));
    let spawns = CHILD_SPAWNS.swap(0, Ordering::Relaxed);
    // Not eprintln!: a write failure (e.g. ENOSPC on stderr) must not turn
    // into a panic that replaces the wrapped command's exit code.
    let _ = writeln!(
        std::io::stderr().lock(),
        "{}",
        format_report(
            PROCESS_START.elapsed(),
            handler_started.duration_since(*PROCESS_START),
            handler,
            child,
            spawns,
            track,
        )
    );
}

fn format_report(
    total: Duration,
    startup: Duration,
    handler: Duration,
    child: Duration,
    spawns: u64,
    track: Duration,
) -> String {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    format!(
        "rtk timings: total={:.1}ms startup={:.1}ms handler={:.1}ms child={:.1}ms spawns={} filter={:.1}ms track={:.1}ms",
        ms(total),
        ms(startup),
        ms(handler),
        ms(child),
        spawns,
        ms(handler.saturating_sub(child)),
        ms(track),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_report_breakdown_line() {
        let line = format_report(
            Duration::from_micros(41_200),
            Duration::from_micros(2_900),
            Duration::from_micros(37_800),
            Duration::from_micros(30_100),
            2,
            Duration::from_micros(500),
        );
        assert_eq!(
            line,
            "rtk timings: total=41.2ms startup=2.9ms handler=37.8ms child=30.1ms spawns=2 filter=7.7ms track=0.5ms"
        );
    }

    #[test]
    fn test_format_report_filter_saturates_when_child_exceeds_handler() {
        // Streaming passthrough can wait on a child slightly past the handler
        // span boundary; filter must clamp to 0, never underflow.
        let line = format_report(
            Duration::from_millis(10),
            Duration::from_millis(1),
            Duration::from_millis(5),
            Duration::from_millis(6),
            1,
            Duration::ZERO,
        );
        assert!(line.contains("filter=0.0ms"), "got: {line}");
    }

    #[test]
    fn test_time_child_returns_closure_result() {
        // Assumes RTK_TIMINGS is unset in the test environment (like
        // hook_cmd's env-var tests); with it set, parallel stream.rs tests
        // share the global accumulators, so only the return value is checked.
        assert_eq!(time_child(|| 7), 7);
    }
}
