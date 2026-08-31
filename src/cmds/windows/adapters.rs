//! Conservative display adapters for CMD built-ins.

use regex::Regex;
use std::sync::OnceLock;

#[derive(Clone, Copy)]
enum DisplayAdapter {
    Dir,
    Set,
    Help,
    Assoc,
    Ftype,
}

impl DisplayAdapter {
    fn parse(adapter: &str) -> Option<Self> {
        match adapter {
            "dir" => Some(Self::Dir),
            "set" => Some(Self::Set),
            "help" => Some(Self::Help),
            "assoc" => Some(Self::Assoc),
            "ftype" => Some(Self::Ftype),
            _ => None,
        }
    }
}

/// Whether an adapter name in the checked-in catalog has an implementation.
pub fn supports_adapter(adapter: &str) -> bool {
    DisplayAdapter::parse(adapter).is_some()
}

/// Whether `source` is a non-mutating display form that may be filtered.
pub fn is_display_form(adapter: &str, source: &str) -> bool {
    let Some(adapter) = DisplayAdapter::parse(adapter) else {
        return false;
    };
    is_display_form_adapter(adapter, source)
}

fn is_display_form_adapter(adapter: DisplayAdapter, source: &str) -> bool {
    let arguments = source_arguments(source);
    match adapter {
        DisplayAdapter::Dir => dir_uses_supported_detailed_layout(arguments),
        DisplayAdapter::Set => {
            !arguments.contains('=')
                && !arguments.starts_with("/a")
                && !arguments.starts_with("/A")
                && !arguments.starts_with("/p")
                && !arguments.starts_with("/P")
        }
        DisplayAdapter::Assoc | DisplayAdapter::Ftype => !arguments.contains('='),
        DisplayAdapter::Help => true,
    }
}

/// Admit only switches whose native output retains the detailed layout parsed
/// by `filter_dir`. CMD accepts combined forms such as `/s/a-d/o:n`; quoted
/// path tokens are never interpreted as switches here.
fn dir_uses_supported_detailed_layout(arguments: &str) -> bool {
    let mut in_quotes = false;
    let mut token_start = None;
    for (index, character) in arguments.char_indices() {
        if character == '"' {
            in_quotes = !in_quotes;
            continue;
        }
        if !in_quotes && character.is_whitespace() {
            if let Some(start) = token_start.take() {
                if !dir_switch_token_is_supported(&arguments[start..index]) {
                    return false;
                }
            }
        } else if !in_quotes && token_start.is_none() {
            token_start = Some(index);
        }
    }
    token_start.is_none_or(|start| dir_switch_token_is_supported(&arguments[start..]))
}

fn dir_switch_token_is_supported(token: &str) -> bool {
    let token = normalize_cmd_caret_escapes(token);
    let Some(switches) = token.strip_prefix('/') else {
        return true;
    };
    switches.split('/').all(dir_switch_is_supported)
}

fn dir_switch_is_supported(switch: &str) -> bool {
    let mut characters = switch.chars();
    let Some(kind) = characters.next() else {
        return false;
    };
    let suffix = characters.as_str();
    match kind.to_ascii_uppercase() {
        'A' => dir_switch_list_is_supported(suffix, "DRAHSILO"),
        'O' => dir_switch_list_is_supported(suffix, "NEGSDA"),
        'T' => dir_time_switch_is_supported(suffix),
        'C' | 'L' | 'N' | 'S' | '4' => suffix.is_empty(),
        '-' => suffix.eq_ignore_ascii_case("c"),
        _ => false,
    }
}

fn dir_switch_list_is_supported(suffix: &str, allowed: &str) -> bool {
    if suffix.is_empty() {
        return true;
    }
    let value = suffix.strip_prefix(':').unwrap_or(suffix);
    if value.is_empty() {
        return false;
    }
    let mut previous_was_minus = false;
    for character in value.chars() {
        if character == '-' {
            if previous_was_minus {
                return false;
            }
            previous_was_minus = true;
        } else if allowed.contains(character.to_ascii_uppercase()) {
            previous_was_minus = false;
        } else {
            return false;
        }
    }
    !previous_was_minus
}

fn dir_time_switch_is_supported(suffix: &str) -> bool {
    if suffix.is_empty() {
        return true;
    }
    let value = suffix.strip_prefix(':').unwrap_or(suffix);
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| "CAW".contains(character.to_ascii_uppercase()))
        && characters.next().is_none()
}

/// CMD removes a caret when it quotes the following metacharacter before DIR
/// receives its switches. Mirror that narrow normalization for eligibility.
fn normalize_cmd_caret_escapes(token: &str) -> String {
    let mut normalized = String::with_capacity(token.len());
    let mut characters = token.chars();
    while let Some(character) = characters.next() {
        if character == '^' {
            if let Some(escaped) = characters.next() {
                normalized.push(escaped);
            } else {
                normalized.push(character);
            }
        } else {
            normalized.push(character);
        }
    }
    normalized
}

