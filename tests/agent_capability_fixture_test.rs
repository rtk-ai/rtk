//! Validate the deterministic agent capability manifest before it is used by
//! integration or benchmark tasks.

use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct Manifest {
    schema_version: u32,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    route: String,
    argv: Vec<String>,
    consumer: String,
    requires_tty: bool,
    expected_exit: i32,
    expected_contract: String,
    fixture: String,
}

#[test]
fn agent_capability_manifest_is_complete_and_deterministic() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = root.join("tests/fixtures/agent_capabilities.json");
    let manifest: Manifest = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path).expect("capability manifest must exist"),
    )
    .expect("capability manifest must be valid JSON");

    assert_eq!(manifest.schema_version, 1);
    assert!(manifest.cases.len() >= 6);

    let mut ids = HashSet::new();
    for case in manifest.cases {
        assert!(
            ids.insert(case.id.clone()),
            "duplicate case id: {}",
            case.id
        );
        assert!(!case.route.trim().is_empty(), "empty route: {}", case.id);
        assert!(!case.argv.is_empty(), "empty argv: {}", case.id);
        assert!(
            case.argv.first().is_some_and(|arg| arg != "rtk"),
            "argv must be typed without a leading rtk: {}",
            case.id
        );
        assert!(
            matches!(case.consumer.as_str(), "agent" | "exact" | "machine"),
            "invalid consumer for {}: {}",
            case.id,
            case.consumer
        );
        assert!((0..=255).contains(&case.expected_exit));
        assert!(
            matches!(
                case.expected_contract.as_str(),
                "ai_owned" | "exact" | "legacy"
            ),
            "invalid output contract for {}: {}",
            case.id,
            case.expected_contract
        );
        let fixture = root.join(&case.fixture);
        assert!(
            fixture.is_file(),
            "missing fixture for {}: {}",
            case.id,
            fixture.display()
        );

        if case.consumer == "machine" {
            assert!(
                !case.requires_tty,
                "machine case must not require a tty: {}",
                case.id
            );
            assert_eq!(case.expected_contract, "exact");
        }
    }
}
