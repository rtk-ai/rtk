//! Recovery-hint dispatch — routes to the sqlite store or legacy tee per `[retriever] mode`.

// Complexity ratchet — see clippy.toml. Ceilings may only fall.
#![deny(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::cognitive_complexity,
    clippy::excessive_nesting,
    clippy::fn_params_excessive_bools,
    clippy::struct_excessive_bools,
    clippy::type_complexity
)]

pub(crate) use crate::core::retriever::MIN_FAILURE_BYTES as MIN_TEE_SIZE;
use crate::core::retriever::{
    self, Capture, RecoveryMode, RetrieverConfig, Stored, MIN_FAILURE_BYTES,
};

fn active() -> Option<(RecoveryMode, RetrieverConfig)> {
    if matches!(std::env::var("RTK_RECALL").ok().as_deref(), Some("0"))
        || matches!(std::env::var("RTK_TEE").ok().as_deref(), Some("0"))
    {
        return None;
    }
    let cfg = recall_cfg();
    match cfg.mode {
        RecoveryMode::Disabled => None,
        mode => Some((mode, cfg)),
    }
}

/// Cached, not a fresh load: the hint paths in search.rs call this once per
/// file, and a disk read plus TOML parse per file is the other half of the
/// per-file overhead that breaches the <10ms startup target. This is a
/// read-only caller that never writes config, which is what cached_config
/// requires.
#[cfg(not(test))]
fn recall_cfg() -> RetrieverConfig {
    crate::core::config::cached_config().retriever.clone()
}

/// Under test the ambient user config is never consulted. Filter unit tests
/// across 20+ modules call `force_tee_*` with whatever config the developer
/// happens to have, which wrote their fixture output into the real
/// `recall.db` and leaked fixture slugs into the daily telemetry ping via
/// `stats_snapshot()`. Recall is therefore off by default in tests;
/// a test that needs the real path installs its own tempdir config with
/// [`with_test_recall`].
#[cfg(test)]
fn recall_cfg() -> RetrieverConfig {
    TEST_RECALL_CFG
        .with(|c| c.borrow().clone())
        .unwrap_or_else(|| RetrieverConfig {
            mode: RecoveryMode::Disabled,
            ..RetrieverConfig::default()
        })
}

#[cfg(test)]
thread_local! {
    static TEST_RECALL_CFG: std::cell::RefCell<Option<RetrieverConfig>> =
        const { std::cell::RefCell::new(None) };
}

/// Serializes every test that installs a recall config against the one test
/// that sets `RTK_RECALL`, which is process-wide and would otherwise switch
/// recall off underneath a concurrently running test .
#[cfg(test)]
pub(crate) static RECALL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Point recall at `cfg` for the duration of `f`. The config itself is
/// thread-local; the lock guards against the process-wide env kill switch.
#[cfg(test)]
pub(crate) fn with_test_recall<T>(cfg: RetrieverConfig, f: impl FnOnce() -> T) -> T {
    let _guard = RECALL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    TEST_RECALL_CFG.with(|c| *c.borrow_mut() = Some(cfg));
    let out = f();
    TEST_RECALL_CFG.with(|c| *c.borrow_mut() = None);
    out
}

/// Run `f` with recall backed by a throwaway store. For filter tests that
/// assert on a truncation/recovery hint: without this the hint paths are inert
/// under test and the filter correctly falls back to passthrough .
#[cfg(test)]
pub(crate) fn with_temp_recall<T>(f: impl FnOnce() -> T) -> T {
    let dir = tempfile::tempdir().expect("tempdir");
    with_test_recall(
        RetrieverConfig {
            mode: RecoveryMode::Sqlite,
            database_path: Some(dir.path().join("recall_test.db")),
            ..RetrieverConfig::default()
        },
        f,
    )
}

fn store_hint(
    cfg: &RetrieverConfig,
    content: &str,
    slug: &str,
    exit_code: Option<i32>,
) -> Option<String> {
    match retriever::store(cfg, content.as_bytes(), Capture::full(slug, exit_code)) {
        Stored::Saved(s) => Some(format!("[full output: rtk recall {}]", s.hash)),
        Stored::Unavailable | Stored::Empty => None,
    }
}

pub fn tee_and_hint(raw: &str, command_slug: &str, exit_code: i32) -> Option<String> {
    if exit_code == 0 || raw.len() < MIN_FAILURE_BYTES {
        return None;
    }
    let (mode, cfg) = active()?;
    match mode {
        RecoveryMode::Disabled => None,
        RecoveryMode::Tee => super::tee_file::tee_and_hint(&cfg, raw, command_slug)
            .inspect(|_| retriever::record_tee_elision(&cfg, command_slug)),
        RecoveryMode::Sqlite => store_hint(&cfg, raw, command_slug, Some(exit_code)),
    }
}

