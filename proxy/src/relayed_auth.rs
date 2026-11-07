use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Default)]
pub struct RelayedAuth {
    held: RwLock<HashMap<String, (String, String)>>,
}

impl RelayedAuth {
    /// Remember the auth header seen on a request, so a later local worker can
    /// reuse it. Only `authorization` and `x-api-key` are kept.
    pub fn remember(&self, upstream: &str, headers: &axum::http::HeaderMap) {
        for name in ["authorization", "x-api-key"] {
            let Some(value) = headers.get(name).and_then(|v| v.to_str().ok()) else {
                continue;
            };
            if value.trim().is_empty() {
                continue;
            }
            if let Ok(mut held) = self.held.write() {
                held.insert(upstream.to_string(), (name.to_string(), value.to_string()));
            }
            return;
        }
    }

    /// The remembered header for `upstream`, as `(name, value)`.
    pub fn borrow(&self, upstream: &str) -> Option<(String, String)> {
        self.held
            .read()
            .ok()
            .and_then(|held| held.get(upstream).cloned())
    }

    /// The credential to attach to an outbound request, or `None` to leave it
    /// as it is. A request that brought its own is never substituted: that is
    /// traffic the agent authenticated itself, and the key it sent is the key
    /// it gets billed on. Only a worker arriving empty-handed is served.
    pub fn lend_if_missing(
        &self,
        upstream: &str,
        headers: &axum::http::HeaderMap,
    ) -> Option<(String, String)> {
        if carries_credential(headers) {
            return None;
        }
        self.borrow(upstream)
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
        a.remember("anthropic", &with("x-api-key", "sk-ant-real"));
        assert_eq!(
            a.borrow("anthropic"),
            Some(("x-api-key".into(), "sk-ant-real".into()))
        );
    }

    #[test]
    fn a_bearer_token_is_remembered_under_its_own_header() {
        let a = RelayedAuth::default();
        a.remember("anthropic", &with("authorization", "Bearer sk-ant-oat01-x"));
        let (name, value) = a.borrow("anthropic").expect("remembered");
        assert_eq!(name, "authorization");
        assert!(value.starts_with("Bearer "));
    }

    #[test]
    fn upstreams_do_not_share_a_credential() {
        let a = RelayedAuth::default();
        a.remember("anthropic", &with("x-api-key", "sk-ant"));
        a.remember("mistral", &with("authorization", "Bearer mistral-key"));
        assert!(a.borrow("openai").is_none());
        assert_eq!(
            a.borrow("mistral"),
            Some(("authorization".into(), "Bearer mistral-key".into()))
        );
        assert_eq!(
            a.borrow("anthropic"),
            Some(("x-api-key".into(), "sk-ant".into())),
            "a second upstream's key never displaces the first's"
        );
    }

    #[test]
    fn a_request_without_auth_does_not_erase_what_is_held() {
        let a = RelayedAuth::default();
        a.remember("anthropic", &with("x-api-key", "sk-ant"));
        a.remember("anthropic", &HeaderMap::new());
        assert!(a.borrow("anthropic").is_some());
    }

    #[test]
    fn an_empty_header_is_not_worth_remembering() {
        let a = RelayedAuth::default();
        a.remember("anthropic", &with("x-api-key", "   "));
        assert!(a.borrow("anthropic").is_none());
    }

    #[test]
    fn a_worker_arriving_without_a_key_is_served_the_remembered_one() {
        let a = RelayedAuth::default();
        a.remember("anthropic", &with("x-api-key", "sk-ant-real"));
        let lent = a.lend_if_missing("anthropic", &HeaderMap::new());
        assert_eq!(lent, Some(("x-api-key".into(), "sk-ant-real".into())));
    }

    #[test]
    fn a_request_with_its_own_key_is_never_substituted() {
        let a = RelayedAuth::default();
        a.remember("anthropic", &with("x-api-key", "sk-ant-remembered"));
        assert_eq!(
            a.lend_if_missing("anthropic", &with("x-api-key", "sk-ant-mine")),
            None,
            "the key a caller sent is the key it gets billed on"
        );
    }

    #[test]
    fn nothing_is_lent_before_anything_is_seen() {
        let a = RelayedAuth::default();
        assert_eq!(a.lend_if_missing("anthropic", &HeaderMap::new()), None);
    }

    #[test]
    fn a_blank_credential_does_not_count_as_one() {
        assert!(!carries_credential(&HeaderMap::new()));
        assert!(!carries_credential(&with("authorization", "")));
        assert!(carries_credential(&with("authorization", "Bearer x")));
    }
}
