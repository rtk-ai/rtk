//! Shared tokenizer for re-classifying an already-`--`-restored passthrough args slice
//! (see [`crate::core::args_utils::restore_double_dash`]) into flags, their values, and
//! positionals, matching the GNU/POSIX-ish conventions used by git, cargo, rg, and friends.
//! Callers keep their own list of which flags take a value (inherently per-tool) and pass it in
//! as a predicate instead of reimplementing the token-walking around it.
//!
//! Not merged with `restore_double_dash`: `Token<'a>` borrows straight from `args`, so
//! tokenizing an owned `Vec<String>` built *inside* this module would tie every `Token` to a
//! value dropped when the function returns.

/// What kind of unit a [`Token`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// The literal `--` separator. Emitted exactly once, for the first `--` encountered. What it
    /// does to the arguments after it is the dialect's [`DashDashRole`] axis.
    DashDash,
    /// `--name` (see `Token::text` for the name, without the leading `--`).
    Long,
    /// A positional/value token — either free-standing or consumed by a preceding `Long`/`Short`
    /// as its separate-token value (see `Token::linked`).
    Positional,
    /// One character of a `-x` / `-xyz` short-option cluster (see `Token::text`, without the
    /// leading `-`). A run of only digits (`-20`) is a widely-used shorthand for a numeric
    /// value in its own right (git log/head/tail's `-N` count) rather than a cluster of
    /// per-digit boolean flags, so it is kept as one `Short` token with the whole digit run as
    /// `text`, never decomposed.
    Short,
}

/// One classified unit of an args slice, as produced by [`tokenize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: TokenKind,
    /// Flag name without leading dash(es) for `Long`/`Short`; raw text for `Positional`; empty
    /// for `DashDash`.
    pub text: &'a str,
    /// Value attached directly to this token: `--flag=value`, or the trailing remainder of a
    /// short cluster (`-A3` → `Short` "A" with `attached: Some("3")`).
    pub attached: Option<&'a str>,
    /// For `Long`/`Short`: index into the returned `Vec` of the `Positional` token consumed as
    /// this flag's separate-token value (only set when `takes_value` returned `true` and there
    /// was no attached value). For a consumed `Positional`: index of the flag token that owns
    /// it. `None` for a free-standing positional, an unconsumed flag, or `DashDash`.
    pub linked: Option<usize>,
    /// Index into the original `args` slice this token was produced from. Every `Short` token
    /// from the same `-xyz` cluster shares one `source_index` (they came from one arg); a
    /// consumed separate-token value always has its own, since it's a distinct arg. Lets a
    /// caller that needs to rebuild exact per-arg boundaries (e.g. whether `-r`/`-n` were typed
    /// as one cluster or two separate flags) do so without re-scanning `args` itself.
    pub source_index: usize,
    /// True if a `Long` token was written with a literal `--` prefix, as opposed to `-flag` or
    /// `/flag` (which tokenize uniformly as `Long` under [`SingleDash::Atomic`], but are *not*
    /// uniformly valid dotnet CLI syntax — see [`has_flag`] vs [`has_double_dash_flag`]). Also
    /// true for `-flag` under [`SingleDash::AtomicAliasingLong`], where the two spellings are
    /// one flag. Always `false` for `Short`/`Positional`/`DashDash`.
    pub double_dash: bool,
    /// True for the `/flag` spelling (dialects with `slash_flags`), which is MSBuild's own switch
    /// syntax rather than dotnet's CLI syntax -- `/l:` is MSBuild's logger-assembly switch, not
    /// dotnet's `-l`/`--logger`. Always `false` otherwise.
    pub slash: bool,
}

impl<'a> Token<'a> {
    /// This token's value, whether attached (`--flag=value`, `-fvalue`) or consumed as a
    /// separate token (`--flag value`, `-f value`). `None` for a boolean flag, an unrecognized
    /// flag, or a non-flag token. `tokens` must be the same slice `self` came from.
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

    /// True for a genuine free-standing positional: `Positional` kind, not itself consumed as
    /// some preceding flag's separate-token value (`Token::linked`).
    pub fn is_free_positional(&self) -> bool {
        self.kind == TokenKind::Positional && self.linked.is_none()
    }
}

/// True if `text` is a non-empty run of ASCII digits, e.g. a `Short` token's text for `-20`
/// (git/head/tail's `-N` count shorthand — see [`TokenKind::Short`]). Exposed so callers that
/// need to tell "this Short token is a digit-run flag" from "this Short token is a single
/// boolean-flag letter" don't re-derive the same predicate the tokenizer itself already used to
/// decide clustering.
pub fn is_digit_run(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// True if `text` (a `Long` token's name) matches `name` under `dialect`'s [`NameCase`] axis.
fn flag_name_matches(text: &str, name: &str, dialect: Dialect) -> bool {
    match dialect.name_case {
        NameCase::Folded => text.eq_ignore_ascii_case(name),
        NameCase::Sensitive => text == name,
    }
}

/// Index into `tokens` of the `--` boundary, if one was emitted (see [`TokenKind::DashDash`]).
/// `tokens[i].source_index` recovers its position in the original args slice, for a caller that
/// needs to insert/compare against raw arg indices rather than the token vec's own index.
pub fn dashdash_index(tokens: &[Token<'_>]) -> Option<usize> {
    tokens.iter().position(|t| t.kind == TokenKind::DashDash)
}

/// The tokens before the `--` boundary, or all of them when there is none. Every
/// [`DashDashRole`] except `EndsOptions` keeps classifying past `--`, so a lookup for the flags
/// the tool reads *globally* has to slice here first -- otherwise it reads what the user
/// forwarded to the test runner, or typed as a gradle task option, as if the tool had seen it
/// as a global flag.
pub fn before_dashdash<'t, 'a>(tokens: &'t [Token<'a>]) -> &'t [Token<'a>] {
    match dashdash_index(tokens) {
        Some(index) => &tokens[..index],
        None => tokens,
    }
}

