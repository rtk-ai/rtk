use super::position::{Position, SiteKind};
use super::schema::PRO_CONDITION_KEYS;
use crate::block::{Event, Subject};
use regex::Regex;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    In,
    NotIn,
    Matches,
    Gt,
    Lt,
    Gte,
    Lte,
    StartsWith,
}

const OP_SUFFIXES: &[(&str, Op)] = &[
    ("_not_in", Op::NotIn),
    ("_starts_with", Op::StartsWith),
    ("_matches", Op::Matches),
    ("_gte", Op::Gte),
    ("_lte", Op::Lte),
    ("_gt", Op::Gt),
    ("_lt", Op::Lt),
    ("_in", Op::In),
];

const NO_SPLIT_KEYS: &[&str] = &["input_missing", "tokens_in"];

#[derive(Debug, Clone)]
pub enum CondValue {
    Str(String),
    Num(f64),
    Bool(bool),
    List(Vec<String>),
    Pattern(Box<Regex>),
}

#[derive(Debug, Clone)]
pub struct Condition {
    pub key: String,
    pub op: Op,
    pub value: CondValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fact {
    Str(String),
    Num(f64),
    Bool(bool),
}

pub type Facts = BTreeMap<String, Fact>;

pub fn split_key(raw: &str) -> (String, Op) {
    if NO_SPLIT_KEYS.contains(&raw) {
        return (raw.to_string(), Op::Eq);
    }
    for (suffix, op) in OP_SUFFIXES {
        if let Some(base) = raw.strip_suffix(suffix) {
            if !base.is_empty() {
                return (base.to_string(), *op);
            }
        }
    }
    (raw.to_string(), Op::Eq)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCheck {
    Ok,
    WrongEvent { hint: Option<String> },
    Unknown,
    TurnVarying,
    Pro,
}

const HOOK_COMMON: &[&str] = &["tool", "input_missing", "intent", "goal", "recent_calls"];
pub const CALL_CONTEXT_KEYS: &[&str] = &["intent", "goal", "recent_calls"];
const AFTER_COMMON: &[&str] = &["format", "bytes", "lines", "tokens", "duration_ms"];
const READ_BOTH: &[&str] = &["file_path", "extension", "basename", "whole_file"];
const READ_BEFORE_ONLY: &[&str] = &["file_exists", "file_bytes", "file_lines"];
const FILE_TARGET: &[&str] = &["file_path", "extension", "basename"];
const SHELL_BOTH: &[&str] = &["command", "is_rtk_command"];
const SHELL_AFTER: &[&str] = &["exit_code"];
const WEB_KEYS: &[&str] = &["url"];
const MCP_KEYS: &[&str] = &["server"];
const PROXY_BLOCK_INTRINSIC: &[&str] = &["tool", "format", "shape", "tokens", "succeeded"];
const PROXY_SESSION_STABLE: &[&str] = &["model", "provider", "session", "project", "request_class"];
const PROXY_TURN_VARYING: &[&str] = &["tokens_in", "message_count", "stream"];

/// Parse a classifier condition key: `class.<name>` / `class.<name>.confidence`
/// (question `choice`), `score.<name>` (question `score`), `noul.<name>`
/// (question `noul`). Returns `(question, classifier name, is_confidence)`.
pub fn classifier_condition(base_key: &str) -> Option<(&'static str, &str, bool)> {
    if let Some(rest) = base_key.strip_prefix("class.") {
        return match rest.strip_suffix(".confidence") {
            Some(name) if !name.is_empty() => Some(("choice", name, true)),
            _ if !rest.is_empty() && !rest.contains('.') => Some(("choice", rest, false)),
            _ => None,
        };
    }
    if let Some(name) = base_key.strip_prefix("score.") {
        return (!name.is_empty() && !name.contains('.')).then_some(("score", name, false));
    }
    if let Some(name) = base_key.strip_prefix("noul.") {
        return (!name.is_empty() && !name.contains('.')).then_some(("noul", name, false));
    }
    None
}

pub fn check_key(position: Position, base_key: &str) -> KeyCheck {
    if PRO_CONDITION_KEYS.contains(&base_key) {
        return KeyCheck::Pro;
    }
    if classifier_condition(base_key).is_some() {
        return match (position.site, position.event) {
            (SiteKind::Hook, Event::AfterTool | Event::BeforeTool) => KeyCheck::Ok,
            (SiteKind::Hook, _) => KeyCheck::WrongEvent { hint: None },
            (SiteKind::Proxy, _) => KeyCheck::WrongEvent {
                hint: Some(
                    "classifier conditions are hook-site (after_tool) only in this build"
                        .to_string(),
                ),
            },
        };
    }
    match position.site {
        SiteKind::Hook => check_hook_key(position.event, position.subject, base_key),
        SiteKind::Proxy => check_proxy_key(position.subject, base_key),
    }
}

fn check_hook_key(event: Event, subject: Subject, key: &str) -> KeyCheck {
    if HOOK_COMMON.contains(&key) || key.starts_with("input.") {
        return KeyCheck::Ok;
    }
    if AFTER_COMMON.contains(&key) {
        return match event {
            Event::AfterTool => KeyCheck::Ok,
            _ => KeyCheck::WrongEvent { hint: None },
        };
    }
    let subject_keys: &[&[&str]] = match subject {
        Subject::Read => &[READ_BOTH, READ_BEFORE_ONLY],
        Subject::Write | Subject::Edit => &[FILE_TARGET],
        Subject::Shell => &[SHELL_BOTH, SHELL_AFTER],
        Subject::Web => &[WEB_KEYS],
        Subject::Search | Subject::Subagent => &[],
        Subject::Mcp => &[MCP_KEYS],
        Subject::Any => &[
            READ_BOTH,
            READ_BEFORE_ONLY,
            FILE_TARGET,
            SHELL_BOTH,
            SHELL_AFTER,
            WEB_KEYS,
            MCP_KEYS,
        ],
        _ => &[],
    };
    let known_for_subject = subject_keys.iter().any(|set| set.contains(&key));
    if !known_for_subject {
        return KeyCheck::Unknown;
    }
    if READ_BEFORE_ONLY.contains(&key) && event != Event::BeforeTool && subject == Subject::Read {
        let hint = if key == "file_lines" {
            Some("`file_lines` is a before_tool condition; for output size use `lines`".to_string())
        } else {
            Some(format!("`{key}` reads the disk before the tool runs"))
        };
        return KeyCheck::WrongEvent { hint };
    }
    if SHELL_AFTER.contains(&key) && event != Event::AfterTool {
        return KeyCheck::WrongEvent {
            hint: Some(format!("`{key}` exists only after the tool ran")),
        };
    }
    KeyCheck::Ok
}

fn check_proxy_key(part: Subject, key: &str) -> KeyCheck {
    if PROXY_BLOCK_INTRINSIC.contains(&key) || PROXY_SESSION_STABLE.contains(&key) {
        return KeyCheck::Ok;
    }
    if PROXY_TURN_VARYING.contains(&key) {
        return match part {
            Subject::User | Subject::Any => KeyCheck::Ok,
            _ => KeyCheck::TurnVarying,
        };
    }
    KeyCheck::Unknown
}

pub fn parse_condition(raw_key: &str, value: &toml::Value) -> Result<Condition, String> {
    let (key, op) = split_key(raw_key);
    let value = match (op, value) {
        (Op::In | Op::NotIn, toml::Value::Array(items)) => {
            let list = items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("`{raw_key}`: list items must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            CondValue::List(list)
        }
        (Op::In | Op::NotIn, _) => {
            return Err(format!("`{raw_key}` expects an array"));
        }
        (Op::Matches, toml::Value::String(s)) => {
            let re = Regex::new(s).map_err(|e| format!("`{raw_key}`: invalid regex: {e}"))?;
            CondValue::Pattern(Box::new(re))
        }
        (Op::Matches, _) => return Err(format!("`{raw_key}` expects a regex string")),
        (Op::Gt | Op::Lt | Op::Gte | Op::Lte, v) => match v.as_integer() {
            Some(n) => CondValue::Num(n as f64),
            None => match v.as_float() {
                Some(f) => CondValue::Num(f),
                None => return Err(format!("`{raw_key}` expects a number")),
            },
        },
        (Op::StartsWith, toml::Value::String(s)) => CondValue::Str(s.clone()),
        (Op::StartsWith, _) => return Err(format!("`{raw_key}` expects a string")),
        (Op::Eq, toml::Value::String(s)) => CondValue::Str(s.clone()),
        (Op::Eq, toml::Value::Boolean(b)) => CondValue::Bool(*b),
        (Op::Eq, toml::Value::Integer(n)) => CondValue::Num(*n as f64),
        (Op::Eq, toml::Value::Float(f)) => CondValue::Num(*f),
        (Op::Eq, toml::Value::Array(items)) if raw_key == "input_missing" => {
            let list = items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("`{raw_key}`: list items must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            CondValue::List(list)
        }
        (Op::Eq, toml::Value::Array(_)) => {
            return Err(format!(
                "`{raw_key}`: bare key with an array (use `{raw_key}_in`)"
            ));
        }
        (Op::Eq, _) => return Err(format!("`{raw_key}`: unsupported value type")),
    };
    Ok(Condition { key, op, value })
}

pub fn parse_when(when: &BTreeMap<String, toml::Value>) -> Result<Vec<Condition>, String> {
    when.iter().map(|(k, v)| parse_condition(k, v)).collect()
}

impl Condition {
    pub fn matches(&self, facts: &Facts) -> bool {
        if self.key == "input_missing" {
            if let CondValue::List(fields) = &self.value {
                return fields
                    .iter()
                    .all(|f| !facts.contains_key(&format!("input.{f}")));
            }
            return false;
        }
        let Some(fact) = facts.get(&self.key) else {
            return false;
        };
        match (&self.op, &self.value, fact) {
            (Op::Eq, CondValue::Str(want), Fact::Str(have)) => want == have,
            (Op::Eq, CondValue::Bool(want), Fact::Bool(have)) => want == have,
            (Op::Eq, CondValue::Num(want), Fact::Num(have)) => want == have,
            (Op::In, CondValue::List(list), Fact::Str(have)) => list.iter().any(|v| v == have),
            (Op::NotIn, CondValue::List(list), Fact::Str(have)) => !list.iter().any(|v| v == have),
            (Op::Matches, CondValue::Pattern(re), Fact::Str(have)) => re.is_match(have),
            (Op::StartsWith, CondValue::Str(want), Fact::Str(have)) => have.starts_with(want),
            (Op::Gt, CondValue::Num(want), Fact::Num(have)) => have > want,
            (Op::Lt, CondValue::Num(want), Fact::Num(have)) => have < want,
            (Op::Gte, CondValue::Num(want), Fact::Num(have)) => have >= want,
            (Op::Lte, CondValue::Num(want), Fact::Num(have)) => have <= want,
            _ => false,
        }
    }
}

pub fn all_match(conditions: &[Condition], facts: &Facts) -> bool {
    conditions.iter().all(|c| c.matches(facts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{Event, Subject};

    fn facts(pairs: &[(&str, Fact)]) -> Facts {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn cond(key: &str, value: toml::Value) -> Condition {
        parse_condition(key, &value).expect("valid condition")
    }

    #[test]
    fn suffix_splitting_finds_the_longest_operator() {
        assert_eq!(split_key("tool"), ("tool".to_string(), Op::Eq));
        assert_eq!(split_key("tool_not_in"), ("tool".to_string(), Op::NotIn));
        assert_eq!(split_key("tokens_gt"), ("tokens".to_string(), Op::Gt));
        assert_eq!(split_key("tokens_in"), ("tokens_in".to_string(), Op::Eq));
        assert_eq!(
            split_key("input_missing"),
            ("input_missing".to_string(), Op::Eq)
        );
        assert_eq!(
            split_key("command_starts_with"),
            ("command".to_string(), Op::StartsWith)
        );
    }

    #[test]
    fn evaluation_covers_the_operator_vocabulary() {
        let f = facts(&[
            ("tool", Fact::Str("shell".into())),
            ("tokens", Fact::Num(500.0)),
            ("command", Fact::Str("ci-run status".into())),
            ("succeeded", Fact::Bool(true)),
        ]);
        assert!(cond("tool", "shell".into()).matches(&f));
        assert!(!cond("tool", "read".into()).matches(&f));
        assert!(cond("tool_in", vec!["shell", "search"].into()).matches(&f));
        assert!(cond("tool_not_in", vec!["read"].into()).matches(&f));
        assert!(cond("tokens_gt", 300.into()).matches(&f));
        assert!(!cond("tokens_gt", 500.into()).matches(&f));
        assert!(cond("tokens_gte", 500.into()).matches(&f));
        assert!(cond("tokens_lt", 501.into()).matches(&f));
        assert!(cond("command_starts_with", "ci-run".into()).matches(&f));
        assert!(cond("command_matches", "^ci-run\\b".into()).matches(&f));
        assert!(cond("succeeded", true.into()).matches(&f));
    }

    #[test]
    fn input_missing_matches_when_the_fields_are_absent() {
        let have_limit = facts(&[("input.limit", Fact::Num(100.0))]);
        let missing = cond(
            "input_missing",
            toml::Value::Array(vec!["limit".into(), "offset".into()]),
        );
        assert!(!missing.matches(&have_limit));
        assert!(missing.matches(&facts(&[])));
    }

    #[test]
    fn missing_fact_never_matches() {
        let f = facts(&[]);
        assert!(!cond("tool", "shell".into()).matches(&f));
        assert!(!cond("tool_not_in", vec!["read"].into()).matches(&f));
    }

    #[test]
    fn bad_regex_is_a_parse_error() {
        assert!(parse_condition("command_matches", &"([".into()).is_err());
    }

    #[test]
    fn vocabulary_is_positional() {
        let after_shell = Position::hook(Event::AfterTool, Subject::Shell);
        assert_eq!(check_key(after_shell, "exit_code"), KeyCheck::Ok);
        assert_eq!(check_key(after_shell, "tokens"), KeyCheck::Ok);
        assert_eq!(check_key(after_shell, "is_rtk_command"), KeyCheck::Ok);
        assert_eq!(check_key(after_shell, "url"), KeyCheck::Unknown);

        let before_shell = Position::hook(Event::BeforeTool, Subject::Shell);
        assert!(matches!(
            check_key(before_shell, "exit_code"),
            KeyCheck::WrongEvent { .. }
        ));
        assert!(matches!(
            check_key(before_shell, "tokens"),
            KeyCheck::WrongEvent { .. }
        ));
        assert_eq!(check_key(before_shell, "command"), KeyCheck::Ok);

        let before_read = Position::hook(Event::BeforeTool, Subject::Read);
        assert_eq!(check_key(before_read, "file_lines"), KeyCheck::Ok);
        assert_eq!(check_key(before_read, "whole_file"), KeyCheck::Ok);
        let after_read = Position::hook(Event::AfterTool, Subject::Read);
        match check_key(after_read, "file_lines") {
            KeyCheck::WrongEvent { hint } => {
                assert!(hint.unwrap_or_default().contains("use `lines`"));
            }
            other => panic!("expected WrongEvent, got {other:?}"),
        }
        assert_eq!(check_key(after_read, "command"), KeyCheck::Unknown);

        let search = Position::hook(Event::BeforeTool, Subject::Search);
        assert_eq!(check_key(search, "input.pattern"), KeyCheck::Ok);
    }

    #[test]
    fn proxy_condition_classes_are_enforced() {
        let tool_result = Position::proxy(Subject::ToolResult);
        assert_eq!(check_key(tool_result, "tool"), KeyCheck::Ok);
        assert_eq!(check_key(tool_result, "shape"), KeyCheck::Ok);
        assert_eq!(check_key(tool_result, "model"), KeyCheck::Ok);
        assert_eq!(check_key(tool_result, "stream"), KeyCheck::TurnVarying);
        assert_eq!(check_key(tool_result, "tokens_in"), KeyCheck::TurnVarying);
        assert_eq!(
            check_key(Position::proxy(Subject::User), "stream"),
            KeyCheck::Ok
        );
        assert_eq!(
            check_key(Position::proxy(Subject::Any), "tokens_in"),
            KeyCheck::Ok
        );
    }

    #[test]
    fn pro_conditions_are_flagged() {
        let p = Position::hook(Event::BeforeTool, Subject::Shell);
        assert_eq!(check_key(p, "team"), KeyCheck::Pro);
        assert_eq!(check_key(p, "user_role"), KeyCheck::Pro);
        assert_eq!(check_key(p, "spend"), KeyCheck::Pro);
    }
}
