//! Enforces the rule on [`display_args`](super::display_args): a tracked label quotes the
//! user's words, so none is built by joining argv with `" "`.
//!
//! The scan starts at every call that records a label, takes the arguments that carry it,
//! and follows the `let` bindings they name, back to the nearest one before the call in the
//! same function, destructured tuples included. A `.join(" ")` anywhere on that path fails
//! the test. Other arguments, such as the output a filter closure formats, are never looked
//! at.

use crate::core::test_isolation::{rust_files, without_test_items};
use regex::Regex;
use std::sync::LazyLock;

/// The calls that record a label, and the positions of the arguments that carry it:
/// the tracker itself, the runner entry points, and the helpers that take a label and
/// pass it on.
static LABEL_CALLS: LazyLock<Vec<(Regex, &'static [usize])>> = LazyLock::new(|| {
    let call = |pattern: &str| Regex::new(pattern).expect("valid regex");
    vec![
        (call(r"\.track(?:_passthrough|_bytes)?\("), &[0, 1]),
        (
            call(
                r"\b(?:runner::run|run_filtered|run_filtered_with_exit|run_streamed|run_err_cmd|run_test_cmd)\(",
            ),
            &[1, 2],
        ),
        (
            call(r"\b(?:run_err_unrunnable|run_test_unrunnable)\("),
            &[0, 1],
        ),
        // gh_cmd.rs, glab_cmd.rs: `(cmd, label, filter)`.
        (call(r"\b(?:run_gh_json|run_glab_json)\("), &[1]),
        // git_cmd.rs: `(bytes, label, rtk_label, timer, exit_code)`.
        (call(r"\bemit_raw_bytes_passthrough\("), &[1, 2]),
        // aws_cmd.rs: `(subcommand, args, verbose, full_sub)`.
        (call(r"\brun_generic\("), &[3]),
        // tracking.rs: `(command, error, fallback_succeeded)`.
        (call(r"\brecord_parse_failure_silent\("), &[0]),
        // search.rs: `(timer, engine, args, real_cmd, ..)`.
        (call(r"\bpassthrough\("), &[3]),
    ]
});

static INLINE_ARG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([A-Za-z_]\w*)").expect("valid regex"));
static FN_ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bfn\s+\w+").expect("valid regex"));
static RAW_STRING_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^b?r(#*)""#).expect("valid regex"));
static TUPLE_LET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\blet\s+\(").expect("valid regex"));
static IDENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Za-z_]\w*\b").expect("valid regex"));

/// Length of the string literal `code` starts with, its quotes included.
fn string_len(code: &str) -> usize {
    let mut chars = code.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '"' => return i + 1,
            _ => {}
        }
    }
    code.len()
}

/// Length of the character literal `code` starts with, or `None` for a lifetime.
fn char_literal_len(code: &str) -> Option<usize> {
    let rest = &code[1..];
    if rest.starts_with('\\') {
        return rest[2..].find('\'').map(|at| at + 4);
    }
    let c = rest.chars().next()?;
    rest[c.len_utf8()..]
        .starts_with('\'')
        .then_some(c.len_utf8() + 2)
}

/// Length of the comment or literal (string, raw string, byte string, character) that
/// starts `code`, if one does. `after_ident` says the previous character belongs to an
/// identifier, where an `r` or `b` is part of the name rather than a literal prefix.
fn skipped_len(code: &str, after_ident: bool) -> Option<usize> {
    if code.starts_with("//") {
        return Some(code.find('\n').unwrap_or(code.len()));
    }
    if let Some(body) = code.strip_prefix("/*") {
        return Some(body.find("*/").map_or(code.len(), |at| at + 4));
    }
    if !after_ident {
        if let Some(open) = RAW_STRING_OPEN.captures(code) {
            let close = format!("\"{}", &open[1]);
            let start = open.get(0).map_or(0, |m| m.end());
            return Some(
                code[start..]
                    .find(&close)
                    .map_or(code.len(), |at| start + at + close.len()),
            );
        }
        if let Some(rest) = code.strip_prefix('b')
            && rest.starts_with(['"', '\''])
        {
            return skipped_len(rest, false).map(|len| len + 1);
        }
    }
    match code.chars().next()? {
        '"' => Some(string_len(code)),
        '\'' => char_literal_len(code),
        _ => None,
    }
}