/// Where RTK's own flags have to be spliced into `args`: before the user's `--`, since no
/// [`DashDashRole`] lets the tool read a *global* option past the boundary -- it is a pathspec,
/// an argument forwarded to another program, or a task's own option. `args_len` when there is
/// no boundary.
///
/// Takes the **whole** token vec, never a slice: `dashdash_index` on a slice whose `--` was
/// cut off reports "no boundary" and this returns `args_len`, which would splice RTK's flags
/// past the boundary -- the exact thing it exists to prevent.
pub fn injection_point(tokens: &[Token<'_>], args_len: usize) -> usize {
    dashdash_index(tokens)
        .map(|index| tokens[index].source_index)
        .unwrap_or(args_len)
}

/// True if `tokens` has a `--` boundary at all.
pub fn has_dashdash(tokens: &[Token<'_>]) -> bool {
    dashdash_index(tokens).is_some()
}

/// True if `name` (matched per `dialect`) appears as a `Long` token anywhere in `tokens`. Under
/// `Dialect::Msbuild`, this matches `-flag`/`--flag`/`/flag` uniformly — correct only for
/// legacy MSBuild.exe passthrough switches (`nologo`, `bl`, `v`); see [`has_double_dash_flag`]
/// for anything else.
pub fn has_flag(tokens: &[Token<'_>], dialect: Dialect, name: &str) -> bool {
    tokens
        .iter()
        .any(|t| t.kind == TokenKind::Long && flag_name_matches(t.text, name, dialect))
}

/// Like [`double_dash_flag_value`], but only reports presence, not the value; only matches a
/// token written with a literal `--` prefix (`Token::double_dash`), not `-flag`/`/flag` under
/// [`Dialect::Msbuild`]. Under that dialect, a single-dash or slash spelling of a modern
/// System.CommandLine option (e.g. dotnet's `--logger`) doesn't just get rejected — it gets
/// misparsed as an unrelated legacy MSBuild switch — so use this (not [`has_flag`]) for any
/// option that isn't a genuine legacy MSBuild.exe passthrough switch.
pub fn has_double_dash_flag(tokens: &[Token<'_>], dialect: Dialect, name: &str) -> bool {
    tokens.iter().any(|t| is_double_dash_flag(t, dialect, name))
}

/// This flag's value, if `name` (matched per `dialect`) appears as a `Long` token written with
/// a literal `--` prefix (`Token::double_dash`) anywhere in `tokens`. See
/// [`has_double_dash_flag`] for why this distinction is load-bearing under `Dialect::Msbuild`.
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

/// Every value for `name` (matched per `dialect`), in order, for a `--`-prefixed flag that can
/// legitimately repeat (e.g. dotnet test's `--logger`, usable more than once) — unlike
/// [`double_dash_flag_value`], which only reports the first match. Occurrences with no value are
/// skipped rather than yielding `None`.
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

/// Shared match predicate behind [`has_double_dash_flag`]/[`double_dash_flag_value`]/
/// [`double_dash_flag_values`]: a `Long` token written with a literal `--` prefix, matching
/// `name` per `dialect`'s naming rules.
fn is_double_dash_flag(t: &Token<'_>, dialect: Dialect, name: &str) -> bool {
    t.kind == TokenKind::Long && t.double_dash && flag_name_matches(t.text, name, dialect)
}

/// What a single-dash multi-character argument (`-abc`) means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleDash {
    /// A cluster of one-character flags, scanned char by char (`-rn` is `-r -n`). A run of only
    /// digits is exempt — see [`TokenKind::Short`]. git, cargo, rg, golangci-lint, gradle.
    Cluster,
    /// One atomic flag name, distinct from its `--` spelling. dotnet's legacy MSBuild switches,
    /// maven's `-pl`/`-gs`/`-amd`. Tagged `TokenKind::Long` despite the single dash — these are
    /// "short options" only in their own tool's vocabulary — so `takes_value` is asked about
    /// them as `Long`, and `Short` is never produced under this value or the next.
    Atomic,
    /// One atomic flag name, and the same flag as its `--` spelling — the token carries
    /// `double_dash: true` whichever prefix was typed. Go's `flag` package.
    AtomicAliasingLong,
}

/// Which separator attaches a value to a flag name (`--flag=v`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attach {
    /// `=` only.
    Equals,
    /// `=` or `:`, whichever comes first — `--logger:trx` and `--logger=trx` are both valid
    /// dotnet CLI syntax.
    EqualsOrColon,
}

/// What a literal `--` does to the arguments after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashDashRole {
    /// Ends option parsing: everything after it is a `Positional`. git, cargo, rg, maven.
    EndsOptions,
    /// Forwards the tail to another program. Classification continues past it, so a lookup for
    /// the tool's *own* flags has to slice with [`before_dashdash`] first. dotnet.
    Forwards,
    /// Ends the tool's *global* option region only; tasks past it, and their own options, keep
    /// the same grammar. gradle.
    ///
    /// Tokenizes identically to `Forwards` — both keep classifying — and differs only in what
    /// the tail belongs to, which decides what a caller may read from it: a `Forwards` tail is
    /// another program's, so reading a flag out of it is always wrong, while this tail is still
    /// the tool's own, just not its global region.
    #[allow(dead_code)] // No in-tree caller yet: gradlew still parses its args by hand.
    EndsGlobalOptions,
}

/// Whether flag names are matched exactly or ASCII case-insensitively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameCase {
    /// Exact. git, cargo, rg, gradle, go, and maven (which distinguishes `-b` from `-B`).
    Sensitive,
    /// ASCII case-insensitive — MSBuild-ecosystem tools fold broadly (`/nologo` == `/NoLogo`).
    Folded,
}

/// One tool's flag grammar, as independent axes.
///
/// A preset ([`Dialect::Posix`] and friends) names a grammar *family* several tools can share —
/// a parser library or a real convention — so the name answers "can my tool reuse this?". A
/// single tool's bespoke parser composes its axes at the call site instead; that is what keeps
/// this list from growing one preset per tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dialect {
    pub single_dash: SingleDash,
    pub attach: Attach,
    pub dash_dash: DashDashRole,
    pub name_case: NameCase,
    /// Whether `/flag` is a switch spelling (MSBuild's own) rather than a path.
    pub slash_flags: bool,
}

// Presets read as grammar names at their call sites, not as constants.
#[allow(non_upper_case_globals)]
impl Dialect {
    /// GNU/POSIX-ish: git, cargo, rg, golangci-lint.
    pub const Posix: Self = Self {
        single_dash: SingleDash::Cluster,
        attach: Attach::Equals,
        dash_dash: DashDashRole::EndsOptions,
        name_case: NameCase::Sensitive,
        slash_flags: false,
    };

    /// MSBuild/dotnet-CLI-ish. `-flag`, `--flag` and `/flag` are all one atomic flag name, so
    /// every flag is tagged `TokenKind::Long` and `TokenKind::Short` is never produced here.
    pub const Msbuild: Self = Self {
        single_dash: SingleDash::Atomic,
        attach: Attach::EqualsOrColon,
        dash_dash: DashDashRole::Forwards,
        name_case: NameCase::Folded,
        slash_flags: true,
    };

