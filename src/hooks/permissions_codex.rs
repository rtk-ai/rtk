//! Advisory parity for literal user prefix rules, never a Codex policy enforcer.
//!
//! Codex's Starlark evaluator and trusted config stack are host-owned. We only
//! interpret a small, literal subset and leave unknown inputs to the host.
use crate::core::user_dirs;
use crate::discover::lexer::{TokenKind, shell_split, split_for_permissions, tokenize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Decision {
    Allow,
    Prompt,
    Forbidden,
}

#[derive(Debug)]
struct Rule {
    pattern: Vec<Vec<String>>,
    decision: Decision,
}

fn evaluate(argv: &[String], rules: &[Rule]) -> Option<Decision> {
    rules
        .iter()
        .filter(|rule| {
            argv.len() >= rule.pattern.len()
                && rule
                    .pattern
                    .iter()
                    .zip(argv)
                    .all(|(alts, arg)| alts.contains(arg))
        })
        .map(|rule| rule.decision)
        .max()
}

pub(crate) fn preserves_rules(cwd: Option<&str>, original: &str, rewritten: &str) -> bool {
    let Some(rules) = load_rules(cwd) else {
        return false;
    };
    if rules.is_empty() {
        return true;
    }
    // Native managed-policy stores cannot be inferred from file probes.
    if cfg!(any(target_os = "macos", windows)) {
        return false;
    }
    if split_for_permissions(original)
        .iter()
        .zip(split_for_permissions(rewritten))
        .any(|(a, b)| a != &b && shell_split(b).first().is_none_or(|arg| arg != "rtk"))
    {
        return false;
    }
    let (Some(before), Some(after)) = (decisions(original, &rules), decisions(rewritten, &rules))
    else {
        return false;
    };
    before == after
        && before
            .iter()
            .all(|d| matches!(d, None | Some(Decision::Allow)))
}

fn decisions(command: &str, rules: &[Rule]) -> Option<Vec<Option<Decision>>> {
    // Only literal words and these separators can share Codex's plain-command
    // segmentation. Defer before RTK's lexer can erase quotes or continuations.
    if !command
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || " _-./:,+%@\t\n;|&".contains(c))
        || tokenize(command).iter().any(|t| {
            t.kind != TokenKind::Arg && !matches!(t.value.as_str(), "&&" | "||" | ";" | "|")
        })
    {
        return None;
    }
    split_for_permissions(command)
        .iter()
        .map(|segment| {
            let argv = shell_split(segment);
            let first = argv.first()?;
            if first.contains('/')
                || [
                    "env", "sudo", "command", "exec", "nohup", "time", "nice", "timeout", "stdbuf",
                    "setsid", "builtin", "eval",
                ]
                .contains(&first.as_str())
                || crate::discover::shell_wrapper::is_shell_wrapper_candidate(segment)
            {
                return None;
            }
            Some(evaluate(&argv, rules))
        })
        .collect()
}

// Unlike exists(), errors (including an inaccessible parent) are not absence.
fn absent(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(e) if e.kind() == ErrorKind::NotFound)
}

