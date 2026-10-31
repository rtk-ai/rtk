use super::builtins;
use super::conditions::{Condition, parse_when};
use super::diag::{Code, Diagnostic, did_you_mean};
use super::position::Position;
use super::schema::{
    ClassifierDef, DirectiveDef, MethodDef, OverrideDef, PipelineFile, RuleDef, TaskDef,
    UpstreamDef,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Builtin,
    User,
    Project,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Builtin => "builtin",
            Scope::User => "user",
            Scope::Project => "project",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub scope: Scope,
    pub label: String,
    pub file: PipelineFile,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guards {
    pub never_worse: bool,
    pub min_size: usize,
}

impl Default for Guards {
    fn default() -> Self {
        Guards {
            never_worse: true,
            min_size: 512,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedRule {
    pub def: RuleDef,
    pub name: String,
    pub position: Position,
    pub origin: String,
    pub scope: Scope,
    pub overridden_by: Vec<String>,
    /// `when`, parsed once. Conditions own compiled regexes; rebuilding them on
    /// every rule evaluation would recompile a `command_matches` pattern for
    /// each tool call, on the hook's 10 ms path. `None` means the
    /// conditions did not parse, and the rule can never match.
    pub when: Option<Vec<Condition>>,
    /// Each `any` group, parsed once, same reasoning.
    pub any: Vec<Option<Vec<Condition>>>,
}

#[derive(Debug, Clone)]
pub struct ResolvedDirective {
    pub def: DirectiveDef,
    pub name: String,
    pub origin: String,
    pub scope: Scope,
    pub overridden_by: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub name: String,
    pub extends: Vec<String>,
    pub rules: BTreeMap<Position, Vec<ResolvedRule>>,
    pub directives: Vec<ResolvedDirective>,
    pub methods: BTreeMap<String, (MethodDef, String)>,
    pub tasks: BTreeMap<String, (TaskDef, String)>,
    pub upstreams: BTreeMap<String, (UpstreamDef, String)>,
    pub classifiers: BTreeMap<String, (ClassifierDef, String)>,
    pub guards: Guards,
}

/// Parse a rule's conditions once, at resolve time. Invalid conditions become
/// `None`, which `rule_matches` treats as "never matches", decided once instead
/// of per tool call.
fn compile_conditions(def: &RuleDef) -> (Option<Vec<Condition>>, Vec<Option<Vec<Condition>>>) {
    let when = parse_when(&def.when).ok();
    let any = def.any.iter().map(|g| parse_when(g).ok()).collect();
    (when, any)
}

impl ResolvedRule {
    pub fn when_value(&self, key: &str) -> Option<&toml::Value> {
        self.def.when.get(key)
    }
}

pub fn effective_rule_name(def: &RuleDef) -> Option<(String, bool)> {
    if let Some(name) = &def.name {
        return Some((name.clone(), true));
    }
    if let Some(first) = def.use_.first() {
        return Some((first.clone(), false));
    }
    def.action.as_ref().map(|a| (a.clone(), false))
}

pub fn effective_directive_name(def: &DirectiveDef) -> Option<(String, bool)> {
    if let Some(name) = &def.name {
        return Some((name.clone(), true));
    }
    def.use_.as_ref().map(|u| (u.clone(), false))
}

fn extends_closure(local: &[&SourceFile], diagnostics: &mut Vec<Diagnostic>) -> Vec<&'static str> {
    let mut closure: Vec<&'static str> = Vec::new();
    let mut queue: Vec<String> = Vec::new();
    for source in local {
        for parent in &source.file.extends {
            if builtins::BUILTIN_NAMES.contains(&parent.as_str()) {
                queue.push(parent.clone());
            } else {
                diagnostics.push(
                    Diagnostic::new(
                        Code::E002,
                        format!(
                            "`extends` takes built-in pipeline names only; `{parent}` is not one"
                        ),
                    )
                    .at(source.label.clone(), None)
                    .hint(format!("built-ins: {}", builtins::BUILTIN_NAMES.join(", "))),
                );
            }
        }
    }
    let mut index = 0;
    while index < queue.len() {
        let name = queue[index].clone();
        index += 1;
        let Some(static_name) = builtins::BUILTIN_NAMES.iter().find(|n| **n == name) else {
            continue;
        };
        if closure.contains(static_name) {
            continue;
        }
        closure.push(static_name);
        if let Some(file) = builtins::builtin(static_name) {
            for parent in &file.extends {
                queue.push(parent.clone());
            }
        }
    }
    if !closure.contains(&builtins::RTK_DEFAULT) {
        closure.push(builtins::RTK_DEFAULT);
    }
    closure
}

pub fn resolve(local: &[SourceFile]) -> (Resolved, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let enabled: Vec<&SourceFile> = local.iter().filter(|s| s.file.enabled).collect();
    let closure = extends_closure(&enabled, &mut diagnostics);

    let mut resolved = Resolved {
        extends: closure.iter().map(|n| n.to_string()).collect(),
        ..Resolved::default()
    };

    let mut inherited_rule_names: BTreeMap<String, (&'static str, RuleDef)> = BTreeMap::new();
    let mut inherited_directive_names: Vec<String> = Vec::new();

    for name in &closure {
        let Some(file) = builtins::builtin(name) else {
            continue;
        };
        for (position, rules) in &file.rules {
            for def in rules {
                let Some((rule_name, _)) = effective_rule_name(def) else {
                    continue;
                };
                if let Some((other_origin, other_def)) = inherited_rule_names.get(&rule_name) {
                    if other_def != def {
                        diagnostics.push(
                            Diagnostic::new(
                                Code::E017,
                                format!(
                                    "`extends`: `{name}` and `{other_origin}` both define rule `{rule_name}` differently"
                                ),
                            ),
                        );
                    }
                    continue;
                }
                inherited_rule_names.insert(rule_name.clone(), (name, def.clone()));
                resolved.rules.entry(*position).or_default().push({
                    let (when, any) = compile_conditions(def);
                    ResolvedRule {
                        def: def.clone(),
                        name: rule_name,
                        position: *position,
                        origin: name.to_string(),
                        scope: Scope::Builtin,
                        overridden_by: Vec::new(),
                        when,
                        any,
                    }
                });
            }
        }
        for def in &file.directives {
            let Some((directive_name, _)) = effective_directive_name(def) else {
                continue;
            };
            if inherited_directive_names.contains(&directive_name) {
                continue;
            }
            inherited_directive_names.push(directive_name.clone());
            resolved.directives.push(ResolvedDirective {
                def: def.clone(),
                name: directive_name,
                origin: name.to_string(),
                scope: Scope::Builtin,
                overridden_by: Vec::new(),
            });
        }
        for (method_name, def) in &file.methods {
            resolved
                .methods
                .entry(method_name.clone())
                .or_insert_with(|| (def.clone(), name.to_string()));
        }
        for (task_name, def) in &file.tasks {
            resolved
                .tasks
                .entry(task_name.clone())
                .or_insert_with(|| (def.clone(), name.to_string()));
        }
        for (upstream_name, def) in &file.upstreams {
            resolved
                .upstreams
                .entry(upstream_name.clone())
                .or_insert_with(|| (def.clone(), name.to_string()));
        }
        for (classifier_name, def) in &file.classifiers {
            resolved
                .classifiers
                .entry(classifier_name.clone())
                .or_insert_with(|| (def.clone(), name.to_string()));
        }
    }

    let mut local_names: BTreeMap<String, (String, bool)> = BTreeMap::new();
    let mut local_rules: Vec<ResolvedRule> = Vec::new();
    let mut local_directives: Vec<ResolvedDirective> = Vec::new();

    for source in &enabled {
        for (position, rules) in &source.file.rules {
            for def in rules {
                let Some((rule_name, explicit)) = effective_rule_name(def) else {
                    continue;
                };
                register_local_name(
                    &rule_name,
                    explicit,
                    source,
                    def.line,
                    &mut local_names,
                    &inherited_rule_names,
                    &inherited_directive_names,
                    &mut diagnostics,
                );
                local_rules.push({
                    let (when, any) = compile_conditions(def);
                    ResolvedRule {
                        def: def.clone(),
                        name: rule_name,
                        position: *position,
                        origin: source.label.clone(),
                        scope: source.scope,
                        overridden_by: Vec::new(),
                        when,
                        any,
                    }
                });
            }
        }
        for def in &source.file.directives {
            let Some((directive_name, explicit)) = effective_directive_name(def) else {
                local_directives.push(ResolvedDirective {
                    def: def.clone(),
                    name: "directive".to_string(),
                    origin: source.label.clone(),
                    scope: source.scope,
                    overridden_by: Vec::new(),
                });
                continue;
            };
            if inherited_directive_names.contains(&directive_name) {
                resolved
                    .directives
                    .retain(|d| d.name != directive_name || d.scope != Scope::Builtin);
            } else {
                register_local_name(
                    &directive_name,
                    explicit,
                    source,
                    def.line,
                    &mut local_names,
                    &inherited_rule_names,
                    &inherited_directive_names,
                    &mut diagnostics,
                );
            }
            local_directives.push(ResolvedDirective {
                def: def.clone(),
                name: directive_name,
                origin: source.label.clone(),
                scope: source.scope,
                overridden_by: Vec::new(),
            });
        }
        for (method_name, def) in &source.file.methods {
            match resolved.methods.get_mut(method_name) {
                Some((existing, origin)) => {
                    if def.chain.is_some() {
                        existing.chain = def.chain.clone();
                    }
                    for (key, value) in &def.options {
                        existing.options.insert(key.clone(), value.clone());
                    }
                    *origin = source.label.clone();
                }
                None => {
                    resolved
                        .methods
                        .insert(method_name.clone(), (def.clone(), source.label.clone()));
                }
            }
        }
        for (task_name, def) in &source.file.tasks {
            match resolved.tasks.get_mut(task_name) {
                Some((existing, origin)) => {
                    if !def.kind.is_empty() {
                        existing.kind = def.kind.clone();
                    }
                    if def.upstream.is_some() {
                        existing.upstream = def.upstream.clone();
                    }
                    if def.temperature.is_some() {
                        existing.temperature = def.temperature;
                    }
                    if def.instructions.is_some() {
                        existing.instructions = def.instructions.clone();
                    }
                    if def.prompt.is_some() {
                        existing.prompt = def.prompt.clone();
                    }
                    *origin = source.label.clone();
                }
                None => {
                    resolved
                        .tasks
                        .insert(task_name.clone(), (def.clone(), source.label.clone()));
                }
            }
        }
        for (classifier_name, def) in &source.file.classifiers {
            resolved
                .classifiers
                .insert(classifier_name.clone(), (def.clone(), source.label.clone()));
        }
        for (upstream_name, def) in &source.file.upstreams {
            match resolved.upstreams.get_mut(upstream_name) {
                Some((existing, origin)) => {
                    if !def.provider.is_empty() {
                        existing.provider = def.provider.clone();
                    }
                    if !def.model.is_empty() {
                        existing.model = def.model.clone();
                    }
                    if def.base_url.is_some() {
                        existing.base_url = def.base_url.clone();
                    }
                    if def.api_key_env.is_some() {
                        existing.api_key_env = def.api_key_env.clone();
                    }
                    if def.api_key_literal {
                        existing.api_key_literal = true;
                    }
                    if def.timeout_secs.is_some() {
                        existing.timeout_secs = def.timeout_secs;
                    }
                    *origin = source.label.clone();
                }
                None => {
                    resolved
                        .upstreams
                        .insert(upstream_name.clone(), (def.clone(), source.label.clone()));
                }
            }
        }
    }

    for scope in [Scope::Project, Scope::User] {
        for rule in local_rules.iter().filter(|r| r.scope == scope) {
            let list = resolved.rules.entry(rule.position).or_default();
            let at = insertion_point(list, scope);
            list.insert(at, rule.clone());
        }
    }
    for scope in [Scope::User, Scope::Project] {
        for directive in local_directives.iter().filter(|d| d.scope == scope) {
            resolved.directives.push(directive.clone());
        }
    }

    let mut guards = Guards::default();
    for name in closure.iter().rev() {
        if let Some(file) = builtins::builtin(name) {
            apply_guards(&mut guards, &file.guards);
        }
    }
    for source in &enabled {
        apply_guards(&mut guards, &source.file.guards);
    }
    resolved.guards = guards;

    for scope in [Scope::Builtin, Scope::User, Scope::Project] {
        for source_overrides in override_sources(&enabled, &closure, scope) {
            let (label, overrides) = source_overrides;
            for (target, patch) in overrides {
                apply_override(&mut resolved, &target, &patch, &label, &mut diagnostics);
            }
        }
    }

    resolved.name = enabled
        .iter()
        .rev()
        .find_map(|s| s.file.name.clone())
        .or_else(|| enabled.iter().flat_map(|s| s.file.extends.clone()).next())
        .unwrap_or_else(|| builtins::RTK_DEFAULT.to_string());

    (resolved, diagnostics)
}

fn insertion_point(rules: &[ResolvedRule], scope: Scope) -> usize {
    match scope {
        Scope::Project => rules
            .iter()
            .take_while(|r| r.scope == Scope::Project)
            .count(),
        Scope::User => rules
            .iter()
            .take_while(|r| r.scope == Scope::Project || r.scope == Scope::User)
            .count(),
        Scope::Builtin => rules.len(),
    }
}

#[allow(clippy::too_many_arguments)]
fn register_local_name(
    name: &str,
    explicit: bool,
    source: &SourceFile,
    line: Option<usize>,
    local_names: &mut BTreeMap<String, (String, bool)>,
    inherited_rules: &BTreeMap<String, (&'static str, RuleDef)>,
    inherited_directives: &[String],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if inherited_rules.contains_key(name) || inherited_directives.iter().any(|d| d == name) {
        diagnostics.push(
            Diagnostic::new(
                Code::E012,
                format!(
                    "rule `{name}` redeclares an inherited name — use [override.{name}] to change it, or pick a new name"
                ),
            )
            .at(source.label.clone(), line),
        );
        return;
    }
    if let Some((other_label, other_explicit)) = local_names.get(name) {
        if explicit && *other_explicit {
            diagnostics.push(
                Diagnostic::new(
                    Code::E013,
                    format!(
                        "rule or directive name `{name}` appears in two loaded files: {other_label} and {}",
                        source.label
                    ),
                )
                .at(source.label.clone(), line),
            );
        } else {
            diagnostics.push(
                Diagnostic::new(
                    Code::W002,
                    format!(
                        "derived name `{name}` collides with a definition in {other_label} — set `name` explicitly"
                    ),
                )
                .at(source.label.clone(), line),
            );
        }
        return;
    }
    local_names.insert(name.to_string(), (source.label.clone(), explicit));
}

fn apply_guards(guards: &mut Guards, def: &super::schema::GuardsDef) {
    if let Some(v) = def.never_worse {
        guards.never_worse = v;
    }
    if let Some(v) = def.min_size {
        guards.min_size = v;
    }
}

fn override_sources(
    enabled: &[&SourceFile],
    closure: &[&'static str],
    scope: Scope,
) -> Vec<(String, Vec<(String, OverrideDef)>)> {
    match scope {
        Scope::Builtin => closure
            .iter()
            .rev()
            .filter_map(|name| {
                builtins::builtin(name).map(|file| {
                    (
                        name.to_string(),
                        file.overrides
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    )
                })
            })
            .collect(),
        other => enabled
            .iter()
            .filter(|s| s.scope == other)
            .map(|s| {
                (
                    s.label.clone(),
                    s.file
                        .overrides
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                )
            })
            .collect(),
    }
}

fn apply_override(
    resolved: &mut Resolved,
    target: &str,
    patch: &OverrideDef,
    label: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for rules in resolved.rules.values_mut() {
        if let Some(rule) = rules.iter_mut().find(|r| r.name == target) {
            patch_rule(rule, patch, label, diagnostics);
            return;
        }
    }
    if let Some(directive) = resolved.directives.iter_mut().find(|d| d.name == target) {
        patch_directive(directive, patch, label);
        return;
    }
    let candidates: Vec<String> = resolved
        .rules
        .values()
        .flatten()
        .map(|r| r.name.clone())
        .chain(resolved.directives.iter().map(|d| d.name.clone()))
        .collect();
    let hint = did_you_mean(target, candidates.iter().map(|s| s.as_str()))
        .map(|n| format!("did you mean `{n}`?"));
    let mut diag = Diagnostic::new(
        Code::E011,
        format!("[override.{target}] targets a rule or directive that does not exist"),
    )
    .at(label.to_string(), patch.line);
    diag.hint = hint;
    diagnostics.push(diag);
}

/// Every key an `[override.<rule>]` may carry. The guide documents E002 as
/// covering unknown override keys, so a key outside this list is reported
/// rather than ignored.
const OVERRIDE_RULE_KEYS: &[&str] = &[
    "enabled", "action", "task", "upstream", "message", "mode", "continue", "shadow", "use", "when",
];

fn patch_rule(
    rule: &mut ResolvedRule,
    patch: &OverrideDef,
    label: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut type_error = |key: &str, expected: &str| {
        diagnostics.push(
            Diagnostic::new(
                Code::E002,
                format!(
                    "[override.{}] `{key}` expects a {expected} — the value was ignored and                      the rule keeps its previous setting",
                    rule.name
                ),
            )
            .at(label.to_string(), patch.line),
        );
    };
    let mut typed_bool = Vec::new();
    let mut typed_str = Vec::new();
    for (key, value) in &patch.patch {
        match key.as_str() {
            "enabled" | "continue" | "shadow" if !value.is_bool() => typed_bool.push(key.clone()),
            "action" | "task" | "upstream" | "message" | "mode" if !value.is_str() => {
                typed_str.push(key.clone())
            }
            _ => {}
        }
    }
    for key in typed_bool {
        type_error(&key, "boolean");
    }
    for key in typed_str {
        type_error(&key, "string");
    }
    for (key, value) in &patch.patch {
        match key.as_str() {
            "enabled" => rule.def.enabled = value.as_bool().unwrap_or(rule.def.enabled),
            "action" => {
                if let Some(v) = value.as_str() {
                    rule.def.action = Some(v.to_string());
                }
            }
            "task" => {
                if let Some(v) = value.as_str() {
                    rule.def.task = Some(v.to_string());
                }
            }
            "upstream" => {
                if let Some(v) = value.as_str() {
                    rule.def.upstream = Some(v.to_string());
                }
            }
            "message" => {
                if let Some(v) = value.as_str() {
                    rule.def.message = Some(v.to_string());
                }
            }
            "mode" => {
                if let Some(v) = value.as_str() {
                    rule.def.mode = Some(v.to_string());
                }
            }
            "continue" => rule.def.continue_ = value.as_bool().unwrap_or(rule.def.continue_),
            "shadow" => rule.def.shadow = value.as_bool().unwrap_or(rule.def.shadow),
            "use" => {
                if let Some(items) = value.as_array() {
                    rule.def.use_ = items
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect();
                }
            }
            "when" => {
                let Some(table) = value.as_table() else {
                    continue;
                };
                if patch.when_replace {
                    let dropped: Vec<String> = rule
                        .def
                        .when
                        .keys()
                        .filter(|k| !table.contains_key(*k))
                        .cloned()
                        .collect();
                    if !dropped.is_empty() {
                        diagnostics.push(
                            Diagnostic::new(
                                Code::W003,
                                format!(
                                    "[override.{}] `when = {{ … }}` dropped keys: {}",
                                    rule.name,
                                    dropped.join(", ")
                                ),
                            )
                            .at(label.to_string(), patch.line),
                        );
                    }
                    rule.def.when = table.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                } else {
                    for (k, v) in table {
                        rule.def.when.insert(k.clone(), v.clone());
                    }
                }
            }
            "name" => diagnostics.push(
                Diagnostic::new(
                    Code::E002,
                    format!(
                        "[override.{}] cannot change `name` — that is a new rule",
                        rule.name
                    ),
                )
                .at(label.to_string(), patch.line),
            ),
            other => {
                let hint =
                    crate::pipeline::diag::did_you_mean(other, OVERRIDE_RULE_KEYS.iter().copied());
                let mut message = format!("[override.{}] unknown key `{other}`", rule.name);
                if let Some(meant) = hint {
                    message.push_str(&format!(" — did you mean `{meant}`?"));
                }
                diagnostics
                    .push(Diagnostic::new(Code::E002, message).at(label.to_string(), patch.line));
            }
        }
    }
    // Evaluation reads the COMPILED conditions, which were built before any
    // override ran. Without this recompile, an overridden `when` would change
    // `def.when` -- what `rtk pipeline show` prints -- while the matcher kept
    // the original thresholds: the CLI would display a configuration the
    // engine was not applying.
    let (when, any) = compile_conditions(&rule.def);
    rule.when = when;
    rule.any = any;
    rule.overridden_by.push(label.to_string());
}

fn patch_directive(directive: &mut ResolvedDirective, patch: &OverrideDef, label: &str) {
    for (key, value) in &patch.patch {
        match key.as_str() {
            "enabled" => directive.def.enabled = value.as_bool().unwrap_or(directive.def.enabled),
            "channel" => directive.def.channel = value.as_str().map(str::to_string),
            "text" => directive.def.text = value.as_str().map(str::to_string),
            other => {
                directive
                    .def
                    .options
                    .insert(other.to_string(), value.clone());
            }
        }
    }
    directive.overridden_by.push(label.to_string());
}

impl Resolved {
    /// Every environment variable the resolved pipeline needs at runtime:
    /// `(env var, upstream, what uses it)`. Hosts print these wherever a user
    /// adopts or inspects a pipeline, so "auth needed" is never a silent
    /// discovery at first use.
    pub fn required_env(&self) -> Vec<(String, String, Vec<String>)> {
        let mut out: Vec<(String, String, Vec<String>)> = Vec::new();
        for (upstream_name, (upstream, _)) in &self.upstreams {
            // A command supplies its own credential, so there is nothing to
            // export and nothing to warn about.
            if upstream.api_key_command.is_some() {
                continue;
            }
            // An upstream that does not name a variable still reads the
            // provider's conventional one, so report that rather than falling
            // silent and failing at first use.
            let env = match upstream.api_key_env.clone() {
                Some(env) => env,
                None => match upstream.provider.as_str() {
                    "anthropic" => "ANTHROPIC_API_KEY".to_string(),
                    "openai" => "OPENAI_API_KEY".to_string(),
                    "google" => "GEMINI_API_KEY".to_string(),
                    _ => continue,
                },
            };
            let mut users: Vec<String> = self
                .tasks
                .iter()
                .filter(|(_, (t, _))| t.upstream.as_deref() == Some(upstream_name))
                .map(|(name, _)| format!("task `{name}`"))
                .collect();
            users.extend(
                self.classifiers
                    .iter()
                    .filter(|(_, (c, _))| c.upstream.as_deref() == Some(upstream_name))
                    .map(|(name, _)| format!("classifier `{name}`")),
            );
            if users.is_empty() {
                continue;
            }
            out.push((env, upstream_name.clone(), users));
        }
        out
    }

    /// Names of the classifiers whose facts the enabled rules at `position`
    /// condition on — what a host adapter must answer (in one batched call)
    /// before evaluating. Empty for every position no rule classifies at,
    /// which keeps the no-classifier hot path free of any inference cost.
    pub fn classifiers_referenced(&self, position: Position) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        for rule in self.rules_at(position) {
            if !rule.def.enabled {
                continue;
            }
            let groups = rule.when.iter().chain(rule.any.iter().flatten());
            for conditions in groups {
                for condition in conditions {
                    if let Some((_, name, _)) =
                        super::conditions::classifier_condition(&condition.key)
                    {
                        out.insert(name.to_string());
                    }
                }
            }
        }
        out
    }

    pub fn rules_at(&self, position: Position) -> &[ResolvedRule] {
        self.rules
            .get(&position)
            .map(|r| r.as_slice())
            .unwrap_or(&[])
    }

    pub fn events_used(&self) -> Vec<(super::position::SiteKind, crate::block::Event)> {
        let mut out = Vec::new();
        for (position, rules) in &self.rules {
            if rules.iter().any(|r| r.def.enabled && !r.def.pro_inert) {
                let key = (position.site, position.event);
                if !out.contains(&key) {
                    out.push(key);
                }
            }
        }
        out
    }

    pub fn subjects_used(
        &self,
        site: super::position::SiteKind,
        event: crate::block::Event,
    ) -> Vec<crate::block::Subject> {
        let mut out = Vec::new();
        for (position, rules) in &self.rules {
            if position.site != site || position.event != event {
                continue;
            }
            if rules.iter().any(|r| r.def.enabled && !r.def.pro_inert)
                && !out.contains(&position.subject)
            {
                out.push(position.subject);
            }
        }
        out
    }

    pub fn find_rule(&self, name: &str) -> Option<&ResolvedRule> {
        self.rules.values().flatten().find(|r| r.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{Event, Subject};
    use crate::pipeline::SiteKind;

    fn parse_local(scope: Scope, label: &str, text: &str) -> SourceFile {
        let result = super::super::schema::parse(text, label);
        assert!(
            result.diagnostics.is_empty(),
            "{label}: {:#?}",
            result.diagnostics
        );
        SourceFile {
            scope,
            label: label.to_string(),
            file: result.file.expect("parses"),
        }
    }

    #[test]
    fn empty_load_set_resolves_to_rtk_default() {
        let (resolved, diagnostics) = resolve(&[]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert_eq!(resolved.name, "rtk:default");
        assert_eq!(resolved.extends, vec!["rtk:default"]);
        let shell = resolved.rules_at(Position::hook(Event::BeforeTool, Subject::Shell));
        assert_eq!(shell.len(), 1);
        assert_eq!(shell[0].name, "rtk-rewrite");
        assert!(
            resolved
                .rules_at(Position::hook(Event::AfterTool, Subject::Shell))
                .is_empty()
        );
        assert_eq!(resolved.guards.min_size, 512);
    }

    #[test]
    fn subjects_used_reports_only_positions_carrying_live_rules() {
        let (resolved, _) = resolve(&[]);
        assert_eq!(
            resolved.subjects_used(SiteKind::Hook, Event::BeforeTool),
            vec![Subject::Shell]
        );
        assert!(
            resolved
                .subjects_used(SiteKind::Hook, Event::AfterTool)
                .is_empty()
        );
    }

    #[test]
    fn subjects_used_widens_when_a_read_rule_is_added() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = \"rtk:default\"\n\
             [task.t]\nkind = \"summarize\"\n\
             [[hook.before_tool.read]]\nname = \"r\"\n\
             when = { file_lines_gt = 400 }\naction = \"route\"\ntask = \"t\"\n",
        );
        let (resolved, _) = resolve(&[user]);
        let mut subjects = resolved.subjects_used(SiteKind::Hook, Event::BeforeTool);
        subjects.sort_by_key(|s| s.as_str());
        assert_eq!(subjects, vec![Subject::Read, Subject::Shell]);
    }

    #[test]
    fn subjects_used_skips_a_disabled_rule() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = \"rtk:default\"\n\
             [[hook.after_tool.web]]\nname = \"w\"\nenabled = false\nuse = [\"ansi\"]\n",
        );
        let (resolved, _) = resolve(&[user]);
        assert!(
            resolved
                .subjects_used(SiteKind::Hook, Event::AfterTool)
                .is_empty()
        );
    }

    #[test]
    fn extends_posthook_inherits_the_chains_once() {
        let user = parse_local(
            Scope::User,
            "~/.config/rtk/pipeline.toml",
            "[pipeline]\nextends = \"rtk:posthook\"\n",
        );
        let (resolved, diagnostics) = resolve(&[user]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert_eq!(resolved.extends, vec!["rtk:posthook", "rtk:default"]);
        let shell = resolved.rules_at(Position::hook(Event::AfterTool, Subject::Shell));
        assert_eq!(shell[0].name, "term-default");
        assert_eq!(shell[0].origin, "rtk:posthook");
    }

    #[test]
    fn shared_ancestor_is_inherited_once() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = [\"rtk:posthook\", \"rtk:proxy\"]\n",
        );
        let (resolved, diagnostics) = resolve(&[user]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let rewrites: Vec<_> = resolved
            .rules_at(Position::hook(Event::BeforeTool, Subject::Shell))
            .iter()
            .filter(|r| r.name == "rtk-rewrite")
            .collect();
        assert_eq!(rewrites.len(), 1);
        let awareness: Vec<_> = resolved
            .directives
            .iter()
            .filter(|d| d.name == "rtk-awareness")
            .collect();
        assert_eq!(awareness.len(), 1);
    }

    #[test]
    fn project_rules_evaluate_before_user_rules_before_inherited() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = \"rtk:posthook\"\n[[hook.after_tool.shell]]\nname = \"mine\"\nuse = [\"ansi\"]\n",
        );
        let project = parse_local(
            Scope::Project,
            ".rtk/pipeline.toml",
            "[[hook.after_tool.shell]]\nname = \"theirs\"\nuse = [\"dedup\"]\n",
        );
        let (resolved, diagnostics) = resolve(&[user, project]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let names: Vec<&str> = resolved
            .rules_at(Position::hook(Event::AfterTool, Subject::Shell))
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, vec!["theirs", "mine", "term-default"]);
    }

    #[test]
    fn cross_file_duplicate_explicit_names_are_e013() {
        let a = parse_local(
            Scope::User,
            "a.toml",
            "[[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"ansi\"]\n",
        );
        let b = parse_local(
            Scope::User,
            "b.toml",
            "[[hook.after_tool.shell]]\nname = \"x\"\nuse = [\"dedup\"]\n",
        );
        let (_, diagnostics) = resolve(&[a, b]);
        let e013 = diagnostics
            .iter()
            .find(|d| d.code == Code::E013)
            .expect("E013");
        assert!(e013.message.contains("a.toml") && e013.message.contains("b.toml"));
    }

    #[test]
    fn redeclaring_an_inherited_name_is_e012() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = \"rtk:posthook\"\n[[hook.after_tool.shell]]\nname = \"term-default\"\nuse = [\"ansi\"]\n",
        );
        let (_, diagnostics) = resolve(&[user]);
        assert!(diagnostics.iter().any(|d| d.code == Code::E012));
    }

    #[test]
    fn derived_name_collision_is_w002() {
        let a = parse_local(
            Scope::User,
            "a.toml",
            "[[hook.after_tool.shell]]\nuse = [\"ansi\"]\n[[hook.after_tool.web]]\nuse = [\"ansi\"]\n",
        );
        let (_, diagnostics) = resolve(&[a]);
        assert!(diagnostics.iter().any(|d| d.code == Code::W002));
    }

    #[test]
    fn override_patches_one_when_key_and_keeps_the_rest() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = \"rtk:delegate-reads\"\n[override.shunt-large-read]\nwhen.file_lines_gt = 500\n",
        );
        let (resolved, diagnostics) = resolve(&[user]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let rule = resolved.find_rule("shunt-large-read").expect("inherited");
        assert_eq!(
            rule.when_value("file_lines_gt")
                .and_then(|v| v.as_integer()),
            Some(500)
        );
        assert!(
            rule.def.when.contains_key("whole_file"),
            "rest of when kept"
        );
        assert_eq!(rule.overridden_by, vec!["u.toml"]);
    }

    #[test]
    fn override_when_table_replaces_and_warns_w003() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = \"rtk:delegate-reads\"\n[override.shunt-large-read]\nwhen = { file_lines_gt = 900 }\n",
        );
        let (resolved, diagnostics) = resolve(&[user]);
        let w003 = diagnostics
            .iter()
            .find(|d| d.code == Code::W003)
            .expect("W003");
        assert!(w003.message.contains("whole_file"));
        let rule = resolved.find_rule("shunt-large-read").expect("inherited");
        assert!(!rule.def.when.contains_key("whole_file"));
        // Asserting only `def.when` is not enough: the compiled form is what
        // evaluation reads.
        let compiled = rule.when.as_ref().expect("compiled conditions");
        assert!(
            compiled.iter().any(|c| c.key == "file_lines"),
            "the overridden threshold must reach the matcher"
        );
        assert!(
            !compiled.iter().any(|c| c.key == "whole_file"),
            "the replaced key must leave the matcher too"
        );
    }

    #[test]
    fn override_enabled_reaches_builtin_disabled_rules() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = \"rtk:posthook\"\n[override.json-reads]\nenabled = true\n",
        );
        let (resolved, diagnostics) = resolve(&[user]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert!(
            resolved
                .find_rule("json-reads")
                .expect("inherited")
                .def
                .enabled
        );
    }

    #[test]
    fn override_unknown_target_is_e011_with_did_you_mean() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[override.rtk-rewrit]\nenabled = false\n",
        );
        let (_, diagnostics) = resolve(&[user]);
        let e011 = diagnostics
            .iter()
            .find(|d| d.code == Code::E011)
            .expect("E011");
        assert!(e011.hint.as_deref().unwrap_or("").contains("rtk-rewrite"));
    }

    #[test]
    fn terse_adds_a_directive_and_no_output_compression() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = \"rtk:terse\"\n",
        );
        let (resolved, diagnostics) = resolve(&[user]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert!(resolved.directives.iter().any(|d| d.name == "terse-output"));
        assert!(
            resolved
                .directives
                .iter()
                .any(|d| d.name == "rtk-awareness")
        );
        for subject in [Subject::Shell, Subject::Read, Subject::Search, Subject::Web] {
            assert!(
                resolved
                    .rules_at(Position::hook(Event::AfterTool, subject))
                    .is_empty(),
                "terse changes what the model writes, never what it reads"
            );
        }
        assert!(resolved.find_rule("json-reads").is_none());
    }

    #[test]
    fn delegate_reads_adoption_block_merges_task_fields_into_the_inherited_kinds() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            r#"