    /// Apache commons-cli: POSIX, except that a short option is a whole multi-character word
    /// (`-pl`, `-am`, `-gs`, `-emp`), so `-abc` never clusters. Maven is the first consumer.
    ///
    /// Does **not** model commons-cli's Java-property options: `-DskipTests=true` tokenizes as
    /// the flag `DskipTests`, not `D` with the value `skipTests=true`. A caller that reads `-D`
    /// values has to split the name itself; one that only needs "this is not a positional" does
    /// not care.
    #[allow(dead_code)] // No in-tree caller yet: mvn still parses its args by hand.
    pub const CommonsCli: Self = Self {
        single_dash: SingleDash::Atomic,
        ..Self::Posix
    };

    /// Go's `flag` package: atomic single-dash options, with `-flag` and `--flag` equivalent.
    #[allow(dead_code)] // No in-tree caller yet: go still parses its args by hand.
    pub const GoFlag: Self = Self {
        single_dash: SingleDash::AtomicAliasingLong,
        ..Self::Posix
    };

    /// Whether arguments are still classified as flags after the `--` boundary.
    const fn classifies_past_dash_dash(self) -> bool {
        !matches!(self.dash_dash, DashDashRole::EndsOptions)
    }

    /// Whether a single-dash multi-character argument is one atomic flag name.
    const fn single_dash_is_atomic(self) -> bool {
        !matches!(self.single_dash, SingleDash::Cluster)
    }
}

/// How a flag's value may be written. The tokenizer branches on this; a caller states it once,
/// per flag, in its `takes_value` predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attachment {
    /// `--flag=v` only. The next argument is never this flag's value -- git's `-M`/`-U`/`-C`/
    /// `-B` take an optional attached number and nothing else.
    AttachedOnly,
    /// `--flag=v` or `--flag v`. `solo_only` restricts the separate-token form to a `Short`
    /// flag that is the whole argument (`git log -n 2`), excluding it when clustered
    /// (`git log -pn 2`, which real git rejects). It has no meaning for a `Long` flag, which is
    /// always the whole argument.
    AttachedOrSeparate { solo_only: bool },
}

/// What a caller's `takes_value` predicate says about one flag's value. Returned inside an
/// `Option`, so "takes no value" is `None` and there is one table per tool rather than one per
/// question asked about the same flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueSpec {
    pub attachment: Attachment,
    /// Whether a not-yet-seen literal `--` may be this flag's value rather than the
    /// end-of-options boundary. A per-tool split, confirmed against each: grep and rg let any
    /// value-taking flag swallow it, git and cargo reject it whichever flag is asking.
    pub claims_dash_dash: bool,
}

impl ValueSpec {
    /// `--flag=v` or `--flag v`, and a literal `--` is the boundary rather than a value. The
    /// common case.
    pub const fn value() -> Self {
        Self {
            attachment: Attachment::AttachedOrSeparate { solo_only: false },
            claims_dash_dash: false,
        }
    }

    /// `--flag=v` only; the next argument stays a separate argument.
    pub const fn attached_only() -> Self {
        Self {
            attachment: Attachment::AttachedOnly,
            claims_dash_dash: false,
        }
    }

    /// Like [`ValueSpec::value`], but a `Short` flag takes a separate value only when it is the
    /// whole argument.
    pub const fn solo_only() -> Self {
        Self {
            attachment: Attachment::AttachedOrSeparate { solo_only: true },
            claims_dash_dash: false,
        }
    }

    /// Lets a literal `--` be this flag's value instead of the end-of-options boundary.
    pub const fn claiming_dash_dash(self) -> Self {
        Self {
            claims_dash_dash: true,
            ..self
        }
    }
}

/// Tokenizes `args` structurally, for a caller asking only which arguments are flags, which are
/// positionals, and where `--` is -- subcommand detection, boundary splitting.
///
/// **No flag takes a value here.** Use [`tokenize_grammar`] for anything that reads a flag's
/// value or counts free positionals: without a grammar, `--grep -p` leaves `-p` looking like a
/// flag of its own and `--filter X` leaves `X` looking like a positional path.
pub fn tokenize<'a, T: AsRef<str>>(args: &'a [T]) -> Vec<Token<'a>> {
    tokenize_scan(args, Dialect::Posix, &|_, _| None)
}

/// Tokenizes `args` under one tool's grammar. `takes_value(kind, name)` returns `Some(spec)` for
/// a flag that takes a value and `None` for one that does not; never panics, a value-taking flag
/// with nothing left to consume simply gets `attached: None, linked: None`.
///
/// Generic over `T: AsRef<str>`, not `OsStr`/`OsString`: `OsStr` exposes almost no
/// string-manipulation API (no `strip_prefix`, `split_once`), so tokenizing it would mean
/// re-deriving that machinery byte-by-byte the way `clap_lex` does internally.
pub fn tokenize_grammar<'a, T: AsRef<str>>(
    args: &'a [T],
    takes_value: &dyn Fn(TokenKind, &str) -> Option<ValueSpec>,
    dialect: Dialect,
) -> Vec<Token<'a>> {
    tokenize_scan(args, dialect, takes_value)
}

/// Groups the mutable scan state threaded through [`tokenize_scan`]'s helper methods
/// (`push_atomic_flag`/`link_next_value`), so a future piece of shared state means adding one
/// field instead of a parameter to every helper and every call site.
struct Scanner<'a, 'p, T> {
    tokens: Vec<Token<'a>>,
    args: &'a [T],
    i: usize,
    dialect: Dialect,
    emitted_dash_dash: bool,
    takes_value: &'p dyn Fn(TokenKind, &str) -> Option<ValueSpec>,
}

impl<'a, 'p, T: AsRef<str>> Scanner<'a, 'p, T> {
    /// Pushes one atomic (non-clustering) flag token — used for `--flag` in every dialect, and
    /// for `-flag`/`/flag` where the dialect says so. `rest` is the flag text with its prefix
    /// already stripped; `prefix` records which one it was. Only the `/flag` spelling is barred
    /// from consuming a separate value: an MSBuild switch attaches its value with `:`
    /// (`/bl:x.binlog`), so `/r` (MSBuild's `restore`) must not swallow the token after it the
    /// way dotnet's own `-r <rid>` does.
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

    /// If `self.args[value_index]` exists and isn't the still-unseen boundary `--`, pushes it as
    /// a `Positional` token linked to `flag_index` (and links `flag_index` back to it). Returns
    /// whether a value was consumed; does *not* itself advance `self.i`. The still-unseen `--`
    /// is swallowed as a value only when the flag's [`ValueSpec::claims_dash_dash`] says so.
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

/// Core implementation shared by both public entry points.
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

        if scanner.emitted_dash_dash && !scanner.dialect.classifies_past_dash_dash() {
            scanner.tokens.push(positional(arg, scanner.i));
            scanner.i += 1;
            continue;
        }