pub fn force_tee_hint(content: &str, command_slug: &str) -> Option<String> {
    if content.is_empty() {
        return None;
    }
    let (mode, cfg) = active()?;
    match mode {
        RecoveryMode::Disabled => None,
        RecoveryMode::Tee => super::tee_file::force_tee_hint(&cfg, content, command_slug)
            .inspect(|_| retriever::record_tee_elision(&cfg, command_slug)),
        RecoveryMode::Sqlite => store_hint(&cfg, content, command_slug, None),
    }
}

pub fn force_tee_tail_hint(
    content: &str,
    command_slug: &str,
    line_offset: usize,
) -> Option<String> {
    if content.is_empty() {
        return None;
    }
    let (mode, cfg) = active()?;
    match mode {
        RecoveryMode::Disabled => None,
        RecoveryMode::Tee => {
            super::tee_file::force_tee_tail_hint(&cfg, content, command_slug, line_offset)
                .inspect(|_| retriever::record_tee_elision(&cfg, command_slug))
        }
        RecoveryMode::Sqlite => tail_hint(&cfg, content, command_slug, line_offset),
    }
}

/// The `[+N hidden: …]` counterpart to [`store_hint`]: same store call, but the
/// hint names how much was withheld rather than offering the whole entry.
fn tail_hint(
    cfg: &RetrieverConfig,
    content: &str,
    slug: &str,
    line_offset: usize,
) -> Option<String> {
    match retriever::store(cfg, content.as_bytes(), Capture::tail(slug, line_offset)) {
        Stored::Saved(s) => Some(format!(
            "[+{} hidden: rtk recall {}]",
            s.hidden_lines, s.hash
        )),
        Stored::Unavailable | Stored::Empty => None,
    }
}

#[cfg(test)]
// Test bodies are linear setup-act-assert scripts; splitting them to satisfy
// the ratchet makes them harder to read. See clippy.toml.
#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::cognitive_complexity,
    clippy::excessive_nesting
)]
mod tests {
    use super::*;

    fn temp_recall_cfg(dir: &std::path::Path) -> RetrieverConfig {
        RetrieverConfig {
            mode: RecoveryMode::Sqlite,
            database_path: Some(dir.join("recall_test.db")),
            ..RetrieverConfig::default()
        }
    }

    /// With no test config installed — the state every filter unit
    /// test runs in — the hint paths must stay inert. This is what stops
    /// fixture output reaching the developer's real recall.db.
    #[test]
    fn test_recall_inert_in_tests_by_default() {
        let big = "x".repeat(1000);
        assert!(tee_and_hint(&big, "cmd", 1).is_none());
        assert!(force_tee_hint(&big, "cmd").is_none());
        assert!(force_tee_tail_hint(&big, "cmd", 5).is_none());
    }

    /// The default must be inertness, not a broken store: with a config
    /// installed the same calls do produce hints.
    #[test]
    fn test_with_test_recall_enables_hints() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("recall_test.db");
        let big = "x".repeat(1000);
        let hint = with_test_recall(temp_recall_cfg(dir.path()), || force_tee_hint(&big, "cmd"));
        assert!(hint.is_some_and(|h| h.contains("rtk recall")));
        assert!(db.exists(), "writes go to the tempdir, not the real store");
    }

    /// The override is scoped: recall is inert again once `f` returns.
    #[test]
    fn test_with_test_recall_restores_inertness() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(1000);
        with_test_recall(temp_recall_cfg(dir.path()), || {
            assert!(force_tee_hint(&big, "cmd").is_some());
        });
        assert!(force_tee_hint(&big, "cmd").is_none());
    }

    /// The env kill switch still wins over an installed config.
    #[test]
    fn test_disabled_env_emits_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(1000);
        with_test_recall(temp_recall_cfg(dir.path()), || {
            let _guard = EnvKill::set();
            assert!(tee_and_hint(&big, "cmd", 1).is_none());
            assert!(force_tee_hint(&big, "cmd").is_none());
            assert!(force_tee_tail_hint(&big, "cmd", 5).is_none());
        });
    }

    /// Sets `RTK_RECALL=0` and restores it on drop. Only ever constructed
    /// inside a `with_test_recall` closure, which already holds
    /// [`RECALL_TEST_LOCK`], so no other config-installing test observes it.
    struct EnvKill;

    impl EnvKill {
        fn set() -> Self {
            std::env::set_var("RTK_RECALL", "0");
            EnvKill
        }
    }

    impl Drop for EnvKill {
        fn drop(&mut self) {
            std::env::remove_var("RTK_RECALL");
        }
    }

    #[test]
    fn test_tee_and_hint_skips_success() {
        let big = "x".repeat(1000);
        assert!(tee_and_hint(&big, "cmd", 0).is_none());
    }

    #[test]
    fn test_tee_and_hint_skips_tiny_failure() {
        assert!(tee_and_hint("tiny", "cmd", 1).is_none());
    }

    #[test]
    fn test_force_tee_hint_skips_empty() {
        assert!(force_tee_hint("", "cmd").is_none());
    }

    #[test]
    fn test_force_tee_tail_hint_skips_empty() {
        assert!(force_tee_tail_hint("", "cmd", 5).is_none());
    }
}
