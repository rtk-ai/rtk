//! Unified Rule system: safety rules, remaps, and warnings as data-driven MD files.
//!
//! Replaces `SafetyAction`, `SafetyRule`, `rule!()` macro, and `get_rules()` from safety.rs.
//! Rules are MD files with YAML frontmatter, loaded from built-in defaults and user directories.

use anyhow::{anyhow, Result};
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

/// A unified rule: safety, remap, warning, or block.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Rule {
    pub name: String,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default = "default_block")]
    pub action: String,
    #[serde(default)]
    pub redirect: Option<String>,
    #[serde(default = "default_always")]
    pub when: String,
    #[serde(default)]
    pub env_var: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(skip)]
    pub message: String,
    #[serde(skip)]
    pub source: String,
}

fn default_block() -> String {
    "block".into()
}
fn default_always() -> String {
    "always".into()
}
fn default_true() -> bool {
    true
}

impl Rule {
    /// Check if rule should apply given current env + predicates.
    pub fn should_apply(&self) -> bool {
        // Env var opt-out check
        if let Some(ref env) = self.env_var {
            if let Ok(val) = std::env::var(env) {
                if val == "0" || val == "false" {
                    return false;
                }
            }
        }
        // When predicate
        check_when(&self.when)
    }
}

// === Predicate Registry ===

type PredicateFn = fn() -> bool;

fn predicate_registry() -> &'static HashMap<&'static str, PredicateFn> {
    static REGISTRY: OnceLock<HashMap<&'static str, PredicateFn>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert("always", (|| true) as PredicateFn);
        m.insert(
            "has_unstaged_changes",
            crate::cmd::predicates::has_unstaged_changes as PredicateFn,
        );
        m
    })
}

pub fn check_when(when: &str) -> bool {
    if when == "always" || when.is_empty() {
        return true;
    }
    if let Some(func) = predicate_registry().get(when) {
        return func();
    }
    // Bash fallback (matches clautorun behavior)
    std::process::Command::new("sh")
        .args(["-c", when])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// === Parse & Load ===

/// Parse a rule from MD content with YAML frontmatter.
pub fn parse_rule(content: &str, source: &str) -> Result<Rule> {
    let trimmed = content.trim();
    let rest = trimmed
        .strip_prefix("---")
        .ok_or_else(|| anyhow!("No frontmatter: missing opening ---"))?;
    let end = rest
        .find("\n---")
        .ok_or_else(|| anyhow!("Unclosed frontmatter: missing closing ---"))?;
    let yaml = &rest[..end];
    let body = rest[end + 4..].trim();
    let mut rule: Rule = serde_yaml::from_str(yaml)?;
    rule.message = body.to_string();
    rule.source = source.to_string();
    Ok(rule)
}

/// Embedded default rules (compiled into binary).
pub const DEFAULT_RULES: &[&str] = &[
    include_str!("../rules/rtk.safety.rm-to-trash.md"),
    include_str!("../rules/rtk.safety.git-reset-hard.md"),
    include_str!("../rules/rtk.safety.git-checkout-dashdash.md"),
    include_str!("../rules/rtk.safety.git-checkout-dot.md"),
    include_str!("../rules/rtk.safety.git-stash-drop.md"),
    include_str!("../rules/rtk.safety.git-clean-fd.md"),
    include_str!("../rules/rtk.safety.git-clean-df.md"),
    include_str!("../rules/rtk.safety.git-clean-f.md"),
    include_str!("../rules/rtk.safety.block-cat.md"),
    include_str!("../rules/rtk.safety.block-sed.md"),
    include_str!("../rules/rtk.safety.block-head.md"),
];

static RULES_CACHE: OnceLock<Vec<Rule>> = OnceLock::new();

/// Load all rules: embedded defaults + user overrides. Cached via OnceLock.
pub fn load_all() -> &'static [Rule] {
    RULES_CACHE.get_or_init(|| {
        let mut rules_by_name: BTreeMap<String, Rule> = BTreeMap::new();

        // 1. Embedded defaults (lowest priority)
        for content in DEFAULT_RULES {
            match parse_rule(content, "builtin") {
                Ok(rule) if rule.enabled => {
                    rules_by_name.insert(rule.name.clone(), rule);
                }
                Ok(rule) => {
                    rules_by_name.remove(&rule.name);
                }
                Err(e) => eprintln!("rtk: bad builtin rule: {e}"),
            }
        }

        // 2. User files (higher priority overrides by name)
        for path in super::discovery::discover_rtk_files() {
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            match parse_rule(&content, &path.display().to_string()) {
                Ok(rule) if rule.enabled => {
                    rules_by_name.insert(rule.name.clone(), rule);
                }
                Ok(rule) => {
                    rules_by_name.remove(&rule.name);
                }
                Err(_) => continue,
            }
        }

        rules_by_name.into_values().collect()
    })
}