// Sorted regular *.rules only; unreadable or symlinked sources defer.
fn rule_files(root: &Path) -> Option<Vec<PathBuf>> {
    let dir = root.join("rules");
    if absent(&dir) {
        return Some(Vec::new());
    }
    if !fs::metadata(&dir).ok()?.is_dir() {
        return None;
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "rules") {
            let kind = entry.file_type().ok()?;
            if kind.is_symlink() {
                return None;
            }
            if kind.is_file() {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Some(paths)
}

fn load_rules(cwd: Option<&str>) -> Option<Vec<Rule>> {
    let cwd = cwd
        .map(PathBuf::from)
        .or_else(|| user_dirs::current_dir().ok())?;
    let cwd = cwd.canonicalize().ok()?;
    let home = user_dirs::env_path("CODEX_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| user_dirs::home().map(|p| p.join(".codex")))?;
    let home = cwd.join(home); // Absolute paths replace cwd; relative homes resolve against it.
    let system = user_dirs::codex_system_config_dir();
    for root in [&home, &system] {
        for name in ["managed_config.toml", "requirements.toml"] {
            if !absent(&root.join(name)) {
                return None;
            }
        }
    }
    if !rule_files(&system)?.is_empty() {
        return None;
    }
    let canonical_home = home.canonicalize().unwrap_or_else(|_| home.clone());
    for dir in user_dirs::ancestors(&cwd) {
        let project = dir.join(".codex");
        if project.canonicalize().ok().as_ref() != Some(&canonical_home)
            && !rule_files(&project)?.is_empty()
        {
            return None;
        }
    }
    let mut rules = Vec::new();
    for path in rule_files(&home)? {
        if fs::metadata(&path).ok()?.len() > 1_048_576 {
            return None;
        }
        rules.extend(Parser::new(&fs::read_to_string(path).ok()?).rules()?);
    }
    Some(rules)
}

/// A literal grammar, not a Starlark interpreter. Unsupported statements,
/// computed values, examples and host_executable declarations all defer.
struct Parser<'a> {
    rest: &'a str,
}

impl<'a> Parser<'a> {
    fn new(rest: &'a str) -> Self {
        Self { rest }
    }

    fn space(&mut self) {
        loop {
            self.rest = self.rest.trim_start_matches([' ', '\t', '\n', '\r']);
            if self.rest.starts_with('#') {
                self.rest = self.rest.split_once('\n').map_or("", |(_, rest)| rest);
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, token: char) -> bool {
        self.space();
        if let Some(rest) = self.rest.strip_prefix(token) {
            self.rest = rest;
            true
        } else {
            false
        }
    }

    fn ident(&mut self) -> Option<&'a str> {
        self.space();
        let len = self
            .rest
            .bytes()
            .take_while(|b| b.is_ascii_alphabetic() || *b == b'_')
            .count();
        if len == 0 {
            return None;
        }
        let (ident, rest) = self.rest.split_at(len);
        self.rest = rest;
        Some(ident)
    }

    fn string(&mut self) -> Option<String> {
        self.space();
        let quote = self.rest.chars().next()?;
        if !matches!(quote, '\'' | '"') {
            return None;
        }
        self.rest = &self.rest[1..];
        let mut value = String::new();
        loop {
            let c = self.rest.chars().next()?;
            self.rest = &self.rest[c.len_utf8()..];
            match c {
                c if c == quote => return Some(value),
                '\\' => {
                    let escaped = self.rest.chars().next()?;
                    self.rest = &self.rest[escaped.len_utf8()..];
                    value.push(match escaped {
                        '\\' | '\'' | '"' => escaped,
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        _ => return None,
                    });
                }
                c if c.is_control() => return None,
                c => value.push(c),
            }
        }
    }

    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> Option<T>) -> Option<Vec<T>> {
        if !self.eat('[') {
            return None;
        }
        let mut values = vec![item(self)?];
        while self.eat(',') {
            if self.eat(']') {
                return Some(values);
            }
            values.push(item(self)?);
        }
        self.eat(']').then_some(values)
    }

    fn alternatives(&mut self) -> Option<Vec<String>> {
        self.space();
        if self.rest.starts_with('[') {
            self.list(Self::string)
        } else {
            Some(vec![self.string()?])
        }
    }

    fn rule(&mut self) -> Option<Rule> {
        if self.ident()? != "prefix_rule" {
            return None;
        }
        // A top-level call cannot put a newline between its name and `(`.
        self.rest = self.rest.trim_start_matches([' ', '\t']);
        self.rest = self.rest.strip_prefix('(')?;
        let mut pattern = None;
        let mut decision = None;
        let mut justification = false;
        loop {
            match self.ident()? {
                "pattern" if pattern.is_none() && self.eat('=') => {
                    pattern = Some(self.list(Self::alternatives)?)
                }
                "decision" if decision.is_none() && self.eat('=') => {
                    decision = Some(match self.string()?.as_str() {
                        "allow" => Decision::Allow,
                        "prompt" => Decision::Prompt,
                        "forbidden" => Decision::Forbidden,
                        _ => return None,
                    });
                }
                "justification" if !justification && self.eat('=') => {
                    if self.string()?.trim().is_empty() {
                        return None;
                    }
                    justification = true;
                }
                _ => return None,
            }
            if self.eat(')') {
                break;
            }
            if !self.eat(',') {
                return None;
            }
            if self.eat(')') {
                break;
            }
        }
        Some(Rule {
            pattern: pattern?,
            decision: decision.unwrap_or(Decision::Allow),
        })
    }

    fn rules(&mut self) -> Option<Vec<Rule>> {
        let mut rules = Vec::new();
        self.space();
        while !self.rest.is_empty() {
            rules.push(self.rule()?);
            let trailing = self.rest;
            self.space();
            let separator = &trailing[..trailing.len() - self.rest.len()];
            if !self.rest.is_empty() && !separator.contains('\n') {
                return None;
            }
        }
        Some(rules)
    }
}

#[cfg(test)]
#[path = "permissions_codex_tests.rs"]
mod tests;
