//! Prometheus exposition — one metrics formatter, meant to be shared with any
//! larger binary built on this crate.
//!
//! This module owns two things so neither gets duplicated downstream:
//!   1. the **metric name constants** ([`names`]) — so a Grafana dashboard
//!      built against this daemon keeps working against a bigger deployment
//!      of the same crate, and
//!   2. the **text-format writer** ([`Exposition`]) — HELP/TYPE headers,
//!      sample lines and label escaping, in one place.
//!
//! A consumer supplies only its own NUMBERS, and may emit extra families
//! through the same [`Exposition`] builder.

/// Canonical metric names, shared with any downstream binary so they never
/// drift apart.
pub mod names {
    pub const BUILD_INFO: &str = "rtkproxy_build_info";
    pub const REQUESTS: &str = "rtkproxy_requests_total";
    pub const ERRORS: &str = "rtkproxy_errors_total";
    pub const TOKENS_INPUT: &str = "rtkproxy_tokens_input_total";
    pub const TOKENS_OUTPUT: &str = "rtkproxy_tokens_output_total";
    pub const CACHE_READ_TOKENS: &str = "rtkproxy_cache_read_tokens_total";
    pub const CACHE_CREATION_TOKENS: &str = "rtkproxy_cache_creation_tokens_total";
    pub const CACHE_HIT_RATIO: &str = "rtkproxy_cache_hit_ratio";
    pub const CACHE_FRACTURES: &str = "rtkproxy_cache_fractures_total";
    pub const CACHE_FRACTURE_SESSIONS: &str = "rtkproxy_cache_fracture_sessions";
    pub const TOKENS_SAVED: &str = "rtkproxy_tokens_saved_total";
    pub const COST_USD: &str = "rtkproxy_cost_usd_total";
    pub const AVG_LATENCY_MS: &str = "rtkproxy_avg_latency_ms";
    pub const REQUESTS_BY_MODEL: &str = "rtkproxy_requests_by_model_total";
    pub const TOKENS_BY_MODEL: &str = "rtkproxy_tokens_by_model_total";
    pub const REQUESTS_BY_CLIENT: &str = "rtkproxy_requests_by_client_total";
}

/// A growing Prometheus text-exposition document (format version 0.0.4). Build one,
/// push metrics onto it, then [`Exposition::into_string`].
#[derive(Default)]
pub struct Exposition {
    body: String,
}

impl Exposition {
    pub fn new() -> Self {
        Self::default()
    }

    /// The conventional `*_build_info{version="x"} 1` gauge.
    pub fn build_info(&mut self, version: &str) -> &mut Self {
        self.decl(names::BUILD_INFO, "Build information.", "gauge");
        self.push(&format!(
            "{}{{version=\"{}\"}} 1",
            names::BUILD_INFO,
            esc(version)
        ));
        self
    }

    /// A single unlabelled counter.
    pub fn counter(&mut self, name: &str, help: &str, value: i64) -> &mut Self {
        self.decl(name, help, "counter");
        self.push(&format!("{name} {value}"));
        self
    }

    /// A single unlabelled counter carrying a float (e.g. USD), 6 decimals.
    pub fn counter_f(&mut self, name: &str, help: &str, value: f64) -> &mut Self {
        self.decl(name, help, "counter");
        self.push(&format!("{name} {value:.6}"));
        self
    }

    /// A single unlabelled gauge (point-in-time), 1 decimal.
    pub fn gauge(&mut self, name: &str, help: &str, value: f64) -> &mut Self {
        self.decl(name, help, "gauge");
        self.push(&format!("{name} {value:.1}"));
        self
    }

    /// A counter with one label: one sample per `(label_value, value)`.
    pub fn series(
        &mut self,
        name: &str,
        help: &str,
        label: &str,
        rows: &[(&str, i64)],
    ) -> &mut Self {
        self.decl(name, help, "counter");
        for (lv, v) in rows {
            self.push(&format!("{name}{{{label}=\"{}\"}} {v}", esc(lv)));
        }
        self
    }

    /// Emit only a HELP/TYPE header; follow with [`Exposition::sample`] lines for
    /// series that carry more than one label (build the label set with [`esc`]).
    pub fn header(&mut self, name: &str, help: &str, kind: &str) -> &mut Self {
        self.decl(name, help, kind);
        self
    }

    /// Push a raw pre-formatted sample line (its labels already escaped).
    pub fn sample(&mut self, line: &str) -> &mut Self {
        self.push(line);
        self
    }

    pub fn into_string(self) -> String {
        self.body
    }

    fn decl(&mut self, name: &str, help: &str, kind: &str) {
        self.body
            .push_str(&format!("# HELP {name} {help}\n# TYPE {name} {kind}\n"));
    }
    fn push(&mut self, line: &str) {
        self.body.push_str(line);
        self.body.push('\n');
    }
}

/// Escape a Prometheus label value (backslash, double-quote, newline).
pub fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_label_values() {
        assert_eq!(esc("a\"b\\c"), "a\\\"b\\\\c");
    }

    #[test]
    fn builds_valid_exposition() {
        let mut e = Exposition::new();
        e.build_info("1.2.3");
        e.counter(names::REQUESTS, "Requests.", 100);
        e.counter_f(names::COST_USD, "Cost.", 0.0025);
        e.gauge(names::AVG_LATENCY_MS, "Latency.", 42.5);
        e.series(
            names::REQUESTS_BY_MODEL,
            "By model.",
            "model",
            &[("opus", 80), ("haiku", 20)],
        );
        e.header(names::TOKENS_BY_MODEL, "Tokens by model.", "counter");
        e.sample(&format!(
            "{}{{model=\"opus\",direction=\"input\"}} 10",
            names::TOKENS_BY_MODEL
        ));
        let out = e.into_string();

        assert!(out.contains("rtkproxy_build_info{version=\"1.2.3\"} 1"));
        assert!(out.contains("rtkproxy_requests_total 100"));
        assert!(out.contains("rtkproxy_cost_usd_total 0.002500"));
        assert!(out.contains("rtkproxy_avg_latency_ms 42.5"));
        assert!(out.contains("rtkproxy_requests_by_model_total{model=\"opus\"} 80"));
        assert!(
            out.contains("rtkproxy_tokens_by_model_total{model=\"opus\",direction=\"input\"} 10")
        );
        // Every HELP has a matching TYPE.
        assert_eq!(out.matches("# HELP").count(), out.matches("# TYPE").count());
    }
}
