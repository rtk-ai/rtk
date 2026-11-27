//! Runtime-mutable settings.
//!
//! Two layers: the boot [`Config`] seeds them, and any row persisted in the
//! store's `app_settings` table overlays it — so a `rtk llm-proxy config set`
//! survives a restart. Everything the forward path reads per request lives
//! here, behind the `Arc<RwLock<..>>` on `AppState`.

use crate::config::Config;
use std::collections::HashMap;

/// `app_settings` keys. A `config set` persists one of these.
pub mod keys {
    pub const ANTHROPIC_UPSTREAM: &str = "anthropic_upstream";
    pub const OPENAI_UPSTREAM: &str = "openai_upstream";
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
}

impl RuntimeSettings {
    /// Seed from the boot config.
    pub fn from_config(c: &Config) -> Self {
        Self {
            anthropic_upstream: c.anthropic_upstream.clone(),
            openai_upstream: c.openai_upstream.clone(),
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
    }

    /// JSON for the settings endpoint. `ccr` is supplied by the caller (it
    /// lives on the shared pipeline flag, not here).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "anthropic_upstream": self.anthropic_upstream,
            "openai_upstream": self.openai_upstream,
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
        }
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
