//! The coverage table is a promise to users about what their agent can do.
//! A harness added to the registry without a row is a silent gap, so the doc
//! is checked against the registry rather than maintained by hand alone.

#![cfg(feature = "engine")]

const DOC: &str = include_str!("../docs/guide/resources/agent-coverage.md");

/// How each harness id is spelled in the doc's table, where the product name
/// differs from the registry id.
fn doc_name(id: &str) -> &str {
    match id {
        "claude-code" => "Claude Code",
        "copilot-cli" => "Copilot CLI",
        "vscode-copilot" => "Copilot (VS Code)",
        "gemini" => "Gemini CLI",
        "droid" => "Factory Droid",
        "vibe" => "Mistral Vibe",
        "omp" => "Oh My Pi",
        "kilocode" => "Kilo Code",
        "cline" => "Cline",
        "windsurf" => "Windsurf / Devin",
        "antigravity" => "Antigravity",
        "kimi" => "Kimi Code",
        "roo" => "Roo Code",
        "opencode" => "OpenCode",
        "openclaw" => "OpenClaw",
        "cursor" => "Cursor",
        "codex" => "Codex",
        "trae" => "Trae",
        "pi" => "Pi",
        "hermes" => "Hermes",
        other => other,
    }
}

#[test]
fn every_registered_harness_has_a_coverage_row() {
    for harness in rtk_engine::harness::HARNESSES {
        let name = doc_name(harness.id);
        assert!(
            DOC.contains(&format!("**{name}**")),
            "agent-coverage.md has no row for `{}` (expected **{name}**)",
            harness.id
        );
    }
}

#[test]
fn the_doc_claims_output_compression_for_exactly_the_harnesses_that_have_it() {
    let confirmed = rtk_engine::harness::HARNESSES
        .iter()
        .filter(|h| {
            h.event(rtk_engine::block::Event::AfterTool)
                .is_some_and(|c| c.can_replace_output)
        })
        .count();
    let claimed = DOC.matches("| **yes**").count() + DOC.matches("| MCP only").count();
    assert_eq!(
        confirmed, claimed,
        "the doc claims output compression for {claimed} agents, the registry has {confirmed}"
    );
}