[pipeline]
extends = "rtk:delegate-reads"

[upstream.flash]
provider    = "google"
model       = "gemini-2.5-flash"
api_key_env = "GEMINI_API_KEY"

[task.bulk-read]
upstream = "flash"

[task.code-write]
upstream = "flash"
"#,
        );
        let (resolved, diagnostics) = resolve(&[user]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let (bulk, _) = &resolved.tasks["bulk-read"];
        assert_eq!(
            bulk.kind, "summarize",
            "kind inherited from rtk:delegate-reads"
        );
        assert_eq!(bulk.upstream.as_deref(), Some("flash"));
        let (write, _) = &resolved.tasks["code-write"];
        assert_eq!(write.kind, "generate");
    }

    #[test]
    fn disabled_file_is_skipped() {
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[pipeline]\nenabled = false\nextends = \"rtk:posthook\"\n",
        );
        let (resolved, _) = resolve(&[user]);
        assert_eq!(resolved.extends, vec!["rtk:default"]);
    }

    #[test]
    fn events_used_follow_the_active_pipeline() {
        let (default, _) = resolve(&[]);
        // rtk:default carries the category-A proxy rule as well as the hook
        // rewrite, so the default pipeline touches both sites.
        let mut used = default.events_used();
        used.sort_by_key(|(site, event)| (format!("{site:?}"), format!("{event:?}")));
        assert_eq!(
            used,
            vec![
                (super::super::position::SiteKind::Hook, Event::BeforeTool),
                (super::super::position::SiteKind::Proxy, Event::Request),
            ]
        );
        let user = parse_local(
            Scope::User,
            "u.toml",
            "[pipeline]\nextends = \"rtk:posthook\"\n",
        );
        let (with_posthook, _) = resolve(&[user]);
        let events = with_posthook.events_used();
        assert!(events.contains(&(super::super::position::SiteKind::Hook, Event::BeforeTool)));
        assert!(events.contains(&(super::super::position::SiteKind::Hook, Event::AfterTool)));
    }

    #[test]
    fn guards_merge_later_wins() {
        let user = parse_local(Scope::User, "u.toml", "[guards]\nmin_size = 1024\n");
        let (resolved, _) = resolve(&[user]);
        assert_eq!(resolved.guards.min_size, 1024);
        assert!(resolved.guards.never_worse);
    }
}

