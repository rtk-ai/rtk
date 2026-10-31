//! Model registry: the single source of truth for model family and
//! prompt-cache rate, plus OPTIONAL operator-supplied prices. rtk ships no
//! prices — `input`/`output` are absent from the embedded `models.toml`, and
//! cost estimates exist only when the operator adds them (via
//! `RTK_PROXY_MODELS_TOML` or a registry edit). Loaded once at startup.

use once_cell::sync::Lazy;
use serde::Deserialize;
use std::sync::RwLock;

/// The embedded default registry — only a **seed**. At startup the DB copy (in
/// `app_settings`, shared by every pod in a stateless deployment) becomes the
/// source of truth; this default is written there on first run.
pub const DEFAULT_TOML: &str = include_str!("../models.toml");

#[derive(Debug, Clone, Deserialize)]
pub struct ModelSpec {
    /// Lowercase substring matched against the model id (specific → generic).
    pub pattern: String,
    #[serde(default = "default_family")]
    pub family: String,
    #[serde(default)]
    pub input: Option<f64>,
    #[serde(default)]
    pub output: Option<f64>,
    #[serde(default = "default_cache_mult")]
    pub cache_read_mult: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DefaultSpec {
    #[serde(default = "default_family")]
    pub family: String,
    #[serde(default)]
    pub input: Option<f64>,
    #[serde(default)]
    pub output: Option<f64>,
    #[serde(default = "default_cache_mult")]
    pub cache_read_mult: f64,
}

fn default_family() -> String {
    "other".to_string()
}
fn default_cache_mult() -> f64 {
    0.1
}

#[derive(Debug, Clone, Deserialize)]
pub struct Registry {
    #[serde(rename = "model", default)]
    pub models: Vec<ModelSpec>,
    pub default: DefaultSpec,
}

impl Registry {
    pub fn parse(toml_str: &str) -> anyhow::Result<Registry> {
        Ok(toml::from_str(toml_str)?)
    }

    /// First spec whose `pattern` is a substring of the (lowercased) model id.
    fn find(&self, model: &str) -> Option<&ModelSpec> {
        let m = model.to_lowercase();
        self.models.iter().find(|s| m.contains(&s.pattern))
    }

    /// (input $/Mtok, output $/Mtok) for a model — only when the operator
    /// supplied both numbers for the matched spec (else the default spec).
    /// rtk ships no prices, so this is `None` out of the box.
    pub fn price(&self, model: &str) -> Option<(f64, f64)> {
        match self.find(model) {
            Some(s) => s.input.zip(s.output),
            None => self.default.input.zip(self.default.output),
        }
    }

    /// What a cached-prefix read costs relative to the input price (0.1 = 10%).
    pub fn cache_read_mult(&self, model: &str) -> f64 {
        self.find(model)
            .map(|s| s.cache_read_mult)
            .unwrap_or(self.default.cache_read_mult)
    }

    /// Family string ("anthropic" | "openai" | "google" | "other").
    pub fn family(&self, model: &str) -> &str {
        self.find(model)
            .map(|s| s.family.as_str())
            .unwrap_or(self.default.family.as_str())
    }
}

/// The process-wide registry. Starts as the embedded default so pricing works
/// before the DB is read (and in tests); `install` overwrites it at startup with
/// the DB copy — the shared source of truth for every pod.
static REGISTRY: Lazy<RwLock<Registry>> = Lazy::new(|| {
    RwLock::new(Registry::parse(DEFAULT_TOML).expect("embedded models.toml must parse"))
});

/// Replace the live registry (called at startup with the DB copy, and on edit).
pub fn install(registry: Registry) {
    if let Ok(mut g) = REGISTRY.write() {
        *g = registry;
    }
}

/// The TOML used to **seed** `app_settings` on first boot — the DB copy is the
/// shared source of truth, so this is only consulted when the key is absent.
/// Returns the `RTK_PROXY_MODELS_TOML` file when set and readable (single-node
/// bootstrap), else the embedded default. It deliberately does NOT override an
/// existing DB value, so replicas never diverge on a per-pod env var.
pub fn boot_seed_toml() -> String {
    if let Ok(path) = std::env::var("RTK_PROXY_MODELS_TOML") {
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                tracing::info!(path, "model registry seed from RTK_PROXY_MODELS_TOML");
                return s;
            }
            Err(e) => tracing::warn!(
                path, error = %e,
                "RTK_PROXY_MODELS_TOML unreadable; seeding with embedded default"
            ),
        }
    }
    DEFAULT_TOML.to_string()
}

/// (input $/Mtok, output $/Mtok) for a model; `None` unless the operator
/// supplied prices. Never invented.
pub fn price(model: &str) -> Option<(f64, f64)> {
    REGISTRY.read().ok().and_then(|r| r.price(model))
}

/// What a cached-prefix read costs relative to the input price (0.1 = 10%).
pub fn cache_read_mult(model: &str) -> f64 {
    REGISTRY
        .read()
        .map(|r| r.cache_read_mult(model))
        .unwrap_or(0.1)
}

/// Family string ("anthropic" | "openai" | "google" | "other").
pub fn family(model: &str) -> String {
    REGISTRY
        .read()
        .map(|r| r.family(model).to_string())
        .unwrap_or_else(|_| "other".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_registry_ships_no_prices() {
        let r = Registry::parse(DEFAULT_TOML).unwrap();
        assert_eq!(r.price("claude-opus-4-8"), None);
        assert_eq!(r.price("gpt-4o-mini"), None);
        assert_eq!(r.price("totally-unknown"), None);
        assert_eq!(r.cache_read_mult("claude-opus-4-8"), 0.1);
        assert_eq!(r.cache_read_mult("gpt-4o"), 0.5);
        assert_eq!(r.family("gpt-4o"), "openai");
        assert_eq!(r.family("gemini-1.5-pro"), "google");
    }

    #[test]
    fn operator_prices_resolve_with_dated_ids_and_default() {
        let toml = r#"
[[model]]
pattern = "opus"
family = "anthropic"
input = 15.0
output = 75.0

[default]
family = "other"
input = 1.0
output = 5.0
"#;
        let r = Registry::parse(toml).unwrap();
        assert_eq!(r.price("claude-opus-4-8-20260101"), Some((15.0, 75.0)));
        assert_eq!(r.price("totally-unknown"), Some((1.0, 5.0)));
    }
}
