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

/// Every config's rules, in the order OpenCode resolves them.
///
/// Two passes, not one loop over both: OpenCode applies the root rules of
/// *every* file first, and only then the agent rules of every file. Reading
/// them per file let a project root allow outrank a global agent ask, which is
/// the opposite of what OpenCode does — and an out-ranked ask is an ask the
/// rewrite would silence.
pub(crate) fn load_opencode_rules(agent: Option<&str>) -> Vec<Rule> {
    rules_from_configs(&opencode_configs(), agent)
}

/// The rules OpenCode resolves for `agent`, or `None` when a legacy key means a
/// different rule to 1.x than to 2.x (see [`legacy_keys_depend_on_the_generation`]).
pub(crate) fn load_judgeable_opencode_rules(agent: Option<&str>) -> Option<Vec<Rule>> {
    let configs = opencode_configs();
    (!legacy_keys_depend_on_the_generation(&configs, agent))
        .then(|| rules_from_configs(&configs, agent))
}

fn rules_from_configs(configs: &[Value], agent: Option<&str>) -> Vec<Rule> {
    let mut rules = Vec::new();
    for config in configs {
        append_config_rules(config, None, &mut rules);
    }
    if let Some(agent) = agent {
        for config in configs {
            append_config_rules(config, Some(agent), &mut rules);
        }
    }
    rules
}

/// OpenCode 2.x renamed the shell tool to `shell` and spelled agent blocks
/// `agents.<name>`. Rules are stored under the axis rtk's evaluator matches on,
/// so both spellings of the tool land on `bash`.
fn permission_axis(name: &str) -> &str {
    if name == "shell" { "bash" } else { name }
}

/// Does a legacy `permission` map hold a shell rule only one OpenCode
/// generation applies?
///
/// 1.x applies a legacy key that globs `bash`. 2.x renames the key `bash` to
/// `shell` and applies any key that globs `shell`. The two disagree on keys
/// like `shell` and `sh*` (2.x only) and `b*` (1.x only). rtk cannot tell which
/// generation it is serving, and judging the rewrite against such a rule
/// silences an ask on one of them, so the caller answers `{}`.
fn legacy_keys_depend_on_the_generation(configs: &[Value], agent: Option<&str>) -> bool {
    let generations_disagree = |key: &str| {
        let applied_by_1x = wildcard_match("bash", key);
        let applied_by_2x = key == "bash" || wildcard_match("shell", key);
        applied_by_1x != applied_by_2x
    };
    let mut blocks = vec!["/permission".to_string()];
    if let Some(name) = agent {
        blocks.push(format!("/agent/{name}/permission"));
        blocks.push(format!("/agents/{name}/permission"));
    }
    configs.iter().any(|config| {
        blocks
            .iter()
            .filter_map(|block| config.pointer(block))
            .filter_map(Value::as_object)
            .any(|map| map.keys().any(|key| generations_disagree(key)))
    })
}

/// One config's rules, in the order OpenCode resolves them: the legacy
/// `permission` map first, then the 2.x `permissions` list, regardless of which
/// the file declares first — a legacy allow plus a list deny has to come out
/// denied. The agent block reads the same way, in both of the spellings 1.x and
/// 2.x used, and loads last.
fn append_config_rules(config: &Value, agent: Option<&str>, rules: &mut Vec<Rule>) {
    let prefix = agent
        .map(|name| format!("/agents/{name}"))
        .unwrap_or_default();
    let legacy = config.pointer(&format!("{prefix}/permission"));
    let modern = config.pointer(&format!("{prefix}/permissions"));
    // 1.x spelled the agent block `agent.<name>`; 2.x spells it
    // `agents.<name>`. Both are legacy maps, so both precede the list.
    let legacy_agent = agent.and_then(|name| config.pointer(&format!("/agent/{name}/permission")));

    if let Some(legacy) = legacy {
        append_rules(legacy, rules);
    }
    if let Some(legacy_agent) = legacy_agent {
        append_rules(legacy_agent, rules);
    }
    append_permission_list(modern, rules);
}