/// The comments and literals of `code` as `(offset, length)` pairs, in source order.
fn skipped_spans(code: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut i = 0;
    while let Some(c) = code[i..].chars().next() {
        let after_ident = code[..i]
            .chars()
            .next_back()
            .is_some_and(|p| p.is_alphanumeric() || p == '_');
        match skipped_len(&code[i..], after_ident) {
            Some(len) => {
                spans.push((i, len));
                i += len;
            }
            None => i += c.len_utf8(),
        }
    }
    spans
}

/// `code` with every comment and literal blanked to spaces, newlines kept, so offsets
/// still line up with `code` but nothing inside them reads as a call or a binding.
fn blanked(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut i = 0;
    for (at, len) in skipped_spans(code) {
        out.push_str(&code[i..at]);
        out.extend(
            code[at..at + len]
                .bytes()
                .map(|b| if b == b'\n' { '\n' } else { ' ' }),
        );
        i = at + len;
    }
    out.push_str(&code[i..]);
    out
}

/// The characters of `blank` outside any bracket opened in it, with their offsets, up to and
/// including the first closer that has no opener. `blank` is [`blanked`] text, so no
/// comment or literal reaches this scan. Closure parameters (`|a, b|`) and turbofish
/// generics (`::<A, B>`) count as brackets, so their commas do not split an argument list.
fn top_level(blank: &str) -> Vec<(usize, char)> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut at_arg_start = true;
    let mut i = 0;
    while let Some(c) = blank[i..].chars().next() {
        let rest = &blank[i..];
        if at_arg_start && rest.starts_with("move") && rest[4..].starts_with([' ', '|']) {
            i += 4;
            continue;
        }
        let len = match c {
            '|' if at_arg_start => rest[1..].find('|').map_or(rest.len(), |at| at + 2),
            '<' if blank[..i].ends_with("::") => generics_len(rest),
            _ => c.len_utf8(),
        };
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => {
                out.push((i, c));
                break;
            }
            ')' | ']' | '}' => depth -= 1,
            '|' | '<' if len > 1 => {}
            _ if depth == 0 => out.push((i, c)),
            _ => {}
        }
        if !c.is_whitespace() {
            at_arg_start = depth == 0 && c == ',';
        }
        i += len;
    }
    out
}

/// Length of the `<..>` generic list `code` starts with, nested lists included.
fn generics_len(code: &str) -> usize {
    let mut depth = 0usize;
    for (at, c) in code.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
    }
    code.len()
}

/// A stretch of source next to its [`blanked`] copy. The two have the same length, so one
/// offset addresses both: structure is read from `blank`, text is taken from `code`.
#[derive(Clone, Copy)]
struct Text<'a> {
    code: &'a str,
    blank: &'a str,
}

impl<'a> Text<'a> {
    fn new(code: &'a str, blank: &'a str) -> Self {
        Self { code, blank }
    }

    fn from(&self, start: usize) -> Self {
        self.range(start, self.code.len())
    }

    fn range(&self, start: usize, end: usize) -> Self {
        Self::new(&self.code[start..end], &self.blank[start..end])
    }

    /// Without the whitespace around `code`.
    fn trimmed(&self) -> Self {
        let lead = self.code.len() - self.code.trim_start().len();
        let kept = self.code.trim().len();
        self.range(lead, lead + kept)
    }
}

/// The arguments of the call whose text starts right after its `(`.
fn call_args(text: Text) -> Vec<Text> {
    let mut args = Vec::new();
    let mut start = 0;
    for (at, c) in top_level(text.blank) {
        if c == ',' || c == ')' {
            args.push(text.range(start, at).trimmed());
            start = at + 1;
        }
        if c == ')' {
            break;
        }
    }
    args
}

/// The statement starting at the beginning of `text`, up to its `;`.
fn statement(text: Text) -> Text {
    let end = top_level(text.blank)
        .into_iter()
        .find(|&(_, c)| c == ';')
        .map_or(text.code.len(), |(at, _)| at);
    text.range(0, end)
}

/// The names an expression reads, `{name}` format arguments of its literals included.
/// Comments are not read.
fn names(expr: Text) -> Vec<String> {
    let mut out: Vec<String> = skipped_spans(expr.code)
        .into_iter()
        .map(|(at, len)| &expr.code[at..at + len])
        .filter(|span| !span.starts_with("//") && !span.starts_with("/*"))
        .flat_map(|literal| INLINE_ARG.captures_iter(literal))
        .map(|c| c[1].to_string())
        .collect();
    out.extend(IDENT.find_iter(expr.blank).map(|m| m.as_str().to_string()));
    out
}