#[cfg(test)]
mod override_when {
    use crate::block::{Event, Subject};
    use crate::pipeline::resolve::{Scope, SourceFile};
    use crate::pipeline::{Fact, Facts, Position, evaluate, parse, resolve};

    fn resolved_from(text: &str) -> crate::pipeline::Resolved {
        let parsed = parse(text, "t.toml");
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let (r, diags) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "t.toml".into(),
            file: parsed.file.expect("parses"),
        }]);
        assert!(!crate::pipeline::has_errors(&diags), "{diags:#?}");
        r
    }

    fn fires(r: &crate::pipeline::Resolved, tokens: f64) -> bool {
        let mut facts = Facts::new();
        facts.insert("tool".into(), Fact::Str("shell".into()));
        facts.insert("tokens".into(), Fact::Num(tokens));
        !evaluate(r, Position::hook(Event::AfterTool, Subject::Shell), &facts).is_empty()
    }

    /// An overridden `when` must change behaviour, not
    /// just what `rtk pipeline show` displays. Both directions.
    #[test]
    fn overriding_when_changes_what_the_rule_matches() {
        let base = "schema_version = 1\n[pipeline]\nname = \"p\"\n\
                    [[hook.after_tool.shell]]\nname = \"cap-big\"\n\
                    when = { tokens_gt = 100 }\nuse = [\"cap\"]\n";
        let raised = format!("{base}[override.cap-big]\nwhen = {{ tokens_gt = 5000 }}\n");
        let r = resolved_from(&raised);
        assert!(
            !fires(&r, 1000.0),
            "threshold raised to 5000: 1000 tokens must no longer match"
        );
        assert!(
            fires(&r, 9000.0),
            "9000 tokens still matches the raised rule"
        );

        let lowered = format!("{base}[override.cap-big]\nwhen = {{ tokens_gt = 10 }}\n");
        let r = resolved_from(&lowered);
        assert!(
            fires(&r, 50.0),
            "threshold lowered to 10: 50 tokens must now match"
        );
    }
}
