use super::constants::{CONFIG_DIR, OPENCODE_SUBDIR};
use super::permissions::PermissionVerdict;
use crate::core::user_dirs;
use crate::discover::lexer::{contains_unattestable_construct, split_for_permissions};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Allow,
    Ask,
    Deny,
}

impl Action {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rule {
    pub(crate) permission: String,
    pub(crate) pattern: String,
    pub(crate) action: Action,
}

pub(crate) fn wildcard_match(text: &str, pattern: &str) -> bool {
    let text = text.replace('\\', "/");
    let pattern = pattern.replace('\\', "/");

    if let Some(base) = pattern.strip_suffix(" *")
        && glob_match(&text, base)
    {
        return true;
    }
    glob_match(&text, &pattern)
}

fn glob_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let (mut ti, mut pi) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || chars_eq(p[pi], t[ti])) {
            ti += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            pi += 1;
            star = Some((pi, ti));
        } else if let Some((resume_pi, resume_ti)) = star {
            pi = resume_pi;
            ti = resume_ti + 1;
            star = Some((resume_pi, resume_ti + 1));
        } else {
            return false;
        }
    }

    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(windows)]
fn chars_eq(a: char, b: char) -> bool {
    a.eq_ignore_ascii_case(&b) || a.to_lowercase().eq(b.to_lowercase())
}

#[cfg(not(windows))]
fn chars_eq(a: char, b: char) -> bool {
    a == b
}

pub(crate) fn evaluate(cmd: &str, rules: &[Rule]) -> Option<Action> {
    rules
        .iter()
        .rev()
        .find(|rule| wildcard_match("bash", &rule.permission) && wildcard_match(cmd, &rule.pattern))
        .map(|rule| rule.action)
}

pub(crate) fn check_command_with_opencode_rules(cmd: &str, rules: &[Rule]) -> PermissionVerdict {
    // `split_for_permissions` returns trimmed, non-empty segments only.
    let actions: Vec<Option<Action>> = split_for_permissions(cmd)
        .iter()
        .map(|segment| evaluate(segment, rules))
        .collect();

    if actions.contains(&Some(Action::Deny)) {
        return PermissionVerdict::Deny;
    }

    if contains_unattestable_construct(cmd) {
        return PermissionVerdict::Ask;
    }

    if actions.is_empty() {
        return PermissionVerdict::Default;
    }

    if actions.iter().all(|a| *a == Some(Action::Allow)) {
        return PermissionVerdict::Allow;
    }

    if actions.contains(&Some(Action::Ask)) {
        return PermissionVerdict::Ask;
    }

    PermissionVerdict::Default
}

pub(crate) fn load_opencode_rules(agent: Option<&str>) -> Vec<Rule> {
    let mut rules = Vec::new();
    for config in opencode_configs() {
        if let Some(permission) = config.get("permission") {
            append_rules(permission, &mut rules);
        }
        if let Some(block) =
            agent.and_then(|name| config.pointer(&format!("/agent/{name}/permission")))
        {
            append_rules(block, &mut rules);
        }
    }
    rules
}