// === Global Option Stripping ===

/// Strip global options that appear between a command and its subcommand.
///
/// Tools like git, cargo, docker, and kubectl accept global options before
/// the subcommand (e.g., `git -C /path --no-pager status`). These must be
/// stripped before pattern matching so that safety rules like `"git reset --hard"`
/// still match `git --no-pager reset --hard`.
///
/// Based on the patterns from upstream PR #99 (hooks/rtk-rewrite.sh).
fn strip_global_options(full_cmd: &str) -> String {
    let words: Vec<&str> = full_cmd.split_whitespace().collect();
    if words.is_empty() {
        return full_cmd.to_string();
    }

    let binary = words[0];
    let rest = &words[1..];

    match binary {
        "git" => {
            // Strip: -C <path>, -c <key=val>, --no-pager, --no-optional-locks,
            //         --bare, --literal-pathspecs, --key=value
            let mut result = vec!["git"];
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i];
                if (w == "-C" || w == "-c") && i + 1 < rest.len() {
                    i += 2; // skip flag + argument
                } else if w.starts_with("--")
                    && w.contains('=')
                    && !w.starts_with("--hard")
                    && !w.starts_with("--force")
                {
                    i += 1; // skip --key=value global options
                } else if matches!(
                    w,
                    "--no-pager"
                        | "--no-optional-locks"
                        | "--bare"
                        | "--literal-pathspecs"
                        | "--paginate"
                        | "--git-dir"
                ) {
                    i += 1; // skip standalone boolean global options
                } else {
                    // First non-global-option word is the subcommand; keep everything from here
                    result.extend_from_slice(&rest[i..]);
                    break;
                }
            }
            result.join(" ")
        }
        "cargo" => {
            // Strip: +toolchain (e.g., cargo +nightly test)
            let mut result = vec!["cargo"];
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i];
                if w.starts_with('+') {
                    i += 1; // skip +toolchain
                } else {
                    result.extend_from_slice(&rest[i..]);
                    break;
                }
            }
            result.join(" ")
        }
        "docker" => {
            // Strip: -H <host>, --context <ctx>, --config <path>, --key=value
            let mut result = vec!["docker"];
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i];
                if matches!(w, "-H" | "--context" | "--config") && i + 1 < rest.len() {
                    i += 2; // skip flag + argument
                } else if w.starts_with("--") && w.contains('=') {
                    i += 1; // skip --key=value
                } else {
                    result.extend_from_slice(&rest[i..]);
                    break;
                }
            }
            result.join(" ")
        }
        "kubectl" => {
            // Strip: --context <ctx>, --kubeconfig <path>, --namespace <ns>, -n <ns>, --key=value
            let mut result = vec!["kubectl"];
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i];
                if matches!(w, "--context" | "--kubeconfig" | "--namespace" | "-n")
                    && i + 1 < rest.len()
                {
                    i += 2; // skip flag + argument
                } else if w.starts_with("--") && w.contains('=') {
                    i += 1; // skip --key=value
                } else {
                    result.extend_from_slice(&rest[i..]);
                    break;
                }
            }
            result.join(" ")
        }
        _ => full_cmd.to_string(),
    }
}

// === Pattern Matching ===

