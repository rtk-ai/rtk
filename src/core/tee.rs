//! Recovery-hint dispatch — routes to the sqlite store or legacy tee per `[retriever] mode`.

use crate::core::config::Config;
pub(crate) use crate::core::retriever::MIN_FAILURE_BYTES as MIN_TEE_SIZE;
use crate::core::retriever::{self, MIN_FAILURE_BYTES, RecoveryMode, RetrieverConfig, Stored};

// Keep the notice outside the brackets so recovery commands and paths stay intact.
pub(super) const RECOVERY_OUTPUT_NOTICE: &str = " (unfiltered; may contain sensitive data)";

fn active() -> Option<(RecoveryMode, RetrieverConfig)> {
    if retriever::recovery_disabled_by_env() {
        return None;
    }
    match Config::load().ok().map(|c| (c.retriever.mode, c.retriever)) {
        Some((RecoveryMode::Disabled, _)) | None => None,
        some => some,
    }
}

fn store_hint(
    cfg: &RetrieverConfig,
    content: &str,
    slug: &str,
    exit_code: Option<i32>,
) -> Option<String> {
    match retriever::store(cfg, content.as_bytes(), slug, exit_code, 1) {
        Stored::Saved(s) => Some(format!(
            "[full output: rtk recall {}]{RECOVERY_OUTPUT_NOTICE}",
            s.reference
        )),
        Stored::Unavailable | Stored::Empty => None,
    }
}

/// Takes the resolved mode and config so tests never reach `Config::load()`,
/// mirroring `hooks::rewrite_cmd::evaluate_with_verdict` (#3146).
fn tee_and_hint_with(
    mode: RecoveryMode,
    cfg: &RetrieverConfig,
    raw: &str,
    command_slug: &str,
    exit_code: i32,
) -> Option<String> {
    match mode {
        RecoveryMode::Disabled => None,
        // Legacy tee semantics, unchanged: `failures` writes on failure only,
        // `always` also archives successful runs whose output the filter shrank.
        RecoveryMode::Tee => {
            if exit_code == 0 && !cfg.tee_on_success {
                return None;
            }
            super::tee_file::tee_and_hint(cfg, raw, command_slug).inspect(|_| {
                if exit_code != 0 {
                    retriever::record_tee_elision(cfg, command_slug);
                }
            })
        }
        RecoveryMode::Sqlite => {
            if exit_code == 0 {
                return None;
            }
            store_hint(cfg, raw, command_slug, Some(exit_code))
        }
    }
}

pub fn tee_and_hint(raw: &str, command_slug: &str, exit_code: i32) -> Option<String> {
    if raw.len() < MIN_FAILURE_BYTES {
        return None;
    }
    let (mode, cfg) = active()?;
    tee_and_hint_with(mode, &cfg, raw, command_slug, exit_code)
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

/// A tail to recover independently, even when several tails share one write.
pub struct TailHint<'a> {
    pub content: &'a str,
    pub command_slug: &'a str,
    pub line_offset: usize,
}

pub fn force_tee_tail_hint(
    content: &str,
    command_slug: &str,
    line_offset: usize,
) -> Option<String> {
    force_tee_tail_hints(&[TailHint {
        content,
        command_slug,
        line_offset,
    }])
    .pop()
    .flatten()
}

