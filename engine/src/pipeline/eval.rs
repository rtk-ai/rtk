use super::conditions::{Fact, Facts, all_match};
use super::position::Position;
use super::resolve::{Resolved, ResolvedRule};
use crate::block::Subject;
use crate::method::Method;
use crate::registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Rewrite,
    Deny,
    Passthrough,
    Route,
}

impl Action {
    pub fn parse(name: &str) -> Option<Action> {
        match name {
            "rewrite" => Some(Action::Rewrite),
            "deny" => Some(Action::Deny),
            "passthrough" => Some(Action::Passthrough),
            "route" => Some(Action::Route),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Action::Rewrite => "rewrite",
            Action::Deny => "deny",
            Action::Passthrough => "passthrough",
            Action::Route => "route",
        }
    }
}

pub enum DecisionKind {
    Act(Action),
    Transform {
        methods: Vec<&'static dyn Method>,
        unresolved: Vec<String>,
    },
}

pub struct Decision<'p> {
    pub rule: &'p ResolvedRule,
    pub kind: DecisionKind,
}

impl Decision<'_> {
    pub fn shadow(&self) -> bool {
        self.rule.def.shadow
    }
}

pub fn rule_matches(rule: &ResolvedRule, facts: &Facts) -> bool {
    match &rule.when {
        Some(conditions) if all_match(conditions, facts) => {}
        // Unparseable conditions can never match — decided at resolve time.
        _ => return false,
    }
    if rule.any.is_empty() {
        return true;
    }
    rule.any
        .iter()
        .any(|group| matches!(group, Some(c) if all_match(c, facts)))
}

pub fn resolve_chain(
    resolved: &Resolved,
    names: &[String],
) -> (Vec<&'static dyn Method>, Vec<String>) {
    let mut methods = Vec::new();
    let mut unresolved = Vec::new();
    for name in names {
        if let Some(chain) = registry::chain(name) {
            methods.extend_from_slice(chain);
            continue;
        }
        if let Some((def, _)) = resolved.methods.get(name) {
            if let Some(entries) = &def.chain {
                let mut ok = true;
                let mut expansion = Vec::new();
                for entry in entries {
                    if let Some(chain) = registry::chain(entry) {
                        expansion.extend_from_slice(chain);
                    } else if let Some(method) = registry::find(entry) {
                        expansion.push(method);
                    } else {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    methods.extend(expansion);
                    continue;
                }
                unresolved.push(name.clone());
                continue;
            }
        }
        if let Some(method) = registry::find(name) {
            methods.push(method);
            continue;
        }
        unresolved.push(name.clone());
    }
    (methods, unresolved)
}

fn decide_rule<'p>(resolved: &Resolved, rule: &'p ResolvedRule) -> Decision<'p> {
    if let Some(action) = rule.def.action.as_deref().and_then(Action::parse) {
        return Decision {
            rule,
            kind: DecisionKind::Act(action),
        };
    }
    let (methods, unresolved) = resolve_chain(resolved, &rule.def.use_);
    Decision {
        rule,
        kind: DecisionKind::Transform {
            methods,
            unresolved,
        },
    }
}

fn control_without_effect(resolved: &Resolved, rule: &ResolvedRule) -> bool {
    let Some(action) = rule.def.action.as_deref().and_then(Action::parse) else {
        return false;
    };
    if rule.def.shadow {
        return true;
    }
    action == Action::Route
        && rule.def.task.as_deref().is_some_and(|task| {
            resolved
                .tasks
                .get(task)
                .is_none_or(|(def, _)| def.upstream.is_none())
        })
}

fn evaluate_list<'p>(
    resolved: &'p Resolved,
    position: Position,
    facts: &Facts,
) -> Vec<Decision<'p>> {
    let mut decisions = Vec::new();
    for rule in resolved.rules_at(position) {
        if !rule.def.enabled || rule.def.pro_inert {
            continue;
        }
        if !rule_matches(rule, facts) {
            continue;
        }
        if control_without_effect(resolved, rule) {
            continue;
        }
        decisions.push(decide_rule(resolved, rule));
        if !rule.def.continue_ {
            break;
        }
    }
    decisions
}