/// Return a compact display only when the native layout is confidently known.
pub fn filter_display(adapter: &str, source: &str, stdout: &str) -> Option<String> {
    let adapter = DisplayAdapter::parse(adapter)?;
    if !is_display_form_adapter(adapter, source) {
        return None;
    }

    match adapter {
        DisplayAdapter::Dir => filter_dir(stdout),
        DisplayAdapter::Set => filter_set(stdout),
        DisplayAdapter::Help => filter_help(stdout),
        DisplayAdapter::Assoc => {
            filter_assignments(stdout, "[assoc]", |name| name.starts_with('.'), false)
        }
        DisplayAdapter::Ftype => filter_assignments(
            stdout,
            "[ftype]",
            |name| !name.is_empty() && !name.chars().any(char::is_whitespace),
            false,
        ),
    }
}

fn source_arguments(source: &str) -> &str {
    let source = source
        .trim_start()
        .strip_prefix('@')
        .unwrap_or(source.trim_start())
        .trim_start();
    source
        .split_once(char::is_whitespace)
        .map_or("", |(_, rest)| rest)
        .trim()
}

fn filter_set(stdout: &str) -> Option<String> {
    let entries = parse_assignments(stdout, |name| !name.is_empty(), true)?;
    if entries.len() <= 8 {
        return Some(format!("[set] {}", entries.join("; ")));
    }
    let shown = entries
        .iter()
        .take(4)
        .copied()
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!(
        "[set] {} vars: {shown}; ... +{} more",
        entries.len(),
        entries.len() - 4
    ))
}

fn filter_assignments<F>(
    stdout: &str,
    label: &str,
    valid_name: F,
    allow_empty_value: bool,
) -> Option<String>
where
    F: Fn(&str) -> bool,
{
    let entries = parse_assignments(stdout, valid_name, allow_empty_value)?;
    Some(format!("{label} {}", entries.join("; ")))
}

fn parse_assignments<F>(stdout: &str, valid_name: F, allow_empty_value: bool) -> Option<Vec<&str>>
where
    F: Fn(&str) -> bool,
{
    let entries = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (name, value) = line.split_once('=')?;
            (valid_name(name) && (allow_empty_value || !value.is_empty())).then_some(line)
        })
        .collect::<Option<Vec<_>>>()?;
    (!entries.is_empty()).then_some(entries)
}

fn filter_help(stdout: &str) -> Option<String> {
    let lines = stdout.lines().map(str::trim_end).collect::<Vec<_>>();
    let description = lines
        .iter()
        .copied()
        .find(|line| is_english_sentence(line.trim()))?
        .trim();
    let usage = lines
        .iter()
        .copied()
        .find(|line| is_usage_line(line.trim()))?
        .trim();
    lines
        .iter()
        .filter(|line| {
            !line.trim().is_empty() && line.trim() != description && line.trim() != usage
        })
        .all(|line| line.starts_with(' '))
        .then(|| format!("[help] {usage}\r\n{description}"))
}

fn is_english_sentence(line: &str) -> bool {
    line.is_ascii()
        && (line.starts_with("Displays ")
            || line.starts_with("Provides ")
            || line.starts_with("Creates ")
            || line.starts_with("Deletes ")
            || line.starts_with("Changes "))
}

fn is_usage_line(line: &str) -> bool {
    let command = line.split_whitespace().next().unwrap_or_default();
    !command.is_empty()
        && command
            .chars()
            .all(|character| character.is_ascii_uppercase())
        && line.is_ascii()
        && line.chars().any(|character| matches!(character, '[' | '<'))
}

fn filter_dir(stdout: &str) -> Option<String> {
    static ENTRY: OnceLock<Regex> = OnceLock::new();
    static FILE_TOTAL: OnceLock<Regex> = OnceLock::new();
    static DIR_TOTAL: OnceLock<Regex> = OnceLock::new();
    let entry = ENTRY.get_or_init(|| {
        Regex::new(
            r"^\d{2}/\d{2}/\d{4}\s+\d{2}:\d{2}\s+(?:AM|PM)\s+(?:(<DIR>)\s+|([0-9][0-9,]*)\s+)(.+)$",
        )
        .expect("static directory entry regex")
    });
    let file_total = FILE_TOTAL
        .get_or_init(|| Regex::new(r"^\s*\d+ File\(s\)").expect("static file total regex"));
    let dir_total =
        DIR_TOTAL.get_or_init(|| Regex::new(r"^\s*\d+ Dir\(s\)").expect("static dir total regex"));

    let mut output = Vec::new();
    let mut path = None;
    let mut entries = 0usize;
    let mut saw_footer = false;
    for line in stdout.lines().map(str::trim_end) {
        if line.trim().is_empty()
            || line.starts_with(" Volume in drive ")
            || line.starts_with(" Volume Serial Number is ")
            || line.trim() == "Total Files Listed:"
        {
            continue;
        }
        if let Some(directory) = line.strip_prefix(" Directory of ") {
            path = Some(directory.trim());
            output.push(format!("[dir] {}", directory.trim()));
            continue;
        }
        if file_total.is_match(line) || dir_total.is_match(line) {
            saw_footer = true;
            continue;
        }
        let captures = entry.captures(line)?;
        let current_path = path?;
        let name = captures.get(3)?.as_str().trim();
        if name.is_empty() || current_path.is_empty() {
            return None;
        }
        let item = if captures.get(1).is_some() {
            format!("D {name}")
        } else {
            format!("F {} {name}", captures.get(2)?.as_str())
        };
        output.push(item);
        entries += 1;
    }
    (path.is_some() && saw_footer && entries > 0).then(|| {
        output.push(format!("{entries} entries"));
        output.join("\r\n")
    })
}