/// Resolve configuration once, batch SQLite writes, then format only committed
/// references. Tee mode retains its existing per-file storage semantics.
pub fn force_tee_tail_hints(tails: &[TailHint<'_>]) -> Vec<Option<String>> {
    if tails.iter().all(|tail| tail.content.is_empty()) {
        return tails.iter().map(|_| None).collect();
    }
    let Some((mode, cfg)) = active() else {
        return tails.iter().map(|_| None).collect();
    };
    force_tee_tail_hints_with(mode, &cfg, tails)
}

fn force_tee_tail_hints_with(
    mode: RecoveryMode,
    cfg: &RetrieverConfig,
    tails: &[TailHint<'_>],
) -> Vec<Option<String>> {
    match mode {
        RecoveryMode::Disabled => tails.iter().map(|_| None).collect(),
        RecoveryMode::Tee => tails
            .iter()
            .map(|tail| {
                if tail.content.is_empty() {
                    return None;
                }
                super::tee_file::force_tee_tail_hint(
                    cfg,
                    tail.content,
                    tail.command_slug,
                    tail.line_offset,
                )
                .inspect(|_| retriever::record_tee_elision(cfg, tail.command_slug))
            })
            .collect(),
        RecoveryMode::Sqlite => {
            let inputs: Vec<_> = tails
                .iter()
                .map(|tail| retriever::StoreInput {
                    content: tail.content.as_bytes(),
                    command: tail.command_slug,
                    exit_code: None,
                    shown_upto: tail.line_offset,
                })
                .collect();
            retriever::store_batch(cfg, &inputs)
                .into_iter()
                .map(|stored| match stored {
                    Stored::Saved(s) if s.hidden_lines > 0 => Some(format!(
                        "[+{} hidden: rtk recall {}]{RECOVERY_OUTPUT_NOTICE}",
                        s.hidden_lines, s.reference
                    )),
                    Stored::Saved(_) | Stored::Unavailable | Stored::Empty => None,
                })
                .collect()
        }
    }
}

/// Posthook recall store subdirectory (own rotation budget: posthook writes
/// must never evict Bash-failure recovery logs).
pub const POSTHOOK_SUBDIR: &str = "posthook";
#[cfg(feature = "engine")]
/// Rotation budget for the posthook recall store.
const POSTHOOK_MAX_FILES: usize = 20;

#[cfg(feature = "engine")]
/// Write raw tool output to `<tee dir>/posthook/` before filtering and return
/// the recall hint. Millisecond + pid filenames: epoch-second naming silently
/// overwrites same-second writes (guaranteed at posthook rates), and the pid
/// disambiguates concurrent hook processes (parallel subagents).
///
/// Takes `config` from the caller — the posthook path loads Config exactly
/// once per invocation. Honors the recovery kill switches like every other
/// tee write; per-file cap + truncation marker reused unchanged.
pub fn posthook_tee_hint(config: &Config, raw: &str, command_slug: &str) -> Option<String> {
    if raw.is_empty() || retriever::recovery_disabled_by_env() {
        return None;
    }
    match config.retriever.mode {
        RecoveryMode::Disabled => return None,
        RecoveryMode::Sqlite => return posthook_store_hint(&config.retriever, raw, command_slug),
        RecoveryMode::Tee => {}
    }

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let dir = super::tee_file::get_tee_dir(&config.retriever)?.join(POSTHOOK_SUBDIR);
    let slug = super::tee_file::sanitize_slug(command_slug);
    let epoch_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let filename = format!("{}_{}-{}_{}.log", epoch_ms, std::process::id(), seq, slug);
    let path = super::tee_file::write_named_tee_file(
        raw,
        &filename,
        &dir,
        config.retriever.tee_max_file_size,
        POSTHOOK_MAX_FILES,
    )?;
    Some(super::tee_file::format_hint(&path))
}

#[cfg(feature = "engine")]
pub const POSTHOOK_RECALL_PREFIX: &str = "posthook:";

#[cfg(feature = "engine")]
fn posthook_store_hint(cfg: &RetrieverConfig, raw: &str, command_slug: &str) -> Option<String> {
    let command = format!("{POSTHOOK_RECALL_PREFIX}{command_slug}");
    let hint = store_hint(cfg, raw, &command, None)?;
    retriever::trim_family(cfg, POSTHOOK_RECALL_PREFIX, POSTHOOK_MAX_FILES);
    Some(hint)
}

#[cfg(feature = "engine")]
/// Resolved posthook recall directory (for the recall-recovery gate: a
/// command reading these files must not be post-filtered again).
pub fn posthook_recall_dir(config: &Config) -> Option<std::path::PathBuf> {
    super::tee_file::get_tee_dir(&config.retriever).map(|d| d.join(POSTHOOK_SUBDIR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::user_env;

    #[cfg(feature = "engine")]
    #[test]
    fn test_posthook_named_writes_rotate_at_budget() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join(POSTHOOK_SUBDIR);
        let raw = "x".repeat(600);
        for i in 0..25 {
            let filename = format!("{i}_9999-{i}_read_data_json.log");
            let path = crate::core::tee_file::write_named_tee_file(
                &raw,
                &filename,
                &dir,
                1_048_576,
                POSTHOOK_MAX_FILES,
            )
            .expect("write succeeds");
            assert!(path.exists());
        }
        let count = std::fs::read_dir(&dir)
            .expect("posthook dir exists")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
            .count();
        assert_eq!(count, 20, "rotation budget is 20 files");
    }

    #[cfg(feature = "engine")]
    #[test]
    fn test_posthook_tee_hint_shape() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut config = Config::default();
        config.retriever.tee_directory = Some(tmp.path().to_path_buf());
        config.retriever.mode = RecoveryMode::Tee;
        let hint = posthook_tee_hint(&config, &"x".repeat(600), "read_data_json")
            .expect("tee mode writes a file");
        assert!(hint.starts_with("[full output: "));
        assert!(hint.ends_with(RECOVERY_OUTPUT_NOTICE));
        assert!(tmp.path().join(POSTHOOK_SUBDIR).exists());
    }

    #[cfg(feature = "engine")]
    #[test]
    fn test_posthook_sqlite_mode_points_at_recall_and_writes_no_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut config = Config::default();
        config.retriever.mode = RecoveryMode::Sqlite;
        config.retriever.tee_directory = Some(tmp.path().join("tee"));
        config.retriever.database_path = Some(tmp.path().join("recall.db"));
        let hint = posthook_tee_hint(&config, &"x".repeat(600), "bash_cargo")
            .expect("sqlite mode stores the output");
        assert!(hint.starts_with("[full output: rtk recall c_"), "{hint}");
        assert!(hint.ends_with(RECOVERY_OUTPUT_NOTICE));
        assert!(
            !tmp.path().join("tee").exists(),
            "sqlite mode must not write tee files"
        );
    }

    #[cfg(feature = "engine")]
    #[test]
    fn test_posthook_sqlite_entries_keep_their_own_budget() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut config = Config::default();
        config.retriever.mode = RecoveryMode::Sqlite;
        config.retriever.database_path = Some(tmp.path().join("recall.db"));
        let failure = retriever::store(
            &config.retriever,
            b"failure log body",
            "cargo test",
            Some(1),
            1,
        );
        assert!(matches!(failure, Stored::Saved(_)));
        for i in 0..30 {
            posthook_tee_hint(&config, &format!("{i}{}", "x".repeat(600)), "bash_ls")
                .expect("stored");
        }
        let conn = rusqlite::Connection::open(tmp.path().join("recall.db")).expect("db");
        let posthook: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recall WHERE command LIKE 'posthook:%'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        let others: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recall WHERE command NOT LIKE 'posthook:%'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(posthook, POSTHOOK_MAX_FILES as i64);
        assert_eq!(
            others, 1,
            "posthook writes must never evict a failure recovery"
        );
    }

    #[cfg(feature = "engine")]
    #[test]
    fn test_posthook_tee_disabled_mode() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut config = Config::default();
        config.retriever.tee_directory = Some(tmp.path().to_path_buf());
        config.retriever.mode = RecoveryMode::Disabled;

        assert!(posthook_tee_hint(&config, &"x".repeat(600), "slug").is_none());
        assert!(!tmp.path().join(POSTHOOK_SUBDIR).exists());
    }

    #[test]
    fn test_recovery_hints_disclose_unfiltered_sensitive_output() {
        for mode in [RecoveryMode::Sqlite, RecoveryMode::Tee] {
            let dir = tempfile::tempdir().expect("temporary recovery directory");
            crate::core::test_isolation::with_root(dir.path(), || {
                let mut cfg = Config::default();
                cfg.retriever = RetrieverConfig {
                    mode,
                    tee_directory: Some(dir.path().join("tee with spaces")),
                    ..RetrieverConfig::default()
                };
                cfg.save().expect("save isolated recovery config");
                let raw = "unfiltered captured output\n".repeat(30);
                let hints = [
                    tee_and_hint(&raw, "failure", 1).expect("failure hint"),
                    force_tee_hint(&raw, "forced").expect("forced hint"),
                    force_tee_tail_hint(&raw, "tail", 21).expect("tail hint"),
                ];
                for (index, hint) in hints.iter().enumerate() {
                    let (recovery, notice) = hint.split_once(']').expect("bracketed recovery");
                    assert_eq!(notice, " (unfiltered; may contain sensitive data)");
                    assert_eq!(hint.lines().count(), 1, "keep prefix-based hint filtering");
                    if mode == RecoveryMode::Sqlite {
                        let prefix = if index == 2 {
                            "[+10 hidden: rtk recall "
                        } else {
                            "[full output: rtk recall "
                        };
                        let hash = recovery
                            .strip_prefix(prefix)
                            .expect("unchanged hint prefix");
                        assert!(
                            hash.strip_prefix("c_")
                                .is_some_and(crate::core::invocation::valid_id)
                        );
                        assert!(
                            !hash.is_empty(),
                            "keep the recall command directly runnable"
                        );
                    } else {
                        let prefix = if index == 2 {
                            "[see remaining: tail -n +21 "
                        } else {
                            "[full output: "
                        };
                        let path = recovery
                            .strip_prefix(prefix)
                            .expect("unchanged hint prefix");
                        assert!(path.starts_with('"') && path.ends_with('"'));
                        assert!(path.contains("tee with spaces"), "keep paths shell-quoted");
                    }
                }
            });
        }
    }

    #[test]
    fn test_unavailable_recovery_does_not_emit_disclosure() {
        for mode in [RecoveryMode::Sqlite, RecoveryMode::Tee] {
            let dir = tempfile::tempdir().expect("temporary recovery directory");
            crate::core::test_isolation::with_root(dir.path(), || {
                let blocked = dir.path().join("regular-file");
                std::fs::write(&blocked, "not a directory").expect("block storage paths");
                let mut cfg = Config::default();
                cfg.retriever = RetrieverConfig {
                    mode,
                    database_path: Some(blocked.join("recall.db")),
                    tee_directory: Some(blocked.join("tee")),
                    ..RetrieverConfig::default()
                };
                cfg.save().expect("save isolated recovery config");
                let raw = "unfiltered captured output\n".repeat(30);
                assert!(tee_and_hint(&raw, "failure", 1).is_none());
                assert!(force_tee_hint(&raw, "forced").is_none());
                assert!(force_tee_tail_hint(&raw, "tail", 21).is_none());
                assert_eq!(
                    crate::core::runner::emit_guarded(&raw, None, &raw),
                    raw,
                    "unavailable storage must leave the raw fallback unchanged"
                );
            });
        }
    }

    #[test]
    fn test_disclosure_counts_toward_never_worse_budget() {
        let dir = tempfile::tempdir().expect("temporary recovery directory");
        let cfg = RetrieverConfig {
            database_path: Some(dir.path().join("recall.db")),
            ..RetrieverConfig::default()
        };
        let raw = "x".repeat(90);
        let hint = store_hint(&cfg, &raw, "budget", None).expect("recovery hint");
        let original_hint = hint.split_once(']').expect("bracketed hint").0.to_owned() + "]";
        let before = crate::core::runner::emit_guarded("short", Some(&original_hint), &raw);
        assert!(
            before.contains("[full output:"),
            "fixture fits before the notice"
        );
        let shown = crate::core::runner::emit_guarded("short", Some(&hint), &raw);
        assert_eq!(
            shown,
            format!("{raw}\n"),
            "notice must not bypass the guard"
        );
    }

    #[test]
    fn test_disabled_env_emits_nothing() {
        let big = "x".repeat(1000);
        let (hint, forced, tail) = user_env::with_vars(&[("RTK_RECALL", Some("0"))], || {
            (
                tee_and_hint(&big, "cmd", 1),
                force_tee_hint(&big, "cmd"),
                force_tee_tail_hint(&big, "cmd", 5),
            )
        });
        assert!(hint.is_none(), "disabled must never emit tokens");
        assert!(forced.is_none());
        assert!(tail.is_none());
    }

    #[test]
    fn test_sqlite_never_stores_a_successful_run() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            database_path: Some(dir.path().join("s.db")),
            tee_on_success: true,
            ..RetrieverConfig::default()
        };
        let big = "x".repeat(1000);
        assert!(
            tee_and_hint_with(RecoveryMode::Sqlite, &cfg, &big, "cmd", 0).is_none(),
            "the sqlite store only records failures and truncations"
        );
        assert!(!dir.path().join("s.db").exists());
    }

    #[test]
    fn test_tee_mode_honors_legacy_always() {
        let dir = tempfile::tempdir().unwrap();
        let base = RetrieverConfig {
            tee_directory: Some(dir.path().to_path_buf()),
            database_path: Some(dir.path().join("s.db")),
            ..RetrieverConfig::default()
        };
        let big = "x".repeat(1000);
        assert!(
            tee_and_hint_with(RecoveryMode::Tee, &base, &big, "cmd", 0).is_none(),
            "failures-only is the legacy default"
        );
        let always = RetrieverConfig {
            tee_on_success: true,
            ..base
        };
        assert!(
            tee_and_hint_with(RecoveryMode::Tee, &always, &big, "cmd", 0).is_some(),
            "legacy always still archives successful runs"
        );
    }

    #[test]
    fn test_tee_and_hint_skips_success() {
        let big = "x".repeat(1000);
        let cfg = RetrieverConfig::default();
        assert!(tee_and_hint_with(RecoveryMode::Sqlite, &cfg, &big, "cmd", 0).is_none());
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
