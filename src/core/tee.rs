//! Recovery-hint dispatch — routes to the sqlite store or legacy tee per `[retriever] mode`.

pub(crate) use crate::core::retriever::MIN_FAILURE_BYTES as MIN_TEE_SIZE;
use crate::core::retriever::{self, RecoveryMode, RetrieverConfig, Stored, MIN_FAILURE_BYTES};

fn active() -> Option<(RecoveryMode, RetrieverConfig)> {
    if matches!(std::env::var("RTK_RECALL").ok().as_deref(), Some("0"))
        || matches!(std::env::var("RTK_TEE").ok().as_deref(), Some("0"))
    {
        return None;
    }
    // Cached, not a fresh load: the hint paths in search.rs call this once per
    // file, and a disk read plus TOML parse per file is the other half of the
    // per-file overhead that breaches the <10ms target (B11/V18). This is a
    // read-only caller that never writes config, which is what cached_config
    // requires.
    let cfg = &crate::core::config::cached_config().retriever;
    match cfg.mode {
        RecoveryMode::Disabled => None,
        mode => Some((mode, cfg.clone())),
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
        RecoveryMode::Sqlite => {
            match retriever::store(&cfg, content.as_bytes(), command_slug, None, line_offset) {
                Stored::Saved(s) => Some(format!(
                    "[+{} hidden: rtk recall {}]",
                    s.hidden_lines, s.hash
                )),
                Stored::Unavailable | Stored::Empty => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabled_env_emits_nothing() {
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
