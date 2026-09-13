//! Recovery-hint dispatch — routes to the sqlite store or legacy tee per `[retriever] mode`.

use crate::core::config::Config;
pub(crate) use crate::core::retriever::MIN_FAILURE_BYTES as MIN_TEE_SIZE;
use crate::core::retriever::{self, MIN_FAILURE_BYTES, RecoveryMode, RetrieverConfig, Stored};

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
        Stored::Saved(s) => Some(format!("[full output: rtk recall {}]", s.hash)),
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
        RecoveryMode::Sqlite => {
            match retriever::store(&cfg, content.as_bytes(), command_slug, None, line_offset) {
                Stored::Saved(s) if s.hidden_lines > 0 => Some(format!(
                    "[+{} hidden: rtk recall {}]",
                    s.hidden_lines, s.hash
                )),
                Stored::Saved(_) | Stored::Unavailable | Stored::Empty => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabled_env_emits_nothing() {
        let _guard = crate::core::utils::TEST_ENV_LOCK.lock().unwrap();
        std::env::set_var("RTK_RECALL", "0");
        let big = "x".repeat(1000);
        let hint = tee_and_hint(&big, "cmd", 1);
        let forced = force_tee_hint(&big, "cmd");
        let tail = force_tee_tail_hint(&big, "cmd", 5);
        std::env::remove_var("RTK_RECALL");
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