fn append_rules(permission: &Value, rules: &mut Vec<Rule>) {
    let Some(entries) = permission.as_object() else {
        return;
    };
    for (name, value) in entries {
        match value {
            Value::String(action) => {
                if let Some(action) = Action::parse(action) {
                    rules.push(Rule {
                        permission: name.clone(),
                        pattern: "*".to_string(),
                        action,
                    });
                }
            }
            Value::Object(patterns) => {
                for (pattern, action) in patterns {
                    if let Some(action) = action.as_str().and_then(Action::parse) {
                        rules.push(Rule {
                            permission: name.clone(),
                            pattern: pattern.clone(),
                            action,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

fn opencode_configs() -> Vec<Value> {
    let mut configs = Vec::new();

    if let Some(path) = user_dirs::env_path("OPENCODE_CONFIG") {
        if let Some(v) = read_config(Path::new(&path)) {
            configs.push(v);
        }
    } else if let Some(dir) = global_opencode_dir() {
        configs.extend(read_first_config(&dir));
    }

    if let Some(root) = project_root_with_config() {
        configs.extend(read_first_config(&root));
    }

    configs
}

fn global_opencode_dir() -> Option<PathBuf> {
    Some(user_dirs::home()?.join(CONFIG_DIR).join(OPENCODE_SUBDIR))
}

fn project_root_with_config() -> Option<PathBuf> {
    let start = user_dirs::current_dir().ok()?;
    user_dirs::ancestors(&start)
        .find(|dir| CONFIG_NAMES.iter().any(|name| dir.join(name).is_file()))
        .map(Path::to_path_buf)
}

const CONFIG_NAMES: [&str; 2] = ["opencode.json", "opencode.jsonc"];

fn read_first_config(dir: &Path) -> Option<Value> {
    CONFIG_NAMES
        .iter()
        .find_map(|name| read_config(&dir.join(name)))
}

fn read_config(path: &Path) -> Option<Value> {
    let content = std::fs::read_to_string(path).ok()?;
    match crate::core::utils::from_json_str::<Value>(&content) {
        Ok(v) => Some(v),
        Err(_) => {
            eprintln!(
                "[rtk] warning: failed to parse permissions from {}",
                path.display()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, action: Action) -> Rule {
        Rule {
            permission: "bash".to_string(),
            pattern: pattern.to_string(),
            action,
        }
    }

    #[test]
    fn trailing_space_star_also_matches_the_bare_command() {
        assert!(wildcard_match("ls", "ls *"));
        assert!(wildcard_match("ls -la", "ls *"));
        assert!(!wildcard_match("lsof", "ls *"));
    }

    #[test]
    fn wildcards_match_the_way_opencode_compiles_them() {
        assert!(wildcard_match("anything at all", "*"));
        assert!(wildcard_match("git status", "git status"));
        assert!(!wildcard_match("git status --short", "git status"));
        assert!(wildcard_match("git push origin", "git * origin"));
        assert!(wildcard_match("cat a", "cat ?"));
        assert!(!wildcard_match("cat ab", "cat ?"));
        assert!(wildcard_match("git status\nrm -rf /", "git status*"));
    }

    #[test]
    fn backslashes_are_read_as_forward_slashes_on_both_sides() {
        assert!(wildcard_match(r"cat src\main.rs", "cat src/main.rs"));
        assert!(wildcard_match("cat src/main.rs", r"cat src\main.rs"));
    }

    #[test]
    fn the_last_matching_rule_wins() {
        let broad_last = [
            rule("git push *", Action::Deny),
            rule("git *", Action::Allow),
        ];
        assert_eq!(
            evaluate("git push --force", &broad_last),
            Some(Action::Allow)
        );

        let narrow_last = [
            rule("git *", Action::Allow),
            rule("git push *", Action::Deny),
        ];
        assert_eq!(
            evaluate("git push --force", &narrow_last),
            Some(Action::Deny)
        );
    }

    #[test]
    fn an_unmatched_command_reports_no_rule_rather_than_ask() {
        assert_eq!(
            evaluate("cargo test", &[rule("git *", Action::Allow)]),
            None
        );
    }

    #[test]
    fn a_rule_for_another_tool_never_matches_bash() {
        let edit_only = [Rule {
            permission: "edit".to_string(),
            pattern: "*".to_string(),
            action: Action::Allow,
        }];
        assert_eq!(evaluate("git status", &edit_only), None);
    }

    #[test]
    fn a_wildcard_permission_axis_covers_bash() {
        let any_tool = [Rule {
            permission: "*".to_string(),
            pattern: "*".to_string(),
            action: Action::Deny,
        }];
        assert_eq!(evaluate("git status", &any_tool), Some(Action::Deny));
    }

    #[test]
    fn the_4195_policy_allows_the_users_command_and_denies_the_rest() {
        let rules = [
            rule("*", Action::Deny),
            rule("git status", Action::Allow),
            rule("git status *", Action::Allow),
        ];
        assert_eq!(
            check_command_with_opencode_rules("git status", &rules),
            PermissionVerdict::Allow
        );
        assert_eq!(
            check_command_with_opencode_rules("git status --short", &rules),
            PermissionVerdict::Allow
        );
        assert_eq!(
            check_command_with_opencode_rules("rm -rf /", &rules),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn one_allowed_segment_never_carries_a_denied_neighbour() {
        let rules = [
            rule("git status", Action::Allow),
            rule("rm *", Action::Deny),
        ];
        assert_eq!(
            check_command_with_opencode_rules("git status && rm -rf /", &rules),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn every_segment_must_be_allowed_for_the_chain_to_be_allowed() {
        let rules = [rule("git status", Action::Allow)];
        assert_eq!(
            check_command_with_opencode_rules("git status && cargo test", &rules),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn no_rules_at_all_is_never_an_allow() {
        assert_eq!(
            check_command_with_opencode_rules("git status", &[]),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn an_unattestable_construct_is_never_auto_allowed() {
        let rules = [rule("*", Action::Allow)];
        assert_eq!(
            check_command_with_opencode_rules("echo $(rm -rf /)", &rules),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn an_ask_rule_reports_ask_not_default() {
        let rules = [rule("git push *", Action::Ask)];
        assert_eq!(
            check_command_with_opencode_rules("git push origin main", &rules),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn a_denied_segment_pre_empts_the_unattestable_ask() {
        // Deny is checked before the unattestable gate: a command that is both
        // must report Deny, or a deny rule could be softened to a prompt.
        let rules = [rule("rm *", Action::Deny)];
        assert_eq!(
            check_command_with_opencode_rules("echo $(date) && rm -rf /", &rules),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn config_rules_keep_the_order_the_file_declares() {
        let config: Value = serde_json::from_str(
            r#"{ "bash": { "git push *": "deny", "git *": "allow", "aaa": "ask" } }"#,
        )
        .expect("valid json");
        let mut rules = Vec::new();
        append_rules(&config, &mut rules);
        assert_eq!(
            rules.iter().map(|r| r.pattern.as_str()).collect::<Vec<_>>(),
            vec!["git push *", "git *", "aaa"],
            "declaration order is load-bearing: the last match wins"
        );
    }

    #[test]
    fn a_string_valued_permission_becomes_one_catch_all_rule() {
        let config: Value = serde_json::from_str(r#"{ "bash": "deny" }"#).expect("valid json");
        let mut rules = Vec::new();
        append_rules(&config, &mut rules);
        assert_eq!(rules, vec![rule("*", Action::Deny)]);
    }

    #[test]
    fn an_unknown_action_is_dropped_rather_than_guessed() {
        let config: Value =
            serde_json::from_str(r#"{ "bash": { "git *": "maybe" } }"#).expect("valid json");
        let mut rules = Vec::new();
        append_rules(&config, &mut rules);
        assert!(rules.is_empty());
    }

    // --- Config discovery (runs against the test scratch, never the
    // developer's own ~/.config/opencode or working directory) ---

    use crate::core::test_isolation;
    use crate::core::user_env;

    fn write_json(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().expect("config path has a parent"))
            .expect("create config directory");
        std::fs::write(path, content).expect("write config file");
    }

    #[test]
    fn a_project_config_is_found_from_a_subdirectory() {
        let tmp = test_isolation::tempdir();
        let root = tmp.path().join("project");
        let sub = root.join("src").join("deep");
        std::fs::create_dir_all(&sub).expect("create subdirectory");
        write_json(
            &root.join("opencode.json"),
            r#"{ "permission": { "bash": "deny" } }"#,
        );

        test_isolation::with_root(&tmp.path().join("home"), || {
            let _entered = test_isolation::enter(&sub);
            let rules = load_opencode_rules(None);
            assert_eq!(rules, vec![rule("*", Action::Deny)]);
        });
    }

    #[test]
    fn the_project_rule_wins_because_it_loads_after_the_global_one() {
        let tmp = test_isolation::tempdir();
        let home = tmp.path().join("home");
        write_json(
            &home
                .join(CONFIG_DIR)
                .join(OPENCODE_SUBDIR)
                .join("opencode.json"),
            r#"{ "permission": { "bash": { "git *": "deny" } } }"#,
        );
        let project = tmp.path().join("project");
        write_json(
            &project.join("opencode.json"),
            r#"{ "permission": { "bash": { "git *": "allow" } } }"#,
        );

        test_isolation::with_root(&home, || {
            let _entered = test_isolation::enter(&project);
            let rules = load_opencode_rules(None);
            assert_eq!(
                rules,
                vec![rule("git *", Action::Deny), rule("git *", Action::Allow)],
                "global loads first, project after — last match wins"
            );
            assert_eq!(
                check_command_with_opencode_rules("git status", &rules),
                PermissionVerdict::Allow
            );
        });
    }

    #[test]
    fn opencode_config_env_is_read_instead_of_the_global_file() {
        let tmp = test_isolation::tempdir();
        let home = tmp.path().join("home");
        write_json(
            &home
                .join(CONFIG_DIR)
                .join(OPENCODE_SUBDIR)
                .join("opencode.json"),
            r#"{ "permission": { "bash": { "git *": "deny" } } }"#,
        );
        let pointed = tmp.path().join("elsewhere").join("my-config.json");
        write_json(
            &pointed,
            r#"{ "permission": { "bash": { "git *": "ask" } } }"#,
        );

        test_isolation::with_root(&home, || {
            user_env::with_path("OPENCODE_CONFIG", Some(&pointed), || {
                let rules = load_opencode_rules(None);
                assert_eq!(
                    rules,
                    vec![rule("git *", Action::Ask)],
                    "rtk reads OPENCODE_CONFIG instead of the global file"
                );
            });
        });
    }

    #[test]
    fn the_nearest_project_config_is_the_one_read() {
        let tmp = test_isolation::tempdir();
        let outer = tmp.path().join("outer");
        let inner = outer.join("inner");
        let sub = inner.join("src");
        std::fs::create_dir_all(&sub).expect("create subdirectory");
        write_json(
            &outer.join("opencode.json"),
            r#"{ "permission": { "bash": "deny" } }"#,
        );
        write_json(
            &inner.join("opencode.json"),
            r#"{ "permission": { "bash": "allow" } }"#,
        );

        test_isolation::with_root(&tmp.path().join("home"), || {
            let _entered = test_isolation::enter(&sub);
            let rules = load_opencode_rules(None);
            assert_eq!(rules, vec![rule("*", Action::Allow)]);
        });
    }

    #[test]
    fn json_is_read_before_jsonc_in_the_same_directory() {
        let tmp = test_isolation::tempdir();
        let project = tmp.path().join("project");
        write_json(
            &project.join("opencode.json"),
            r#"{ "permission": { "bash": { "git *": "allow" } } }"#,
        );
        write_json(
            &project.join("opencode.jsonc"),
            r#"{ "permission": { "bash": { "git *": "deny" } } }"#,
        );

        test_isolation::with_root(&tmp.path().join("home"), || {
            let _entered = test_isolation::enter(&project);
            let rules = load_opencode_rules(None);
            assert_eq!(rules, vec![rule("git *", Action::Allow)]);
        });
    }

    /// `check_command_for_agent` must route `Host::OpenCode` through this
    /// module's rules, not another host's (empty) rule source, where every
    /// verdict would collapse to `Default`.
    #[test]
    fn check_command_for_agent_consults_opencode_rules() {
        use crate::hooks::permissions::{Host, check_command_for_agent};

        let tmp = test_isolation::tempdir();
        let project = tmp.path().join("project");
        write_json(
            &project.join("opencode.json"),
            r#"{ "permission": { "bash": "deny" } }"#,
        );

        test_isolation::with_root(&tmp.path().join("home"), || {
            let _entered = test_isolation::enter(&project);
            assert_eq!(
                check_command_for_agent("git status", Host::OpenCode, None),
                PermissionVerdict::Deny
            );
        });
    }

    #[test]
    fn agent_rules_load_after_root_rules_so_they_win_ties() {
        let tmp = test_isolation::tempdir();
        let project = tmp.path().join("project");
        write_json(
            &project.join("opencode.json"),
            r#"{
                "permission": { "bash": { "git *": "deny" } },
                "agent": {
                    "staged-review": { "permission": { "bash": { "git *": "allow" } } }
                }
            }"#,
        );

        test_isolation::with_root(&tmp.path().join("home"), || {
            let _entered = test_isolation::enter(&project);
            let rules = load_opencode_rules(Some("staged-review"));
            assert_eq!(
                rules,
                vec![rule("git *", Action::Deny), rule("git *", Action::Allow)],
                "the agent block appends after root, matching OpenCode's resolution"
            );

            let without_agent = load_opencode_rules(None);
            assert_eq!(without_agent, vec![rule("git *", Action::Deny)]);
        });
    }
}