        if arg == "--" {
            if scanner.emitted_dash_dash {
                // A second (or later) literal "--" is never itself the boundary — it's just
                // ordinary text at this point, under every dialect.
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

        if scanner.dialect.slash_flags {
            if let Some(rest) = arg.strip_prefix('/') {
                // A real MSBuild switch name never contains another '/' -- without this guard,
                // an absolute Unix path would misclassify as a Long flag (e.g. "tmp/results").
                // KNOWN LIMITATION: a single-segment path (`/app`) is indistinguishable from a
                // genuine switch by structure alone; this pure function has no I/O to resolve it
                // the way real MSBuild does (a filesystem check), but the impact is narrow --
                // only the loose flag lookup ([`has_flag`]) is affected.
                let (name_part, _) = split_attached(rest, scanner.dialect);
                if !rest.is_empty() && !name_part.contains('/') {
                    scanner.push_atomic_flag(rest, FlagPrefix::Slash);
                    continue;
                }
            }
        }

        if arg.len() > 1 && arg.starts_with('-') {
            if scanner.dialect.single_dash_is_atomic() {
                // Under `AtomicAliasingLong` the single-dash spelling *is* the `--` flag, so it
                // has to answer to `has_double_dash_flag` as well.
                let prefix = match scanner.dialect.single_dash {
                    SingleDash::AtomicAliasingLong => FlagPrefix::DashDash,
                    SingleDash::Cluster | SingleDash::Atomic => FlagPrefix::Dash,
                };
                scanner.push_atomic_flag(&arg[1..], prefix);
                continue;
            }

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

/// Splits `s` into `(name, attached_value)` on the first separator allowed by the dialect's
/// [`Attach`] axis.
fn split_attached(s: &str, dialect: Dialect) -> (&str, Option<&str>) {
    let sep_pos = match dialect.attach {
        Attach::Equals => s.find('='),
        Attach::EqualsOrColon => s.find(['=', ':']),
    };
    match sep_pos {
        Some(pos) => (&s[..pos], Some(&s[pos + 1..])),
        None => (s, None),
    }
}

/// Base constructor for a freshly-scanned token: `attached`/`linked` default to `None`. Every
/// token-construction site builds on this via struct-update syntax instead of a full literal.
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

/// How a flag was spelled. All three can tokenize as `Long`, but they are not interchangeable:
/// MSBuild's `/flag` attaches its value with `:` and never consumes the next argument, while
/// dotnet's own `-flag`/`--flag` do.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FlagPrefix {
    DashDash,
    Dash,
    Slash,
}

fn positional(text: &str, source_index: usize) -> Token<'_> {
    token(TokenKind::Positional, text, source_index, FlagPrefix::Dash)
}

#[cfg(test)]
mod frozen;

#[cfg(test)]
mod differential;

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn attached_only_bars_a_separate_value_in_both_kinds() {
        // Short: git's `-M`/`-U` take an optional attached number and never the next token.
        let args = owned(&["-M", "50", "f.txt"]);
        let tokens = tokenize_grammar(
            &args,
            &|_, name| (name == "M").then(ValueSpec::attached_only),
            Dialect::Posix,
        );
        assert_eq!(tokens[0].text, "M");
        assert_eq!(tokens[0].linked, None, "-M must not claim the 50");
        assert_eq!(tokens[0].value(&tokens), None);
        assert!(tokens[1].is_free_positional());

        // Long: previously unexpressible -- the old API could only bar a Short flag.
        let args = owned(&["--min-parents", "2"]);
        let tokens = tokenize_grammar(
            &args,
            &|_, name| (name == "min-parents").then(ValueSpec::attached_only),
            Dialect::Posix,
        );
        assert_eq!(tokens[0].linked, None);
        assert!(tokens[1].is_free_positional());

        // The attached spelling still works for both.
        let args = owned(&["-M50", "--min-parents=2"]);
        let tokens = tokenize_grammar(
            &args,
            &|_, name| matches!(name, "M" | "min-parents").then(ValueSpec::attached_only),
            Dialect::Posix,
        );
        assert_eq!(tokens[0].value(&tokens), Some("50"));
        assert_eq!(tokens[1].value(&tokens), Some("2"));
    }

    #[test]
    fn solo_only_restricts_a_short_flag_to_the_whole_argument() {
        let takes = |_: TokenKind, name: &str| (name == "n").then(ValueSpec::solo_only);

        let solo = owned(&["-n", "2"]);
        let tokens = tokenize_grammar(&solo, &takes, Dialect::Posix);
        assert_eq!(tokens[0].value(&tokens), Some("2"));

        // Clustered: real git rejects `git log -pn 2`, so the 2 stays a positional.
        let clustered = owned(&["-pn", "2"]);
        let tokens = tokenize_grammar(&clustered, &takes, Dialect::Posix);
        let n = tokens.iter().find(|t| t.text == "n").expect("n token");
        assert_eq!(n.value(&tokens), None);
        assert!(tokens.last().expect("positional").is_free_positional());
    }

    #[test]
    fn claiming_dash_dash_is_per_flag_not_global() {
        let args = owned(&["-e", "--", "f.txt"]);

        // Default: `--` is the boundary, so -e gets no value and f.txt is past it.
        let tokens = tokenize_grammar(
            &args,
            &|_, name| (name == "e").then(ValueSpec::value),
            Dialect::Posix,
        );
        assert_eq!(tokens[0].value(&tokens), None);
        assert_eq!(tokens[1].kind, TokenKind::DashDash);

        // grep/rg: -e claims the literal `--` as its pattern.
        let tokens = tokenize_grammar(
            &args,
            &|_, name| (name == "e").then(|| ValueSpec::value().claiming_dash_dash()),
            Dialect::Posix,
        );
        assert_eq!(tokens[0].value(&tokens), Some("--"));
        assert!(tokens.iter().all(|t| t.kind != TokenKind::DashDash));
    }

    #[test]
    fn empty_args_yield_no_tokens() {
        let args = owned(&[]);
        assert!(tokenize(&args).is_empty());
    }

    #[test]
    fn dash_p_after_double_dash_is_positional_not_a_flag() {
        // Regression: `git log -- -p` must not misread the pathspec "-p" as the patch flag
        //.
        let args = owned(&["--", "-p"]);
        let tokens = tokenize(&args);

        assert_eq!(tokens[0].kind, TokenKind::DashDash);
        assert_eq!(tokens[1].kind, TokenKind::Positional);
        assert_eq!(tokens[1].text, "-p");
    }

    #[test]
    fn second_double_dash_is_positional_text_not_another_separator() {
        let args = owned(&["--", "--", "file"]);
        let tokens = tokenize(&args);

        assert_eq!(tokens[0].kind, TokenKind::DashDash);
        assert_eq!(tokens[1].kind, TokenKind::Positional);
        assert_eq!(tokens[1].text, "--");
        assert_eq!(tokens[2].kind, TokenKind::Positional);
        assert_eq!(tokens[2].text, "file");
    }

    #[test]
    fn value_taking_long_flag_consumes_and_links_next_token() {
        // Regression: `--grep -p` must treat "-p" as --grep's value, not the patch flag
        //.
        let args = owned(&["--grep", "-p"]);
        let tokens = tokenize_grammar(
            &args,
            &|kind, name| (kind == TokenKind::Long && name == "grep").then(ValueSpec::value),
            Dialect::Posix,
        );

        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].kind, TokenKind::Long);
        assert_eq!(tokens[0].text, "grep");
        assert_eq!(tokens[0].linked, Some(1));
        assert_eq!(tokens[1].kind, TokenKind::Positional);
        assert_eq!(tokens[1].text, "-p");
        assert_eq!(tokens[1].linked, Some(0));
    }

    #[test]
    fn value_taking_long_flag_never_swallows_the_unseen_boundary_dashdash() {
        // Regression: verified against real git that a value-taking flag can never claim the
        // still-unseen boundary "--" as its value -- `git log --grep -- pattern` fails with
        // "Option '--grep' requires a value" rather than treating "--" as the search pattern.
        let args = owned(&["--grep", "--", "pattern"]);
        let tokens = tokenize_grammar(
            &args,
            &|kind, name| (kind == TokenKind::Long && name == "grep").then(ValueSpec::value),
            Dialect::Posix,
        );

        assert_eq!(tokens[0].kind, TokenKind::Long);
        assert_eq!(tokens[0].text, "grep");
        assert_eq!(
            tokens[0].linked, None,
            "--grep must not claim -- as its value"
        );
        assert_eq!(tokens[1].kind, TokenKind::DashDash);
        assert_eq!(tokens[2].kind, TokenKind::Positional);
        assert_eq!(tokens[2].text, "pattern");
    }

    #[test]
    fn value_taking_short_flag_never_swallows_the_unseen_boundary_dashdash() {
        let args = owned(&["-A", "--", "pattern"]);
        let takes = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Short && name == "A").then(ValueSpec::value)
        };
        let tokens = tokenize_grammar(&args, &takes, Dialect::Posix);

        assert_eq!(tokens[0].kind, TokenKind::Short);
        assert_eq!(tokens[0].text, "A");
        assert_eq!(tokens[0].linked, None, "-A must not claim -- as its value");
        assert_eq!(tokens[1].kind, TokenKind::DashDash);
        assert_eq!(tokens[2].text, "pattern");
    }

    #[test]
    fn value_taking_flag_may_consume_a_dashdash_after_the_boundary_was_already_emitted() {
        // Once past the boundary, a further "--" is ordinary text and fair game as a value --
        // verified against real git: `git log -- -- pattern` succeeds (both are pathspecs).
        // Msbuild is the dialect that keeps classifying flags after the boundary, so it's the
        // one where a flag could even encounter a second "--" as its candidate value.
        let args = owned(&["--", "--logger", "--"]);
        let takes = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Long && name == "logger").then(ValueSpec::value)
        };
        let tokens = tokenize_grammar(&args, &takes, Dialect::Msbuild);

        assert_eq!(tokens[0].kind, TokenKind::DashDash);
        assert_eq!(tokens[1].kind, TokenKind::Long);
        assert_eq!(tokens[1].text, "logger");
        assert_eq!(
            tokens[1].linked,
            Some(2),
            "-- after the boundary was already emitted is just text, and --logger may claim it"
        );
        assert_eq!(tokens[2].kind, TokenKind::Positional);
        assert_eq!(tokens[2].text, "--");
    }

    #[test]
    fn attached_long_value_does_not_consult_predicate() {
        let args = owned(&["--grep=-p"]);
        // A predicate that always panics would fail this test if consulted; false is enough
        // to prove it wasn't needed either way, so assert the value came from the "=" form.
        let tokens = tokenize(&args);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].text, "grep");
        assert_eq!(tokens[0].attached, Some("-p"));
        assert_eq!(tokens[0].linked, None);
    }

    #[test]
    fn optional_value_long_flags_do_not_consume_next_token() {
        // Regression: -U / --unified / --expand-tabs / --max-parents only take an *attached*
        // value; a following bare token is not theirs.
        for flag in ["unified", "expand-tabs", "max-parents"] {
            let args = owned(&[&format!("--{flag}"), "-p"]);
            let tokens = tokenize(&args);

            assert_eq!(tokens[0].linked, None, "--{flag} should not link a value");
            // "-p" is still its own Short("p") token, just not linked to --{flag} as its value.
            assert_eq!(tokens[1].kind, TokenKind::Short);
            assert_eq!(tokens[1].text, "p");
            assert_eq!(
                tokens[1].linked, None,
                "-p after --{flag} must stay independent"
            );
        }
    }

    #[test]
    fn required_value_long_flags_do_consume_next_token() {
        // --diff-algorithm/--diff-filter take a required, separate-token value (rtk commit
        // 84169e2).
        for flag in ["diff-algorithm", "diff-filter"] {
            let args = owned(&[&format!("--{flag}"), "-p"]);
            let takes = |kind: TokenKind, name: &str| {
                (kind == TokenKind::Long && (name == "diff-algorithm" || name == "diff-filter"))
                    .then(ValueSpec::value)
            };
            let tokens = tokenize_grammar(&args, &takes, Dialect::Posix);

            assert_eq!(tokens[0].linked, Some(1), "--{flag} should link its value");
            assert_eq!(tokens[1].text, "-p");
        }
    }

    #[test]
    fn value_taking_flag_at_end_of_args_degrades_gracefully() {
        let args = owned(&["--grep"]);
        let tokens = tokenize_grammar(
            &args,
            &|kind, name| (kind == TokenKind::Long && name == "grep").then(ValueSpec::value),
            Dialect::Posix,
        );

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].attached, None);
        assert_eq!(tokens[0].linked, None);
    }

    #[test]
    fn short_cluster_of_booleans_yields_one_token_per_char() {
        let args = owned(&["-riI"]);
        let tokens = tokenize(&args);

        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].text, "r");
        assert_eq!(tokens[1].text, "i");
        assert_eq!(tokens[2].text, "I");
        assert!(tokens.iter().all(|t| t.kind == TokenKind::Short));
        // All three chars came from the one "-riI" arg.
        assert!(tokens.iter().all(|t| t.source_index == 0));
    }

    #[test]
    fn source_index_distinguishes_one_cluster_from_separate_flags() {
        // "-rn" (one arg, one cluster) vs "-r" "-n" (two separate args) classify
        // identically char-by-char, but a caller that needs to know whether they
        // were typed together can tell via source_index.
        let clustered = owned(&["-rn"]);
        let tokens = tokenize(&clustered);
        assert_eq!(tokens[0].source_index, tokens[1].source_index);

        let separate = owned(&["-r", "-n"]);
        let tokens = tokenize(&separate);
        assert_ne!(tokens[0].source_index, tokens[1].source_index);
    }

    #[test]
    fn short_cluster_value_flag_takes_attached_remainder() {
        let args = owned(&["-A3"]);
        let takes = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Short && name == "A").then(ValueSpec::value)
        };
        let tokens = tokenize_grammar(&args, &takes, Dialect::Posix);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].text, "A");
        assert_eq!(tokens[0].attached, Some("3"));
        assert_eq!(tokens[0].linked, None);
    }

    #[test]
    fn short_flag_without_attached_remainder_consumes_next_token() {
        let args = owned(&["-A", "3"]);
        let takes = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Short && name == "A").then(ValueSpec::value)
        };
        let tokens = tokenize_grammar(&args, &takes, Dialect::Posix);

        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].linked, Some(1));
        assert_eq!(tokens[1].text, "3");
        assert_eq!(tokens[1].linked, Some(0));
    }

    #[test]
    fn short_cluster_stops_consuming_chars_after_value_taking_one() {
        // "-rA3": r is boolean, A takes the attached "3", nothing after A is scanned.
        let args = owned(&["-rA3"]);
        let takes = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Short && name == "A").then(ValueSpec::value)
        };
        let tokens = tokenize_grammar(&args, &takes, Dialect::Posix);

        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].text, "r");
        assert_eq!(tokens[1].text, "A");
        assert_eq!(tokens[1].attached, Some("3"));
    }

    #[test]
    fn digit_run_short_flag_stays_one_token_not_a_cluster() {
        // git log/head/tail's "-20" limit shorthand must not decompose into Short('2'),
        // Short('0') — there's no such thing as boolean digit flags.
        let args = owned(&["-20"]);
        let tokens = tokenize(&args);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Short);
        assert_eq!(tokens[0].text, "20");
    }

    #[test]
    fn bare_single_dash_is_positional() {
        let args = owned(&["-"]);
        let tokens = tokenize(&args);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Positional);
        assert_eq!(tokens[0].text, "-");
    }

    #[test]
    fn plain_positionals_pass_through_unclassified() {
        let args = owned(&["main", "feature/auth"]);
        let tokens = tokenize(&args);

        assert_eq!(tokens.len(), 2);
        assert!(tokens.iter().all(|t| t.kind == TokenKind::Positional));
    }

    // --- Dialect::Msbuild ---

    #[test]
    fn msbuild_single_dash_flag_is_atomic_not_a_cluster() {
        // dotnet's "-nologo" is one flag name, not a POSIX cluster of n/o/l/o/g/o.
        let args = owned(&["-nologo"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Long);
        assert_eq!(tokens[0].text, "nologo");
    }

    #[test]
    fn value_is_none_on_a_consumed_positional_and_safe_on_a_slice() {
        let args = owned(&["--grep", "x", "file.rs"]);
        let takes_value = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Long && name == "grep").then(ValueSpec::value)
        };
        let tokens = tokenize_grammar(&args, &takes_value, Dialect::Posix);

        assert_eq!(tokens[0].value(&tokens), Some("x"));
        // The consumed token links back at its owner; that owner's name is not its value.
        assert_eq!(tokens[1].value(&tokens), None);

        // A caller holding a slice must not index out of the slice and panic.
        let slice = &tokens[..1];
        assert_eq!(slice[0].value(slice), None);
    }

    #[test]
    fn msbuild_slash_flag_never_consumes_a_separate_value() {
        // `/r` is MSBuild's boolean `restore`, not dotnet's `-r <rid>`: an MSBuild switch takes
        // its value attached with `:`, so `/r` must leave the next arg alone. Reading it as a
        // value hid a following `-bl:<file>` from dotnet's own binlog detection.
        let takes_value = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Long && name == "r").then(ValueSpec::value)
        };
        let slash = owned(&["/r", "-bl:my.binlog"]);
        let tokens = tokenize_grammar(&slash, &takes_value, Dialect::Msbuild);
        assert_eq!(tokens[0].text, "r");
        assert_eq!(tokens[0].linked, None);
        assert_eq!(tokens[1].kind, TokenKind::Long);
        assert_eq!(tokens[1].text, "bl");
        assert_eq!(tokens[1].attached, Some("my.binlog"));

        // The dash spelling is dotnet's own `-r <rid>`, which does consume the next token.
        let dash = owned(&["-r", "linux-x64"]);
        let tokens = tokenize_grammar(&dash, &takes_value, Dialect::Msbuild);
        assert_eq!(tokens[0].value(&tokens), Some("linux-x64"));
    }

    #[test]
    fn msbuild_slash_prefix_is_recognized_as_a_flag() {
        let args = owned(&["/nologo"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Long);
        assert_eq!(tokens[0].text, "nologo");
    }

    #[test]
    fn msbuild_slash_alone_is_positional() {
        let args = owned(&["/"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Positional);
        assert_eq!(tokens[0].text, "/");
    }

    #[test]
    fn msbuild_absolute_path_is_positional_not_a_flag() {
        // Real MSBuild never treats a multi-segment "/a/b" as a switch attempt.
        let takes = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Long && name == "nologo").then(ValueSpec::value)
        };
        let args = owned(&["/tmp/results"]);
        let tokens = tokenize_grammar(&args, &takes, Dialect::Msbuild);

        assert_eq!(tokens[0].kind, TokenKind::Positional);
        assert_eq!(tokens[0].text, "/tmp/results");
    }

    #[test]
    fn msbuild_single_segment_slash_flag_is_still_a_flag() {
        // A genuine single-segment MSBuild switch (no internal '/') must still classify as Long,
        // including when it carries an attached value whose own text contains '/'.
        let args = owned(&["/nologo", "/p:OutDir=/tmp/out"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);

        assert_eq!(tokens[0].kind, TokenKind::Long);
        assert_eq!(tokens[0].text, "nologo");
        assert_eq!(tokens[1].kind, TokenKind::Long);
        assert_eq!(tokens[1].text, "p");
        assert_eq!(tokens[1].attached, Some("OutDir=/tmp/out"));
    }

    #[test]
    fn msbuild_colon_and_equals_both_attach_a_value() {
        for arg in ["--logger:trx", "--logger=trx"] {
            let args = owned(&[arg]);
            let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);

            assert_eq!(tokens[0].text, "logger", "for {arg}");
            assert_eq!(tokens[0].attached, Some("trx"), "for {arg}");
        }
    }

    #[test]
    fn msbuild_separate_token_value_still_works() {
        let args = owned(&["--results-directory", "/tmp/out"]);
        let takes = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Long && name == "results-directory").then(ValueSpec::value)
        };
        let tokens = tokenize_grammar(&args, &takes, Dialect::Msbuild);

        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].linked, Some(1));
        assert_eq!(tokens[1].text, "/tmp/out");
    }

    #[test]
    fn msbuild_dashdash_is_a_forwarding_boundary_not_end_of_options() {
        // dotnet's `--` hands the rest to a different receiving parser (the VSTest/MTP test
        // host); unlike Posix, that doesn't stop classification -- flags after it (e.g.
        // --report-trx, forwarded to the test host) must still be recognized as flags.
        let args = owned(&["--", "-nologo"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);

        assert_eq!(tokens[0].kind, TokenKind::DashDash);
        assert_eq!(tokens[1].kind, TokenKind::Long);
        assert_eq!(tokens[1].text, "nologo");
    }

    #[test]
    fn msbuild_flag_after_dashdash_still_consumes_its_separate_value() {
        // Regression: `dotnet test <proj> -- --results-directory /tmp/out` -- the value must
        // still link to its flag even though it's past `--`, matching real forwarded-flag
        // semantics (unlike Posix, where nothing after `--` is ever a flag at all).
        let args = owned(&["--", "--results-directory", "/tmp/out"]);
        let takes = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Long && name == "results-directory").then(ValueSpec::value)
        };
        let tokens = tokenize_grammar(&args, &takes, Dialect::Msbuild);

        assert_eq!(tokens[0].kind, TokenKind::DashDash);
        assert_eq!(tokens[1].kind, TokenKind::Long);
        assert_eq!(tokens[1].linked, Some(2));
        assert_eq!(tokens[2].text, "/tmp/out");
    }

    #[test]
    fn msbuild_second_dashdash_is_positional_not_another_boundary() {
        // Regression: DashDash must be emitted exactly once even under Msbuild, where
        // classification doesn't stop at `--` (unlike Posix, where a second `--` already falls
        // into the seen_dash_dash positional catch-all for free).
        let args = owned(&["--", "a", "--", "b"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);

        assert_eq!(tokens[0].kind, TokenKind::DashDash);
        assert_eq!(tokens[1].kind, TokenKind::Positional);
        assert_eq!(tokens[1].text, "a");
        assert_eq!(tokens[2].kind, TokenKind::Positional);
        assert_eq!(tokens[2].text, "--");
        assert_eq!(tokens[3].kind, TokenKind::Positional);
        assert_eq!(tokens[3].text, "b");
        assert_eq!(
            tokens
                .iter()
                .filter(|t| t.kind == TokenKind::DashDash)
                .count(),
            1
        );
    }

    #[test]
    fn msbuild_dialect_never_produces_short_tokens() {
        let args = owned(&["-a", "-bc", "/d", "--e"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);

        assert!(tokens.iter().all(|t| t.kind != TokenKind::Short));
    }

    #[test]
    fn posix_dialect_unaffected_by_slash_or_colon() {
        // The default (tokenize == Dialect::Posix) must not gain '/' or ':' handling.
        let args = owned(&["feature/auth", "--pretty:oops"]);
        let tokens = tokenize(&args);

        assert_eq!(tokens[0].kind, TokenKind::Positional);
        assert_eq!(tokens[0].text, "feature/auth");
        assert_eq!(tokens[1].kind, TokenKind::Long);
        assert_eq!(tokens[1].text, "pretty:oops");
        assert_eq!(tokens[1].attached, None);
    }

    // --- has_flag / has_double_dash_flag / double_dash_flag_value(s) ---

    #[test]
    fn msbuild_has_flag_is_case_insensitive() {
        let args = owned(&["-NoLogo"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);

        assert!(has_flag(&tokens, Dialect::Msbuild, "nologo"));
        assert!(has_flag(&tokens, Dialect::Msbuild, "NOLOGO"));
    }

    #[test]
    fn posix_has_flag_and_flag_value_are_case_sensitive() {
        // git/cargo/rg/golangci-lint don't fold case; "--Grep" is not "--grep".
        let args = owned(&["--Grep"]);
        let tokens = tokenize(&args);

        assert!(has_flag(&tokens, Dialect::Posix, "Grep"));
        assert!(!has_flag(&tokens, Dialect::Posix, "grep"));
    }

    #[test]
    fn has_flag_ignores_short_and_positional_tokens() {
        // A Short "n" or a positional literally spelled "nologo" must not satisfy a Long
        // flag-name lookup for "nologo".
        let args = owned(&["-n", "nologo"]);
        let tokens = tokenize(&args);

        assert!(!has_flag(&tokens, Dialect::Posix, "nologo"));
        assert!(!has_flag(&tokens, Dialect::Posix, "n"));
    }

    #[test]
    fn double_dash_flag_value_is_case_insensitive_but_prefix_strict() {
        let args = owned(&["--Logger:trx"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);

        assert_eq!(
            double_dash_flag_value(&tokens, Dialect::Msbuild, "logger"),
            Some("trx")
        );
        assert_eq!(
            double_dash_flag_value(&tokens, Dialect::Msbuild, "LOGGER"),
            Some("trx")
        );
    }

    #[test]
    fn double_dash_flag_rejects_single_dash_and_slash_spellings() {
        // Regression: verified against a real dotnet 9 SDK that dotnet's own
        // System.CommandLine-parsed options (unlike legacy MSBuild.exe passthrough switches
        // like -nologo) are double-dash-only -- "-results-directory"/"/results-directory" get
        // misparsed as unrelated MSBuild switches, not treated as this flag at all.
        let args = owned(&["-results-directory", "/tmp/out"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);

        assert!(has_flag(&tokens, Dialect::Msbuild, "results-directory"));
        assert!(!has_double_dash_flag(
            &tokens,
            Dialect::Msbuild,
            "results-directory"
        ));
        assert_eq!(
            double_dash_flag_value(&tokens, Dialect::Msbuild, "results-directory"),
            None
        );

        let args = owned(&["/results-directory", "/tmp/out"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::Msbuild);
        assert!(!has_double_dash_flag(
            &tokens,
            Dialect::Msbuild,
            "results-directory"
        ));
    }

    #[test]
    fn double_dash_flag_values_reports_every_occurrence_not_just_the_first() {
        // Regression: dotnet test's --logger can legitimately repeat
        // (`--logger "console;verbosity=normal" --logger trx`) -- unlike
        // double_dash_flag_value, which only reports the first match, every occurrence must be
        // checkable.
        let args = owned(&["--logger:console;verbosity=normal", "--logger", "trx"]);
        let takes = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Long && name == "logger").then(ValueSpec::value)
        };
        let tokens = tokenize_grammar(&args, &takes, Dialect::Msbuild);

        let values: Vec<&str> =
            double_dash_flag_values(&tokens, Dialect::Msbuild, "logger").collect();
        assert_eq!(values, vec!["console;verbosity=normal", "trx"]);
    }

    // --- Dialect::CommonsCli ---

    #[test]
    fn slash_flag_guard_follows_the_attach_axis_not_a_fixed_separator_set() {
        // `slash_flags` and `attach` are independent, so a `/`-prefixed path must stay a path
        // when `:` is not an attach separator -- splitting on a fixed `['=', ':']` would hide
        // the second `/` and promote `/opt:a/b` to a flag.
        let dialect = Dialect {
            slash_flags: true,
            ..Dialect::Posix
        };
        let args = owned(&["/opt:a/b"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, dialect);
        assert!(tokens[0].is_free_positional());
        assert_eq!(tokens[0].text, "/opt:a/b");
    }

    #[test]
    fn commons_cli_short_options_are_atomic_words_not_clusters() {
        // `mvn -Bo validate` is an error against the real binary; `-pl` could not exist if
        // single-dash args clustered.
        let args = owned(&["-pl", "core", "test"]);
        // Asked about as `Long`, not `Short`: a predicate keyed on `Short` would never fire and
        // `-pl` would silently stop claiming its value.
        let takes = |kind: TokenKind, name: &str| {
            (kind == TokenKind::Long && name == "pl").then(ValueSpec::value)
        };
        let tokens = tokenize_grammar(&args, &takes, Dialect::CommonsCli);

        assert_eq!(tokens[0].kind, TokenKind::Long);
        assert_eq!(tokens[0].text, "pl");
        assert_eq!(tokens[0].value(&tokens), Some("core"));
        assert_eq!(tokens[2].text, "test");
        assert!(tokens[2].is_free_positional());
    }

    #[test]
    fn commons_cli_java_property_is_one_flag_name_not_a_d_with_a_value() {
        // The documented limit of the preset: `-D` is a commons-cli Java-property option, which
        // no axis models. Pinned so a caller reading `-D` values sees it must split the name.
        let args = owned(&["-DskipTests=true", "test"]);
        let takes = |_: TokenKind, name: &str| (name == "D").then(ValueSpec::value);
        let tokens = tokenize_grammar(&args, &takes, Dialect::CommonsCli);

        assert_eq!(tokens[0].text, "DskipTests");
        assert_eq!(tokens[0].attached, Some("true"));
        assert!(!has_flag(&tokens, Dialect::CommonsCli, "D"));
        // Still not a positional, which is all goal/task detection needs.
        assert!(!tokens[0].is_free_positional());
        assert!(tokens[1].is_free_positional());
    }

    #[test]
    fn commons_cli_flag_names_are_case_sensitive_unlike_msbuild() {
        let args = owned(&["-B"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::CommonsCli);
        assert!(has_flag(&tokens, Dialect::CommonsCli, "B"));
        assert!(!has_flag(&tokens, Dialect::CommonsCli, "b"));
    }

    #[test]
    fn commons_cli_dashdash_ends_option_parsing() {
        let args = owned(&["--", "-pl", "core"]);
        let takes = |_: TokenKind, name: &str| (name == "pl").then(ValueSpec::value);
        let tokens = tokenize_grammar(&args, &takes, Dialect::CommonsCli);

        assert_eq!(tokens[0].kind, TokenKind::DashDash);
        assert!(tokens[1..].iter().all(|t| t.is_free_positional()));
    }

    #[test]
    fn commons_cli_slash_prefixed_path_is_never_a_flag() {
        let args = owned(&["/opt/build"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::CommonsCli);
        assert!(tokens[0].is_free_positional());
    }

    // --- DashDashRole::EndsGlobalOptions ---

    /// Gradle's parser is `org.gradle.cli`, shared with nothing, so it gets no preset: a caller
    /// composes it from `Posix` at its own call site. This is that composition.
    const GRADLE: Dialect = Dialect {
        dash_dash: DashDashRole::EndsGlobalOptions,
        ..Dialect::Posix
    };

    #[test]
    fn ends_global_options_still_clusters_short_flags_like_posix() {
        // `gradle -qi tasks` is accepted; `gradle -qp /w` is not, hence solo_only on `p`.
        let args = owned(&["-qi", "tasks"]);
        let takes = |_: TokenKind, name: &str| (name == "p").then(ValueSpec::solo_only);
        let tokens = tokenize_grammar(&args, &takes, GRADLE);

        assert_eq!(tokens[0].text, "q");
        assert_eq!(tokens[1].text, "i");
        assert_eq!(tokens[0].source_index, tokens[1].source_index);
        assert_eq!(tokens[2].text, "tasks");
    }

    #[test]
    fn ends_global_options_keeps_parsing_tasks_and_their_options() {
        // Gradle's `--` ends the *global* option region only: `test --tests X` past it is still
        // a task plus its own value-taking option, not three bare positionals.
        let args = owned(&["--", "test", "--tests", "com.example.Foo"]);
        let takes = |_: TokenKind, name: &str| (name == "tests").then(ValueSpec::value);
        let tokens = tokenize_grammar(&args, &takes, GRADLE);

        assert_eq!(tokens[1].text, "test");
        assert!(tokens[1].is_free_positional());
        assert_eq!(tokens[2].kind, TokenKind::Long);
        assert_eq!(tokens[2].value(&tokens), Some("com.example.Foo"));
        assert!(!tokens[3].is_free_positional());
    }

    // --- Dialect::GoFlag ---

    #[test]
    fn goflag_single_and_double_dash_are_the_same_flag() {
        let takes = |_: TokenKind, name: &str| (name == "run").then(ValueSpec::value);
        for spelling in ["-run", "--run"] {
            let args = owned(&[spelling, "TestFoo"]);
            let tokens = tokenize_grammar(&args, &takes, Dialect::GoFlag);
            assert_eq!(tokens[0].kind, TokenKind::Long);
            assert!(tokens[0].double_dash, "for {spelling}");
            assert_eq!(
                double_dash_flag_value(&tokens, Dialect::GoFlag, "run"),
                Some("TestFoo"),
                "for {spelling}"
            );
        }
    }

    #[test]
    fn goflag_single_dash_word_is_one_flag_not_a_cluster() {
        let args = owned(&["-count=1"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::GoFlag);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].text, "count");
        assert_eq!(tokens[0].attached, Some("1"));
    }

    #[test]
    fn goflag_positionals_never_claim_the_double_dash_spelling() {
        let args = owned(&["./...", "--", "-v"]);
        let tokens = tokenize_grammar(&args, &|_, _| None, Dialect::GoFlag);
        assert!(tokens.iter().all(|t| t.kind != TokenKind::Long));
        assert!(tokens.iter().all(|t| !t.double_dash));
    }
}