/// Does this 2.x list rule's action cover the shell tool?
///
/// OpenCode matches the action as a glob against `shell`, so `"sh*"` and `"*"`
/// cover it while `"edit"` does not. `"bash"` does not either: in the 2.x list
/// OpenCode applies no rule whose action only names the 1.x tool, and counting
/// one would let an allow it ignores outrank an ask it applies.
fn action_covers_shell(action: &str) -> bool {
    wildcard_match("shell", action)
}

/// OpenCode 2.x's native rule list: `{ "action": "shell", "resource": "git *",
/// "effect": "deny" }`. Entries for other tools cannot match a shell command
/// and are skipped.
fn append_permission_list(list: Option<&Value>, rules: &mut Vec<Rule>) {
    let Some(entries) = list.and_then(Value::as_array) else {
        return;
    };
    for entry in entries {
        let action = entry
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !action_covers_shell(action) {
            continue;
        }
        let Some(effect) = entry
            .get("effect")
            .and_then(Value::as_str)
            .and_then(Action::parse)
        else {
            continue;
        };
        rules.push(Rule {
            permission: "bash".to_string(),
            pattern: entry
                .get("resource")
                .and_then(Value::as_str)
                .unwrap_or("*")
                .to_string(),
            action: effect,
        });
    }
}

