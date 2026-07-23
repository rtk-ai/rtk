//! Session context — carries the Claude Code session id when available.
//!
//! Session-scoped features (currently output dedup) key on this id so they only
//! ever act within a single Claude Code session. The id is populated once, from
//! the `--session` global flag (injected by the hook, see `hooks::hook_cmd`) or
//! the `RTK_SESSION_ID` environment variable. For manual invocations neither is
//! present, `id()` is `None`, and session-scoped features no-op — keeping raw
//! `rtk <cmd>` output byte-for-byte unchanged.

use std::sync::OnceLock;

static SESSION: OnceLock<SessionCtx> = OnceLock::new();

/// Resolved session identity for the current process.
#[derive(Debug, Clone, Default)]
pub struct SessionCtx {
    id: Option<String>,
}

impl SessionCtx {
    /// Resolve from the CLI flag, falling back to `RTK_SESSION_ID`. Blank or
    /// whitespace-only values are treated as absent.
    pub fn resolve(cli_flag: Option<String>) -> Self {
        Self {
            id: resolve_from(cli_flag, std::env::var("RTK_SESSION_ID").ok()),
        }
    }

    /// The session id, if known.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }
}

/// Pure resolution core: flag wins over env; blank values are dropped. Kept
/// separate so it can be unit-tested without touching the real environment.
fn resolve_from(cli_flag: Option<String>, env_val: Option<String>) -> Option<String> {
    cli_flag
        .filter(|s| !s.trim().is_empty())
        .or(env_val)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Initialize the process-global session context. The first call wins; later
/// calls are ignored, which keeps re-entrant paths and tests safe.
pub fn init(cli_flag: Option<String>) {
    let _ = SESSION.set(SessionCtx::resolve(cli_flag));
}

/// The process-global session context. If `init` was never called (code paths
/// that bypass `run_cli`), falls back to env-only resolution.
pub fn current() -> &'static SessionCtx {
    SESSION.get_or_init(|| SessionCtx::resolve(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_takes_precedence_over_env() {
        assert_eq!(
            resolve_from(Some("abc123".into()), Some("env-id".into())),
            Some("abc123".into())
        );
    }

    #[test]
    fn env_used_when_flag_absent() {
        assert_eq!(
            resolve_from(None, Some("env-id".into())),
            Some("env-id".into())
        );
    }

    #[test]
    fn both_absent_yields_none() {
        assert_eq!(resolve_from(None, None), None);
    }

    #[test]
    fn blank_flag_falls_through_to_env() {
        assert_eq!(
            resolve_from(Some("   ".into()), Some("env-id".into())),
            Some("env-id".into())
        );
    }

    #[test]
    fn blank_flag_and_no_env_yields_none() {
        assert_eq!(resolve_from(Some("  ".into()), None), None);
    }

    #[test]
    fn value_is_trimmed() {
        assert_eq!(
            resolve_from(Some("  abc  ".into()), None),
            Some("abc".into())
        );
    }
}