/// What the last `let` in `text` binds `name` to: the statement `let name = ..`, or for
/// `let (.., name, ..) = (.., expr, ..)` the matching element. The whole statement when a
/// destructured right side is not a tuple of the same length.
fn binding<'a>(text: Text<'a>, name: &str) -> Option<Text<'a>> {
    let plain = Regex::new(&format!(r"\blet\s+(?:mut\s+)?{name}\b")).expect("valid regex");
    let tuple =
        Regex::new(&format!(r"^let\s+\(([^()=]*\b{name}\b[^()=]*)\)\s*=")).expect("valid regex");
    let at = plain
        .find_iter(text.blank)
        .chain(
            TUPLE_LET
                .find_iter(text.blank)
                .filter(|m| tuple.is_match(&text.blank[m.start()..])),
        )
        .map(|m| m.start())
        .max()?;
    let stmt = statement(text.from(at));
    let Some(pattern) = tuple.captures(stmt.blank) else {
        return Some(stmt);
    };
    let names: Vec<&str> = pattern[1]
        .split(',')
        .map(|n| {
            n.trim()
                .trim_start_matches('&')
                .trim_start_matches("mut ")
                .trim()
        })
        .collect();
    let rhs = stmt.from(pattern.get(0).map_or(0, |m| m.end()));
    let lead = rhs.blank.len() - rhs.blank.trim_start().len();
    let elements = if rhs.blank[lead..].starts_with('(') {
        call_args(rhs.from(lead + 1))
    } else {
        Vec::new()
    };
    match names.iter().position(|n| *n == name) {
        Some(i) if elements.len() == names.len() => Some(elements[i]),
        _ => Some(stmt),
    }
}

