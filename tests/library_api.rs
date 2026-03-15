use rtk::config::Config;
use rtk::rewrite::{rewrite_command, rewrite_with_config};
use rtk::toml_filter::{apply_filter, find_filter_in, TomlFilterRegistry};

#[test]
fn rewrite_public_api_rewrites_supported_commands() {
    assert_eq!(
        rewrite_command("git status", &[]),
        Some("rtk git status".to_string())
    );
}

#[test]
fn rewrite_with_config_respects_excluded_commands() {
    let mut config = Config::default();
    config.hooks.exclude_commands = vec!["git".to_string()];

    assert_eq!(rewrite_with_config("git status", &config), None);
}

#[test]
fn toml_filter_registry_from_toml_str_builds_reusable_filters() {
    let registry = TomlFilterRegistry::from_toml_str(
        r#"
schema_version = 1

[filters.tests]
match_command = "^pytest"
keep_lines_matching = ["^FAIL", "^PASS"]
"#,
        "test",
    )
    .expect("valid filter registry");

    let filter = find_filter_in("pytest tests/", &registry.filters).expect("matching filter");
    let output = apply_filter(filter, "noise\nPASS first\nFAIL second");

    assert_eq!(output, "PASS first\nFAIL second");
}
