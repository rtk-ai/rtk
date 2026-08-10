//! Runs TOML filter inline tests to make sure filter rules work correctly.

use anyhow::Result;

use crate::core::toml_filter;

/// Run TOML filter inline tests.
///
/// - `filter`: if `Some`, only run tests for that filter name
/// - `require_all`: fail if any filter has no inline tests
pub fn run(filter: Option<String>, require_all: bool) -> Result<()> {
    let results = toml_filter::run_filter_tests(filter.as_deref());

    let total = results.outcomes.len();
    let passed = results.outcomes.iter().filter(|o| o.passed).count();
    let failed = total - passed;

    // Print failures with details
    for outcome in &results.outcomes {
        if !outcome.passed {
            eprintln!(
                "FAIL [{}] {}\n  expected: {:?}\n  actual:   {:?}",
                outcome.filter_name, outcome.test_name, outcome.expected, outcome.actual
            );
        }
    }

    if total == 0 {
        println!("No inline tests found.");
    } else {
        println!("{}/{} tests passed", passed, total);
    }

    if require_all && !results.filters_without_tests.is_empty() {
        for name in &results.filters_without_tests {
            eprintln!("MISSING tests for filter: {}", name);
        }
        anyhow::bail!(
            "{} filter(s) have no inline tests (use --require-all in CI)",
            results.filters_without_tests.len()
        );
    }

    if failed > 0 {
        anyhow::bail!("{} test(s) failed", failed);
    }

    Ok(())
}
