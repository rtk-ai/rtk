//! Measure the compiled program size of every rule in the ruleset.
//!
//! The regex crate's default 10MB ceiling is a hard failure, but the real
//! concern is cumulative: rtk compiles the whole set, so a rule costing
//! megabytes is a budget problem even when it builds. `\w` is Unicode-aware by
//! default and compiles to a large UTF-8 automaton; multiplied by a bounded
//! repeat it dominates everything else.
//!
//! Run with `cargo run --release --example regex_probe`.

use regex::{Regex, RegexBuilder};
use serde::Deserialize;

#[derive(Deserialize)]
struct Ruleset {
    rule: Vec<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    id: String,
    pattern: String,
}

/// Smallest `size_limit` the pattern still compiles under -- a proxy for how
/// much memory the compiled program occupies.
fn program_size(pattern: &str) -> Option<usize> {
    RegexBuilder::new(pattern)
        .size_limit(64 * 1024 * 1024)
        .build()
        .ok()?;
    let (mut lo, mut hi) = (1usize, 64 * 1024 * 1024);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if RegexBuilder::new(pattern).size_limit(mid).build().is_ok() {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Some(lo)
}

fn human(n: usize) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.1} KB", n as f64 / (1 << 10) as f64)
    } else {
        format!("{n} B")
    }
}

fn main() {
    println!("=== ASCII-class rewrite of the two oversized rules ===");
    let vault = format!("hvb.{}", "a1B2c3D4e5-_".repeat(16));
    let pypi = format!("pypi-AgEIcHlwaS5vcmc{}", "a1B2c3D4e5-_".repeat(8));
    for (label, pat, sample) in [
        (
            "vault  unicode \\w {138,300}",
            r#"\b(hvb\.[\w-]{138,300})(?:[\x60'"\s;]|\\[nr]|$)"#,
            &vault,
        ),
        (
            "vault  ascii  {138,300}",
            r#"\b(hvb\.[0-9A-Za-z_-]{138,300})(?:[\x60'"\s;]|\\[nr]|$)"#,
            &vault,
        ),
        (
            "pypi   unicode \\w {50,1000}",
            r#"pypi-AgEIcHlwaS5vcmc[\w-]{50,1000}"#,
            &pypi,
        ),
        (
            "pypi   ascii  {50,1000}",
            r#"pypi-AgEIcHlwaS5vcmc[0-9A-Za-z_-]{50,1000}"#,
            &pypi,
        ),
    ] {
        match (Regex::new(pat), program_size(pat)) {
            (Ok(re), Some(sz)) => println!(
                "  {label:<30} {:>10}   matches={}",
                human(sz),
                re.is_match(sample)
            ),
            (Err(_), Some(sz)) => {
                println!("  {label:<30} {:>10}   OVER DEFAULT LIMIT", human(sz))
            }
            _ => println!("  {label:<30} {:>10}", "uncompilable"),
        }
    }

    println!("\n=== cost of every rule in the current ruleset ===");
    let toml_src = include_str!("../src/core/rules/secrets.toml");
    // Parsed, not string-scanned: the previous version keyed on the emitter's
    // exact `id      = "` spacing and would silently measure nothing if that
    // ever changed.
    let rs: Ruleset = toml::from_str(toml_src).expect("secrets.toml must parse");
    let mut rows: Vec<(String, Option<usize>, bool)> = rs
        .rule
        .iter()
        .map(|r| {
            (
                r.id.clone(),
                program_size(&r.pattern),
                Regex::new(&r.pattern).is_ok(),
            )
        })
        .collect();

    rows.sort_by_key(|(_, sz, _)| std::cmp::Reverse(sz.unwrap_or(0)));
    let total: usize = rows.iter().filter_map(|(_, s, _)| *s).sum();
    println!("  rules measured : {}", rows.len());
    println!("  TOTAL compiled : {}", human(total));
    println!("  mean per rule  : {}", human(total / rows.len().max(1)));
    println!("\n  top 12 by cost:");
    for (id, sz, fits) in rows.iter().take(12) {
        let flag = if *fits { "" } else { "  <-- OVER 10MB LIMIT" };
        println!(
            "    {id:<45} {:>10}{flag}",
            sz.map(human).unwrap_or_else(|| "n/a".into())
        );
    }
    let cheap = rows
        .iter()
        .filter(|(_, s, _)| s.unwrap_or(0) < 65536)
        .count();
    println!("\n  rules under 64KB: {cheap}/{}", rows.len());
}
