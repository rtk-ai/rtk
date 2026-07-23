use super::lexer::shell_split;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Engine {
    Grep,
    Rg,
}

impl Engine {
    fn parse(program: &str) -> Option<Self> {
        match program {
            "grep" => Some(Self::Grep),
            "rg" => Some(Self::Rg),
            _ => None,
        }
    }

    fn short_value_flag(self, flag: char) -> Option<&'static str> {
        match (self, flag) {
            (_, 'A') => Some("-A"),
            (_, 'B') => Some("-B"),
            (_, 'C') => Some("-C"),
            (_, 'm') => Some("--max-count"),
            (Self::Rg, 'g') => Some("--glob"),
            (Self::Rg, 'E') => Some("--encoding"),
            (Self::Rg, 'r') => Some("--replace"),
            _ => None,
        }
    }

    fn push_short_flag(self, flags: &mut Vec<String>, flag: char) -> bool {
        let normalized = match (self, flag) {
            (_, 'H') => "--with-filename",
            (Self::Rg, 'I') => "--no-filename",
            (Self::Rg, 'L') => "--follow",
            (Self::Rg, 'l') => "--files-with-matches",
            (Self::Grep, 'n' | 'E' | 'r' | 'R')
            | (Self::Rg, 'n')
            | (_, 'i' | 'F' | 'w' | 'v' | 'x')
            | (Self::Rg, 'o' | 'c' | 'q') => {
                flags.push(format!("-{flag}"));
                return true;
            }
            _ => return false,
        };
        flags.push(normalized.to_string());
        true
    }
}

pub(super) fn rewrite(cmd_part: &str, redirect_suffix: &str) -> Option<String> {
    if loses_shell_semantics(cmd_part) || cmd_part.contains("''") || cmd_part.contains("\"\"") {
        return None;
    }

    let tokens = shell_split(cmd_part);
    let (program, args) = tokens.split_first()?;
    let engine = Engine::parse(program)?;

    if args.iter().any(|arg| {
        matches!(arg.as_str(), "--version" | "-V" | "--help")
            || (engine == Engine::Rg && arg == "-h")
    }) {
        return None;
    }

    let mut flags = Vec::new();
    let mut positionals = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            positionals.extend(args[i + 1..].iter().cloned());
            break;
        }

        // POSIX grep stops option parsing at the first operand. GNU and BSD grep
        // only accept later options as an extension, so leave that shape raw.
        if engine == Engine::Grep && !positionals.is_empty() && arg.starts_with('-') && arg != "-" {
            return None;
        }

        if arg.starts_with("--") {
            if let Some((name, _)) = arg.split_once('=') {
                if !supports_long_value(engine, name, true) {
                    return None;
                }
                flags.push(arg.clone());
                i += 1;
                continue;
            }

            if supports_long_value(engine, arg, false) {
                flags.push(arg.clone());
                flags.push(args.get(i + 1)?.clone());
                i += 2;
                continue;
            }

            if is_supported_long_flag(engine, arg) {
                flags.push(arg.clone());
                i += 1;
                continue;
            }

            return None;
        }

        if let Some(cluster) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            let consumed_next = push_short_cluster(engine, cluster, args.get(i + 1), &mut flags)?;
            i += if consumed_next { 2 } else { 1 };
            continue;
        }

        positionals.push(arg.clone());
        i += 1;
    }

    let pattern = positionals.first()?;
    if positionals
        .iter()
        .any(|arg| arg.starts_with('-') && arg != "-")
    {
        return None;
    }

    // The first positional enters clap's trailing-var-arg region. Native flags
    // can then precede paths, which is safer for both BSD and GNU grep.
    let command = match engine {
        Engine::Grep => "grep",
        Engine::Rg => "rg",
    };
    let mut parts = vec![
        "rtk".to_string(),
        command.to_string(),
        shell_quote_arg(pattern),
    ];
    parts.extend(flags.iter().map(|arg| shell_quote_arg(arg)));
    parts.extend(positionals.iter().skip(1).map(|arg| shell_quote_arg(arg)));
    Some(format!("{}{}", parts.join(" "), redirect_suffix))
}

