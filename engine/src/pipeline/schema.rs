use super::diag::{Code, Diagnostic, did_you_mean};
use super::position::{Position, PositionError, SiteKind};
use std::collections::BTreeMap;

pub const SCHEMA_VERSION: i64 = 1;

pub const PRO_RULE_KEYS: &[&str] = &["locked"];
pub const PRO_CONDITION_KEYS: &[&str] = &["team", "user_role", "spend"];
pub const PRO_METHODS: &[&str] = &["redact"];

const RULE_KEYS: &[&str] = &[
    "name", "when", "any", "use", "action", "task", "upstream", "message", "mode", "continue",
    "enabled", "shadow",
];
const DIRECTIVE_KEYS: &[&str] = &["name", "use", "text", "file", "channel", "enabled"];
const TASK_KEYS: &[&str] = &[
    "kind",
    "upstream",
    "temperature",
    "instructions",
    "prompt",
    "context",
];
const CLASSIFIER_KEYS: &[&str] = &[
    "question",
    "instructions",
    "upstream",
    "input_chars",
    "min_confidence",
    "labels",
    "rubric",
    "context",
];
const UPSTREAM_KEYS: &[&str] = &[
    "provider",
    "model",
    "base_url",
    "api_key_env",
    "api_key_command",
    "api_key",
    "timeout_secs",
];
const GUARD_KEYS: &[&str] = &["never_worse", "min_size"];
const REMOVED_GUARD_KEYS: &[&str] = &["max_recovery_rate"];
const PIPELINE_KEYS: &[&str] = &["name", "extends", "enabled"];
const TOP_KEYS: &[&str] = &[
    "schema_version",
    "pipeline",
    "hook",
    "proxy",
    "directive",
    "method",
    "task",
    "upstream",
    "override",
    "classifier",
    "guards",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleDef {
    pub name: Option<String>,
    pub when: BTreeMap<String, toml::Value>,
    pub any: Vec<BTreeMap<String, toml::Value>>,
    pub use_: Vec<String>,
    pub action: Option<String>,
    pub task: Option<String>,
    pub upstream: Option<String>,
    pub message: Option<String>,
    pub mode: Option<String>,
    pub continue_: bool,
    pub enabled: bool,
    pub shadow: bool,
    pub pro_inert: bool,
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DirectiveDef {
    pub name: Option<String>,
    pub use_: Option<String>,
    pub text: Option<String>,
    pub file: Option<String>,
    pub channel: Option<String>,
    pub enabled: bool,
    pub options: BTreeMap<String, toml::Value>,
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MethodDef {
    pub chain: Option<Vec<String>>,
    pub options: BTreeMap<String, toml::Value>,
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskDef {
    pub kind: String,
    pub upstream: Option<String>,
    pub temperature: Option<f64>,
    pub instructions: Option<String>,
    pub prompt: Option<String>,
    pub context: bool,
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpstreamDef {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    /// A command whose stdout is the credential, for a key rtk should not hold
    /// and an environment variable cannot carry — the same shape as Claude
    /// Code's `apiKeyHelper`.
    pub api_key_command: Option<String>,
    pub api_key_literal: bool,
    pub timeout_secs: Option<u64>,
    pub line: Option<usize>,
}

/// `[classifier.<name>]` — a typed question a host adapter asks an inference
/// endpoint about a block of tool output, whose answer becomes the facts
/// `class.<name>` / `score.<name>` / `noul.<name>` that rules condition on.
/// The engine owns only the declaration; asking is a host concern.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClassifierDef {
    /// Question primitive: `choice` (pick a label), `score` (rubric 0-1),
    /// `noul` (truth 0-1).
    pub question: String,
    pub instructions: Option<String>,
    /// `[upstream.*]` binding that serves the question.
    pub upstream: Option<String>,
    /// How much of the block to send (head), in characters. Default 2000.
    pub input_chars: Option<usize>,
    /// Below this confidence the fact is not inserted, so no rule matches —
    /// fail-open. Default 0.8 for `choice`.
    pub min_confidence: Option<f64>,
    /// `choice` labels: name -> criteria sentence the model judges against.
    pub labels: BTreeMap<String, String>,
    /// `score` rubric: ordered level descriptions, worst to best (the provider
    /// API requires them as a list).
    pub rubric: Vec<String>,
    pub context: bool,
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OverrideDef {
    pub patch: toml::Table,
    pub when_replace: bool,
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GuardsDef {
    pub never_worse: Option<bool>,
    pub min_size: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PipelineFile {
    pub name: Option<String>,
    pub extends: Vec<String>,
    pub enabled: bool,
    pub rules: BTreeMap<Position, Vec<RuleDef>>,
    pub directives: Vec<DirectiveDef>,
    pub methods: BTreeMap<String, MethodDef>,
    pub tasks: BTreeMap<String, TaskDef>,
    pub upstreams: BTreeMap<String, UpstreamDef>,
    pub overrides: BTreeMap<String, OverrideDef>,
    pub classifiers: BTreeMap<String, ClassifierDef>,
    pub guards: GuardsDef,
}

pub struct ParseResult {
    pub file: Option<PipelineFile>,
    pub diagnostics: Vec<Diagnostic>,
}

struct Parser<'a> {
    label: &'a str,
    raw: &'a str,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn diag(&mut self, code: Code, message: String) -> &mut Diagnostic {
        self.diagnostics
            .push(Diagnostic::new(code, message).at(self.label, None));
        self.diagnostics.last_mut().expect("just pushed")
    }

    fn header_line(&self, header: &str, occurrence: usize) -> Option<usize> {
        self.raw
            .lines()
            .enumerate()
            .filter(|(_, l)| l.trim_start().starts_with(header))
            .map(|(i, _)| i + 1)
            .nth(occurrence)
    }
}

pub fn parse(text: &str, label: &str) -> ParseResult {
    let table: toml::Table = match toml::from_str(text) {
        Ok(table) => table,
        Err(e) => {
            return ParseResult {
                file: None,
                diagnostics: vec![
                    Diagnostic::new(Code::Load, format!("not valid TOML: {e}")).at(label, None),
                ],
            };
        }
    };

    let mut parser = Parser {
        label,
        raw: text,
        diagnostics: Vec::new(),
    };
    let mut file = PipelineFile {
        enabled: true,
        ..PipelineFile::default()
    };

    if let Some(version) = table.get("schema_version") {
        if version.as_integer() != Some(SCHEMA_VERSION) {
            parser.diag(
                Code::Load,
                format!("unsupported schema_version {version} (expected {SCHEMA_VERSION})"),
            );
        }
    }

    for (key, value) in &table {
        match key.as_str() {
            "schema_version" => {}
            "pipeline" => parse_pipeline_meta(&mut parser, value, &mut file),
            "hook" => parse_site(&mut parser, SiteKind::Hook, value, &mut file),
            "proxy" => parse_site(&mut parser, SiteKind::Proxy, value, &mut file),
            "directive" => parse_directives(&mut parser, value, &mut file),
            "method" => {
                parse_named_tables(&mut parser, value, "method", |parser, name, t, line| {
                    file.methods
                        .insert(name.to_string(), parse_method(parser, name, t, line));
                })
            }
            "task" => parse_named_tables(&mut parser, value, "task", |parser, name, t, line| {
                if let Some(task) = parse_task(parser, name, t, line) {
                    file.tasks.insert(name.to_string(), task);
                }
            }),
            "upstream" => {
                parse_named_tables(&mut parser, value, "upstream", |parser, name, t, line| {
                    if let Some(upstream) = parse_upstream(parser, name, t, line) {
                        file.upstreams.insert(name.to_string(), upstream);
                    }
                })
            }
            "override" => {
                parse_named_tables(&mut parser, value, "override", |parser, name, t, line| {
                    let when_replace = override_replaces_when(parser.raw, name);
                    file.overrides.insert(
                        name.to_string(),
                        OverrideDef {
                            patch: t.clone(),
                            when_replace,
                            line,
                        },
                    );
                })
            }
            "classifier" => {
                parse_named_tables(&mut parser, value, "classifier", |parser, name, t, line| {
                    if let Some(classifier) = parse_classifier(parser, name, t, line) {
                        file.classifiers.insert(name.to_string(), classifier);
                    }
                })
            }
            "guards" => parse_guards(&mut parser, value, &mut file),
            other => {
                let hint = did_you_mean(other, TOP_KEYS.iter().copied())
                    .map(|n| format!("did you mean `{n}`?"));
                let diag = parser.diag(Code::E001, format!("unknown section `[{other}]`"));
                diag.hint = hint;
            }
        }
    }

    ParseResult {
        file: Some(file),
        diagnostics: parser.diagnostics,
    }
}

fn parse_pipeline_meta(parser: &mut Parser, value: &toml::Value, file: &mut PipelineFile) {
    let Some(table) = value.as_table() else {
        parser.diag(Code::E002, "[pipeline] must be a table".to_string());
        return;
    };
    for (key, value) in table {
        match key.as_str() {
            "name" => file.name = value.as_str().map(str::to_string),
            "enabled" => file.enabled = value.as_bool().unwrap_or(true),
            "extends" => match value {
                toml::Value::String(one) => file.extends = vec![one.clone()],
                toml::Value::Array(items) => {
                    file.extends = items
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect();
                }
                _ => {
                    parser.diag(
                        Code::E002,
                        "`extends` takes a built-in name or a list of them".to_string(),
                    );
                }
            },
            other => {
                unknown_key(parser, other, PIPELINE_KEYS, "[pipeline]");
            }
        }
    }
}

fn unknown_key(parser: &mut Parser, key: &str, known: &[&str], context: &str) {
    let hint = did_you_mean(key, known.iter().copied()).map(|n| format!("did you mean `{n}`?"));
    let diag = parser.diag(Code::E002, format!("unknown key `{key}` on {context}"));
    diag.hint = hint;
}

fn parse_site(parser: &mut Parser, site: SiteKind, value: &toml::Value, file: &mut PipelineFile) {
    let Some(events) = value.as_table() else {
        parser.diag(
            Code::E001,
            format!("[{}] must contain event tables", site.as_str()),
        );
        return;
    };
    for (event_name, subjects) in events {
        let Some(subjects) = subjects.as_table() else {
            parser.diag(
                Code::E001,
                format!(
                    "[{}.{}] must contain subject tables",
                    site.as_str(),
                    event_name
                ),
            );
            continue;
        };
        for (subject_name, rules) in subjects {
            match Position::parse_parts(site, event_name, subject_name) {
                Ok(position) => parse_rules(parser, position, rules, file),
                Err(PositionError::UnknownEvent(event)) => {
                    parser.diag(
                        Code::E001,
                        format!(
                            "unknown position `[[{}.{}.{}]]`: `{}` is not an event at the {} site",
                            site.as_str(),
                            event,
                            subject_name,
                            event,
                            site.as_str()
                        ),
                    );
                }
                Err(PositionError::UnknownSubject(subject)) => {
                    let vocabulary: Vec<&str> = match site {
                        SiteKind::Hook => crate::block::HOOK_SUBJECTS
                            .iter()
                            .map(|s| s.as_str())
                            .collect(),
                        SiteKind::Proxy => crate::block::PROXY_PARTS
                            .iter()
                            .map(|s| s.as_str())
                            .collect(),
                    };
                    let hint = did_you_mean(&subject, vocabulary.iter().copied())
                        .map(|n| format!("did you mean `{n}`?"));
                    let diag = parser.diag(
                        Code::E018,
                        format!(
                            "unknown subject `{}` under `{}.{}`",
                            subject,
                            site.as_str(),
                            event_name
                        ),
                    );
                    diag.hint = hint;
                }
            }
        }
    }
}

fn parse_rules(
    parser: &mut Parser,
    position: Position,
    value: &toml::Value,
    file: &mut PipelineFile,
) {
    let Some(items) = value.as_array() else {
        parser.diag(
            Code::E001,
            format!(
                "{} must be an array of tables (use double brackets)",
                position.header()
            ),
        );
        return;
    };
    let mut parsed = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(table) = item.as_table() else {
            continue;
        };
        let line = parser.header_line(&position.header(), index);
        parsed.push(parse_rule(parser, position, table, line));
    }
    file.rules.entry(position).or_default().extend(parsed);
}

fn parse_rule(
    parser: &mut Parser,
    position: Position,
    table: &toml::Table,
    line: Option<usize>,
) -> RuleDef {
    let mut rule = RuleDef {
        continue_: false,
        enabled: true,
        shadow: false,
        line,
        ..RuleDef::default()
    };
    let mut mistyped: Vec<(String, &str)> = Vec::new();
    for (key, value) in table {
        match key.as_str() {
            "name" => rule.name = value.as_str().map(str::to_string),
            "when" => rule.when = flatten_when(parser, position, value),
            "any" => {
                if let Some(groups) = value.as_array() {
                    rule.any = groups
                        .iter()
                        .map(|g| flatten_when(parser, position, g))
                        .collect();
                }
            }
            "use" => {
                match value {
                    toml::Value::Array(items) => {
                        rule.use_ = items
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect();
                    }
                    _ => {
                        parser.diag(
                            Code::E002,
                            format!("`use` takes a list of methods at {}", position.render()),
                        );
                    }
                };
            }
            "action" | "task" | "upstream" | "message" | "mode" => match value.as_str() {
                Some(text) => {
                    let text = Some(text.to_string());
                    match key.as_str() {
                        "action" => rule.action = text,
                        "task" => rule.task = text,
                        "upstream" => rule.upstream = text,
                        "mode" => rule.mode = text,
                        _ => rule.message = text,
                    }
                }
                None => mistyped.push((key.clone(), "a string")),
            },
            "continue" | "enabled" | "shadow" => match value.as_bool() {
                Some(flag) => match key.as_str() {
                    "continue" => rule.continue_ = flag,
                    "enabled" => rule.enabled = flag,
                    _ => rule.shadow = flag,
                },
                None => mistyped.push((key.clone(), "true or false")),
            },
            other if PRO_RULE_KEYS.contains(&other) => {
                rule.pro_inert = true;
                let diag = parser.diag(
                    Code::W009,
                    format!("`{other}` is not available in this edition — the rule is inert; the file still loads"),
                );
                diag.line = line;
                diag.context = Some(position.header());
            }
            other => {
                let label = rule_label(&rule);
                let diag_context = if label.is_empty() {
                    position.header()
                } else {
                    format!("{} {label}", position.header())
                };
                let hint = did_you_mean(other, RULE_KEYS.iter().copied())
                    .map(|n| format!("did you mean `{n}`?"));
                let diag = parser.diag(Code::E002, format!("unknown key `{other}` on a rule"));
                diag.line = line;
                diag.context = Some(diag_context);
                diag.hint = hint;
            }
        }
    }
    if !mistyped.is_empty() {
        rule.enabled = false;
        let label = rule_label(&rule);
        let context = if label.is_empty() {
            position.header()
        } else {
            format!("{} {label}", position.header())
        };
        for (key, expected) in mistyped {
            let diag = parser.diag(
                Code::W024,
                format!("`{key}` must be {expected} — this rule is disabled; the rest of the file still loads"),
            );
            diag.line = line;
            diag.context = Some(context.clone());
        }
    }
    rule
}

fn rule_label(rule: &RuleDef) -> String {
    match &rule.name {
        Some(name) => format!("\"{name}\""),
        None => String::new(),
    }
}

fn flatten_when(
    parser: &mut Parser,
    position: Position,
    value: &toml::Value,
) -> BTreeMap<String, toml::Value> {
    let mut out = BTreeMap::new();
    let Some(table) = value.as_table() else {
        parser.diag(
            Code::E002,
            format!("`when` must be a table at {}", position.render()),
        );
        return out;
    };
    for (key, value) in table {
        if key == "input" {
            if let Some(nested) = value.as_table() {
                for (inner, v) in nested {
                    out.insert(format!("input.{inner}"), v.clone());
                }
                continue;
            }
        }
        out.insert(key.clone(), value.clone());
    }
    out
}

fn parse_directives(parser: &mut Parser, value: &toml::Value, file: &mut PipelineFile) {
    let Some(items) = value.as_array() else {
        parser.diag(
            Code::E001,
            "[[directive]] must be an array of tables".to_string(),
        );
        return;
    };
    for (index, item) in items.iter().enumerate() {
        let Some(table) = item.as_table() else {
            continue;
        };
        let line = parser.header_line("[[directive]]", index);
        let mut directive = DirectiveDef {
            enabled: true,
            line,
            ..DirectiveDef::default()
        };
        for (key, value) in table {
            match key.as_str() {
                "name" => directive.name = value.as_str().map(str::to_string),
                "use" => directive.use_ = value.as_str().map(str::to_string),
                "text" => directive.text = value.as_str().map(str::to_string),
                "file" => directive.file = value.as_str().map(str::to_string),
                "channel" => directive.channel = value.as_str().map(str::to_string),
                "enabled" => directive.enabled = value.as_bool().unwrap_or(true),
                other => {
                    if directive.use_.is_some() || table.contains_key("use") {
                        directive.options.insert(other.to_string(), value.clone());
                    } else {
                        unknown_key(parser, other, DIRECTIVE_KEYS, "a directive");
                    }
                }
            }
        }
        let sources = [&directive.use_, &directive.text, &directive.file]
            .iter()
            .filter(|v| v.is_some())
            .count();
        if sources != 1 {
            let diag = parser.diag(
                Code::E006,
                "a directive takes exactly one of `use`, `text`, `file`".to_string(),
            );
            diag.line = line;
            diag.context = Some("[[directive]]".to_string());
        }
        file.directives.push(directive);
    }
}

fn parse_named_tables(
    parser: &mut Parser,
    value: &toml::Value,
    section: &str,
    mut each: impl FnMut(&mut Parser, &str, &toml::Table, Option<usize>),
) {
    let Some(entries) = value.as_table() else {
        parser.diag(Code::E001, format!("[{section}.<name>] must be tables"));
        return;
    };
    for (name, entry) in entries {
        let Some(table) = entry.as_table() else {
            parser.diag(Code::E002, format!("[{section}.{name}] must be a table"));
            continue;
        };
        let line = parser.header_line(&format!("[{section}.{name}]"), 0);
        each(parser, name, table, line);
    }
}

fn parse_method(
    parser: &mut Parser,
    name: &str,
    table: &toml::Table,
    line: Option<usize>,
) -> MethodDef {
    let mut method = MethodDef {
        line,
        ..MethodDef::default()
    };
    for (key, value) in table {
        if key == "chain" {
            method.chain = value.as_array().map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            });
            if method.chain.is_none() {
                parser.diag(
                    Code::E002,
                    format!("[method.{name}] `chain` takes a list of method names"),
                );
            }
        } else {
            method.options.insert(key.clone(), value.clone());
        }
    }
    method
}

fn parse_task(
    parser: &mut Parser,
    name: &str,
    table: &toml::Table,
    line: Option<usize>,
) -> Option<TaskDef> {
    let mut task = TaskDef {
        line,
        ..TaskDef::default()
    };
    for (key, value) in table {
        match key.as_str() {
            "kind" => task.kind = value.as_str().unwrap_or_default().to_string(),
            "upstream" => task.upstream = value.as_str().map(str::to_string),
            "temperature" => {
                task.temperature = value
                    .as_float()
                    .or_else(|| value.as_integer().map(|n| n as f64))
            }
            "instructions" => task.instructions = value.as_str().map(str::to_string),
            "prompt" => task.prompt = value.as_str().map(str::to_string),
            "context" => match value.as_bool() {
                Some(flag) => task.context = flag,
                None => {
                    parser.diag(
                        Code::E002,
                        format!("[task.{name}] `context` takes true or false"),
                    );
                }
            },
            other => unknown_key(parser, other, TASK_KEYS, &format!("[task.{name}]")),
        }
    }
    Some(task)
}

fn parse_classifier(
    parser: &mut Parser,
    name: &str,
    table: &toml::Table,
    line: Option<usize>,
) -> Option<ClassifierDef> {
    let mut classifier = ClassifierDef {
        line,
        ..ClassifierDef::default()
    };
    for (key, value) in table {
        match key.as_str() {
            "question" => classifier.question = value.as_str().unwrap_or_default().to_string(),
            "instructions" => classifier.instructions = value.as_str().map(str::to_string),
            "upstream" => classifier.upstream = value.as_str().map(str::to_string),
            "input_chars" => {
                classifier.input_chars = value.as_integer().and_then(|n| usize::try_from(n).ok())
            }
            "min_confidence" => {
                classifier.min_confidence = value
                    .as_float()
                    .or_else(|| value.as_integer().map(|n| n as f64))
            }
            "labels" => {
                if let Some(t) = value.as_table() {
                    for (label, criteria) in t {
                        classifier.labels.insert(
                            label.clone(),
                            criteria.as_str().unwrap_or_default().to_string(),
                        );
                    }
                }
            }
            "context" => match value.as_bool() {
                Some(flag) => classifier.context = flag,
                None => {
                    parser.diag(
                        Code::E002,
                        format!("[classifier.{name}] `context` takes true or false"),
                    );
                }
            },
            "rubric" => {
                if let Some(list) = value.as_array() {
                    classifier.rubric = list
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect();
                }
            }
            other => unknown_key(
                parser,
                other,
                CLASSIFIER_KEYS,
                &format!("[classifier.{name}]"),
            ),
        }
    }
    Some(classifier)
}

fn parse_upstream(
    parser: &mut Parser,
    name: &str,
    table: &toml::Table,
    line: Option<usize>,
) -> Option<UpstreamDef> {
    let mut upstream = UpstreamDef {
        line,
        ..UpstreamDef::default()
    };
    for (key, value) in table {
        match key.as_str() {
            "provider" => upstream.provider = value.as_str().unwrap_or_default().to_string(),
            "model" => upstream.model = value.as_str().unwrap_or_default().to_string(),
            "base_url" => upstream.base_url = value.as_str().map(str::to_string),
            "api_key_env" => upstream.api_key_env = value.as_str().map(str::to_string),
            "api_key_command" => upstream.api_key_command = value.as_str().map(str::to_string),
            "api_key" => upstream.api_key_literal = true,
            "timeout_secs" => {
                upstream.timeout_secs = value.as_integer().and_then(|n| u64::try_from(n).ok())
            }
            other => unknown_key(parser, other, UPSTREAM_KEYS, &format!("[upstream.{name}]")),
        }
    }
    Some(upstream)
}

fn override_replaces_when(raw: &str, name: &str) -> bool {
    let header = format!("[override.{name}]");
    let mut inside = false;
    let mut in_multiline: Option<&str> = None;
    for line in raw.lines() {
        // A TOML multiline string can contain header- or key-shaped lines
        // verbatim; scanning into one would flip this to replace-mode and
        // drop `when` keys the user never replaced (W003 on a merge).
        if let Some(delim) = in_multiline {
            if line.contains(delim) {
                in_multiline = None;
            }
            continue;
        }
        for delim in [r#"""""#, "'''"] {
            if line.matches(delim).count() % 2 == 1 {
                in_multiline = Some(delim);
            }
        }
        if in_multiline.is_some() {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            inside = trimmed.starts_with(&header);
            continue;
        }
        if inside {
            let no_space: String = trimmed
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '=')
                .collect();
            if no_space == "when" {
                return true;
            }
        }
    }
    false
}

fn parse_guards(parser: &mut Parser, value: &toml::Value, file: &mut PipelineFile) {
    let Some(table) = value.as_table() else {
        parser.diag(Code::E002, "[guards] must be a table".to_string());
        return;
    };
    for (key, value) in table {
        match key.as_str() {
            "never_worse" => file.guards.never_worse = value.as_bool(),
            "min_size" => {
                file.guards.min_size = value.as_integer().and_then(|n| usize::try_from(n).ok())
            }
            other if REMOVED_GUARD_KEYS.contains(&other) => {
                parser.diag(
                    Code::W010,
                    format!(
                        "`{other}` was removed — recovery reads are reported by `rtk gain`, never enforced; delete the key"
                    ),
                );
            }
            other => unknown_key(parser, other, GUARD_KEYS, "[guards]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mistyped_rule_field_warns_and_disables_only_that_rule() {
        let parsed = parse(
            "[[hook.before_tool.read]]\nname = \"off\"\nenabled = \"false\"\naction = \"deny\"\n\n[[hook.before_tool.read]]\nname = \"bad\"\naction = 5\n\n[[hook.before_tool.read]]\nname = \"ok\"\naction = \"deny\"\n",
            "t.toml",
        );
        let w024 = parsed
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::W024)
            .count();
        assert_eq!(w024, 2, "{:#?}", parsed.diagnostics);
        let file = parsed.file.expect("the file still loads");
        let rules: Vec<(&str, bool)> = file
            .rules
            .values()
            .flatten()
            .map(|r| (r.name.as_deref().unwrap_or(""), r.enabled))
            .collect();
        assert!(rules.contains(&("off", false)), "{rules:?}");
        assert!(rules.contains(&("bad", false)), "{rules:?}");
        assert!(rules.contains(&("ok", true)), "{rules:?}");
    }
    use crate::block::{Event, Subject};

    fn ok(text: &str) -> PipelineFile {
        let result = parse(text, "test.toml");
        assert!(
            result.diagnostics.is_empty(),
            "unexpected diagnostics: {:#?}",
            result.diagnostics
        );
        result.file.expect("parses")
    }

    #[test]
    fn parses_the_guide_layout() {
        let file = ok(r#"
schema_version = 1

[pipeline]
name = "mine"
extends = ["rtk:posthook", "rtk:proxy"]

[[hook.after_tool.shell]]
name     = "term-default"
when     = { is_rtk_command = false }
use      = ["term"]

[[hook.before_tool.read]]
name   = "shunt-large-read"
when   = { file_lines_gt = 350, whole_file = true }
action = "route"
task   = "bulk-read"

[[proxy.request.tool_result]]
name = "ccr"
when = { tool_not_in = ["read"], tokens_gt = 1500 }
use  = ["ccr"]

[[directive]]
use   = "rtk-awareness"
level = "full"

[method.gentle]
chain = ["ansi", "dedup"]

[task.bulk-read]
kind     = "summarize"
upstream = "flash"

[upstream.flash]
provider    = "google"
model       = "gemini-2.5-flash"
api_key_env = "GEMINI_API_KEY"

[override.ccr]
when.tokens_gt = 500

[guards]
min_size = 512
"#);
        assert_eq!(file.extends, vec!["rtk:posthook", "rtk:proxy"]);
        let shell = &file.rules[&Position::hook(Event::AfterTool, Subject::Shell)];
        assert_eq!(shell[0].name.as_deref(), Some("term-default"));
        assert_eq!(shell[0].use_, vec!["term"]);
        let read = &file.rules[&Position::hook(Event::BeforeTool, Subject::Read)];
        assert_eq!(read[0].action.as_deref(), Some("route"));
        assert_eq!(read[0].task.as_deref(), Some("bulk-read"));
        assert_eq!(file.directives[0].use_.as_deref(), Some("rtk-awareness"));
        assert_eq!(file.directives[0].options["level"].as_str(), Some("full"));
        assert_eq!(
            file.methods["gentle"].chain.as_deref(),
            Some(&["ansi".to_string(), "dedup".to_string()][..])
        );
        assert_eq!(file.tasks["bulk-read"].kind, "summarize");
        assert_eq!(file.upstreams["flash"].provider, "google");
        assert!(file.overrides.contains_key("ccr"));
        assert_eq!(file.guards.min_size, Some(512));
    }

    #[test]
    fn nested_input_conditions_flatten() {
        let file = ok(r#"
[[hook.before_tool.read]]
name   = "big"
when   = { input = { limit_gt = 100 }, input_missing = ["offset"] }
action = "deny"
"#);
        let rules = &file.rules[&Position::hook(Event::BeforeTool, Subject::Read)];
        assert!(rules[0].when.contains_key("input.limit_gt"));
        assert!(rules[0].when.contains_key("input_missing"));
    }

    #[test]
    fn unknown_section_is_e001_with_hint() {
        let result = parse("[pipelin]\nname = \"x\"\n", "test.toml");
        let diag = &result.diagnostics[0];
        assert_eq!(diag.code, Code::E001);
        assert!(diag.hint.as_deref().unwrap_or("").contains("pipeline"));
    }

    #[test]
    fn unknown_event_is_e001_and_unknown_subject_is_e018() {
        let result = parse(
            "[[hook.during_tool.shell]]\nname = \"x\"\naction = \"deny\"\n",
            "t",
        );
        assert_eq!(result.diagnostics[0].code, Code::E001);

        let result = parse(
            "[[hook.after_tool.bash]]\nname = \"x\"\nuse = [\"term\"]\n",
            "t",
        );
        assert_eq!(result.diagnostics[0].code, Code::E018);

        let result = parse(
            "[[proxy.request.shell]]\nname = \"x\"\nuse = [\"ccr\"]\n",
            "t",
        );
        assert_eq!(result.diagnostics[0].code, Code::E018);
    }

    #[test]
    fn unknown_rule_key_is_e002_with_line() {
        let result = parse(
            "[[hook.after_tool.shell]]\nname = \"t\"\nwhne = { tool = \"x\" }\nuse = [\"term\"]\n",
            "t",
        );
        let diag = &result.diagnostics[0];
        assert_eq!(diag.code, Code::E002);
        assert_eq!(diag.line, Some(1));
        assert!(diag.hint.as_deref().unwrap_or("").contains("when"));
    }

    #[test]
    fn removed_max_recovery_rate_warns_and_still_loads() {
        let result = parse(
            "[guards]\nnever_worse = true\nmin_size = 256\nmax_recovery_rate = 0.15\n",
            "t",
        );
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].code, Code::W010);
        assert_eq!(
            result.diagnostics[0].code.severity(),
            crate::pipeline::Severity::Warning
        );
        let file = result.file.expect("file still loads");
        assert_eq!(file.guards.min_size, Some(256));
        assert_eq!(file.guards.never_worse, Some(true));
    }

    #[test]
    fn locked_is_w009_and_marks_the_rule_inert() {
        let result = parse(
            "[[hook.after_tool.shell]]\nname = \"t\"\nuse = [\"term\"]\nlocked = true\n",
            "t",
        );
        assert_eq!(result.diagnostics[0].code, Code::W009);
        let file = result.file.expect("loads");
        let rules = &file.rules[&Position::hook(Event::AfterTool, Subject::Shell)];
        assert!(rules[0].pro_inert);
    }

    #[test]
    fn directive_needs_exactly_one_source() {
        let result = parse("[[directive]]\nchannel = \"session_start\"\n", "t");
        assert_eq!(result.diagnostics[0].code, Code::E006);

        let result = parse("[[directive]]\nuse = \"terse-output\"\ntext = \"x\"\n", "t");
        assert_eq!(result.diagnostics[0].code, Code::E006);
    }

    #[test]
    fn literal_api_key_is_recorded_for_e015() {
        let file = ok_with_diags(
            "[upstream.x]\nprovider = \"openai\"\nmodel = \"gpt\"\napi_key = \"sk-123\"\n",
        );
        assert!(file.upstreams["x"].api_key_literal);
    }

    fn ok_with_diags(text: &str) -> PipelineFile {
        parse(text, "t").file.expect("parses")
    }

    #[test]
    fn task_without_kind_parses_for_definition_merge() {
        let result = parse("[task.review]\nupstream = \"flash\"\n", "t");
        assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
        assert!(result.file.expect("parses").tasks["review"].kind.is_empty());
    }

    #[test]
    fn invalid_toml_is_a_load_error() {
        let result = parse("не toml [", "broken.toml");
        assert!(result.file.is_none());
        assert_eq!(result.diagnostics[0].code, Code::Load);
    }

    #[test]
    fn disabled_file_parses_with_enabled_false() {
        let file = ok("[pipeline]\nenabled = false\n");
        assert!(!file.enabled);
    }
}