/// Whether `expr` calls `.join(" ")` outside any comment or literal.
fn joins_with_a_space(expr: Text) -> bool {
    expr.blank
        .match_indices(".join(")
        .any(|(at, _)| expr.code[at..].starts_with(r#".join(" ")"#))
}

/// How many label calls `code` makes, and the label expressions among them that reach a
/// `.join(" ")`.
fn space_joined_labels(code: &str) -> (usize, Vec<String>) {
    let blank = blanked(code);
    let whole = Text::new(code, &blank);
    let mut calls = 0;
    let mut offenders = Vec::new();
    for (call, positions) in LABEL_CALLS.iter() {
        for head in call.find_iter(&blank) {
            if blank[..head.start()].trim_end().ends_with("fn") {
                continue;
            }
            calls += 1;
            // Where the enclosing function starts: a binding before it is another function's.
            let body = FN_ITEM
                .find_iter(&blank[..head.start()])
                .last()
                .map_or(0, |item| item.start());
            let args = call_args(whole.from(head.end()));
            let mut todo: Vec<Text> = positions
                .iter()
                .filter_map(|&i| args.get(i).copied())
                .collect();
            let mut seen = Vec::new();
            while let Some(expr) = todo.pop() {
                if joins_with_a_space(expr) {
                    let line = expr.code.lines().next().unwrap_or(expr.code);
                    offenders.push(line.trim().to_string());
                    break;
                }
                for name in names(expr) {
                    if seen.contains(&name) {
                        continue;
                    }
                    if let Some(expr) = binding(whole.range(body, head.start()), &name) {
                        todo.push(expr);
                    }
                    seen.push(name);
                }
            }
        }
    }
    (calls, offenders)
}

// A `cfg(test)` item, so the scan below strips this planted code from its own walk.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scan_follows_a_label_and_ignores_the_output() {
        let code = r#"
            fn f(args: &[String]) {
                let words = args.join(" ");
                let tracked = format!("git log {}", words);
                timer.track(&tracked, &format!("rtk {tracked}"), &raw, &lines.join(" "));
                let label = display_args(args);
                runner::run_filtered(cmd, "docker", &label, |raw| raw.join(" "), opts);
            }
            fn g(args: &[String]) {
                let shown = args.join(" ");
                println!("{shown}");
            }
            fn h(cmd: Command) {
                runner::run_filtered(cmd, "docker", &shown, |raw| raw, opts);
            }
        "#;
        let (calls, offenders) = space_joined_labels(code);
        assert_eq!(calls, 3);
        assert_eq!(offenders, vec![r#"let words = args.join(" ")"#]);
    }

    /// The helpers that pass a label on are calls too, and a label bound by destructuring
    /// is followed to its own element of the tuple.
    #[test]
    fn the_scan_covers_label_helpers_and_destructuring() {
        let code = r##"
            fn f(args: &[String]) {
                run_gh_json(cmd, &args.join(" "), |v| v.to_string());
                run_generic(sub, args, 0, &args.join(" "));
                emit_raw_bytes_passthrough(&bytes, &display_args(args), "rtk x", &timer, 0);
                let (label, rtk) = (args.join(" "), 1);
                timer.track(&label, "rtk x", &raw, &shown);
            }
            fn g(args: &[String]) {
                let (label, shown) = (display_args(args), lines.join(" "));
                timer.track(&label, "rtk x", &raw, &shown);
            }
            fn h(args: &[String]) {
                let raw_command = args.join(" ");
                core::tracking::record_parse_failure_silent(&raw_command, &message, true);
                emit_raw_bytes_passthrough(move |a, b| a, &args.join(" "), "rtk x", &t, 0);
                runner::run(cmd, from_fn::<A, B>(x), &args.join(" "), mode, opts);
                runner::run_filtered(cmd, "x", &display_args(args), |a, b| a.join(" "), o);
            }
            fn k(args: &[String]) {
                // timer.track(&args.join(" "), "rtk x", &raw, &shown);
                /* runner::run(cmd, "x", &args.join(" "), mode, opts); */
                let pattern = r"\.track(&x, &y)";
                runner::run_filtered(cmd, r#"x", y"#, &args.join(" "), |raw| raw, o);
            }
        "##;
        let (calls, mut offenders) = space_joined_labels(code);
        offenders.sort();
        assert_eq!(calls, 10);
        assert_eq!(
            offenders,
            vec![
                r#"&args.join(" ")"#,
                r#"&args.join(" ")"#,
                r#"&args.join(" ")"#,
                r#"&args.join(" ")"#,
                r#"&args.join(" ")"#,
                r#"args.join(" ")"#,
                r#"let raw_command = args.join(" ")"#,
            ]
        );
    }

    /// A comment inside an argument or a binding is not code, whatever it mentions.
    #[test]
    fn a_comment_inside_a_label_is_not_an_offender() {
        let code = r#"
            fn f(args: &[String]) {
                timer.track(&tracked /* was args.join(" ") */, "rtk x", &raw, &shown);
                let label = format!(
                    // old: words.join(" ")
                    "git {}",
                    display_args(args)
                );
                timer.track(&label, "rtk x", &raw, &shown);
            }
            fn g(args: &[String]) {
                timer.track(&args /* note */ .join(" "), "rtk x", &raw, &shown);
            }
        "#;
        let (calls, offenders) = space_joined_labels(code);
        assert_eq!(calls, 3);
        assert_eq!(offenders, vec![r#"&args /* note */ .join(" ")"#]);
    }

    #[test]
    fn a_name_inside_a_comment_is_not_followed() {
        let code = r#"
            fn f(args: &[String]) {
                let words = args.join(" ");
                timer.track(&label /* was "{words}" */, "rtk x", &raw, &shown);
            }
            fn g(args: &[String]) {
                let words = args.join(" ");
                timer.track(&format!("git {words}"), "rtk x", &raw, &shown);
            }
        "#;
        let (calls, offenders) = space_joined_labels(code);
        assert_eq!(calls, 2);
        assert_eq!(offenders, vec![r#"let words = args.join(" ")"#]);
    }

    #[test]
    fn no_tracked_label_is_a_space_join() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut calls = 0;
        let mut offenders = Vec::new();
        for path in rust_files(&root.join("src")) {
            let code = without_test_items(&std::fs::read_to_string(&path).expect("read source"));
            let (found, joined) = space_joined_labels(&code);
            calls += found;
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            offenders.extend(joined.into_iter().map(|expr| format!("{relative}: {expr}")));
        }
        // The sources are found through a path baked in at compile time; a run that reached
        // none of them would otherwise report success.
        assert!(calls > 100, "the scan reached only {calls} label calls");
        assert!(
            offenders.is_empty(),
            "these labels join argv with spaces, so `a b` and `'a b'` record the same text; \
             build them with `display_args` (or `quote_word` for one word):\n  {}",
            offenders.join("\n  ")
        );
    }
}
