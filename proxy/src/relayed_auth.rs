//! The credential a local worker can borrow instead of being given one.
//!
//! The daemon already relays the agent's auth header on every request. A task
//! worker routing to the *same provider* needs exactly that credential, and
//! asking the user to export a second copy of a key their agent is already
//! sending is friction with nothing behind it.
//!
//! So the header is remembered, in memory only, and served back to a request
//! that arrives on the loopback listener without one.
//!
//! # Why this is not a new exposure
//!
//! Anything that can reach 127.0.0.1 is running as the user, and a process
//! running as the user can already read the credential directly — from the
//! keychain, the environment, or the agent's own settings file. Holding it in
//! memory grants nothing that was not already reachable. It is never written
//! to disk, never logged, and never leaves the machine except to the provider
//! it came from.
//!
//! The one case this does change is a sandboxed process that can open a
//! loopback socket but cannot read the user keychain. That is narrow, and
//! `relay_borrowing = false` turns the lending off for anyone who disagrees.

use std::sync::RwLock;

use crate::model::Provider;

#[derive(Default)]
pub struct RelayedAuth {
    anthropic: RwLock<Option<(String, String)>>,
    openai: RwLock<Option<(String, String)>>,
}

impl RelayedAuth {
    /// Remember the auth header seen on a request, so a later local worker can
    /// reuse it. Only `authorization` and `x-api-key` are kept.
    pub fn remember(&self, provider: Provider, headers: &axum::http::HeaderMap) {
        for name in ["authorization", "x-api-key"] {
            let Some(value) = headers.get(name).and_then(|v| v.to_str().ok()) else {
                continue;
            };
            if value.trim().is_empty() {
                continue;
            }
            let slot = match provider {
                Provider::Anthropic => &self.anthropic,
                _ => &self.openai,
            };
            if let Ok(mut held) = slot.write() {
                *held = Some((name.to_string(), value.to_string()));
            }
            return;
        }
    }

    /// The remembered header for `provider`, as `(name, value)`.
    pub fn borrow(&self, provider: Provider) -> Option<(String, String)> {
        let slot = match provider {
            Provider::Anthropic => &self.anthropic,
            _ => &self.openai,
        };
        slot.read().ok().and_then(|held| held.clone())
    }

    /// The credential to attach to an outbound request, or `None` to leave it
    /// as it is. A request that brought its own is never substituted: that is
    /// traffic the agent authenticated itself, and the key it sent is the key
    /// it gets billed on. Only a worker arriving empty-handed is served.
    pub fn lend_if_missing(
        &self,
        provider: Provider,
        headers: &axum::http::HeaderMap,
    ) -> Option<(String, String)> {
        if carries_credential(headers) {
            return None;
        }
        self.borrow(provider)
    }
}

/// Whether a request already authenticates itself.
pub fn carries_credential(headers: &axum::http::HeaderMap) -> bool {
    ["authorization", "x-api-key"].iter().any(|name| {
        headers
            .get(*name)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| !v.trim().is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn with(name: &'static str, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(name, value.parse().unwrap());
        h
    }

    #[test]
    fn an_api_key_is_remembered_and_lent_back() {
        let a = RelayedAuth::default();
        a.remember(Provider::Anthropic, &with("x-api-key", "sk-ant-real"));
        assert_eq!(
            a.borrow(Provider::Anthropic),
            Some(("x-api-key".into(), "sk-ant-real".into()))
        );
    }

    #[test]
    fn a_bearer_token_is_remembered_under_its_own_header() {
        let a = RelayedAuth::default();
        a.remember(
            Provider::Anthropic,
            &with("authorization", "Bearer sk-ant-oat01-x"),
        );
        let (name, value) = a.borrow(Provider::Anthropic).expect("remembered");
        assert_eq!(name, "authorization");
        assert!(value.starts_with("Bearer "));
    }

    #[test]
    fn providers_do_not_share_a_credential() {
        let a = RelayedAuth::default();
        a.remember(Provider::Anthropic, &with("x-api-key", "sk-ant"));
        assert!(a.borrow(Provider::Openai).is_none());
    }

    #[test]
    fn a_request_without_auth_does_not_erase_what_is_held() {
        let a = RelayedAuth::default();
        a.remember(Provider::Anthropic, &with("x-api-key", "sk-ant"));
        a.remember(Provider::Anthropic, &HeaderMap::new());
        assert!(a.borrow(Provider::Anthropic).is_some());
    }

    #[test]
    fn an_empty_header_is_not_worth_remembering() {
        let a = RelayedAuth::default();
        a.remember(Provider::Anthropic, &with("x-api-key", "   "));
        assert!(a.borrow(Provider::Anthropic).is_none());
    }

    #[test]
    fn a_worker_arriving_without_a_key_is_served_the_remembered_one() {
        let a = RelayedAuth::default();
        a.remember(Provider::Anthropic, &with("x-api-key", "sk-ant-real"));
        let lent = a.lend_if_missing(Provider::Anthropic, &HeaderMap::new());
        assert_eq!(lent, Some(("x-api-key".into(), "sk-ant-real".into())));
    }

    #[test]
    fn a_request_with_its_own_key_is_never_substituted() {
        let a = RelayedAuth::default();
        a.remember(Provider::Anthropic, &with("x-api-key", "sk-ant-remembered"));
        assert_eq!(
            a.lend_if_missing(Provider::Anthropic, &with("x-api-key", "sk-ant-mine")),
            None,
            "the key a caller sent is the key it gets billed on"
        );
    }

    #[test]
    fn nothing_is_lent_before_anything_is_seen() {
        let a = RelayedAuth::default();
        assert_eq!(
            a.lend_if_missing(Provider::Anthropic, &HeaderMap::new()),
            None
        );
    }

    #[test]
    fn a_blank_credential_does_not_count_as_one() {
        assert!(!carries_credential(&HeaderMap::new()));
        assert!(!carries_credential(&with("authorization", "")));
        assert!(carries_credential(&with("authorization", "Bearer x")));
    }
}
