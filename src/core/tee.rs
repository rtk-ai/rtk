//! Recovery-hint dispatch — routes to the sqlite store or legacy tee per `[retriever] mode`.

// These modules opt into clippy's complexity lints; the rest of the crate
// predates them and is not held to the same ceilings. The thresholds come
// from whatever clippy.toml is in scope and resolve at clippy's defaults when
// there is none, so this costs a tree without one nothing.
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

/// What a recovery hint is filed under.
///
/// Two different things used to share one `&str` parameter: the name of a
/// *kind* of command, which is what `recall_stats` counts, and the name of one
/// *invocation*, which is what a tee filename needs to stay unique. Passing a
/// runtime string satisfied both, so per-invocation data reached the stats
/// table — `cargo_{subcommand}`, `bun_{subcmd}`, `deno_{subcmd}` and a grep
/// slug carrying 32 characters of file path, each opening a row that nothing
/// closed. The table has a cap now, but a cap is a limit on the damage, not a
/// bound on what enters.
///
/// The variants are the admissible ways to name something, and each carries
/// its own evidence that the set of stats keys it can produce is finite:
///
/// - `Static` — one literal. The finite set is the literals in this tree.
/// - `Composed` — a family plus parts that are themselves `&'static str`, so
///   the joined name still comes from the literals in this tree. This is what
///   keeps `cargo_clippy` and `aws_ec2_describe-instances` distinct in the
///   stats rather than folding them to their family.
/// - `Detailed` — a runtime value that is *not* bounded, and so never reaches
///   the stats key at all. It reaches the tee filename, which needs it.
/// - `Configured` — a name from the user's own config. Bounded by how many
///   filters they have defined, which is the one case where the bound lives
///   outside this tree.
///
/// There is deliberately no conversion from `String` or `&str`: a caller
/// holding a runtime string has to say which of the last two it means.
#[derive(Clone, Copy)]
pub enum Slug<'a> {
    Static(&'static str),
    Composed {
        family: &'static str,
        parts: &'a [&'static str],
    },
    Detailed {
        family: &'static str,
        detail: &'a str,
    },
    Configured(&'a str),
}

impl From<&'static str> for Slug<'static> {
    fn from(name: &'static str) -> Self {
        Slug::Static(name)
    }
}

/// `family` and its parts as one underscore-joined name, skipping empty parts
/// so a caller need not special-case a subcommand it does not have.
fn compose(family: &str, parts: &[&str]) -> String {
    std::iter::once(family)
        .chain(parts.iter().copied())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

impl Slug<'_> {
    /// The key `recall_stats` counts under. Every variant answers from a
    /// finite set; this is the whole of the bound.
    pub(crate) fn stats_key(&self) -> String {
        match self {
            Slug::Static(name) | Slug::Configured(name) => (*name).to_string(),
            Slug::Composed { family, parts } => compose(family, parts),
            Slug::Detailed { family, .. } => (*family).to_string(),
        }
    }

    /// The unbounded half, when there is one. Only `Detailed` has it, and it is
    /// the half a tee filename needs and the stats table must not see.
    pub(crate) fn detail(&self) -> Option<&str> {
        match self {
            Slug::Detailed { detail, .. } => Some(detail),
            _ => None,
        }
    }

    /// The name a tee file is written under, and the `command` column of a
    /// recall row. Unlike the stats key this keeps the detail, because two
    /// files written in the same second need to differ and a reader of
    /// `recall --list` wants to know which grep it was.
    pub(crate) fn full(&self) -> String {
        match self {
            Slug::Static(name) | Slug::Configured(name) => (*name).to_string(),
            Slug::Composed { family, parts } => compose(family, parts),
            Slug::Detailed { family, detail } => format!("{family}_{detail}"),
        }
    }
}

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
    slug: &Slug<'_>,
    exit_code: Option<i32>,
) -> Option<String> {
    let (command, key) = (slug.full(), slug.stats_key());
    let capture = Capture::full(&command, exit_code).keyed(&key);
    match retriever::store(cfg, content.as_bytes(), capture) {
        Stored::Saved(s) => Some(format!("[full output: rtk recall {}]", s.hash)),
        Stored::Unavailable | Stored::Empty => None,
    }
}

pub fn tee_and_hint<'a>(raw: &str, slug: impl Into<Slug<'a>>, exit_code: i32) -> Option<String> {
    if exit_code == 0 || raw.len() < MIN_FAILURE_BYTES {
        return None;
    }
    let (mode, cfg) = active()?;
    let slug = slug.into();
    match mode {
        RecoveryMode::Disabled => None,
        RecoveryMode::Tee => super::tee_file::tee_and_hint(&cfg, raw, &slug)
            .inspect(|_| retriever::record_tee_elision(&cfg, &slug.stats_key())),
        RecoveryMode::Sqlite => store_hint(&cfg, raw, &slug, Some(exit_code)),
    }
}

pub fn force_tee_hint<'a>(content: &str, slug: impl Into<Slug<'a>>) -> Option<String> {
    if content.is_empty() {
        return None;
    }
    let (mode, cfg) = active()?;
    let slug = slug.into();
    match mode {
        RecoveryMode::Disabled => None,
        RecoveryMode::Tee => super::tee_file::force_tee_hint(&cfg, content, &slug)
            .inspect(|_| retriever::record_tee_elision(&cfg, &slug.stats_key())),
        RecoveryMode::Sqlite => store_hint(&cfg, content, &slug, None),
    }
}

