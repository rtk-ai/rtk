//! Optional cost estimation. rtk ships **no prices**: the model registry
//! carries families and cache-rate ratios only, and every dollar figure in the
//! analytics exists only when the operator has added their own prices to
//! `models.toml` (via `RTK_PROXY_MODELS_TOML` or a registry edit). With no
//! prices configured, cost functions return `None` and the dashboards fall
//! back to token-based metrics — rtk never invents a price.

/// What a cached-prefix read costs relative to the input price (registry-driven,
/// per model/provider). Anthropic 0.1x, OpenAI 0.5x, Google ~0.25x. A ratio,
/// not a price — always available.
pub fn cache_read_mult_of(model: &str) -> f64 {
    crate::models::cache_read_mult(model)
}

/// Cache-write multiplier relative to the model's input price (Anthropic
/// bills prefix creation at 1.25x). The READ multiplier is per-provider and
/// comes from the model registry via [`cache_read_mult_of`].
pub const CACHE_WRITE_MULT: f64 = 1.25;

/// Estimated USD cost of a request, accounting for prompt caching.
/// `input_tokens` are fresh (uncached) input tokens; cache writes (creation)
/// bill at 1.25x and cache reads at the registry's per-provider rate.
/// `None` when the operator has not configured a price for the model.
pub fn cost_usd_cached(
    model: &str,
    input_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    output_tokens: i64,
) -> Option<f64> {
    let (pin, pout) = crate::models::price(model)?;
    let inp = input_tokens.max(0) as f64;
    let create = cache_creation_tokens.max(0) as f64;
    let read = cache_read_tokens.max(0) as f64;
    let out = output_tokens.max(0) as f64;
    Some(
        (inp * pin + create * pin * CACHE_WRITE_MULT + read * pin * cache_read_mult_of(model))
            / 1_000_000.0
            + out * pout / 1_000_000.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRICED: &str = r#"
[[model]]
pattern = "opus"
family = "anthropic"
input = 15.0
output = 75.0
cache_read_mult = 0.1

[default]
family = "other"
cache_read_mult = 0.1
"#;

    #[test]
    fn costs_exist_only_when_the_operator_supplies_prices() {
        crate::models::install(
            crate::models::Registry::parse(crate::models::DEFAULT_TOML).unwrap(),
        );
        assert_eq!(cost_usd_cached("claude-opus-4-8", 1000, 0, 0, 500), None);
        assert_eq!(cost_usd_cached("totally-unknown", 1000, 0, 0, 500), None);

        crate::models::install(crate::models::Registry::parse(PRICED).unwrap());
        // 1000 fresh input @15, 2000 cache_creation @15*1.25, 4000 cache_read
        // @15*0.1, 500 output @75.
        let c = cost_usd_cached("claude-opus-4-8", 1000, 2000, 4000, 500).expect("priced");
        let expected = (1000.0 * 15.0 + 2000.0 * 15.0 * 1.25 + 4000.0 * 15.0 * 0.10) / 1e6
            + 500.0 * 75.0 / 1e6;
        assert!((c - expected).abs() < 1e-9, "got {c}, want {expected}");
        // A model outside the operator's table still has no price.
        assert_eq!(cost_usd_cached("gpt-4o-mini", 1000, 0, 0, 0), None);
        crate::models::install(
            crate::models::Registry::parse(crate::models::DEFAULT_TOML).unwrap(),
        );
    }
}
