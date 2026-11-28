use super::conditions::{KeyCheck, check_key, classifier_condition, parse_condition, split_key};
use super::diag::{Code, Diagnostic, did_you_mean};
use super::eval::Action;
use super::position::{Position, SiteKind};
use super::resolve::{Resolved, ResolvedRule, Scope};
use super::schema::PRO_METHODS;
use crate::block::{Event, Subject};
use crate::method::CostClass;
use crate::registry;

/// Task kind names, read from the kind registry so a new kind cannot be
/// accepted by the runner and refused by `validate`.
/// Task kind names, read from the kind registry so a new kind cannot be
/// accepted by the runner and refused by `validate`.
pub fn task_kind_names() -> Vec<&'static str> {
    crate::task::kinds().iter().map(|k| k.name()).collect()
}
pub const DIRECTIVE_CHANNELS: &[&str] = &["awareness_file", "session_start", "per_turn"];

const FILTER_KEYS: &[&str] = &[
    "match_command",
    "match_output",
    "strip_ansi",
    "strip_lines_matching",
    "keep_lines_matching",
    "max_lines",
    "head_lines",
    "tail_lines",
    "on_empty",
    "replace",
    "description",
    "tests",
];

pub struct ValidateCtx<'a> {
    pub proxy_running: Option<bool>,
    pub reserved_commands: &'a [&'a str],
    pub is_reserved_command: Option<fn(&str) -> bool>,
    pub known_directives: &'a [&'a str],
}

impl Default for ValidateCtx<'_> {
    fn default() -> Self {
        ValidateCtx {
            proxy_running: None,
            reserved_commands: &[],
            is_reserved_command: None,
            known_directives: &["rtk-awareness", "terse-output", "route-tasks"],
        }
    }
}