pub fn force_tee_tail_hint<'a>(
    content: &str,
    slug: impl Into<Slug<'a>>,
    line_offset: usize,
) -> Option<String> {
    if content.is_empty() {
        return None;
    }
    let (mode, cfg) = active()?;
    let slug = slug.into();
    match mode {
        RecoveryMode::Disabled => None,
        RecoveryMode::Tee => {
            super::tee_file::force_tee_tail_hint(&cfg, content, &slug, line_offset)
                .inspect(|_| retriever::record_tee_elision(&cfg, &slug.stats_key()))
        }
        RecoveryMode::Sqlite => tail_hint(&cfg, content, &slug, line_offset),
    }
}

/// The `[+N hidden: …]` counterpart to [`store_hint`]: same store call, but the
/// hint names how much was withheld rather than offering the whole entry.
fn tail_hint(
    cfg: &RetrieverConfig,
    content: &str,
    slug: &Slug<'_>,
    line_offset: usize,
) -> Option<String> {
    let (command, key) = (slug.full(), slug.stats_key());
    let capture = Capture::tail(&command, line_offset).keyed(&key);
    match retriever::store(cfg, content.as_bytes(), capture) {
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

    /// The whole point of the type: a per-invocation value reaches the tee
    /// filename and the `command` column, and never the stats key.
    #[test]
    fn test_detailed_keeps_detail_out_of_the_stats_key() {
        let slug = Slug::Detailed {
            family: "grep",
            detail: "3_src_core_retriever_rs",
        };
        assert_eq!(slug.stats_key(), "grep");
        assert_eq!(slug.full(), "grep_3_src_core_retriever_rs");
    }

    /// Composed parts are `&'static str`, so the joined name is still drawn
    /// from the literals in this tree — which is why it may reach the stats
    /// key without bounding it to the family.
    #[test]
    fn test_composed_keeps_its_parts_in_both_names() {
        let slug = Slug::Composed {
            family: "aws",
            parts: &["ec2", "describe-instances"],
        };
        assert_eq!(slug.stats_key(), "aws_ec2_describe-instances");
        assert_eq!(slug.full(), "aws_ec2_describe-instances");
    }

    /// A subcommand-less caller composes with an empty part rather than
    /// special-casing, and must not get a trailing underscore for it.
    #[test]
    fn test_composed_skips_empty_parts() {
        let slug = Slug::Composed {
            family: "cargo",
            parts: &[""],
        };
        assert_eq!(slug.stats_key(), "cargo");
    }

    /// A name from the user's own config is bounded by how many filters they
    /// have, not by this tree, and is carried as itself.
    #[test]
    fn test_configured_is_carried_verbatim() {
        let name = String::from("my-custom-filter");
        let slug = Slug::Configured(&name);
        assert_eq!(slug.stats_key(), "my-custom-filter");
        assert_eq!(slug.full(), "my-custom-filter");
    }

    /// The recall side counts the family too, not the path the filename
    /// carries. Both halves of the count are bounded or neither is.
    #[test]
    fn test_tee_filename_hides_its_detail_from_the_recall_side() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tee_dir = dir.path().join("tee");
        let cfg = RetrieverConfig {
            mode: RecoveryMode::Tee,
            tee_directory: Some(tee_dir.clone()),
            ..RetrieverConfig::default()
        };
        let slug = Slug::Detailed {
            family: "grep",
            detail: "7_src_core_retriever_rs",
        };

        let path = super::super::tee_file::force_tee_hint(&cfg, "matches\n", &slug)
            .expect("tee file written");

        // The filename keeps the detail — two overflows in one second must not
        // collide — but marks where the countable half ends.
        assert!(path.contains("grep__7_src_core_retriever_rs"), "{path}");

        let stem = std::path::Path::new(
            path.trim_start_matches("[full output: ")
                .trim_end_matches("]"),
        )
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .expect("stem");
        let after_epoch = stem.split_once('_').expect("epoch").1;
        assert_eq!(
            crate::hooks::rewrite_cmd::bounded_half(after_epoch),
            "grep",
            "the recall side must count the family, not the path"
        );
    }

    /// The store files the counts under the bounded name while the row keeps
    /// the detailed one. Asserted through `store` rather than on the type, so
    /// it covers the wiring and not just the rendering.
    #[test]
    fn test_store_counts_the_family_and_records_the_detail() {
        use crate::core::retriever::{stats_snapshot_with, RecoveryMode, RetrieverConfig};

        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = RetrieverConfig {
            mode: RecoveryMode::Sqlite,
            database_path: Some(dir.path().join("recall.db")),
            ..RetrieverConfig::default()
        };

        for path in ["src_one_rs", "src_two_rs", "src_three_rs"] {
            let slug = Slug::Detailed {
                family: "grep",
                detail: path,
            };
            let content = format!("matches in {path}\n");
            assert!(store_hint(&cfg, &content, &slug, None).is_some());
        }

        let stats = stats_snapshot_with(&cfg).expect("stats");
        let grep: Vec<_> = stats.iter().filter(|s| s.slug == "grep").collect();
        assert_eq!(grep.len(), 1, "three greps must share one stats row");
        assert_eq!(grep[0].elisions, 3);
        assert!(
            !stats.iter().any(|s| s.slug.contains("src_one_rs")),
            "no path may appear in a stats key: {:?}",
            stats.iter().map(|s| &s.slug).collect::<Vec<_>>()
        );
    }
}
