//! Runtime-mutable settings.
//!
//! Two layers: the boot [`Config`] seeds them, and any row persisted in the
//! store's `app_settings` table overlays it — so a `rtk llm-proxy config set`
//! survives a restart. Everything the forward path reads per request lives
//! here, behind the `Arc<RwLock<..>>` on `AppState`.

use crate::config::Config;
use std::collections::{BTreeMap, HashMap};

/// `app_settings` keys. A `config set` persists one of these.
pub mod keys {
    pub const ANTHROPIC_UPSTREAM: &str = "anthropic_upstream";
    pub const OPENAI_UPSTREAM: &str = "openai_upstream";
    pub const UPSTREAM_PREFIX: &str = "upstream.";
    /// CCR (Compress-Cache-Retrieve), the reversible context compressor —
    /// see `pipeline::ccr`. On by default; requires the ICM memory store.
    pub const CCR_ENABLED: &str = "ccr";
    /// Ingest-filter aggressiveness: off | light | balanced | aggressive.
    pub const COMPRESSION: &str = "compression";
    /// Pixel-compress, the EXPERIMENTAL render-text-as-PNG stage.
    pub const PIXEL_COMPRESS: &str = "pixel_compress";
}

/// The live, runtime-mutable settings. CCR is kept separately as the shared
/// flag wired into the pipeline (`AppState::ccr_enabled`).
#[derive(Debug, Clone)]
pub struct RuntimeSettings {
    pub anthropic_upstream: String,
    pub openai_upstream: String,
    pub upstreams: BTreeMap<String, String>,
}

impl RuntimeSettings {
    /// Seed from the boot config.
    pub fn from_config(c: &Config) -> Self {
        Self {
            anthropic_upstream: c.anthropic_upstream.clone(),
            openai_upstream: c.openai_upstream.clone(),
            upstreams: c.upstreams.clone(),
        }
    }

    /// Overlay persisted `app_settings` rows onto the seed (DB wins over config).
    pub fn apply_persisted(&mut self, map: &HashMap<String, String>) {
        if let Some(v) = map.get(keys::ANTHROPIC_UPSTREAM).and_then(|v| non_empty(v)) {
            self.anthropic_upstream = v;
        }
        if let Some(v) = map.get(keys::OPENAI_UPSTREAM).and_then(|v| non_empty(v)) {
            self.openai_upstream = v;
        }
        for (key, value) in map {
            let Some(name) = key.strip_prefix(keys::UPSTREAM_PREFIX) else {
                continue;
            };
            let Some(url) = non_empty(value) else {
                continue;
            };
            if crate::config::validate_upstream_name(name).is_ok() {
                self.upstreams.insert(name.to_string(), url);
            }
        }
    }

    pub fn upstream(&self, name: &str) -> Option<String> {
        match name {
            crate::config::ANTHROPIC_UPSTREAM_NAME => Some(self.anthropic_upstream.clone()),
            crate::config::OPENAI_UPSTREAM_NAME => Some(self.openai_upstream.clone()),
            other => self.upstreams.get(other).cloned(),
        }
    }

    pub fn upstream_names(&self) -> Vec<String> {
        let mut names = vec![
            crate::config::ANTHROPIC_UPSTREAM_NAME.to_string(),
            crate::config::OPENAI_UPSTREAM_NAME.to_string(),
        ];
        names.extend(self.upstreams.keys().cloned());
        names
    }

    /// JSON for the settings endpoint. `ccr` is supplied by the caller (it
    /// lives on the shared pipeline flag, not here).
    pub fn to_json(&self) -> serde_json::Value {
        let upstreams: serde_json::Map<String, serde_json::Value> = self
            .upstream_names()
            .into_iter()
            .filter_map(|name| {
                self.upstream(&name)
                    .map(|url| (name, serde_json::Value::String(url)))
            })
            .collect();
        serde_json::json!({
            "anthropic_upstream": self.anthropic_upstream,
            "openai_upstream": self.openai_upstream,
            "upstreams": upstreams,
        })
    }
}

/// Parse a lenient boolean (`1|true|yes|on` → true).
pub fn parse_bool(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn non_empty(v: &str) -> Option<String> {
    let t = v.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed() -> RuntimeSettings {
        RuntimeSettings {
            anthropic_upstream: "https://api.anthropic.com".into(),
            openai_upstream: "https://api.openai.com".into(),
            upstreams: BTreeMap::new(),
        }
    }

    #[test]
    fn named_upstreams_resolve_by_name_and_persist_under_a_prefix() {
        let mut s = seed();
        s.upstreams
            .insert("mistral".into(), "https://api.mistral.ai".into());
        assert_eq!(
            s.upstream("anthropic").as_deref(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(
            s.upstream("mistral").as_deref(),
            Some("https://api.mistral.ai")
        );
        assert_eq!(s.upstream("nope"), None);
        assert_eq!(s.upstream_names(), vec!["anthropic", "openai", "mistral"]);

        let mut map = HashMap::new();
        map.insert(
            "upstream.mistral".to_string(),
            "http://127.0.0.1:9".to_string(),
        );
        map.insert("upstream.v1".to_string(), "http://127.0.0.1:1".to_string());
        map.insert("upstream.acme".to_string(), "   ".to_string());
        s.apply_persisted(&map);
        assert_eq!(s.upstream("mistral").as_deref(), Some("http://127.0.0.1:9"));
        assert_eq!(
            s.upstream("v1"),
            None,
            "a reserved name never becomes an upstream"
        );
        assert_eq!(s.upstream("acme"), None);
        assert_eq!(s.to_json()["upstreams"]["mistral"], "http://127.0.0.1:9");
        assert_eq!(
            s.to_json()["upstreams"]["anthropic"],
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn persisted_overrides_seed() {
        let mut s = seed();
        let mut map = HashMap::new();
        map.insert(
            keys::OPENAI_UPSTREAM.into(),
            "https://api.groq.com/openai".into(),
        );

        s.apply_persisted(&map);
        assert_eq!(s.openai_upstream, "https://api.groq.com/openai");
        // Untouched key keeps the seeded value.
        assert_eq!(s.anthropic_upstream, "https://api.anthropic.com");
    }

    #[test]
    fn blank_persisted_upstream_does_not_erase_the_seed() {
        let mut s = seed();
        let mut map = HashMap::new();
        map.insert(keys::ANTHROPIC_UPSTREAM.into(), "   ".into());
        s.apply_persisted(&map);
        assert_eq!(s.anthropic_upstream, "https://api.anthropic.com");
    }

    #[test]
    fn parse_bool_accepts_the_usual_spellings() {
        for t in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(parse_bool(t), "{t} should parse true");
        }
        for f in ["0", "false", "no", "off", ""] {
            assert!(!parse_bool(f), "{f} should parse false");
        }
    }
}