pub fn validate(resolved: &Resolved, ctx: &ValidateCtx) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    if !resolved.guards.never_worse {
        out.push(Diagnostic::new(
            Code::E002,
            "[guards] never_worse cannot be disabled",
        ));
    }

    let mut has_proxy_rules = false;
    for (position, rules) in &resolved.rules {
        let mut catch_all_before: Option<&str> = None;
        for rule in rules {
            validate_rule(resolved, rule, ctx, &mut out);
            // Only rules the user wrote are actionable here: a built-in
            // proxy rule being inert without a daemon is not something they
            // can fix, and warning about it would fire for every CLI-only
            // install since rtk:default carries one.
            if position.site == SiteKind::Proxy
                && rule.def.enabled
                && !crate::pipeline::builtins::BUILTIN_NAMES.contains(&rule.origin.as_str())
            {
                has_proxy_rules = true;
            }
            if position.site == SiteKind::Proxy && rule.def.enabled {
                let at = |code, message: &str| {
                    Diagnostic::new(code, message.to_string())
                        .at(rule.origin.clone(), rule.def.line)
                        .context(format!("{} \"{}\"", position.header(), rule.name))
                };
                if rule.scope == Scope::Project {
                    out.push(at(
                        Code::W017,
                        "proxy.request rules in a project file are not read — one daemon serves every project, so it loads only your user pipeline; move the rule there",
                    ));
                }
                if rule.def.shadow {
                    out.push(at(
                        Code::W018,
                        "`shadow` is not recorded under proxy.request yet — the rule is skipped and `rtk gain --shadow` will not show it",
                    ));
                }
            }
            if rule.def.enabled && !rule.def.pro_inert {
                if let Some(blocker) = catch_all_before {
                    out.push(
                        Diagnostic::new(
                            Code::W001,
                            format!(
                                "rule `{}` is unreachable — `{blocker}` above it matches everything and does not `continue`",
                                rule.name
                            ),
                        )
                        .at(rule.origin.clone(), rule.def.line)
                        .context(format!("{} \"{}\"", position.header(), rule.name)),
                    );
                }
                if rule.def.when.is_empty() && rule.def.any.is_empty() && !rule.def.continue_ {
                    catch_all_before = Some(&rule.name);
                }
            }
        }
    }

    if has_proxy_rules && ctx.proxy_running == Some(false) {
        out.push(Diagnostic::new(
            Code::W005,
            "proxy.request rules are present but the RTK proxy is not running — they are inert",
        ));
    }

    if let Some(rewrite) = resolved.find_rule("rtk-rewrite") {
        if !rewrite.def.enabled {
            out.push(Diagnostic::new(
                Code::W006,
                "`rtk-rewrite` is disabled — RTK's primary saving is off",
            ));
        }
    }

    for directive in &resolved.directives {
        if let Some(use_) = &directive.def.use_ {
            if !ctx.known_directives.contains(&use_.as_str()) {
                out.push(
                    Diagnostic::new(Code::E007, format!("unknown directive `{use_}`"))
                        .at(directive.origin.clone(), directive.def.line),
                );
            }
        }
        if let Some(channel) = &directive.def.channel {
            if directive.def.enabled && matches!(channel.as_str(), "session_start" | "per_turn") {
                out.push(
                    Diagnostic::new(
                        Code::W019,
                        format!(
                            "directive channel `{channel}` is not delivered yet — the directive is skipped; use `awareness_file` (the default)"
                        ),
                    )
                    .at(directive.origin.clone(), directive.def.line),
                );
            }
            if !DIRECTIVE_CHANNELS.contains(&channel.as_str()) {
                out.push(
                    Diagnostic::new(
                        Code::E002,
                        format!(
                            "unknown directive channel `{channel}` (one of: {})",
                            DIRECTIVE_CHANNELS.join(", ")
                        ),
                    )
                    .at(directive.origin.clone(), directive.def.line),
                );
            }
        }
    }

    for (name, (def, origin)) in &resolved.methods {
        let filter_keys: Vec<&str> = def
            .options
            .keys()
            .map(|k| k.as_str())
            .filter(|k| FILTER_KEYS.contains(k))
            .collect();
        if !filter_keys.is_empty() {
            out.push(
                Diagnostic::new(
                    Code::E016,
                    format!(
                        "[method.{name}] carries filter keys ({}) — custom command filters live in filters.toml, unchanged",
                        filter_keys.join(", ")
                    ),
                )
                .at(origin.clone(), def.line)
                .hint("see `filters.toml` in the guide; the pipeline file never defines a filter"),
            );
        }
        let is_builtin_method = registry::find(name).is_some() || registry::chain(name).is_some();
        if is_builtin_method {
            // Parsed, validated, consumed by nothing: MethodCtx carries no
            // options, so accepting `[method.cap] width = 100` would tell the
            // user they had tuned something. Refuse until options actually
            // reach methods, so configuration never lies.
            let unconsumed: Vec<&str> = def
                .options
                .keys()
                .map(|k| k.as_str())
                .filter(|k| !FILTER_KEYS.contains(k) && *k != "chain")
                .collect();
            if !unconsumed.is_empty() {
                out.push(
                    Diagnostic::new(
                        Code::E016,
                        format!(
                            "[method.{name}] carries options no built-in method reads yet ({}) —                              remove them; method options are not consumed in this release",
                            unconsumed.join(", ")
                        ),
                    )
                    .at(origin.clone(), def.line),
                );
            }
        }
        if is_builtin_method && def.chain.is_some() {
            out.push(
                Diagnostic::new(
                    Code::E016,
                    format!(
                        "[method.{name}] names a built-in method and cannot carry `chain` — options only"
                    ),
                )
                .at(origin.clone(), def.line),
            );
        }
        if let Some(entries) = &def.chain {
            for entry in entries {
                if method_exists(resolved, entry) {
                    continue;
                }
                out.push(
                    Diagnostic::new(
                        Code::E007,
                        format!("[method.{name}] chain names unknown method `{entry}`"),
                    )
                    .at(origin.clone(), def.line),
                );
            }
        }
    }

    for (name, (task, origin)) in &resolved.tasks {
        if task.kind.is_empty() {
            out.push(
                Diagnostic::new(
                    Code::E014,
                    format!("[task.{name}] has no `kind` and no inherited task defines one"),
                )
                .at(origin.clone(), task.line),
            );
        } else if !task_kind_names().contains(&task.kind.as_str()) {
            out.push(
                Diagnostic::new(
                    Code::E014,
                    format!(
                        "[task.{name}] has unknown kind `{}` (one of: {})",
                        task.kind,
                        task_kind_names().join(", ")
                    ),
                )
                .at(origin.clone(), task.line),
            );
        }
        if let Some(upstream) = &task.upstream {
            if !resolved.upstreams.contains_key(upstream) {
                out.push(
                    Diagnostic::new(
                        Code::E014,
                        format!("[task.{name}] names unknown upstream `{upstream}`"),
                    )
                    .at(origin.clone(), task.line),
                );
            }
        }
        if ctx.reserved_commands.contains(&name.as_str())
            || ctx
                .is_reserved_command
                .is_some_and(|reserved| reserved(name))
        {
            out.push(
                Diagnostic::new(
                    Code::E014,
                    format!("task name `{name}` collides with a built-in rtk command"),
                )
                .at(origin.clone(), task.line),
            );
        }
    }

    for (name, (classifier, origin)) in &resolved.classifiers {
        if !matches!(classifier.question.as_str(), "choice" | "score" | "noul") {
            out.push(
                Diagnostic::new(
                    Code::E019,
                    format!(
                        "[classifier.{name}] question `{}` is not one of choice, score, noul",
                        classifier.question
                    ),
                )
                .at(origin.clone(), classifier.line),
            );
        }
        if classifier.question == "choice" && classifier.labels.is_empty() {
            out.push(
                Diagnostic::new(
                    Code::E019,
                    format!("[classifier.{name}] question `choice` needs a [classifier.{name}.labels] table"),
                )
                .at(origin.clone(), classifier.line),
            );
        }
        if classifier.question == "score" && classifier.rubric.is_empty() {
            out.push(
                Diagnostic::new(
                    Code::E019,
                    format!(
                        "[classifier.{name}] question `score` needs `rubric = [..]` — ordered level descriptions, worst to best"
                    ),
                )
                .at(origin.clone(), classifier.line),
            );
        }
        match &classifier.upstream {
            Some(upstream) if !resolved.upstreams.contains_key(upstream) => {
                out.push(
                    Diagnostic::new(
                        Code::E019,
                        format!("[classifier.{name}] names unknown upstream `{upstream}`"),
                    )
                    .at(origin.clone(), classifier.line),
                );
            }
            None => {
                out.push(
                    Diagnostic::new(
                        Code::E019,
                        format!("[classifier.{name}] has no upstream — nothing can answer it"),
                    )
                    .at(origin.clone(), classifier.line),
                );
            }
            _ => {}
        }
        if classifier
            .min_confidence
            .is_some_and(|c| !(0.0..=1.0).contains(&c))
        {
            out.push(
                Diagnostic::new(
                    Code::E019,
                    format!("[classifier.{name}] min_confidence must be between 0 and 1"),
                )
                .at(origin.clone(), classifier.line),
            );
        }
    }

    for (position, rules) in &resolved.rules {
        for rule in rules {
            let groups = std::iter::once(&rule.def.when).chain(rule.def.any.iter());
            for group in groups {
                for (raw_key, value) in group {
                    let (base, _) = split_key(raw_key);
                    let Some((question, cname, is_confidence)) = classifier_condition(&base) else {
                        continue;
                    };
                    let _ = position;
                    let Some((def, _)) = resolved.classifiers.get(cname) else {
                        out.push(
                            Diagnostic::new(
                                Code::E019,
                                format!(
                                    "rule `{}` conditions on `{base}` but no [classifier.{cname}] is declared",
                                    rule.name
                                ),
                            )
                            .at(rule.origin.clone(), rule.def.line),
                        );
                        continue;
                    };
                    if def.question != question {
                        out.push(
                            Diagnostic::new(
                                Code::E019,
                                format!(
                                    "`{base}` expects a `{question}` classifier, but [classifier.{cname}] is `{}`",
                                    def.question
                                ),
                            )
                            .at(rule.origin.clone(), rule.def.line),
                        );
                        continue;
                    }
                    if question == "choice" && !is_confidence {
                        let mut candidates: Vec<&str> = Vec::new();
                        if let Some(one) = value.as_str() {
                            candidates.push(one);
                        }
                        if let Some(list) = value.as_array() {
                            candidates.extend(list.iter().filter_map(|v| v.as_str()));
                        }
                        for label in candidates {
                            if !def.labels.contains_key(label) {
                                out.push(
                                    Diagnostic::new(
                                        Code::E019,
                                        format!(
                                            "`{base}` tests label `{label}`, which [classifier.{cname}] does not define (labels: {})",
                                            def.labels.keys().cloned().collect::<Vec<_>>().join(", ")
                                        ),
                                    )
                                    .at(rule.origin.clone(), rule.def.line),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    for (name, (upstream, origin)) in &resolved.upstreams {
        if upstream.api_key_literal {
            out.push(
                Diagnostic::new(
                    Code::E015,
                    format!(
                        "[upstream.{name}] carries a literal `api_key` — use `api_key_env` naming the environment variable"
                    ),
                )
                .at(origin.clone(), upstream.line),
            );
        }
    }

    out
}

fn method_exists(resolved: &Resolved, name: &str) -> bool {
    registry::find(name).is_some()
        || registry::chain(name).is_some()
        || registry::proxy_info(name).is_some()
        || resolved
            .methods
            .get(name)
            .map(|(def, _)| def.chain.is_some())
            .unwrap_or(false)
}

fn validate_rule(
    resolved: &Resolved,
    rule: &ResolvedRule,
    _ctx: &ValidateCtx,
    out: &mut Vec<Diagnostic>,
) {
    let def = &rule.def;
    let position = rule.position;
    let context = format!("{} \"{}\"", position.header(), rule.name);
    let mut push = |diagnostic: Diagnostic| {
        out.push(
            diagnostic
                .at(rule.origin.clone(), def.line)
                .context(context.clone()),
        );
    };

    if position.site == SiteKind::Proxy {
        match position.subject {
            Subject::User | Subject::System | Subject::Tools => {
                push(Diagnostic::new(
                    Code::W008,
                    format!(
                        "`proxy.request.{}` transforms are not active in this release — the rule is parsed and inert",
                        position.subject.as_str()
                    ),
                ));
                return;
            }
            // `proxy.request.model` routes to an upstream rather than a task:
            // the upstream already names the provider, the model, and where to
            // reach it, so one rule covers both "same provider, cheaper model"
            // and "a different Anthropic-shaped endpoint".
            Subject::Model => {
                match (def.action.as_deref(), &def.upstream) {
                    (Some("route"), None) => push(Diagnostic::new(
                        Code::E014,
                        "`proxy.request.model` routing needs `upstream = \"<name>\"`",
                    )),
                    (Some("route"), Some(name)) if !resolved.upstreams.contains_key(name) => {
                        push(Diagnostic::new(
                            Code::E014,
                            format!("`route` names unknown upstream `{name}`"),
                        ))
                    }
                    (Some("route"), _) => push(Diagnostic::new(
                        Code::W008,
                        "`proxy.request.model` routing is not active in this release — the rule is parsed and inert",
                    )),
                    _ => push(Diagnostic::new(
                        Code::E006,
                        "`proxy.request.model` takes `action = \"route\"`",
                    )),
                }
                return;
            }
            _ => {}
        }
    }

    let has_use = !def.use_.is_empty();
    let has_action = def.action.is_some();
    if has_use == has_action {
        push(Diagnostic::new(
            Code::E006,
            "a rule takes exactly one of `use` (transform chain) or `action` (control)",
        ));
        return;
    }

    if let Some(action_name) = def.action.as_deref() {
        let Some(action) = Action::parse(action_name) else {
            push(Diagnostic::new(
                Code::E005,
                format!("unknown action `{action_name}`"),
            ));
            return;
        };
        let legal = matches!(
            (position.site, position.event, position.subject, action),
            (
                SiteKind::Hook,
                Event::BeforeTool,
                Subject::Shell,
                Action::Rewrite
            ) | (
                SiteKind::Hook,
                Event::BeforeTool,
                _,
                Action::Deny | Action::Passthrough | Action::Route
            ) | (
                SiteKind::Hook,
                Event::AfterTool,
                _,
                Action::Passthrough | Action::Route
            )
        );
        if !legal {
            push(Diagnostic::new(
                Code::E005,
                format!(
                    "`action = \"{}\"` is not legal at {}",
                    action.as_str(),
                    position.render()
                ),
            ));
        }
        if action == Action::Route && position == Position::hook(Event::BeforeTool, Subject::Edit) {
            push(Diagnostic::new(
                Code::E005,
                "`route` is not legal before an edit — a worker would rewrite the whole file from a spec instead of applying the change; use `deny` or `passthrough`",
            ));
        }
        if action == Action::Deny && position == Position::hook(Event::BeforeTool, Subject::Write) {
            push(Diagnostic::new(
                Code::W004,
                "`deny` under `write` — the body is already generated; a deny only buys a retry",
            ));
        }
        if action == Action::Deny && references_classifier(def) {
            push(Diagnostic::new(
                Code::E020,
                "`deny` cannot depend on a classifier — when the judge is unavailable or unsure the protection would silently disappear; gate it on fixed conditions, or use `route` / `passthrough`",
            ));
        }
        if action == Action::Route && position == Position::hook(Event::AfterTool, Subject::Read) {
            push(Diagnostic::new(
                Code::W021,
                "`route` after a read replaces the file with a summary — a later Edit needs the exact bytes, so keep it to files the agent only reads",
            ));
        }
        if action == Action::Route {
            match &def.task {
                None => push(Diagnostic::new(
                    Code::E014,
                    "`route` needs `task = \"<name>\"`",
                )),
                Some(task) => match resolved.tasks.get(task) {
                    None => push(Diagnostic::new(
                        Code::E014,
                        format!("`route` names unknown task `{task}`"),
                    )),
                    Some((task_def, _)) => {
                        if let Some(reason) = route_kind_mismatch(position, &task_def.kind) {
                            push(Diagnostic::new(
                                Code::W023,
                                format!(
                                    "task `{task}` is a `{}` task — {reason}; the route will fail when it fires",
                                    task_def.kind
                                ),
                            ));
                        }
                        if task_def.upstream.is_none() {
                            push(Diagnostic::new(
                                Code::W007,
                                format!(
                                    "task `{task}` has no upstream — the route rule is inert until one is bound"
                                ),
                            ));
                        }
                    }
                },
            }
        }
    }

    if let Some(message) = def
        .mode
        .as_deref()
        .and_then(|mode| route_mode_problem(position, def.action.as_deref(), mode))
    {
        push(Diagnostic::new(Code::E005, message));
    }

    if has_use && position.site == SiteKind::Hook && position.event == Event::BeforeTool {
        push(Diagnostic::new(
            Code::E005,
            "a transform chain cannot run before the tool — there is no output yet",
        ));
    }

    let mut groups: Vec<&std::collections::BTreeMap<String, toml::Value>> = vec![&def.when];
    groups.extend(def.any.iter());
    for group in groups {
        for (raw_key, value) in group {
            let (base, _) = split_key(raw_key);
            match check_key(position, &base) {
                KeyCheck::Ok => {
                    if let Err(message) = parse_condition(raw_key, value) {
                        push(Diagnostic::new(Code::E002, message));
                    }
                }
                KeyCheck::WrongEvent { hint } => {
                    let mut d = Diagnostic::new(
                        Code::E003,
                        format!(
                            "condition `{base}` is not available at `{}.{}`",
                            position.site.as_str(),
                            position.event.as_str()
                        ),
                    );
                    d.hint = hint;
                    push(d);
                }
                KeyCheck::Unknown => {
                    if def.action.is_some() {
                        let mut d = Diagnostic::new(
                            Code::E004,
                            format!(
                                "condition `{base}` is not available for subject `{}`",
                                position.subject.as_str()
                            ),
                        );
                        d.hint = Some(
                            "control rules (action = ...) fail closed: every condition must be known on this rtk"
                                .to_string(),
                        );
                        push(d);
                    } else {
                        push(Diagnostic::new(
                            Code::W015,
                            format!(
                                "condition `{base}` is not available for subject `{}` — the rule can never match on this rtk (newer version?); the file still loads",
                                position.subject.as_str()
                            ),
                        ));
                    }
                }
                KeyCheck::TurnVarying => {
                    push(Diagnostic::new(
                        Code::E010,
                        format!(
                            "condition `{base}` varies per turn and `{}` is part of the cached prefix",
                            position.subject.as_str()
                        ),
                    ));
                }
                KeyCheck::Pro => {
                    push(Diagnostic::new(
                        Code::W009,
                        format!(
                            "condition `{base}` is not available in this edition — the rule is inert; the file still loads"
                        ),
                    ));
                }
            }
        }
    }

    // Four after_tool subjects have no extraction arm in the hook adapter:
    // nothing pulls their text out of the event, so a transform chain there
    // fires and receives nothing. Same silent-no-op class as W011; warned,
    // not refused, because the anchors are reserved for future wiring.
    if position.site == SiteKind::Hook
        && position.event == Event::AfterTool
        && !after_tool_extractable(position.subject)
        && (!def.use_.is_empty() || def.action.as_deref() == Some("route"))
    {
        push(Diagnostic::new(
            Code::W013,
            format!(
                "no extraction exists for `{}` output — this rule never receives it, so it is inert",
                position.subject.as_str()
            ),
        ));
    }

    let mut current_format = def.when.get("format").and_then(|v| v.as_str());
    let mut live_methods = 0usize;
    for (index, name) in def.use_.iter().enumerate() {
        if PRO_METHODS.contains(&name.as_str()) {
            push(Diagnostic::new(
                Code::W009,
                format!("method `{name}` is not available in this edition — the rule is inert"),
            ));
            continue;
        }
        let info = registry::info_of(name).or_else(|| {
            registry::chain(name).map(registry::chain_info).or_else(|| {
                resolved
                    .methods
                    .get(name)
                    .and_then(|(d, _)| d.chain.as_ref())
                    .map(|entries| user_chain_info(resolved, entries))
            })
        });
        let Some(info) = info else {
            let known = registry::known_names();
            let hint = did_you_mean(name, known.iter().map(|s| s.as_str()))
                .map(|n| format!("did you mean `{n}`?"));
            let mut d = if def.action.is_some() {
                Diagnostic::new(Code::E007, format!("unknown method or chain `{name}`"))
            } else {
                Diagnostic::new(
                    Code::W014,
                    format!(
                        "unknown method or chain `{name}` — skipped at runtime, the rest of the chain still runs; the file still loads (newer rtk or a plugin you don't have?)"
                    ),
                )
            };
            d.hint = hint;
            push(d);
            continue;
        };

        if position.site == SiteKind::Hook
            && position.event == Event::AfterTool
            && info.cost > CostClass::Fast
        {
            push(Diagnostic::new(
                Code::W022,
                format!(
                    "method `{name}` is slow (cost {:?}) — every matching tool call waits for it after the tool runs",
                    info.cost
                ),
            ));
        }
        // A proxy-resident method has no implementation at a hook site: the
        // chain resolves without it and the rule runs as a no-op. Warned rather
        // than refused — an error discards the whole file, and a rule that does
        // nothing is not worth costing someone every other rule. Checked after
        // the cost rule so `ccr`, which is Slow, also gets W022.
        if position.site != SiteKind::Proxy {
            let offenders = proxy_members(resolved, name);
            if !offenders.is_empty() {
                let which = if offenders.len() == 1 && offenders[0] == *name {
                    format!("`{name}`")
                } else {
                    format!("`{name}` (via {})", offenders.join(", "))
                };
                push(Diagnostic::new(
                    Code::W011,
                    format!(
                        "{which} runs only in the proxy — it is skipped at {}, so this rule is inert",
                        position.render()
                    ),
                ));
                continue;
            }
        }
        if position.site == SiteKind::Proxy && !info.cache_stable {
            push(Diagnostic::new(
                Code::E010,
                format!("method `{name}` is not cache-stable and cannot run under `proxy.request`"),
            ));
        }
        if position.site == SiteKind::Proxy
            && info.cache_stable
            && !matches!(
                position.subject,
                Subject::User | Subject::System | Subject::Tools | Subject::Model
            )
        {
            let skipped: Vec<(String, &'static str)> = chain_members(resolved, name)
                .into_iter()
                .filter_map(|m| registry::proxy_skip_reason(&m, position.subject).map(|r| (m, r)))
                .collect();
            if let Some((_, reason)) = skipped.first() {
                let which = if skipped.len() == 1 && skipped[0].0 == *name {
                    format!("`{name}`")
                } else {
                    let names: Vec<&str> = skipped.iter().map(|(m, _)| m.as_str()).collect();
                    format!("`{name}` (via {})", names.join(", "))
                };
                push(Diagnostic::new(
                    Code::W016,
                    format!(
                        "{which} is skipped at {} — {reason}; the file still loads",
                        position.render()
                    ),
                ));
            }
        }
        live_methods += 1;
        if info.terminal && index + 1 != def.use_.len() {
            push(Diagnostic::new(
                Code::E008,
                format!("method `{name}` is terminal and must be last in the chain"),
            ));
        }
        if !info.consumes.is_empty() {
            if let Some(format) = current_format {
                if !info.consumes.iter().any(|f| f.as_str() == format) {
                    push(Diagnostic::new(
                        Code::E008,
                        format!("method `{name}` does not consume `{format}`"),
                    ));
                }
            }
        }
        match info.produces {
            crate::method::Produces::Same => {}
            crate::method::Produces::Format(format) => current_format = Some(format.as_str()),
        }
    }

    // The consequence worth naming on its own: whatever the cause, a rule that
    // resolves to no methods matches, reports success and changes nothing.
    // `explain` already shows this as `-> []`; it should not pass validate in
    // silence.
    if !def.use_.is_empty() && live_methods == 0 {
        push(Diagnostic::new(
            Code::W012,
            "every method in this rule is skipped here — the rule matches and does nothing"
                .to_string(),
        ));
    }
}

/// The proxy-resident methods `name` would contribute at a non-proxy site,
/// looking through a chain so one cannot be smuggled in via `[method.*]`.
pub fn after_tool_extractable(subject: Subject) -> bool {
    matches!(
        subject,
        Subject::Read
            | Subject::Shell
            | Subject::Search
            | Subject::Web
            | Subject::Mcp
            | Subject::Subagent
            | Subject::Any
    )
}

fn references_classifier(def: &super::schema::RuleDef) -> bool {
    std::iter::once(&def.when)
        .chain(def.any.iter())
        .flat_map(|group| group.keys())
        .any(|key| super::conditions::classifier_condition(&split_key(key).0).is_some())
}

pub const ROUTE_MODES: &[&str] = &["suggest", "swap", "answer"];

fn route_mode_problem(position: Position, action: Option<&str>, mode: &str) -> Option<String> {
    if action != Some("route") {
        return Some("`mode` only applies to a `route` rule".to_string());
    }
    if !ROUTE_MODES.contains(&mode) {
        return Some(format!(
            "unknown route mode `{mode}` — expected `suggest`, `swap` or `answer`"
        ));
    }
    if mode == "suggest" {
        return None;
    }
    if position.site != SiteKind::Hook || position.event != Event::BeforeTool {
        return Some(format!(
            "`mode = \"{mode}\"` only applies before a tool — after a tool a route already replaces the output in place"
        ));
    }
    match (mode, position.subject) {
        ("swap", Subject::Shell) => None,
        ("swap", _) => Some(
            "`mode = \"swap\"` only applies to `shell` — other tools cannot be replaced by a command; use `answer`"
                .to_string(),
        ),
        ("answer", Subject::Read | Subject::Search | Subject::Web) => None,
        ("answer", Subject::Shell) => Some(
            "`mode = \"answer\"` does not apply to `shell` — use `swap`, which runs the task as the agent's own command"
                .to_string(),
        ),
        ("answer", other) => Some(format!(
            "`mode = \"answer\"` applies to `read`, `search` and `web`, not `{}`",
            other.as_str()
        )),
        _ => None,
    }
}

pub fn route_kind_mismatch(position: Position, kind: &str) -> Option<&'static str> {
    if !["summarize", "digest", "generate"].contains(&kind) {
        return None;
    }
    let (fits, reason): (&[&str], &'static str) = match (position.event, position.subject) {
        (Event::AfterTool, _) => (
            &["digest"],
            "after a tool only a `digest` task can replace the output",
        ),
        (_, Subject::Shell | Subject::Web | Subject::Search) => (
            &["digest"],
            "this call is handed over as a command, which only `digest` runs",
        ),
        (_, Subject::Read) => (
            &["summarize", "digest"],
            "a read is handed over as a file path (`summarize` or `digest`)",
        ),
        (_, Subject::Write) => (
            &["generate"],
            "writing a file is delegated with a spec and a target (`generate`)",
        ),
        (_, Subject::Subagent) => (
            &["summarize"],
            "a subagent's question is answered over files (`summarize`)",
        ),
        _ => (
            &[],
            "no built-in task kind can take this call — use a plugin kind",
        ),
    };
    (!fits.contains(&kind)).then_some(reason)
}

fn chain_members(resolved: &Resolved, name: &str) -> Vec<String> {
    if let Some(chain) = registry::chain(name) {
        return chain.iter().map(|m| m.name().to_string()).collect();
    }
    if let Some(entries) = resolved
        .methods
        .get(name)
        .and_then(|(def, _)| def.chain.as_ref())
    {
        return entries
            .iter()
            .flat_map(|e| match registry::chain(e) {
                Some(chain) => chain.iter().map(|m| m.name().to_string()).collect(),
                None => vec![e.clone()],
            })
            .collect();
    }
    vec![name.to_string()]
}

pub fn proxy_members(resolved: &Resolved, name: &str) -> Vec<String> {
    if registry::is_proxy_resident(name) {
        return vec![name.to_string()];
    }
    let entries: Vec<String> = if let Some(chain) = registry::chain(name) {
        chain.iter().map(|m| m.name().to_string()).collect()
    } else if let Some((def, _)) = resolved.methods.get(name) {
        def.chain.clone().unwrap_or_default()
    } else {
        Vec::new()
    };
    entries
        .into_iter()
        .filter(|e| registry::is_proxy_resident(e))
        .collect()
}

fn user_chain_info(resolved: &Resolved, entries: &[String]) -> crate::method::MethodInfo {
    let mut info = crate::method::MethodInfo::LOSSLESS_TRANSFORM;
    let member = |entry: &String| {
        registry::info_of(entry).or_else(|| registry::chain(entry).map(registry::chain_info))
    };
    for entry in entries {
        if let Some(entry_info) = member(entry) {
            if entry_info.lossy {
                info.lossy = true;
            }
            if entry_info.cost > info.cost {
                info.cost = entry_info.cost;
            }
            if !entry_info.cache_stable {
                info.cache_stable = false;
            }
        }
    }
    // Same trio as `registry::chain_info`, same reason: without it a named
    // user chain would validate as consume-anything/never-terminal and bypass
    // E008 for the methods it wraps.
    if let Some(entry_info) = entries.first().and_then(member) {
        info.consumes = entry_info.consumes;
    }
    if let Some(entry_info) = entries.last().and_then(member) {
        info.produces = entry_info.produces;
        info.terminal = entry_info.terminal;
    }
    let _ = resolved;
    info
}

#[cfg(test)]
mod tests {
    use super::super::resolve::{Scope, SourceFile, resolve};
    use super::*;

    fn diagnostics_for(text: &str) -> Vec<Diagnostic> {
        let parsed = super::super::schema::parse(text, "u.toml");
        let mut all = parsed.diagnostics;
        let file = parsed.file.expect("parses");
        let (resolved, resolve_diags) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "u.toml".to_string(),
            file,
        }]);
        all.extend(resolve_diags);
        all.extend(validate(&resolved, &ValidateCtx::default()));
        all
    }

    const CLASSIFY_OK: &str = r#"
schema_version = 1
[pipeline]
extends = "rtk:posthook"

[upstream.judge]
provider    = "judge"
model       = "judge-v1"
base_url    = "https://judge.example.com/v1/systemone"
api_key_env = "JUDGE_API_KEY"

[classifier.intent]
upstream = "judge"
question = "choice"
instructions = "What kind of tool output is this?"

[classifier.intent.labels]
test_failure = "Failing test output"
build_log    = "Compiler or build output"

[[hook.after_tool.shell]]
name = "route-failures"
when = { "class.intent" = "test_failure", tokens_gt = 1500 }
use  = ["term"]
"#;

    #[test]
    fn classifier_pipeline_validates_and_is_referenced() {
        use super::super::position::Position;
        use crate::block::{Event, Subject};
        let parsed = super::super::schema::parse(CLASSIFY_OK, "u.toml");
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let (resolved, diags) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "u.toml".to_string(),
            file: parsed.file.expect("parses"),
        }]);
        assert!(diags.is_empty(), "{diags:#?}");
        let validate_diags = validate(&resolved, &ValidateCtx::default());
        assert!(
            !validate_diags.iter().any(|d| d.code == Code::E019),
            "{validate_diags:#?}"
        );
        let (def, _) = &resolved.classifiers["intent"];
        assert_eq!(def.question, "choice");
        assert_eq!(def.labels.len(), 2);
        let referenced =
            resolved.classifiers_referenced(Position::hook(Event::AfterTool, Subject::Shell));
        assert_eq!(referenced.into_iter().collect::<Vec<_>>(), vec!["intent"]);
        let none = resolved.classifiers_referenced(Position::hook(Event::AfterTool, Subject::Read));
        assert!(none.is_empty(), "no read rule classifies");
    }

    #[test]
    fn classifier_condition_errors_are_e019() {
        assert_code(
            &CLASSIFY_OK.replace("\"class.intent\"", "\"class.missing\""),
            Code::E019,
        );
        assert_code(
            &CLASSIFY_OK.replace(
                "\"class.intent\" = \"test_failure\"",
                "\"class.intent\" = \"not_a_label\"",
            ),
            Code::E019,
        );
        assert_code(
            &CLASSIFY_OK.replace("question = \"choice\"", "question = \"vibes\""),
            Code::E019,
        );
        assert_code(
            &CLASSIFY_OK.replace("upstream = \"judge\"", "upstream = \"ghost\""),
            Code::E019,
        );
        assert_code(
            &CLASSIFY_OK.replace(
                "\"class.intent\" = \"test_failure\"",
                "\"score.intent_gt\" = 0.5",
            ),
            Code::E019,
        );
    }

    #[test]
    fn classifier_keys_work_before_and_after_a_tool() {
        use super::super::conditions::{KeyCheck, check_key};
        use super::super::position::Position;
        use crate::block::{Event, Subject};
        assert!(matches!(
            check_key(
                Position::hook(Event::AfterTool, Subject::Shell),
                "class.intent"
            ),
            KeyCheck::Ok
        ));
        assert!(matches!(
            check_key(
                Position::hook(Event::AfterTool, Subject::Read),
                "score.quality"
            ),
            KeyCheck::Ok
        ));
        for subject in [
            Subject::Shell,
            Subject::Read,
            Subject::Web,
            Subject::Mcp,
            Subject::Subagent,
        ] {
            assert!(matches!(
                check_key(Position::hook(Event::BeforeTool, subject), "class.intent"),
                KeyCheck::Ok
            ));
        }
        assert!(matches!(
            check_key(Position::proxy(Subject::ToolResult), "class.intent"),
            KeyCheck::WrongEvent { .. }
        ));
    }

    fn assert_code(text: &str, code: Code) {
        let diagnostics = diagnostics_for(text);
        assert!(
            diagnostics.iter().any(|d| d.code == code),
            "expected {code:?} in: {diagnostics:#?}"
        );
    }

    #[test]
    fn the_builtins_validate_clean() {
        for extends in [
            "rtk:default",
            "rtk:posthook",
            "rtk:proxy",
            "rtk:delegate-reads",
            "rtk:terse",
        ] {
            let text = format!("[pipeline]\nextends = \"{extends}\"\n");
            let diagnostics: Vec<Diagnostic> = diagnostics_for(&text)
                .into_iter()
                .filter(|d| d.code != Code::W007)
                .collect();
            assert!(diagnostics.is_empty(), "{extends}: {diagnostics:#?}");
        }
    }

    #[test]
    fn delegate_reads_without_upstream_is_w007_inert() {
        assert_code("[pipeline]\nextends = \"rtk:delegate-reads\"\n", Code::W007);
    }

    #[test]
    fn e003_condition_at_the_wrong_event() {
        assert_code(
            "[[hook.after_tool.read]]\nname = \"x\"\nwhen = { file_lines_gt = 100 }\nuse = [\"lockfile\"]\n",
            Code::E003,
        );
    }

    #[test]
    fn e004_condition_for_the_wrong_subject() {
        assert_code(
            "[[hook.before_tool.read]]\nname = \"x\"\nwhen = { command = \"ls\" }\naction = \"deny\"\n",
            Code::E004,
        );
    }

    /// A proxy-resident method at a hook site has no implementation there: the
    /// chain resolves without it and the rule runs as a no-op. Warned, not
    /// refused — an error discards the whole file, and the rule is harmless.
    /// A chain on a subject with no extraction arm fires and receives
    /// nothing — the same silent-no-op class as W011, per anchor.
    #[test]
    fn w013_unextractable_after_tool_subject_is_inert() {
        for subject in ["write", "edit"] {
            let text = format!("[[hook.after_tool.{subject}]]\nname = \"x\"\nuse = [\"ansi\"]\n");
            assert_code(&text, Code::W013);
            assert!(
                !crate::pipeline::has_errors(&diagnostics_for(&text)),
                "{subject} must warn, not error"
            );
        }
    }

    #[test]
    fn w013_does_not_fire_where_extraction_exists() {
        for subject in ["read", "shell", "search", "web", "mcp", "subagent", "any"] {
            let diags = diagnostics_for(&format!(
                "[[hook.after_tool.{subject}]]\nname = \"x\"\nuse = [\"ansi\"]\n"
            ));
            assert!(
                !diags.iter().any(|d| d.code == Code::W013),
                "{subject}: {diags:#?}"
            );
        }
    }

    #[test]
    fn w011_proxy_method_outside_the_proxy_is_inert() {
        for method in ["cache-align", "retire-pointer"] {
            let text = format!("[[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"{method}\"]\n");
            assert_code(&text, Code::W011);
            // A warning must not cost the user the rest of their file.
            assert!(
                !crate::pipeline::has_errors(&diagnostics_for(&text)),
                "{method} must not be an error"
            );
        }
    }

    fn codes_in(scope: Scope, text: &str) -> Vec<Code> {
        let parsed = super::super::schema::parse(text, "p.toml");
        let (resolved, _) = resolve(&[SourceFile {
            scope,
            label: "p.toml".to_string(),
            file: parsed.file.expect("parses"),
        }]);
        validate(&resolved, &ValidateCtx::default())
            .into_iter()
            .map(|d| d.code)
            .collect()
    }

    const PROXY_MD: &str = "[[proxy.request.tool_result]]\nname = \"x\"\nuse = [\"web-md\"]\n";

    #[test]
    fn w017_flags_proxy_rules_in_a_project_file_only() {
        assert!(codes_in(Scope::Project, PROXY_MD).contains(&Code::W017));
        assert!(!codes_in(Scope::User, PROXY_MD).contains(&Code::W017));
        let hook = "[[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"term\"]\n";
        assert!(!codes_in(Scope::Project, hook).contains(&Code::W017));
    }

    #[test]
    fn w018_flags_shadow_on_a_proxy_rule_only() {
        let shadow = format!("{PROXY_MD}shadow = true\n");
        assert!(codes_in(Scope::User, &shadow).contains(&Code::W018));
        assert!(!codes_in(Scope::User, PROXY_MD).contains(&Code::W018));
        let hook = "[[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"term\"]\nshadow = true\n";
        assert!(!codes_in(Scope::User, hook).contains(&Code::W018));
    }

    #[test]
    fn w019_flags_undelivered_directive_channels() {
        for channel in ["session_start", "per_turn"] {
            let text = format!(
                "[[directive]]\nname = \"d\"\ntext = \"be brief\"\nchannel = \"{channel}\"\n"
            );
            assert!(
                codes_in(Scope::User, &text).contains(&Code::W019),
                "{channel}"
            );
        }
        let ok = "[[directive]]\nname = \"d\"\ntext = \"be brief\"\nchannel = \"awareness_file\"\n";
        assert!(!codes_in(Scope::User, ok).contains(&Code::W019));
    }

    #[test]
    fn new_warnings_never_fire_on_shipped_pipelines() {
        for name in crate::pipeline::BUILTIN_NAMES {
            let codes = codes_in(Scope::User, &format!("[pipeline]\nextends = \"{name}\"\n"));
            for code in [Code::W017, Code::W018, Code::W019] {
                assert!(!codes.contains(&code), "{name}: {code:?}");
            }
        }
    }

    fn w016(text: &str) -> Vec<Diagnostic> {
        diagnostics_for(text)
            .into_iter()
            .filter(|d| d.code == Code::W016)
            .collect()
    }

    #[test]
    fn w016_flags_a_proxy_method_that_needs_a_recall_copy() {
        let text = "[[proxy.request.tool_result]]\nname = \"x\"\nuse = [\"cap\"]\n";
        let found = w016(text);
        assert_eq!(found.len(), 1, "{:#?}", diagnostics_for(text));
        assert!(found[0].message.contains("recall"), "{:?}", found[0]);
        assert!(!crate::pipeline::has_errors(&diagnostics_for(text)));
    }

    #[test]
    fn w016_stays_quiet_for_methods_the_proxy_runs() {
        for method in [
            "web-md",
            "md-slim",
            "base64-elide",
            "lockfile",
            "ansi",
            "dedup",
        ] {
            let text =
                format!("[[proxy.request.tool_result]]\nname = \"x\"\nuse = [\"{method}\"]\n");
            assert!(w016(&text).is_empty(), "{method}: {:#?}", w016(&text));
        }
        for text in [
            "[[proxy.request.cache]]\nname = \"x\"\nuse = [\"cache-align\"]\n",
            "[[proxy.request.tool_use]]\nname = \"x\"\nuse = [\"retire-pointer\"]\n",
            "[[proxy.request.tool_result]]\nname = \"x\"\nuse = [\"ccr\"]\n",
        ] {
            assert!(w016(text).is_empty(), "{text}: {:#?}", w016(text));
        }
    }

    #[test]
    fn w016_flags_a_method_at_a_part_the_proxy_does_not_transform() {
        let text = "[[proxy.request.assistant]]\nname = \"x\"\nuse = [\"web-md\"]\n";
        assert_eq!(w016(text).len(), 1, "{:#?}", diagnostics_for(text));
    }

    #[test]
    fn w016_names_the_skipped_members_of_a_chain() {
        let found = w016("[[proxy.request.tool_result]]\nname = \"x\"\nuse = [\"term\"]\n");
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("cap"), "{:?}", found[0]);
    }

    #[test]
    fn w016_never_fires_on_shipped_pipelines() {
        for name in crate::pipeline::BUILTIN_NAMES {
            let text = format!("[pipeline]\nextends = \"{name}\"\n");
            assert!(w016(&text).is_empty(), "{name}: {:#?}", w016(&text));
        }
    }

    /// The same method smuggled through a user chain must not slip past.
    #[test]
    fn w011_looks_through_a_user_chain() {
        let diags = diagnostics_for(
            "[method.mine]\nchain = [\"ansi\", \"cache-align\"]\n\
             [[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"mine\"]\n",
        );
        let w011: Vec<_> = diags.iter().filter(|d| d.code == Code::W011).collect();
        assert_eq!(w011.len(), 1, "{diags:#?}");
        assert!(w011[0].message.contains("cache-align"), "{:?}", w011[0]);
    }

    #[test]
    fn w011_does_not_fire_where_the_method_belongs() {
        let diags =
            diagnostics_for("[[proxy.request.cache]]\nname = \"align\"\nuse = [\"cache-align\"]\n");
        assert!(!diags.iter().any(|d| d.code == Code::W011), "{diags:#?}");
    }

    #[test]
    fn w012_flags_a_rule_that_resolves_to_nothing() {
        assert_code(
            "[[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"cache-align\"]\n",
            Code::W012,
        );
    }

    /// One surviving method means the rule still does something: W011 for the
    /// misplaced entry, but no claim that the rule is a no-op.
    #[test]
    fn w012_is_silent_when_a_method_survives() {
        let diags = diagnostics_for(
            "[[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"ansi\", \"cache-align\"]\n",
        );
        assert!(diags.iter().any(|d| d.code == Code::W011), "{diags:#?}");
        assert!(!diags.iter().any(|d| d.code == Code::W012), "{diags:#?}");
    }

    #[test]
    fn e005_action_not_legal_at_position() {
        assert_code(
            "[[hook.before_tool.shell]]\nname = \"x\"\nuse = [\"term\"]\n",
            Code::E005,
        );
        assert_code(
            "[[hook.after_tool.shell]]\nname = \"x\"\naction = \"deny\"\n",
            Code::E005,
        );
    }

    /// A route substitutes a worker's answer for the tool's own result. After a
    /// read, write or edit that would break edit-after-read: the agent asked
    /// for the file it is about to modify, and there is no question to answer.
    #[test]
    fn route_is_legal_at_every_hook_position_but_before_an_edit() {
        for event in ["before_tool", "after_tool"] {
            for subject in [
                "read", "write", "edit", "shell", "search", "web", "subagent", "mcp", "any",
            ] {
                if event == "before_tool" && subject == "edit" {
                    continue;
                }
                let text = format!(
                    "[task.t]\nkind = \"digest\"\n[[hook.{event}.{subject}]]\nname = \"x\"\naction = \"route\"\ntask = \"t\"\n"
                );
                assert!(
                    !diagnostics_for(&text).iter().any(|d| d.code == Code::E005),
                    "{event}.{subject}: {:#?}",
                    diagnostics_for(&text)
                );
            }
        }
    }

    #[test]
    fn call_context_keys_work_at_every_hook_position() {
        for event in ["before_tool", "after_tool"] {
            for subject in ["read", "shell", "web", "mcp", "subagent", "any"] {
                let text = format!(
                    "[[hook.{event}.{subject}]]\nname = \"x\"\nwhen = {{ intent_matches = \"(?i)edit\", goal_matches = \"fix\", recent_calls_matches = \"Edit\" }}\naction = \"passthrough\"\n"
                );
                let diags = diagnostics_for(&text);
                assert!(
                    !crate::pipeline::has_errors(&diags),
                    "{event}.{subject}: {diags:#?}"
                );
            }
        }
        let proxy = "[[proxy.request.tool_result]]\nname = \"x\"\nwhen = { intent_matches = \"x\" }\nuse = [\"ansi\"]\n";
        assert!(
            diagnostics_for(proxy)
                .iter()
                .any(|d| matches!(d.code, Code::E004 | Code::W015 | Code::E003))
        );
    }

    #[test]
    fn w021_route_after_a_read_warns_about_edits() {
        let text = "[task.t]\nkind = \"digest\"\n[[hook.after_tool.read]]\nname = \"x\"\naction = \"route\"\ntask = \"t\"\n";
        assert_code(text, Code::W021);
        assert!(!crate::pipeline::has_errors(&diagnostics_for(text)));
    }

    #[test]
    fn w023_names_a_task_kind_that_cannot_take_the_call() {
        let routed = |pos: &str, kind: &str| {
            diagnostics_for(&format!(
                "[task.t]\nkind = \"{kind}\"\n[[hook.{pos}]]\nname = \"x\"\naction = \"route\"\ntask = \"t\"\n"
            ))
            .iter()
            .any(|d| d.code == Code::W023)
        };
        for (pos, kind) in [
            ("before_tool.shell", "digest"),
            ("before_tool.web", "digest"),
            ("before_tool.read", "summarize"),
            ("before_tool.write", "generate"),
            ("before_tool.subagent", "summarize"),
            ("after_tool.mcp", "digest"),
        ] {
            assert!(!routed(pos, kind), "{pos} + {kind} fits");
        }
        for (pos, kind) in [
            ("before_tool.shell", "summarize"),
            ("before_tool.write", "digest"),
            ("before_tool.mcp", "digest"),
            ("after_tool.shell", "summarize"),
        ] {
            assert!(routed(pos, kind), "{pos} + {kind} must warn");
        }
    }

    #[test]
    fn route_before_an_edit_is_e005() {
        let rule = |action: &str| {
            diagnostics_for(&format!(
                "[task.t]\nkind = \"generate\"\n[[hook.before_tool.edit]]\nname = \"x\"\naction = \"{action}\"\ntask = \"t\"\n"
            ))
        };
        assert!(rule("route").iter().any(|d| d.code == Code::E005));
        assert!(!rule("deny").iter().any(|d| d.code == Code::E005));
        assert!(!rule("passthrough").iter().any(|d| d.code == Code::E005));
    }

    fn mode_rule(position: &str, kind: &str, extra: &str) -> String {
        format!(
            "[upstream.w]\nprovider = \"anthropic\"\nmodel = \"m\"\n[task.t]\nkind = \"{kind}\"\nupstream = \"w\"\n[[hook.{position}]]\nname = \"x\"\nwhen = {{ tokens_gt = 1 }}\n{extra}\n"
        )
    }

    #[test]
    fn route_modes_are_legal_where_they_can_work() {
        let ok = |position: &str, kind: &str, mode: &str| {
            let text = mode_rule(
                position,
                kind,
                &format!("action = \"route\"\ntask = \"t\"\nmode = \"{mode}\""),
            )
            .replace("tokens_gt = 1", "tool = \"x\"");
            let diags = diagnostics_for(&text);
            !diags
                .iter()
                .any(|d| d.code == Code::E005 || d.code == Code::E002)
        };
        assert!(ok("before_tool.shell", "digest", "swap"));
        assert!(ok("before_tool.read", "summarize", "answer"));
        assert!(ok("before_tool.search", "digest", "answer"));
        assert!(ok("before_tool.web", "digest", "answer"));
        assert!(ok("before_tool.read", "summarize", "suggest"));
        assert!(ok("after_tool.shell", "digest", "suggest"));
        assert!(!ok("before_tool.read", "summarize", "swap"));
        assert!(!ok("before_tool.shell", "digest", "answer"));
        assert!(!ok("before_tool.subagent", "summarize", "answer"));
        assert!(!ok("after_tool.shell", "digest", "answer"));
        assert!(!ok("before_tool.shell", "digest", "inline"));
    }

    #[test]
    fn mode_only_applies_to_route() {
        let text = mode_rule(
            "before_tool.shell",
            "digest",
            "action = \"deny\"\nmode = \"swap\"",
        )
        .replace("tokens_gt = 1", "command_matches = \"x\"");
        assert_code(&text, Code::E005);
    }

    #[test]
    fn context_is_a_boolean_on_tasks_and_classifiers() {
        let text = "[upstream.w]\nprovider = \"anthropic\"\nmodel = \"m\"\n[task.t]\nkind = \"digest\"\nupstream = \"w\"\ncontext = true\n[classifier.c]\nupstream = \"w\"\nquestion = \"noul\"\ncontext = true\n";
        let parsed = super::super::schema::parse(text, "u.toml");
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let file = parsed.file.expect("parses");
        assert!(file.tasks["t"].context);
        assert!(file.classifiers["c"].context);
        let bad = super::super::schema::parse(
            "[task.t]\nkind = \"digest\"\ncontext = \"yes\"\n",
            "u.toml",
        );
        assert!(bad.diagnostics.iter().any(|d| d.code == Code::E002));
    }

    #[test]
    fn e020_deny_cannot_depend_on_a_classifier() {
        let judge = "[upstream.j]\nprovider = \"anthropic\"\nmodel = \"m\"\nbase_url = \"http://127.0.0.1:1\"\napi_key_env = \"K\"\n[classifier.big]\nupstream = \"j\"\nquestion = \"noul\"\ninstructions = \"is it big?\"\n[task.t]\nkind = \"digest\"\nupstream = \"j\"\n";
        let deny = format!(
            "{judge}[[hook.before_tool.shell]]\nname = \"x\"\nwhen = {{ \"noul.big_gte\" = 0.9 }}\naction = \"deny\"\n"
        );
        assert_code(&deny, Code::E020);
        let route = format!(
            "{judge}[[hook.before_tool.shell]]\nname = \"x\"\nwhen = {{ \"noul.big_gte\" = 0.9 }}\naction = \"route\"\ntask = \"t\"\n"
        );
        let diags = diagnostics_for(&route);
        assert!(!crate::pipeline::has_errors(&diags), "{diags:#?}");
        let fixed = format!(
            "{judge}[[hook.before_tool.shell]]\nname = \"x\"\nwhen = {{ command_matches = \"rm\" }}\naction = \"deny\"\n"
        );
        assert!(!diagnostics_for(&fixed).iter().any(|d| d.code == Code::E020));
    }
    #[test]
    fn route_is_legal_where_a_worker_can_answer_for_the_tool() {
        let positions = [
            "hook.before_tool.read",
            "hook.before_tool.shell",
            "hook.after_tool.shell",
            "hook.after_tool.search",
            "hook.after_tool.web",
            "hook.after_tool.mcp",
        ];
        for position in positions {
            let text = format!(
                "[upstream.u]\nprovider = \"google\"\nmodel = \"m\"\n\
                 api_key_env = \"K\"\n[task.t]\nkind = \"digest\"\nupstream = \"u\"\n\
                 [[{position}]]\nname = \"x\"\naction = \"route\"\ntask = \"t\"\n"
            );
            let diagnostics = diagnostics_for(&text);
            assert!(
                !diagnostics.iter().any(|d| d.code == Code::E005),
                "{position}: {diagnostics:#?}"
            );
        }
    }

    #[test]
    fn e006_use_and_action_are_exclusive_and_required() {
        assert_code(
            "[[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"term\"]\naction = \"passthrough\"\n",
            Code::E006,
        );
        assert_code("[[hook.after_tool.shell]]\nname = \"x\"\n", Code::E006);
    }

    #[test]
    fn w014_unknown_method_in_a_transform_rule_degrades_with_hint() {
        let diagnostics =
            diagnostics_for("[[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"trem\"]\n");
        let w014 = diagnostics
            .iter()
            .find(|d| d.code == Code::W014)
            .expect("W014");
        assert!(w014.hint.as_deref().unwrap_or("").contains("term"));
        assert!(
            !crate::pipeline::has_errors(&diagnostics),
            "a transform rule with an unknown method must not discard the file: {diagnostics:#?}"
        );
    }

    #[test]
    fn w015_unknown_condition_in_a_transform_rule_degrades() {
        let text =
            "[[hook.after_tool.shell]]\nname = \"x\"\nwhen = { bogus_key = 1 }\nuse = [\"term\"]\n";
        let diagnostics = diagnostics_for(text);
        assert!(
            diagnostics.iter().any(|d| d.code == Code::W015),
            "{diagnostics:#?}"
        );
        assert!(
            !crate::pipeline::has_errors(&diagnostics),
            "a transform rule with an unknown condition must not discard the file: {diagnostics:#?}"
        );
    }

    #[test]
    fn server_condition_is_valid_for_the_mcp_subject() {
        let text = "[[hook.before_tool.mcp]]\nname = \"x\"\nwhen = { server = \"linear\" }\naction = \"deny\"\nmessage = \"ask first\"\n";
        let diagnostics = diagnostics_for(text);
        assert!(
            !diagnostics
                .iter()
                .any(|d| matches!(d.code, Code::E003 | Code::E004)),
            "{diagnostics:#?}"
        );
        let text = "[[hook.before_tool.shell]]\nname = \"x\"\nwhen = { server = \"linear\" }\naction = \"deny\"\n";
        let diagnostics = diagnostics_for(text);
        assert!(
            diagnostics.iter().any(|d| d.code == Code::E004),
            "`server` must stay mcp-only: {diagnostics:#?}"
        );
    }

    #[test]
    fn unknown_condition_on_a_control_rule_stays_a_hard_error() {
        let text = "[[hook.before_tool.read]]\nname = \"x\"\nwhen = { bogus_key = 1 }\naction = \"deny\"\n";
        let diagnostics = diagnostics_for(text);
        let e004 = diagnostics
            .iter()
            .find(|d| d.code == Code::E004)
            .expect("a deny rule with an unknown condition must fail closed");
        assert!(e004.hint.as_deref().unwrap_or("").contains("fail closed"));
        assert!(crate::pipeline::has_errors(&diagnostics));
    }

    #[test]
    fn e008_terminal_not_last() {
        assert_code(
            "[[hook.after_tool.read]]\nname = \"x\"\nwhen = { format = \"json\" }\nuse = [\"toon\", \"cap\"]\n",
            Code::E008,
        );
    }

    #[test]
    fn w022_slow_method_after_tool_warns() {
        let text = "[[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"ccr\"]\n";
        assert_code(text, Code::W022);
        assert!(!crate::pipeline::has_errors(&diagnostics_for(text)));
    }

    #[test]
    fn e010_turn_varying_condition_on_a_cached_part() {
        assert_code(
            "[[proxy.request.tool_result]]\nname = \"x\"\nwhen = { stream = true }\nuse = [\"ccr\"]\n",
            Code::E010,
        );
    }

    #[test]
    fn e014_task_errors() {
        assert_code("[task.review]\nkind = \"judge\"\n", Code::E014);
        assert_code(
            "[task.review]\nkind = \"summarize\"\nupstream = \"missing\"\n",
            Code::E014,
        );
        assert_code(
            "[[hook.before_tool.read]]\nname = \"x\"\naction = \"route\"\ntask = \"nope\"\n",
            Code::E014,
        );
    }

    #[test]
    fn e015_literal_api_key() {
        assert_code(
            "[upstream.x]\nprovider = \"openai\"\nmodel = \"gpt\"\napi_key = \"sk-1\"\n",
            Code::E015,
        );
    }

    #[test]
    fn e016_filter_keys_in_a_method_table() {
        let diagnostics = diagnostics_for(
            "[method.my_ci]\nmatch_command = \"^ci\\\\b\"\nstrip_lines_matching = [\"^INFO\"]\n",
        );
        let e016 = diagnostics
            .iter()
            .find(|d| d.code == Code::E016)
            .expect("E016");
        assert!(e016.message.contains("filters.toml"));
    }

    #[test]
    fn e016_chain_on_a_builtin_method_name() {
        assert_code("[method.ansi]\nchain = [\"dedup\"]\n", Code::E016);
    }

    #[test]
    fn w001_unreachable_rule() {
        assert_code(
            "[[hook.after_tool.shell]]\nname = \"all\"\nuse = [\"term\"]\n[[hook.after_tool.shell]]\nname = \"never\"\nuse = [\"ansi\"]\n",
            Code::W001,
        );
    }

    #[test]
    fn w004_deny_under_write() {
        assert_code(
            "[[hook.before_tool.write]]\nname = \"x\"\naction = \"deny\"\nmessage = \"no\"\n",
            Code::W004,
        );
    }

    #[test]
    fn w005_proxy_rules_without_the_proxy() {
        // Extending a builtin must NOT warn -- the user cannot act on a
        // built-in rule being inert. Writing your own proxy rule must.
        let parsed = super::super::schema::parse(
            "[pipeline]\nextends = \"rtk:proxy\"\n\
             [[proxy.request.tool_result]]\nname = \"mine\"\n\
             when = { tokens_gt = 1500 }\nuse = [\"ccr\"]\n",
            "u.toml",
        );
        let (resolved, _) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "u.toml".to_string(),
            file: parsed.file.expect("parses"),
        }]);
        let ctx = ValidateCtx {
            proxy_running: Some(false),
            ..ValidateCtx::default()
        };
        let diagnostics = validate(&resolved, &ctx);
        assert!(diagnostics.iter().any(|d| d.code == Code::W005));
    }

    #[test]
    fn w006_rewrite_disabled() {
        assert_code("[override.rtk-rewrite]\nenabled = false\n", Code::W006);
    }

    #[test]
    fn w008_v1_1_proxy_parts_are_inert() {
        assert_code(
            "[[proxy.request.system]]\nname = \"x\"\nuse = [\"toon\"]\n",
            Code::W008,
        );
    }

    #[test]
    fn w009_pro_methods_and_conditions() {
        assert_code(
            "[[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"redact\"]\n",
            Code::W009,
        );
        assert_code(
            "[[hook.before_tool.shell]]\nname = \"x\"\nwhen = { team_in = [\"core\"] }\naction = \"deny\"\n",
            Code::W009,
        );
    }

    #[test]
    fn e014_task_name_colliding_with_an_rtk_command() {
        let parsed = super::super::schema::parse("[task.gain]\nkind = \"summarize\"\n", "u.toml");
        let (resolved, _) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "u.toml".to_string(),
            file: parsed.file.expect("parses"),
        }]);
        let ctx = ValidateCtx {
            reserved_commands: &["gain", "init"],
            ..ValidateCtx::default()
        };
        let diagnostics = validate(&resolved, &ctx);
        assert!(diagnostics.iter().any(|d| d.code == Code::E014));
    }

    #[test]
    fn e014_task_name_rejected_by_the_host_predicate() {
        let parsed = super::super::schema::parse("[task.make]\nkind = \"digest\"\n", "u.toml");
        let (resolved, _) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "u.toml".to_string(),
            file: parsed.file.expect("parses"),
        }]);
        let ctx = ValidateCtx {
            is_reserved_command: Some(|name| name == "make"),
            ..ValidateCtx::default()
        };
        let diagnostics = validate(&resolved, &ctx);
        assert!(diagnostics.iter().any(|d| d.code == Code::E014));
        assert!(
            !validate(&resolved, &ValidateCtx::default())
                .iter()
                .any(|d| d.code == Code::E014)
        );
    }

    #[test]
    fn never_worse_cannot_be_disabled() {
        assert_code("[guards]\nnever_worse = false\n", Code::E002);
    }
}