fn supports_long_value(engine: Engine, flag: &str, attached: bool) -> bool {
    match (engine, flag) {
        (_, "--after-context" | "--before-context" | "--max-count") => true,
        (Engine::Grep, "--context") => attached,
        (Engine::Rg, "--context" | "--glob" | "--encoding" | "--replace") => true,
        _ => false,
    }
}

fn is_supported_long_flag(engine: Engine, flag: &str) -> bool {
    matches!(
        flag,
        "--line-number"
            | "--ignore-case"
            | "--fixed-strings"
            | "--word-regexp"
            | "--with-filename"
            | "--invert-match"
            | "--line-regexp"
    ) || matches!(
        (engine, flag),
        (
            Engine::Grep,
            "--extended-regexp" | "--recursive" | "--dereference-recursive"
        ) | (
            Engine::Rg,
            "--only-matching"
                | "--count"
                | "--no-filename"
                | "--files-with-matches"
                | "--files-without-match"
                | "--quiet"
                | "--follow"
                | "--no-line-number"
        )
    )
}

fn push_short_cluster(
    engine: Engine,
    cluster: &str,
    next: Option<&String>,
    flags: &mut Vec<String>,
) -> Option<bool> {
    let mut chars = cluster.chars();
    while let Some(flag) = chars.next() {
        if let Some(normalized) = engine.short_value_flag(flag) {
            let inline: String = chars.collect();
            flags.push(normalized.to_string());
            if inline.is_empty() {
                flags.push(next?.clone());
                return Some(true);
            }
            flags.push(inline);
            return Some(false);
        }
        if !engine.push_short_flag(flags, flag) {
            return None;
        }
    }
    Some(false)
}

fn shell_quote_arg(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '='))
    {
        arg.to_string()
    } else if !arg.is_empty()
        && arg.chars().all(|c| {
            c.is_ascii_alphanumeric() || c.is_ascii_whitespace() || matches!(c, '_' | '-' | '.')
        })
    {
        format!("\"{arg}\"")
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

fn loses_shell_semantics(cmd: &str) -> bool {
    let mut chars = cmd.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut at_word_start = true;
    let mut brace_depth = 0usize;
    let mut previous_brace_dot = false;

    while let Some(ch) = chars.next() {
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }

        if in_double {
            match ch {
                '"' => in_double = false,
                '$' | '`' => return true,
                '\\' => match chars.next() {
                    Some('$' | '`' | '"' | '\\' | '\n') => {}
                    _ => return true,
                },
                _ => {}
            }
            continue;
        }

        match ch {
            '\\' => {
                if chars.next().is_none() {
                    return true;
                }
                at_word_start = false;
                previous_brace_dot = false;
            }
            '\'' => {
                in_single = true;
                at_word_start = false;
                previous_brace_dot = false;
            }
            '"' => {
                in_double = true;
                at_word_start = false;
                previous_brace_dot = false;
            }
            '$' | '`' | '*' | '?' | '[' | '(' | ')' | '<' | '>' | '|' | '&' | ';' => {
                return true;
            }
            '#' if at_word_start => return true,
            '~' if at_word_start => return true,
            '{' => {
                brace_depth += 1;
                at_word_start = false;
                previous_brace_dot = false;
            }
            '}' => {
                brace_depth = brace_depth.saturating_sub(1);
                at_word_start = false;
                previous_brace_dot = false;
            }
            ',' if brace_depth > 0 => return true,
            '.' if brace_depth > 0 => {
                if previous_brace_dot {
                    return true;
                }
                at_word_start = false;
                previous_brace_dot = true;
            }
            ' ' | '\t' => {
                at_word_start = true;
                previous_brace_dot = false;
            }
            '\n' | '\r' => return true,
            _ => {
                at_word_start = false;
                previous_brace_dot = false;
            }
        }
    }

    in_single || in_double
}

#[cfg(test)]
#[path = "grep_rewrite_tests.rs"]
mod tests;
