//! Frozen copy of this module as it stood before `Dialect` became a struct of axes, kept as
//! the reference oracle for the differential test. Never edit to match new behaviour: a diff
//! against it is the only proof that `Posix` and `Msbuild` still tokenize identically.

#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    DashDash,
    Long,
    Positional,
    Short,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: TokenKind,
    pub text: &'a str,
    pub attached: Option<&'a str>,
    pub linked: Option<usize>,
    pub source_index: usize,
    pub double_dash: bool,
    pub slash: bool,
}

impl<'a> Token<'a> {
    pub fn value(&self, tokens: &[Token<'a>]) -> Option<&'a str> {
        if self.kind == TokenKind::Positional {
            // `linked` points the other way here -- at the flag that consumed this token, whose
            // *name* is not this token's value.
            return None;
        }
        self.attached.or_else(|| {
            // Indices address the vec this token came from; a caller holding a slice of it
            // (before_dashdash, `tokens[i + 1..]`) would otherwise index out of bounds, and a
            // panic in a filter is the one thing RTK must never do.
            self.linked
                .and_then(|index| tokens.get(index))
                .map(|token| token.text)
        })
    }

    pub fn is_free_positional(&self) -> bool {
        self.kind == TokenKind::Positional && self.linked.is_none()
    }
}

pub fn is_digit_run(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

fn flag_name_matches(text: &str, name: &str, dialect: Dialect) -> bool {
    match dialect {
        Dialect::Msbuild => text.eq_ignore_ascii_case(name),
        Dialect::Posix => text == name,
    }
}

pub fn dashdash_index(tokens: &[Token<'_>]) -> Option<usize> {
    tokens.iter().position(|t| t.kind == TokenKind::DashDash)
}

pub fn before_dashdash<'t, 'a>(tokens: &'t [Token<'a>]) -> &'t [Token<'a>] {
    match dashdash_index(tokens) {
        Some(index) => &tokens[..index],
        None => tokens,
    }
}

pub fn injection_point(tokens: &[Token<'_>], args_len: usize) -> usize {
    dashdash_index(tokens)
        .map(|index| tokens[index].source_index)
        .unwrap_or(args_len)
}

pub fn has_dashdash(tokens: &[Token<'_>]) -> bool {
    dashdash_index(tokens).is_some()
}

pub fn has_flag(tokens: &[Token<'_>], dialect: Dialect, name: &str) -> bool {
    tokens
        .iter()
        .any(|t| t.kind == TokenKind::Long && flag_name_matches(t.text, name, dialect))
}

pub fn has_double_dash_flag(tokens: &[Token<'_>], dialect: Dialect, name: &str) -> bool {
    tokens.iter().any(|t| is_double_dash_flag(t, dialect, name))
}

pub fn double_dash_flag_value<'a>(
    tokens: &[Token<'a>],
    dialect: Dialect,
    name: &str,
) -> Option<&'a str> {
    tokens
        .iter()
        .find(|t| is_double_dash_flag(t, dialect, name))
        .and_then(|t| t.value(tokens))
}

pub fn double_dash_flag_values<'a, 't>(
    tokens: &'t [Token<'a>],
    dialect: Dialect,
    name: &'t str,
) -> impl Iterator<Item = &'a str> + 't {
    tokens
        .iter()
        .filter(move |t| is_double_dash_flag(t, dialect, name))
        .filter_map(|t| t.value(tokens))
}

fn is_double_dash_flag(t: &Token<'_>, dialect: Dialect, name: &str) -> bool {
    t.kind == TokenKind::Long && t.double_dash && flag_name_matches(t.text, name, dialect)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Msbuild,
    Posix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attachment {
    AttachedOnly,
    AttachedOrSeparate { solo_only: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueSpec {
    pub attachment: Attachment,
    pub claims_dash_dash: bool,
}

impl ValueSpec {
    pub const fn value() -> Self {
        Self {
            attachment: Attachment::AttachedOrSeparate { solo_only: false },
            claims_dash_dash: false,
        }
    }

    pub const fn attached_only() -> Self {
        Self {
            attachment: Attachment::AttachedOnly,
            claims_dash_dash: false,
        }
    }

    pub const fn solo_only() -> Self {
        Self {
            attachment: Attachment::AttachedOrSeparate { solo_only: true },
            claims_dash_dash: false,
        }
    }

    pub const fn claiming_dash_dash(self) -> Self {
        Self {
            claims_dash_dash: true,
            ..self
        }
    }
}

pub fn tokenize<'a, T: AsRef<str>>(args: &'a [T]) -> Vec<Token<'a>> {
    tokenize_scan(args, Dialect::Posix, &|_, _| None)
}

pub fn tokenize_grammar<'a, T: AsRef<str>>(
    args: &'a [T],
    takes_value: &dyn Fn(TokenKind, &str) -> Option<ValueSpec>,
    dialect: Dialect,
) -> Vec<Token<'a>> {
    tokenize_scan(args, dialect, takes_value)
}

struct Scanner<'a, 'p, T> {
    tokens: Vec<Token<'a>>,
    args: &'a [T],
    i: usize,
    dialect: Dialect,
    emitted_dash_dash: bool,
    takes_value: &'p dyn Fn(TokenKind, &str) -> Option<ValueSpec>,
}

impl<'a, 'p, T: AsRef<str>> Scanner<'a, 'p, T> {
    fn push_atomic_flag(&mut self, rest: &'a str, prefix: FlagPrefix) {
        let (name, attached) = split_attached(rest, self.dialect);
        let flag_index = self.tokens.len();
        let source_index = self.i;
        self.tokens.push(Token {
            attached,
            ..token(TokenKind::Long, name, source_index, prefix)
        });
        self.i += 1;

        if attached.is_none() && prefix != FlagPrefix::Slash {
            // `solo_only` cannot apply here: a Long flag is always the whole argument.
            if let Some(spec) = (self.takes_value)(TokenKind::Long, name) {
                if spec.attachment != Attachment::AttachedOnly
                    && self.link_next_value(flag_index, self.i, spec)
                {
                    self.i += 1;
                }
            }
        }
    }

    fn link_next_value(&mut self, flag_index: usize, value_index: usize, spec: ValueSpec) -> bool {
        let Some(next) = self.args.get(value_index) else {
            return false;
        };
        if next.as_ref() == "--" && !self.emitted_dash_dash && !spec.claims_dash_dash {
            return false;
        }
        let token_index = self.tokens.len();
        self.tokens.push(Token {
            linked: Some(flag_index),
            ..positional(next.as_ref(), value_index)
        });
        self.tokens[flag_index].linked = Some(token_index);
        true
    }
}

fn tokenize_scan<'a, T: AsRef<str>>(
    args: &'a [T],
    dialect: Dialect,
    takes_value: &dyn Fn(TokenKind, &str) -> Option<ValueSpec>,
) -> Vec<Token<'a>> {
    let mut scanner = Scanner {
        tokens: Vec::with_capacity(args.len()),
        args,
        i: 0,
        dialect,
        emitted_dash_dash: false,
        takes_value,
    };

    while scanner.i < scanner.args.len() {
        let arg = scanner.args[scanner.i].as_ref();

        // Posix stops classifying at `--`; Msbuild's `--` is a forwarding boundary, so it keeps
        // classifying flags past it (see TokenKind::DashDash).
        if scanner.emitted_dash_dash && scanner.dialect == Dialect::Posix {
            scanner.tokens.push(positional(arg, scanner.i));
            scanner.i += 1;
            continue;
        }

        if arg == "--" {
            if scanner.emitted_dash_dash {
                // A second (or later) literal "--" is never itself the boundary — it's just
                // ordinary text at this point, in both dialects.
                scanner.tokens.push(positional(arg, scanner.i));
            } else {
                scanner
                    .tokens
                    .push(token(TokenKind::DashDash, "", scanner.i, FlagPrefix::Dash));
                scanner.emitted_dash_dash = true;
            }
            scanner.i += 1;
            continue;
        }

        if let Some(rest) = arg.strip_prefix("--") {
            scanner.push_atomic_flag(rest, FlagPrefix::DashDash);
            continue;
        }

        if scanner.dialect == Dialect::Msbuild {
            if let Some(rest) = arg.strip_prefix('/') {
                // A real MSBuild switch name never contains another '/' -- without this guard,
                // an absolute Unix path would misclassify as a Long flag (e.g. "tmp/results").
                // KNOWN LIMITATION: a single-segment path (`/app`) is indistinguishable from a
                // genuine switch by structure alone; this pure function has no I/O to resolve it
                // the way real MSBuild does (a filesystem check), but the impact is narrow --
                // only the loose flag lookup ([`has_flag`]) is affected.
                let name_part = rest.split(['=', ':']).next().unwrap_or(rest);
                if !rest.is_empty() && !name_part.contains('/') {
                    scanner.push_atomic_flag(rest, FlagPrefix::Slash);
                    continue;
                }
            }
            if arg.len() > 1 && arg.starts_with('-') {
                scanner.push_atomic_flag(&arg[1..], FlagPrefix::Dash);
                continue;
            }
        } else if arg.len() > 1 && arg.starts_with('-') {
            let cluster = &arg[1..];

            if is_digit_run(cluster) {
                scanner.tokens.push(token(
                    TokenKind::Short,
                    cluster,
                    scanner.i,
                    FlagPrefix::Dash,
                ));
                scanner.i += 1;
                continue;
            }

            let mut consumed_next = false;
            let source_index = scanner.i;

            for (offset, ch) in cluster.char_indices() {
                let char_len = ch.len_utf8();
                let char_text = &cluster[offset..offset + char_len];
                let flag_index = scanner.tokens.len();
                scanner.tokens.push(token(
                    TokenKind::Short,
                    char_text,
                    source_index,
                    FlagPrefix::Dash,
                ));

                if let Some(spec) = (scanner.takes_value)(TokenKind::Short, char_text) {
                    let remainder = &cluster[offset + char_len..];
                    if !remainder.is_empty() {
                        scanner.tokens[flag_index].attached = Some(remainder);
                    } else {
                        // is_solo: offset == 0 with an empty remainder means this char is the
                        // *entire* cluster (the arg was e.g. just "-n"); a later offset, or any
                        // remainder, means it's genuinely clustered with something else.
                        let is_solo = offset == 0;
                        let takes_separate = match spec.attachment {
                            Attachment::AttachedOnly => false,
                            Attachment::AttachedOrSeparate { solo_only } => !solo_only || is_solo,
                        };
                        if takes_separate {
                            consumed_next =
                                scanner.link_next_value(flag_index, source_index + 1, spec);
                        }
                    }
                    break;
                }
            }

            scanner.i += if consumed_next { 2 } else { 1 };
            continue;
        }

        scanner.tokens.push(positional(arg, scanner.i));
        scanner.i += 1;
    }

    scanner.tokens
}

fn split_attached(s: &str, dialect: Dialect) -> (&str, Option<&str>) {
    let sep_pos = match dialect {
        Dialect::Posix => s.find('='),
        Dialect::Msbuild => s.find(['=', ':']),
    };
    match sep_pos {
        Some(pos) => (&s[..pos], Some(&s[pos + 1..])),
        None => (s, None),
    }
}

fn token(kind: TokenKind, text: &str, source_index: usize, prefix: FlagPrefix) -> Token<'_> {
    Token {
        kind,
        text,
        attached: None,
        linked: None,
        source_index,
        double_dash: prefix == FlagPrefix::DashDash,
        slash: prefix == FlagPrefix::Slash,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FlagPrefix {
    DashDash,
    Dash,
    Slash,
}

fn positional(text: &str, source_index: usize) -> Token<'_> {
    token(TokenKind::Positional, text, source_index, FlagPrefix::Dash)
}