#[cfg(test)]
mod rule_shape_checks {
    use super::*;
    use crate::pipeline::resolve::{Scope, SourceFile, resolve};
    use crate::pipeline::schema::parse;

    fn diags_and_resolved(text: &str) -> (crate::pipeline::Resolved, Vec<Diagnostic>) {
        let parsed = parse(text, "t.toml");
        let (resolved, mut diags) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "t.toml".into(),
            file: parsed.file.expect("parses"),
        }]);
        diags.extend(parsed.diagnostics);
        diags.extend(validate(&resolved, &ValidateCtx::default()));
        (resolved, diags)
    }

    const BASE: &str = "schema_version = 1\n[pipeline]\nname = \"p\"\n\
        [[hook.after_tool.shell]]\nname = \"r\"\nwhen = { tokens_gt = 100 }\nuse = [\"cap\"]\n";

    /// A typo'd override key must be named, not swallowed.
    #[test]
    fn override_key_typos_are_diagnosed() {
        let (_, diags) = diags_and_resolved(&format!("{BASE}[override.r]\nenabld = false\n"));
        assert!(
            diags
                .iter()
                .any(|d| d.code == Code::E002 && d.message.contains("enabld")),
            "unknown override key must raise E002: {diags:#?}"
        );
    }

    /// A mistyped value must be named; the rule keeps its old state
    /// either way, but silently is how a user believes they disabled a rule
    /// that still runs.
    #[test]
    fn override_type_mismatches_are_diagnosed() {
        let (resolved, diags) =
            diags_and_resolved(&format!("{BASE}[override.r]\nenabled = \"false\"\n"));
        let rule = resolved.find_rule("r").expect("rule");
        assert!(rule.def.enabled, "the mistyped value must not apply");
        assert!(
            diags
                .iter()
                .any(|d| d.code == Code::E002 && d.message.contains("boolean")),
            "string-for-bool must raise a type diagnostic: {diags:#?}"
        );
    }

    /// Wrapping a terminal method in a named chain must not bypass E008.
    #[test]
    fn named_chains_cannot_smuggle_a_terminal_method_past_e008() {
        let direct = "schema_version = 1\n[pipeline]\nname = \"p\"\n\
             [[hook.after_tool.read]]\nname = \"r\"\nuse = [\"toon\", \"cap\"]\n";
        let (_, d1) = diags_and_resolved(direct);
        assert!(
            d1.iter().any(|d| d.code == Code::E008),
            "control: direct use fires E008: {d1:#?}"
        );
        let wrapped = "schema_version = 1\n[pipeline]\nname = \"p\"\n\
             [method.mine]\nchain = [\"toon\"]\n\
             [[hook.after_tool.read]]\nname = \"r\"\nuse = [\"mine\", \"cap\"]\n";
        let (_, d2) = diags_and_resolved(wrapped);
        assert!(
            d2.iter().any(|d| d.code == Code::E008),
            "the wrapper must fire E008 exactly like the direct form: {d2:#?}"
        );
    }

    /// Options on a built-in method are parsed, validated, and consumed
    /// by nothing. Until they reach methods, accepting them would mislead.
    #[test]
    fn builtin_method_options_are_refused_not_discarded() {
        let (_, diags) = diags_and_resolved(
            "schema_version = 1\n[pipeline]\nname = \"p\"\n\
             [method.cap]\nwidth = 100\n\
             [[hook.after_tool.shell]]\nname = \"r\"\nuse = [\"cap\"]\n",
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == Code::E016 && d.message.contains("width")),
            "an option nothing consumes must be refused, not silently dropped: {diags:#?}"
        );
    }

    /// The replace-vs-merge decision for an override's `when` scans raw
    /// TOML; a multiline string containing header-shaped lines must not fool
    /// it into replace-mode, dropping keys (W003) the user never replaced.
    #[test]
    fn multiline_strings_do_not_fool_the_when_replace_scan() {
        let text = format!(
            "{BASE}[[directive]]\nname = \"doc\"\ntext = \"\"\"\n[override.r]\nwhen = {{ x = 1 }}\n\"\"\"\n\
             [override.r]\nwhen.bytes_gt = 5\n"
        );
        let (resolved, diags) = diags_and_resolved(&text);
        assert!(
            !diags.iter().any(|d| d.code == Code::W003),
            "dotted-key merge must not be treated as replace: {diags:#?}"
        );
        let rule = resolved.find_rule("r").expect("rule");
        assert!(
            rule.def.when.contains_key("tokens_gt") && rule.def.when.contains_key("bytes_gt"),
            "merge semantics: both keys survive: {:?}",
            rule.def.when
        );
    }
}