pub fn evaluate<'p>(
    resolved: &'p Resolved,
    position: Position,
    facts: &Facts,
) -> Vec<Decision<'p>> {
    let specific = evaluate_list(resolved, position, facts);
    if !specific.is_empty() || position.subject == Subject::Any {
        return specific;
    }
    evaluate_list(resolved, position.any_sibling(), facts)
}

pub fn interpolate(template: &str, facts: &Facts) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                match facts.get(&after[..end]) {
                    Some(Fact::Str(s)) => out.push_str(s),
                    Some(Fact::Num(n)) => out.push_str(&format_number(*n)),
                    Some(Fact::Bool(b)) => out.push_str(if *b { "true" } else { "false" }),
                    None => out.push_str(&rest[start..start + end + 3]),
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn route_message(task: &str, facts: &Facts, custom: Option<&str>) -> String {
    if let Some(custom) = custom.filter(|c| !c.trim().is_empty()) {
        return interpolate(custom, facts);
    }
    if let Some(Fact::Str(command)) = facts.get("command") {
        return format!(
            "`{command}` is handled by a worker model. Run: `rtk {task} --question \"<what you need to know>\" -- {command}`"
        );
    }
    let text = |key: &str| match facts.get(key) {
        Some(Fact::Str(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    };
    let tool = text("tool").unwrap_or_default();
    match tool.as_str() {
        "web" => {
            if let Some(url) = text("url").or_else(|| text("input.url")) {
                return format!(
                    "Fetching {url} is handled by a worker model. Run: `rtk {task} --question \"<what you need to know>\" -- curl -sL {url}`"
                );
            }
        }
        "search" => {
            if let Some(pattern) = text("input.pattern") {
                let path = text("input.path").unwrap_or_else(|| ".".to_string());
                return format!(
                    "This search is handled by a worker model. Run: `rtk {task} --question \"<what you need to know>\" -- rg -n '{pattern}' {path}`"
                );
            }
        }
        "write" => {
            let target = text("file_path").unwrap_or_else(|| "<file>".to_string());
            return format!(
                "Writing {target} is handled by a worker model. Run: `rtk {task} --spec \"<what the file must do>\" --reference <a similar file> --target {target}`"
            );
        }
        "subagent" => {
            let question = text("input.description")
                .unwrap_or_else(|| "<the subagent's question>".to_string());
            return format!(
                "This subagent task is handled by a worker model. Run: `rtk {task} --question \"{question}\" <the files to look at>`"
            );
        }
        "read" | "" => {}
        other => {
            return format!(
                "`{other}` is handled by a worker model. Run: `rtk {task} --question \"<what you need to know>\"`"
            );
        }
    }
    let file_path = match facts.get("file_path") {
        Some(Fact::Str(path)) => path.clone(),
        _ => "<file>".to_string(),
    };
    let lines = match facts.get("file_lines") {
        Some(Fact::Num(n)) => format!(" ({} lines)", format_number(*n)),
        _ => String::new(),
    };
    format!(
        "Read of {file_path}{lines} is blocked. Run: `rtk {task} --question \"<what you need to know>\" {file_path}`"
    )
}

fn format_number(value: f64) -> String {
    if value.fract() != 0.0 {
        return format!("{value}");
    }
    let digits = format!("{}", value as i64);
    let (sign, digits) = digits
        .strip_prefix('-')
        .map(|d| ("-", d))
        .unwrap_or(("", digits.as_str()));
    let mut grouped = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}{grouped}")
}

#[cfg(test)]
mod tests {
    use super::super::resolve::{Scope, SourceFile, resolve};
    use super::*;

    fn facts_of(pairs: &[(&str, &str)]) -> Facts {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Fact::Str(v.to_string())))
            .collect()
    }

    #[test]
    fn route_messages_name_a_runnable_command_per_tool() {
        let web = route_message(
            "look",
            &facts_of(&[("tool", "web"), ("url", "https://x.dev/a")]),
            None,
        );
        assert!(
            web.contains("rtk look") && web.contains("-- curl -sL https://x.dev/a"),
            "{web}"
        );
        let search = route_message(
            "look",
            &facts_of(&[
                ("tool", "search"),
                ("input.pattern", "fn main"),
                ("input.path", "src"),
            ]),
            None,
        );
        assert!(search.contains("-- rg -n 'fn main' src"), "{search}");
        let write = route_message(
            "write",
            &facts_of(&[("tool", "write"), ("file_path", "src/a.rs")]),
            None,
        );
        assert!(
            write.contains("--target src/a.rs") && write.contains("--spec"),
            "{write}"
        );
        let edit = route_message(
            "write",
            &facts_of(&[("tool", "edit"), ("file_path", "src/a.rs")]),
            None,
        );
        assert!(!edit.contains("--target"), "{edit}");
        let blank = route_message("dg", &facts_of(&[("command", "cargo test")]), Some("  "));
        assert!(blank.contains("rtk dg"), "{blank}");
        let sub = route_message(
            "explore",
            &facts_of(&[
                ("tool", "subagent"),
                ("input.description", "find the auth flow"),
            ]),
            None,
        );
        assert!(sub.contains("--question \"find the auth flow\""), "{sub}");
        let mcp = route_message(
            "ask",
            &facts_of(&[("tool", "mcp__linear__list_issues")]),
            None,
        );
        assert!(
            mcp.contains("`mcp__linear__list_issues` is handled") && mcp.contains("rtk ask"),
            "{mcp}"
        );
        let shell = route_message(
            "t",
            &facts_of(&[("tool", "shell"), ("command", "npm test")]),
            None,
        );
        assert!(shell.contains("-- npm test"), "{shell}");
        let read = route_message(
            "r",
            &facts_of(&[("tool", "read"), ("file_path", "a.rs")]),
            None,
        );
        assert!(read.starts_with("Read of a.rs"), "{read}");
    }
    use crate::block::Event;

    fn load(scope: Scope, label: &str, text: &str) -> SourceFile {
        let parsed = super::super::schema::parse(text, label);
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        SourceFile {
            scope,
            label: label.to_string(),
            file: parsed.file.expect("parses"),
        }
    }

    fn read_decisions(text: &str) -> Vec<String> {
        let (resolved, _) = resolve(&[load(Scope::User, "t", text)]);
        let facts = str_facts(&[("tool", "read")]);
        evaluate(
            &resolved,
            Position::hook(Event::BeforeTool, Subject::Read),
            &facts,
        )
        .iter()
        .map(|d| d.rule.name.clone())
        .collect()
    }

    #[test]
    fn a_shadowed_control_rule_never_enforces_and_lets_later_rules_match() {
        let alone = "[pipeline]\nname = \"t\"\n\n[[hook.before_tool.read]]\nname = \"trial\"\nshadow = true\naction = \"deny\"\n";
        assert!(read_decisions(alone).is_empty());
        let then_real =
            format!("{alone}\n[[hook.before_tool.read]]\nname = \"real\"\naction = \"deny\"\n");
        assert_eq!(read_decisions(&then_real), vec!["real".to_string()]);
    }

    #[test]
    fn a_route_to_a_task_without_upstream_is_inert_and_a_bound_one_routes() {
        let unbound = "[pipeline]\nname = \"t\"\n\n[task.look]\nkind = \"summarize\"\n\n[[hook.before_tool.read]]\nname = \"r\"\naction = \"route\"\ntask = \"look\"\n";
        assert!(read_decisions(unbound).is_empty());
        let bound = unbound.replace(
            "[task.look]\nkind = \"summarize\"\n",
            "[upstream.u]\nprovider = \"anthropic\"\nmodel = \"m\"\n\n[task.look]\nkind = \"summarize\"\nupstream = \"u\"\n",
        );
        assert_eq!(read_decisions(&bound), vec!["r".to_string()]);
    }

    fn str_facts(pairs: &[(&str, &str)]) -> Facts {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Fact::Str(v.to_string())))
            .collect()
    }

    #[test]
    fn default_pipeline_rewrites_shell_and_nothing_else() {
        let (resolved, _) = resolve(&[]);
        let decisions = evaluate(
            &resolved,
            Position::hook(Event::BeforeTool, Subject::Shell),
            &str_facts(&[("command", "ls -la")]),
        );
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].rule.name, "rtk-rewrite");
        assert!(matches!(
            decisions[0].kind,
            DecisionKind::Act(Action::Rewrite)
        ));

        let after = evaluate(
            &resolved,
            Position::hook(Event::AfterTool, Subject::Shell),
            &str_facts(&[("format", "term")]),
        );
        assert!(after.is_empty());
    }

    #[test]
    fn posthook_selects_the_branch_chains_by_subject_and_format() {
        let user = load(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = \"rtk:posthook\"\n",
        );
        let (resolved, _) = resolve(&[user]);

        let mut facts = str_facts(&[("format", "term")]);
        facts.insert("is_rtk_command".into(), Fact::Bool(false));
        let term = evaluate(
            &resolved,
            Position::hook(Event::AfterTool, Subject::Shell),
            &facts,
        );
        assert_eq!(term.len(), 1);
        let DecisionKind::Transform {
            methods,
            unresolved,
        } = &term[0].kind
        else {
            panic!("expected transform");
        };
        assert!(unresolved.is_empty());
        let names: Vec<&str> = methods.iter().map(|m| m.name()).collect();
        assert_eq!(
            names,
            vec!["ansi", "progress", "dedup", "base64-elide", "cap"]
        );

        let mut rtk_facts = str_facts(&[("format", "term")]);
        rtk_facts.insert("is_rtk_command".into(), Fact::Bool(true));
        assert!(
            evaluate(
                &resolved,
                Position::hook(Event::AfterTool, Subject::Shell),
                &rtk_facts,
            )
            .is_empty()
        );

        let matches = evaluate(
            &resolved,
            Position::hook(Event::AfterTool, Subject::Search),
            &str_facts(&[("format", "matches")]),
        );
        let DecisionKind::Transform { methods, .. } = &matches[0].kind else {
            panic!("expected transform");
        };
        assert_eq!(
            methods.iter().map(|m| m.name()).collect::<Vec<_>>(),
            vec!["grep-group"]
        );

        let lockfile = evaluate(
            &resolved,
            Position::hook(Event::AfterTool, Subject::Read),
            &str_facts(&[("basename", "Cargo.lock"), ("format", "lockfile")]),
        );
        assert_eq!(lockfile[0].rule.name, "lockfiles");

        let json = evaluate(
            &resolved,
            Position::hook(Event::AfterTool, Subject::Read),
            &str_facts(&[("basename", "data.json"), ("format", "json")]),
        );
        assert!(json.is_empty(), "json-reads ships disabled");
    }

    #[test]
    fn user_chain_methods_expand() {
        let user = load(
            Scope::User,
            "u.toml",
            "[method.gentle]\nchain = [\"ansi\", \"dedup\"]\n[[hook.after_tool.shell]]\nname = \"g\"\nuse = [\"gentle\"]\n",
        );
        let (resolved, _) = resolve(&[user]);
        let decisions = evaluate(
            &resolved,
            Position::hook(Event::AfterTool, Subject::Shell),
            &Facts::new(),
        );
        let DecisionKind::Transform {
            methods,
            unresolved,
        } = &decisions[0].kind
        else {
            panic!("expected transform");
        };
        assert!(unresolved.is_empty());
        assert_eq!(
            methods.iter().map(|m| m.name()).collect::<Vec<_>>(),
            vec!["ansi", "dedup"]
        );
    }

    #[test]
    fn any_list_is_evaluated_only_when_the_specific_subject_matched_nothing() {
        let user = load(
            Scope::User,
            "u.toml",
            r#"
[[hook.after_tool.shell]]
name = "specific"
when = { format = "term" }
use  = ["ansi"]

[[hook.after_tool.any]]
name = "fallback"
use  = ["dedup"]
"#,
        );
        let (resolved, _) = resolve(&[user]);
        let hit = evaluate(
            &resolved,
            Position::hook(Event::AfterTool, Subject::Shell),
            &str_facts(&[("format", "term")]),
        );
        assert_eq!(hit[0].rule.name, "specific");

        let fallback = evaluate(
            &resolved,
            Position::hook(Event::AfterTool, Subject::Shell),
            &str_facts(&[("format", "json")]),
        );
        assert_eq!(fallback[0].rule.name, "fallback");
    }

    #[test]
    fn passthrough_rule_shadows_the_rewrite_for_what_it_matches() {
        let user = load(
            Scope::User,
            "u.toml",
            r#"
[[hook.before_tool.shell]]
name   = "keep-diffs"
when   = { command_matches = "^git (diff|show)\\b" }
action = "passthrough"
"#,
        );
        let (resolved, _) = resolve(&[user]);
        let kept = evaluate(
            &resolved,
            Position::hook(Event::BeforeTool, Subject::Shell),
            &str_facts(&[("command", "git diff HEAD~1")]),
        );
        assert!(matches!(
            kept[0].kind,
            DecisionKind::Act(Action::Passthrough)
        ));

        let rewritten = evaluate(
            &resolved,
            Position::hook(Event::BeforeTool, Subject::Shell),
            &str_facts(&[("command", "ls -la")]),
        );
        assert!(matches!(
            rewritten[0].kind,
            DecisionKind::Act(Action::Rewrite)
        ));
    }

    #[test]
    fn delegate_reads_routes_large_whole_file_reads() {
        let user = load(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = \"rtk:delegate-reads\"\n\n[upstream.u]\nprovider = \"anthropic\"\nmodel = \"m\"\n\n[task.bulk-read]\nkind = \"summarize\"\nupstream = \"u\"\n",
        );
        let (resolved, _) = resolve(&[user]);
        let mut facts = str_facts(&[("file_path", "src/Service.java")]);
        facts.insert("file_lines".into(), Fact::Num(4120.0));
        facts.insert("whole_file".into(), Fact::Bool(true));
        let decisions = evaluate(
            &resolved,
            Position::hook(Event::BeforeTool, Subject::Read),
            &facts,
        );
        assert_eq!(decisions[0].rule.name, "shunt-large-read");
        assert!(matches!(
            decisions[0].kind,
            DecisionKind::Act(Action::Route)
        ));
        let message = route_message(
            decisions[0].rule.def.task.as_deref().unwrap_or(""),
            &facts,
            decisions[0].rule.def.message.as_deref(),
        );
        assert_eq!(
            message,
            "Read of src/Service.java (4,120 lines) is blocked. Run: `rtk bulk-read --question \"<what you need to know>\" src/Service.java`"
        );

        let mut partial = facts.clone();
        partial.insert("whole_file".into(), Fact::Bool(false));
        assert!(
            evaluate(
                &resolved,
                Position::hook(Event::BeforeTool, Subject::Read),
                &partial,
            )
            .is_empty()
        );
    }

    #[test]
    fn interpolation_replaces_known_placeholders() {
        let mut facts = str_facts(&[("file_path", "big.log"), ("command", "cat big.log")]);
        facts.insert("file_lines".into(), Fact::Num(1234.0));
        assert_eq!(
            interpolate("${file_path} has ${file_lines} lines: ${command}", &facts),
            "big.log has 1,234 lines: cat big.log"
        );
    }

    #[test]
    fn interpolation_is_single_pass() {
        let facts = str_facts(&[("command", "echo ${tool}"), ("tool", "shell")]);
        assert_eq!(
            interpolate("blocked: ${command}", &facts),
            "blocked: echo ${tool}"
        );
        assert_eq!(
            interpolate("${nope} ${command}", &facts),
            "${nope} echo ${tool}"
        );
        assert_eq!(interpolate("tail ${command", &facts), "tail ${command");
        assert_eq!(interpolate("$} ${ {} $", &facts), "$} ${ {} $");
    }

    #[test]
    fn shadow_flag_reaches_the_decision() {
        let user = load(
            Scope::User,
            "u.toml",
            "[[hook.after_tool.shell]]\nname = \"s\"\nuse = [\"ansi\"]\nshadow = true\n",
        );
        let (resolved, _) = resolve(&[user]);
        let decisions = evaluate(
            &resolved,
            Position::hook(Event::AfterTool, Subject::Shell),
            &Facts::new(),
        );
        assert!(decisions[0].shadow());
    }
}