/// Check if a rule matches a command.
///
/// - Single-word pattern: exact binary match (avoids "cat" matching "catalog")
/// - Multi-word pattern: prefix match on full command string (with global option stripping)
/// - Raw mode (binary=None): word-boundary search (handles "sudo rm")
pub fn matches_rule(rule: &Rule, binary: Option<&str>, full_cmd: &str) -> bool {
    rule.patterns.iter().any(|pat| {
        if pat.contains(' ') {
            // Multi-word: prefix match, also try with global options stripped
            let normalized = strip_global_options(full_cmd);
            full_cmd.starts_with(pat.as_str()) || normalized.starts_with(pat.as_str())
        } else if let Some(bin) = binary {
            // Parsed mode: exact binary
            bin == pat
        } else {
            // Raw mode: word-boundary (handles "sudo rm", "/usr/bin/rm")
            full_cmd
                .split_whitespace()
                .any(|w| w == pat || w.ends_with(&format!("/{pat}")))
        }
    })
}

// === Remap Helper ===

/// Try to expand a single-word remap alias (e.g., "t --lib" → "cargo test --lib").
///
/// Only matches single-word patterns with `action: "rewrite"`. Multi-word rewrites
/// are safety rules handled by `check()`. Order: remap → safety → execute.
pub fn try_remap(raw: &str) -> Option<String> {
    let first_word = raw.split_whitespace().next()?;
    for rule in load_all() {
        if rule.action != "rewrite" {
            continue;
        }
        // Only remap single-word pattern matches (aliases like "t" → "cargo test")
        if !rule
            .patterns
            .iter()
            .any(|p| !p.contains(' ') && p == first_word)
        {
            continue;
        }
        if !rule.should_apply() {
            continue;
        }
        if let Some(ref redirect) = rule.redirect {
            let rest = raw[first_word.len()..].trim();
            return Some(redirect.replace("{args}", rest));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rule_valid() {
        let content = "---\nname: test-rule\npatterns: [rm]\naction: trash\n---\nSafety message.";
        let rule = parse_rule(content, "test").unwrap();
        assert_eq!(rule.name, "test-rule");
        assert_eq!(rule.patterns, vec!["rm"]);
        assert_eq!(rule.action, "trash");
        assert_eq!(rule.message, "Safety message.");
        assert_eq!(rule.source, "test");
    }

    #[test]
    fn test_parse_rule_no_frontmatter() {
        let content = "No frontmatter here";
        assert!(parse_rule(content, "test").is_err());
    }

    #[test]
    fn test_parse_rule_unclosed_frontmatter() {
        let content = "---\nname: broken\n";
        assert!(parse_rule(content, "test").is_err());
    }

    #[test]
    fn test_parse_rule_message_body() {
        let content = "---\nname: test\n---\n\nLine 1\n\nLine 2";
        let rule = parse_rule(content, "test").unwrap();
        assert_eq!(rule.message, "Line 1\n\nLine 2");
    }

    #[test]
    fn test_parse_rule_defaults() {
        let content = "---\nname: minimal\n---\n";
        let rule = parse_rule(content, "test").unwrap();
        assert_eq!(rule.action, "block"); // default
        assert_eq!(rule.when, "always"); // default
        assert!(rule.enabled); // default true
        assert!(rule.patterns.is_empty()); // default empty
    }

    #[test]
    fn test_parse_rule_all_fields() {
        let content = r#"---
name: full
patterns: ["git reset --hard"]
action: rewrite
redirect: "git stash && git reset --hard {args}"
when: has_unstaged_changes
env_var: RTK_SAFE_COMMANDS
enabled: true
---
Full message."#;
        let rule = parse_rule(content, "builtin").unwrap();
        assert_eq!(rule.name, "full");
        assert_eq!(rule.patterns, vec!["git reset --hard"]);
        assert_eq!(rule.action, "rewrite");
        assert_eq!(
            rule.redirect.as_deref(),
            Some("git stash && git reset --hard {args}")
        );
        assert_eq!(rule.when, "has_unstaged_changes");
        assert_eq!(rule.env_var.as_deref(), Some("RTK_SAFE_COMMANDS"));
        assert!(rule.enabled);
        assert_eq!(rule.message, "Full message.");
    }

    #[test]
    fn test_matches_rule_single_word_binary() {
        let content = "---\nname: test\npatterns: [rm]\n---\n";
        let rule = parse_rule(content, "test").unwrap();
        assert!(matches_rule(&rule, Some("rm"), "rm file.txt"));
        assert!(!matches_rule(&rule, Some("rmdir"), "rmdir empty"));
    }

    #[test]
    fn test_matches_rule_multiple_patterns_in_one_rule() {
        let content =
            "---\nname: test\npatterns: [\"chmod -R 777\", \"chmod 777\"]\naction: warn\n---\n";
        let rule = parse_rule(content, "test").unwrap();
        assert_eq!(rule.patterns.len(), 2);
        assert!(matches_rule(&rule, Some("chmod"), "chmod -R 777 /tmp"));
        assert!(matches_rule(&rule, Some("chmod"), "chmod 777 /tmp"));
        assert!(!matches_rule(&rule, Some("chmod"), "chmod 755 /tmp"));
    }

    #[test]
    fn test_matches_rule_multi_word_prefix() {
        let content = "---\nname: test\npatterns: [\"git reset --hard\"]\n---\n";
        let rule = parse_rule(content, "test").unwrap();
        assert!(matches_rule(&rule, Some("git"), "git reset --hard HEAD~1"));
        assert!(!matches_rule(&rule, Some("git"), "git reset --soft HEAD"));
    }

    #[test]
    fn test_matches_rule_raw_mode_word_boundary() {
        let content = "---\nname: test\npatterns: [rm]\n---\n";
        let rule = parse_rule(content, "test").unwrap();
        // Raw mode: None for binary
        assert!(matches_rule(&rule, None, "rm file.txt"));
        assert!(matches_rule(&rule, None, "sudo rm file.txt"));
        assert!(matches_rule(&rule, None, "/usr/bin/rm file.txt"));
        // Should NOT match substrings
        assert!(!matches_rule(&rule, None, "trim file.txt"));
        assert!(!matches_rule(&rule, None, "farm --harvest"));
    }

    #[test]
    fn test_should_apply_env_var_opt_out() {
        let content = "---\nname: test\npatterns: [rm]\nenv_var: RTK_TEST_VAR\n---\n";
        let rule = parse_rule(content, "test").unwrap();

        // No env var set → applies (opt-out model)
        assert!(rule.should_apply());

        // Set to "0" → disabled
        std::env::set_var("RTK_TEST_VAR", "0");
        assert!(!rule.should_apply());

        // Set to "false" → disabled
        std::env::set_var("RTK_TEST_VAR", "false");
        assert!(!rule.should_apply());

        // Set to "1" → enabled
        std::env::set_var("RTK_TEST_VAR", "1");
        assert!(rule.should_apply());

        std::env::remove_var("RTK_TEST_VAR");
    }

    #[test]
    fn test_should_apply_when_always() {
        let content = "---\nname: test\nwhen: always\n---\n";
        let rule = parse_rule(content, "test").unwrap();
        assert!(rule.should_apply());
    }

    #[test]
    fn test_load_all_includes_builtins() {
        let rules = load_all();
        assert!(
            rules.len() >= 11,
            "Should have at least 11 built-in rules, got {}",
            rules.len()
        );
        // Check specific built-in names
        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"rm-to-trash"));
        assert!(names.contains(&"block-cat"));
        assert!(names.contains(&"git-reset-hard"));
    }

    #[test]
    fn test_check_when_always() {
        assert!(check_when("always"));
        assert!(check_when(""));
    }

    #[test]
    fn test_check_when_builtin_predicate() {
        // has_unstaged_changes is registered - should not panic
        let _ = check_when("has_unstaged_changes");
    }

    #[test]
    fn test_check_when_bash_fallback() {
        assert!(check_when("true"));
        assert!(!check_when("false"));
    }

    #[test]
    fn test_try_remap_no_match() {
        // "ls" is not a registered remap alias
        assert!(try_remap("ls -la").is_none());
    }

    // Note: try_remap with a match requires user-defined rules in discovery dirs,
    // which is tested in E2E tests rather than unit tests.

    #[test]
    fn test_rule_override_by_name() {
        // Simulate: builtin rule overridden by user rule with same name
        let mut rules_by_name: BTreeMap<String, Rule> = BTreeMap::new();

        let builtin = parse_rule(
            "---\nname: rm-to-trash\npatterns: [rm]\naction: trash\n---\nBuiltin message.",
            "builtin",
        )
        .unwrap();
        rules_by_name.insert(builtin.name.clone(), builtin);

        // User override: same name, different action
        let user_rule = parse_rule(
            "---\nname: rm-to-trash\npatterns: [rm]\naction: block\n---\nUser blocked rm.",
            "~/.config/rtk/rtk.safety.rm-to-trash.md",
        )
        .unwrap();
        rules_by_name.insert(user_rule.name.clone(), user_rule);

        let rules: Vec<Rule> = rules_by_name.into_values().collect();
        assert_eq!(rules.len(), 1); // Overridden, not duplicated
        assert_eq!(rules[0].action, "block"); // User's action wins
        assert_eq!(rules[0].message, "User blocked rm."); // User's message wins
    }

    #[test]
    fn test_rule_disabled_override_removes() {
        // Simulate: user disables a builtin rule
        let mut rules_by_name: BTreeMap<String, Rule> = BTreeMap::new();

        let builtin = parse_rule(
            "---\nname: block-cat\npatterns: [cat]\naction: suggest_tool\n---\nUse Read.",
            "builtin",
        )
        .unwrap();
        rules_by_name.insert(builtin.name.clone(), builtin);

        // User disables it
        let disabled = parse_rule(
            "---\nname: block-cat\nenabled: false\n---\nDisabled by user.",
            "~/.config/rtk/rtk.safety.block-cat.md",
        )
        .unwrap();
        assert!(!disabled.enabled);

        // The load_all logic: enabled=false removes from map
        if !disabled.enabled {
            rules_by_name.remove(&disabled.name);
        }

        assert!(rules_by_name.is_empty()); // Rule removed
    }

    #[test]
    fn test_all_builtin_rules_parse_successfully() {
        for (i, content) in DEFAULT_RULES.iter().enumerate() {
            let result = parse_rule(content, "builtin");
            assert!(
                result.is_ok(),
                "Built-in rule #{} failed to parse: {:?}",
                i,
                result.err()
            );
            let rule = result.unwrap();
            assert!(!rule.name.is_empty(), "Rule #{} has empty name", i);
            assert!(
                rule.enabled,
                "Rule #{} ({}) should be enabled",
                i, rule.name
            );
        }
    }

    #[test]
    fn test_all_builtin_rules_have_patterns() {
        for content in DEFAULT_RULES {
            let rule = parse_rule(content, "builtin").unwrap();
            assert!(
                !rule.patterns.is_empty(),
                "Rule '{}' has no patterns",
                rule.name
            );
        }
    }

    // === Error Robustness Tests ===

    #[test]
    fn test_parse_rule_empty_string() {
        assert!(parse_rule("", "test").is_err());
    }

    #[test]
    fn test_parse_rule_binary_garbage() {
        assert!(parse_rule("\x00\x01\x02 garbage", "test").is_err());
    }

    #[test]
    fn test_parse_rule_valid_frontmatter_invalid_yaml() {
        let content = "---\n: : : not valid yaml\n---\nbody";
        assert!(parse_rule(content, "test").is_err());
    }

    #[test]
    fn test_parse_rule_missing_name_field() {
        // YAML without required 'name' field
        let content = "---\npatterns: [rm]\n---\nbody";
        assert!(parse_rule(content, "test").is_err());
    }

    #[test]
    fn test_parse_rule_only_frontmatter_delimiters() {
        let content = "---\n---\n";
        // Empty YAML → missing name → error
        assert!(parse_rule(content, "test").is_err());
    }

    #[test]
    fn test_parse_rule_extra_fields_ignored() {
        // Unknown fields in YAML should be silently ignored (serde default)
        let content = "---\nname: test\nunknown_field: 42\nextra: true\n---\nbody";
        let rule = parse_rule(content, "test");
        assert!(
            rule.is_ok(),
            "Unknown fields should be ignored, got: {:?}",
            rule.err()
        );
        assert_eq!(rule.unwrap().name, "test");
    }

    #[test]
    fn test_check_when_nonexistent_command() {
        // A nonsense bash command should return false (not panic)
        assert!(!check_when("totally_nonexistent_command_xyz_12345"));
    }

    #[test]
    fn test_try_remap_empty_string() {
        assert!(try_remap("").is_none());
    }

    #[test]
    fn test_try_remap_whitespace_only() {
        assert!(try_remap("   ").is_none());
    }

    #[test]
    fn test_matches_rule_empty_patterns() {
        let content = "---\nname: no-patterns\n---\n";
        let rule = parse_rule(content, "test").unwrap();
        assert!(!matches_rule(&rule, Some("rm"), "rm file"));
        assert!(!matches_rule(&rule, None, "rm file"));
    }

    // === Precedence Chain Tests ===

    #[test]
    fn test_full_precedence_chain_builtin_global_project() {
        // Simulates the full load_all() precedence: builtin → global → project
        let mut rules_by_name: BTreeMap<String, Rule> = BTreeMap::new();

        // 1. Builtin (lowest priority): action=trash
        let builtin = parse_rule(
            "---\nname: rm-to-trash\npatterns: [rm]\naction: trash\n---\nBuiltin.",
            "builtin",
        )
        .unwrap();
        rules_by_name.insert(builtin.name.clone(), builtin);

        // 2. Global user file (~/.config/rtk/): action=warn (user edited the exported file)
        let global = parse_rule(
            "---\nname: rm-to-trash\npatterns: [rm]\naction: warn\n---\nGlobal user override.",
            "~/.config/rtk/rtk.safety.rm-to-trash.md",
        )
        .unwrap();
        rules_by_name.insert(global.name.clone(), global);

        // 3. Project-local (.rtk/): action=block (project-specific)
        let project = parse_rule(
            "---\nname: rm-to-trash\npatterns: [rm]\naction: block\n---\nProject override.",
            "/project/.rtk/rtk.safety.rm-to-trash.md",
        )
        .unwrap();
        rules_by_name.insert(project.name.clone(), project);

        let rules: Vec<Rule> = rules_by_name.into_values().collect();
        assert_eq!(rules.len(), 1, "Should be 1 rule after all overrides");
        assert_eq!(rules[0].action, "block", "Project-local should win");
        assert_eq!(rules[0].source, "/project/.rtk/rtk.safety.rm-to-trash.md");
    }

    #[test]
    fn test_user_edited_export_overrides_builtin() {
        // User exports builtins then edits one: edited file should override compiled builtin
        let mut rules_by_name: BTreeMap<String, Rule> = BTreeMap::new();

        // Compiled builtin
        let builtin = parse_rule(
            "---\nname: block-cat\npatterns: [cat]\naction: suggest_tool\nredirect: Read\n---\nBuiltin.",
            "builtin",
        )
        .unwrap();
        rules_by_name.insert(builtin.name.clone(), builtin);

        // User-edited export: changed redirect
        let edited = parse_rule(
            "---\nname: block-cat\npatterns: [cat]\naction: suggest_tool\nredirect: \"Read (with limit=50)\"\n---\nUser customized.",
            "~/.config/rtk/rtk.safety.block-cat.md",
        )
        .unwrap();
        rules_by_name.insert(edited.name.clone(), edited);

        let rules: Vec<Rule> = rules_by_name.into_values().collect();
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].redirect.as_deref(),
            Some("Read (with limit=50)"),
            "User-edited redirect should win"
        );
        assert!(rules[0].source.contains(".config/rtk/"));
    }

    #[test]
    fn test_project_local_disable_overrides_global_and_builtin() {
        // Project disables a rule that exists both in builtins and global
        let mut rules_by_name: BTreeMap<String, Rule> = BTreeMap::new();

        // Builtin
        let builtin = parse_rule(
            "---\nname: block-sed\npatterns: [sed]\naction: suggest_tool\n---\nBuiltin.",
            "builtin",
        )
        .unwrap();
        rules_by_name.insert(builtin.name.clone(), builtin);

        // Global user file (same as builtin, maybe exported)
        let global = parse_rule(
            "---\nname: block-sed\npatterns: [sed]\naction: suggest_tool\n---\nGlobal.",
            "~/.config/rtk/rtk.safety.block-sed.md",
        )
        .unwrap();
        rules_by_name.insert(global.name.clone(), global);

        // Project-local disables it
        let disabled = parse_rule(
            "---\nname: block-sed\nenabled: false\n---\nDisabled for this project.",
            "/project/.rtk/rtk.safety.block-sed.md",
        )
        .unwrap();
        if !disabled.enabled {
            rules_by_name.remove(&disabled.name);
        }

        assert!(
            rules_by_name.is_empty(),
            "Project-local disable should remove rule entirely"
        );
    }

    // === Global Option Stripping (PR #99 parity) ===
    // Table-driven: (input, expected_output) pairs covering git, cargo, docker, kubectl.

    #[test]
    fn test_strip_global_options() {
        let cases: &[(&str, &str)] = &[
            // Git: single flags
            ("git --no-pager status", "git status"),
            ("git -C /path/to/project status", "git status"),
            ("git -c core.autocrlf=true diff", "git diff"),
            ("git --git-dir=/path/.git status", "git status"),
            ("git --no-optional-locks status", "git status"),
            ("git --bare log --oneline", "git log --oneline"),
            ("git --literal-pathspecs add .", "git add ."),
            // Git: multiple globals stacked
            (
                "git -C /path --no-pager --no-optional-locks reset --hard",
                "git reset --hard",
            ),
            // Git: subcommand flags preserved (not stripped)
            ("git reset --hard HEAD~1", "git reset --hard HEAD~1"),
            ("git checkout --force main", "git checkout --force main"),
            // Git: no globals (identity)
            ("git status", "git status"),
            ("git log --oneline -10", "git log --oneline -10"),
            // Cargo: toolchain prefix
            ("cargo +nightly test", "cargo test"),
            ("cargo +stable build --release", "cargo build --release"),
            ("cargo test", "cargo test"), // no prefix (identity)
            // Docker: global flags
            ("docker --context prod ps", "docker ps"),
            ("docker -H tcp://host:2375 images", "docker images"),
            ("docker --config /tmp/.docker run hello", "docker run hello"),
            ("docker ps", "docker ps"), // no globals (identity)
            // Kubectl: global flags
            ("kubectl -n kube-system get pods", "kubectl get pods"),
            (
                "kubectl --context prod --namespace default describe pod foo",
                "kubectl describe pod foo",
            ),
            ("kubectl --kubeconfig=/path get svc", "kubectl get svc"),
            ("kubectl get pods", "kubectl get pods"), // no globals (identity)
            // Non-matching commands (identity)
            ("rm -rf /tmp/foo", "rm -rf /tmp/foo"),
            ("cat file.txt", "cat file.txt"),
            ("echo hello", "echo hello"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                strip_global_options(input),
                *expected,
                "strip_global_options({input:?})"
            );
        }
    }

    // === Rule Matching with Global Options (PR #99 parity) ===
    // Multi-word safety patterns must match even with global options inserted.

    #[test]
    fn test_matches_rule_with_global_options() {
        let cases: &[(&str, &str, bool)] = &[
            // (pattern, full_cmd, expected_match)
            ("git reset --hard", "git --no-pager reset --hard HEAD", true),
            ("git reset --hard", "git -C /path reset --hard", true),
            (
                "git reset --hard",
                "git -C /p --no-pager --no-optional-locks reset --hard",
                true,
            ),
            ("git checkout .", "git -C /project checkout .", true),
            (
                "git checkout --",
                "git --no-pager checkout -- file.txt",
                true,
            ),
            (
                "git clean -fd",
                "git -C /path --no-pager --no-optional-locks clean -fd",
                true,
            ),
            ("git stash drop", "git --no-pager stash drop", true),
            // No globals: direct match still works
            ("git reset --hard", "git reset --hard HEAD~1", true),
            ("git checkout .", "git checkout .", true),
            // Non-matching
            ("git reset --hard", "git reset --soft HEAD", false),
            ("git checkout .", "git checkout main", false),
        ];
        for (pattern, full_cmd, expected) in cases {
            let yaml = format!("---\nname: test\npatterns: [\"{pattern}\"]\n---\n");
            let rule = parse_rule(&yaml, "test").unwrap();
            let binary = full_cmd.split_whitespace().next();
            assert_eq!(
                matches_rule(&rule, binary, full_cmd),
                *expected,
                "matches_rule(pat={pattern:?}, cmd={full_cmd:?})"
            );
        }
    }

    #[test]
    fn test_matches_rule_empty_command() {
        let content = "---\nname: test\npatterns: [rm]\n---\n";
        let rule = parse_rule(content, "test").unwrap();
        // Parsed mode: binary match is independent of full_cmd
        assert!(matches_rule(&rule, Some("rm"), ""));
        // Raw mode: empty string has no words → no match
        assert!(!matches_rule(&rule, None, ""));
    }
}