fn append_rules(permission: &Value, rules: &mut Vec<Rule>) {
    let Some(entries) = permission.as_object() else {
        return;
    };
    for (name, value) in entries {
        let permission = permission_axis(name).to_string();
        match value {
            Value::String(action) => {
                if let Some(action) = Action::parse(action) {
                    rules.push(Rule {
                        permission,
                        pattern: "*".to_string(),
                        action,
                    });
                }
            }
            Value::Object(patterns) => {
                for (pattern, action) in patterns {
                    if let Some(action) = action.as_str().and_then(Action::parse) {
                        rules.push(Rule {
                            permission: permission.clone(),
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

/// Does the agent's markdown file declare permission rules?
///
/// Agents can be markdown files with a YAML frontmatter rather than a config
/// entry, and OpenCode reads the rules from both. rtk reads the configs but not
/// the files, so an ask or a deny declared only in a file was invisible and the
/// rewrite silenced it. The rules themselves are not parsed — only the presence
/// of a `permission:` or `permissions:` key matters, and when one is there the
/// answer is `{}`, which is the safe direction.
///
/// An agent file with no such key keeps its rewrites, since most of them carry
/// only a prompt and a model.
fn agent_markdown_declares_rules(agent: &str) -> bool {
    let name = format!("{agent}.md");
    if name.contains('/') || name.contains('\\') {
        return true; // not a plain id, so assume rules are somewhere we cannot see
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    roots.extend(global_opencode_dir());
    if let Ok(start) = user_dirs::current_dir() {
        roots.extend(user_dirs::ancestors(&start).map(|dir| dir.join(PROJECT_OPENCODE_SUBDIR)));
    }
    let name = name.as_str();
    roots
        .iter()
        .flat_map(|root| {
            AGENT_SUBDIRS
                .iter()
                .map(move |sub| root.join(sub).join(name))
        })
        .any(|file| {
            std::fs::read_to_string(file).is_ok_and(|body| frontmatter_declares_rules(&body))
        })
}

/// Could the active agent be carrying rules rtk cannot see?
///
/// True when an agent markdown file exists and declares a `permission:` or
/// `permissions:` key. The rules themselves are not parsed; the caller answers
/// `{}`, which is the safe direction.
pub(crate) fn agent_rules_unreadable(agent: Option<&str>) -> bool {
    agent.is_some_and(agent_markdown_declares_rules)
}

/// Does this markdown frontmatter carry a `permission:` or `permissions:` key?
fn frontmatter_declares_rules(body: &str) -> bool {
    let frontmatter = body
        .trim_start_matches('\u{feff}')
        .strip_prefix("---")
        // An unclosed block still counts: a missed rule silences an ask.
        .map(|rest| rest.split_once("\n---").map_or(rest, |(yaml, _)| yaml))
        .unwrap_or_default();
    frontmatter.lines().any(|line| {
        let line = line.trim_end();
        // Top-level only: an indented key belongs to some other block.
        !line.starts_with([' ', '\t', '-'])
            && line.split_once(':').is_some_and(|(key, _)| {
                matches!(
                    key.trim().trim_matches(['"', '\'']),
                    "permission" | "permissions"
                )
            })
    })
}

/// OpenCode loads agent files from both spellings of the directory.
const AGENT_SUBDIRS: [&str; 2] = ["agents", "agent"];

/// The project-level config directory is `.opencode`, not `opencode` like the
/// one under `~/.config`.
const PROJECT_OPENCODE_SUBDIR: &str = ".opencode";

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
    // JSONC, because the `.jsonc` extension exists for comments and trailing
    // commas and OpenCode's own examples use it. Parsing either extension as
    // plain JSON dropped every rule in any file that carried a `//`.
    match crate::core::utils::from_jsonc_str::<Value>(&content) {
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
    fn a_global_agent_ask_outranks_a_project_root_allow() {
        // OpenCode reads every file's root rules first, then every file's agent
        // rules. Reading them per file let the project allow win here, which
        // silences an ask the user wrote.
        let tmp = test_isolation::tempdir();
        let home = tmp.path().join("home");
        write_json(
            &home
                .join(CONFIG_DIR)
                .join(OPENCODE_SUBDIR)
                .join("opencode.json"),
            r#"{ "agents": { "build": { "permissions": [
                   { "action": "shell", "resource": "ls *", "effect": "ask" } ] } } }"#,
        );
        let project = tmp.path().join("project");
        write_json(
            &project.join("opencode.json"),
            r#"{ "permissions": [ { "action": "shell", "resource": "*", "effect": "allow" } ] }"#,
        );

        test_isolation::with_root(&home, || {
            let _entered = test_isolation::enter(&project);
            let rules = load_opencode_rules(Some("build"));
            assert_eq!(
                rules,
                vec![rule("*", Action::Allow), rule("ls *", Action::Ask),],
                "project root allow first, global agent ask after"
            );
            assert_eq!(
                check_command_with_opencode_rules("ls -la", &rules),
                PermissionVerdict::Ask,
                "the agent ask has to survive the project allow"
            );
        });
    }

    #[test]
    fn a_legacy_agent_map_outranks_the_agent_list_in_the_same_file() {
        // Both spellings are legacy maps, so both precede the 2.x list. Reading
        // the list first let it decide over the map OpenCode would have used.
        let config: Value = serde_json::from_str(
            r#"{
                "agent": { "build": { "permission": { "bash": { "*": "allow" } } } },
                "agents": { "build": { "permissions": [
                    { "action": "shell", "resource": "ls *", "effect": "ask" } ] } }
            }"#,
        )
        .expect("valid json");
        let mut rules = Vec::new();
        append_config_rules(&config, Some("build"), &mut rules);
        assert_eq!(
            rules,
            vec![rule("*", Action::Allow), rule("ls *", Action::Ask)],
            "legacy map before the list, whichever order the file declares them"
        );
    }

    #[test]
    fn a_wildcard_action_is_a_shell_rule_too() {
        let config: Value = serde_json::from_str(
            r#"{ "permissions": [
                { "action": "sh*", "resource": "ls *", "effect": "ask" },
                { "action": "edit", "resource": "ls *", "effect": "deny" } ] }"#,
        )
        .expect("valid json");
        let mut rules = Vec::new();
        append_config_rules(&config, None, &mut rules);
        assert_eq!(
            rules,
            vec![rule("ls *", Action::Ask)],
            "sh* covers the shell tool; edit is another tool and cannot match"
        );
    }

    #[test]
    fn a_bash_action_in_the_2x_list_is_not_a_shell_rule() {
        // OpenCode 2.x ignores a list rule whose action only names the 1.x
        // tool. Counting this allow would put it after the shell ask, and the
        // rewrite would silence that ask.
        let config: Value = serde_json::from_str(
            r#"{ "permissions": [
                { "action": "shell", "resource": "ls *", "effect": "ask" },
                { "action": "bash", "resource": "*", "effect": "allow" } ] }"#,
        )
        .expect("valid json");
        let mut rules = Vec::new();
        append_config_rules(&config, None, &mut rules);
        assert_eq!(rules, vec![rule("ls *", Action::Ask)]);
        assert_eq!(
            check_command_with_opencode_rules("ls -la", &rules),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn legacy_keys_one_generation_ignores_make_the_rules_version_dependent() {
        let flagged = |config: &str, agent: Option<&str>| {
            let tmp = test_isolation::tempdir();
            let project = tmp.path().join("project");
            write_json(&project.join("opencode.json"), config);
            test_isolation::with_root(&tmp.path().join("home"), || {
                let _entered = test_isolation::enter(&project);
                legacy_keys_depend_on_the_generation(&opencode_configs(), agent)
            })
        };
        assert!(flagged(
            r#"{ "permission": { "bash": { "ls *": "ask" }, "shell": "allow" } }"#,
            None
        ));
        assert!(flagged(
            r#"{ "permission": { "sh*": { "ls *": "ask" } } }"#,
            None
        ));
        assert!(
            flagged(r#"{ "permission": { "b*": "allow" } }"#, None),
            "1.x applies it, 2.x does not"
        );
        assert!(!flagged(
            r#"{ "permission": { "bash": "ask", "*": "allow", "edit": "deny" } }"#,
            None
        ));
        let agent_map = r#"{ "agent": { "build": { "permission": { "shell": "allow" } } } }"#;
        assert!(
            flagged(agent_map, Some("build")),
            "an agent's own map counts for that agent"
        );
        assert!(!flagged(agent_map, None), "and only for that agent");
        assert!(
            flagged(
                r#"{ "agents": { "build": { "permission": { "sh*": "allow" } } } }"#,
                Some("build")
            ),
            "the 2.x agent block's legacy map counts too"
        );
        assert!(
            !flagged(
                r#"{ "permissions": [{ "action": "shell", "resource": "*", "effect": "allow" }] }"#,
                None
            ),
            "the 2.x list exists only on 2.x, so it is not version dependent"
        );
    }

    #[test]
    fn frontmatter_rule_keys_are_found_in_every_top_level_spelling() {
        let declares =
            |front: &str| frontmatter_declares_rules(&format!("---\n{front}\n---\nbody\n"));
        assert!(declares("permissions:\n  - action: shell"));
        assert!(declares("permission:\n  bash: ask"));
        assert!(declares("\"permissions\":\n  - action: shell"));
        assert!(declares("'permission': {bash: ask}"));
        assert!(declares("permissions :\n  - action: shell"));
        assert!(
            frontmatter_declares_rules("\u{feff}---\npermission: ask\n---\nbody\n"),
            "a byte-order mark before the frontmatter"
        );
        assert!(
            frontmatter_declares_rules("---\npermission:\n  bash: ask\n"),
            "a frontmatter block that is never closed"
        );
        assert!(!declares("description: x\nmodel: y"));
        assert!(
            !declares("tools:\n  permissions: x"),
            "an indented key belongs to another block"
        );
        assert!(
            !frontmatter_declares_rules("---\ndescription: x\n---\npermissions: in the body\n"),
            "the body is the prompt, not frontmatter"
        );
    }

    #[test]
    fn agent_files_are_looked_up_in_every_directory_opencode_reads() {
        let rules = "---\npermissions:\n  - action: shell\n    resource: ls *\n    effect: ask\n---\nbody\n";
        let tmp = test_isolation::tempdir();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let nested = project.join("sub").join("dir");
        std::fs::create_dir_all(&nested).expect("create nested dir");
        let write = |dir: PathBuf, name: &str| {
            std::fs::create_dir_all(&dir).expect("create agent dir");
            std::fs::write(dir.join(format!("{name}.md")), rules).expect("write agent file");
        };
        write(project.join(".opencode").join("agent"), "singular");
        write(project.join(".opencode").join("agents"), "fromancestor");
        write(
            home.join(CONFIG_DIR).join(OPENCODE_SUBDIR).join("agents"),
            "global",
        );

        test_isolation::with_root(&home, || {
            let _entered = test_isolation::enter(&nested);
            assert!(
                agent_rules_unreadable(Some("singular")),
                "the singular agent/ directory"
            );
            assert!(
                agent_rules_unreadable(Some("fromancestor")),
                "a .opencode above the cwd"
            );
            assert!(
                agent_rules_unreadable(Some("global")),
                "the global agents directory"
            );
            assert!(
                agent_rules_unreadable(Some("team/reviewer")),
                "a nested id fails closed"
            );
            assert!(!agent_rules_unreadable(Some("nowhere")));
        });
    }

    #[test]
    fn a_markdown_agent_with_rules_makes_the_rewrite_step_aside() {
        let tmp = test_isolation::tempdir();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let agents = project.join(".opencode").join("agents");
        std::fs::create_dir_all(&agents).expect("create agents directory");
        std::fs::write(
            agents.join("rtkprobe.md"),
            "---\ndescription: probes\npermissions:\n  - action: shell\n    resource: ls *\n    effect: ask\n---\nbody\n",
        )
        .expect("write agent file");

        test_isolation::with_root(&home, || {
            let _entered = test_isolation::enter(&project);
            assert!(agent_rules_unreadable(Some("rtkprobe")));
            assert!(
                !agent_rules_unreadable(Some("someotheragent")),
                "an agent with no file keeps its rewrites"
            );
            assert!(!agent_rules_unreadable(None));
        });
    }

    #[test]
    fn an_agent_file_without_a_rules_key_keeps_its_rewrites() {
        let tmp = test_isolation::tempdir();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let agents = project.join(".opencode").join("agents");
        std::fs::create_dir_all(&agents).expect("create agents directory");
        std::fs::write(
            agents.join("plainer.md"),
            "---\ndescription: just a prompt\nmodel: anthropic/claude\n---\nbody\n",
        )
        .expect("write agent file");

        test_isolation::with_root(&home, || {
            let _entered = test_isolation::enter(&project);
            assert!(!agent_rules_unreadable(Some("plainer")));
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

    #[test]
    fn jsonc_comments_and_trailing_commas_keep_the_rules() {
        let tmp = test_isolation::tempdir();
        let project = tmp.path().join("project");
        write_json(
            &project.join("opencode.jsonc"),
            r#"{
  // never push from this project
  /* and block comments too */
  "permission": {
    "bash": {
      "git push *": "deny",   // inline comment
      "ls *": "ask",
    },
  },
}"#,
        );

        test_isolation::with_root(&tmp.path().join("home"), || {
            let _entered = test_isolation::enter(&project);
            let rules = load_opencode_rules(None);
            assert_eq!(
                rules,
                vec![rule("git push *", Action::Deny), rule("ls *", Action::Ask)],
                "a comment or trailing comma must not cost a rule (#4524)"
            );
        });
    }

    #[test]
    fn jsonc_keys_that_look_like_comments_are_content() {
        let tmp = test_isolation::tempdir();
        let project = tmp.path().join("project");
        write_json(
            &project.join("opencode.jsonc"),
            r#"{ "permission": { "bash": { "//server/share": "deny", "a/*b": "ask" } } }"#,
        );

        test_isolation::with_root(&tmp.path().join("home"), || {
            let _entered = test_isolation::enter(&project);
            let rules = load_opencode_rules(None);
            assert_eq!(
                rules,
                vec![
                    rule("//server/share", Action::Deny),
                    rule("a/*b", Action::Ask)
                ],
                "comment markers inside a string are part of the pattern"
            );
        });
    }

    #[test]
    fn a_bom_and_a_leading_comment_still_parse() {
        let tmp = test_isolation::tempdir();
        let project = tmp.path().join("project");
        write_json(
            &project.join("opencode.jsonc"),
            "\u{feff}// header comment\n{ \"permission\": { \"bash\": \"deny\" } }\n",
        );

        test_isolation::with_root(&tmp.path().join("home"), || {
            let _entered = test_isolation::enter(&project);
            assert_eq!(load_opencode_rules(None), vec![rule("*", Action::Deny)]);
        });
    }

    #[test]
    fn genuinely_broken_json_is_still_rejected() {
        // Stripping comments must not turn broken input into a parse that
        // succeeds and yields the wrong rules.
        assert!(crate::core::utils::from_jsonc_str::<Value>("{ \"a\": }").is_err());
        assert!(crate::core::utils::from_jsonc_str::<Value>("{ \"a\": 1").is_err());
        assert!(crate::core::utils::from_jsonc_str::<Value>("not json at all").is_err());
        // An unterminated block comment leaves nothing valid behind.
        assert!(crate::core::utils::from_jsonc_str::<Value>("{ /* open\n\"a\": 1 }").is_err());
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
    fn the_2x_permission_list_is_read() {
        let config: Value = serde_json::from_str(
            r#"{ "permissions": [
                { "action": "shell", "resource": "git *", "effect": "allow" },
                { "action": "shell", "resource": "git push *", "effect": "deny" }
            ] }"#,
        )
        .expect("valid json");
        let mut rules = Vec::new();
        append_permission_list(config.get("permissions"), &mut rules);
        assert_eq!(
            rules,
            vec![
                rule("git *", Action::Allow),
                rule("git push *", Action::Deny)
            ],
            "list order is load-bearing: the last match wins"
        );
        assert_eq!(
            check_command_with_opencode_rules("git push origin main", &rules),
            PermissionVerdict::Deny
        );
        assert_eq!(
            check_command_with_opencode_rules("git status", &rules),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn a_list_entry_without_a_resource_covers_every_command() {
        let config: Value =
            serde_json::from_str(r#"{ "permissions": [{ "action": "*", "effect": "deny" }] }"#)
                .expect("valid json");
        let mut rules = Vec::new();
        append_permission_list(config.get("permissions"), &mut rules);
        assert_eq!(rules, vec![rule("*", Action::Deny)]);
    }

    #[test]
    fn list_entries_for_other_tools_are_dropped() {
        let config: Value = serde_json::from_str(
            r#"{ "permissions": [
                { "action": "edit", "resource": "src/*", "effect": "allow" },
                { "action": "webfetch", "resource": "*", "effect": "deny" }
            ] }"#,
        )
        .expect("valid json");
        let mut rules = Vec::new();
        append_permission_list(config.get("permissions"), &mut rules);
        assert!(rules.is_empty());
    }

    #[test]
    fn a_list_entry_with_an_unknown_effect_is_dropped_rather_than_guessed() {
        let config: Value = serde_json::from_str(
            r#"{ "permissions": [{ "action": "shell", "resource": "*", "effect": "maybe" }] }"#,
        )
        .expect("valid json");
        let mut rules = Vec::new();
        append_permission_list(config.get("permissions"), &mut rules);
        assert!(rules.is_empty());
    }

    #[test]
    fn the_shell_alias_of_bash_matches_a_shell_command() {
        let config: Value =
            serde_json::from_str(r#"{ "permission": { "shell": "deny" } }"#).expect("valid json");
        let mut rules = Vec::new();
        append_rules(config.get("permission").expect("permission"), &mut rules);
        assert_eq!(rules, vec![rule("*", Action::Deny)]);
        assert_eq!(
            check_command_with_opencode_rules("git status", &rules),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn the_legacy_map_is_applied_before_the_2x_list_whatever_the_file_order() {
        let tmp = test_isolation::tempdir();
        let project = tmp.path().join("project");
        // The list is declared first and still has to lose to the legacy map.
        write_json(
            &project.join("opencode.json"),
            r#"{ "permissions": [
                { "action": "shell", "resource": "git *", "effect": "deny" }
            ],
            "permission": { "bash": { "git *": "allow" } } }"#,
        );

        test_isolation::with_root(&tmp.path().join("home"), || {
            let _entered = test_isolation::enter(&project);
            let rules = load_opencode_rules(None);
            assert_eq!(
                rules,
                vec![rule("git *", Action::Allow), rule("git *", Action::Deny)],
                "the legacy map loads first, so the list can still deny what it allows"
            );
            assert_eq!(
                check_command_with_opencode_rules("git status", &rules),
                PermissionVerdict::Deny
            );
        });
    }

    #[test]
    fn an_agents_2x_block_loads_after_the_root_rules_so_it_wins_ties() {
        let tmp = test_isolation::tempdir();
        let project = tmp.path().join("project");
        write_json(
            &project.join("opencode.json"),
            r#"{ "permission": { "bash": { "git *": "deny" } },
                "agents": { "build": { "permissions": [
                    { "action": "shell", "resource": "git *", "effect": "allow" }
                ] } } }"#,
        );

        test_isolation::with_root(&tmp.path().join("home"), || {
            let _entered = test_isolation::enter(&project);
            let rules = load_opencode_rules(Some("build"));
            assert_eq!(
                rules,
                vec![rule("git *", Action::Deny), rule("git *", Action::Allow)],
                "the agent block appends after root, matching OpenCode's resolution"
            );
            assert_eq!(load_opencode_rules(None), vec![rule("git *", Action::Deny)]);
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
