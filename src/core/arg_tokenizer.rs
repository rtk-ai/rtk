//! Shared tokenizer for re-classifying an already-`--`-restored passthrough args slice
//! (see [`crate::core::args_utils::restore_double_dash`]) into flags, their values, and
//! positionals, matching the GNU/POSIX-ish conventions used by git, cargo, rg, and friends.
//! Each tool declares its flag grammar once, as [`Grammar`] data (which flags take a value, and
//! how), and [`tokenize_grammar`] does the token-walking around it.
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
    /// `text`.
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
    /// this flag's separate-token value (only set when the grammar gives this flag a separate
    /// value and there was no attached value). For a consumed `Positional`: index of the flag
    /// token that owns it. `None` for a free-standing positional, an unconsumed flag, or
    /// `DashDash`.
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
    /// For `Long`/`Short`: the flag this token is in the grammar that read it
    /// ([`Grammar::flag`]), looked up once while tokenizing, so every question asked about the
    /// token ([`Token::is`], [`Token::is_one_of`]) compares against it rather than searching the
    /// grammar again. `None` for an undeclared flag, for every token [`tokenize`] produces, and
    /// for `Positional`/`DashDash`.
    pub flag: Option<&'static Flag>,
    /// The flags of the grammar that read this token, which [`Token::is`] and
    /// [`Token::is_one_of`] hold each flag they are asked about to.
    declared: Declared,
}

/// A grammar's flag tables as a [`Token`] keeps them. Its `Debug` leaves the tables out, so a
/// printed token stays one line.
#[derive(Clone, Copy)]
struct Declared(&'static [&'static [Flag]]);

/// Tables compare by address: every token of one [`tokenize_grammar`] call holds the same one.
/// Tokens of two calls compare equal only when their grammars' tables share an address, which
/// Rust does not promise for two uses of one `const`.
impl PartialEq for Declared {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for Declared {}

impl std::fmt::Debug for Declared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Declared(..)")
    }
}

impl Declared {
    /// Panics, in a debug build, unless `flag` is declared in these tables exactly as given. A
    /// flag a grammar does not declare, or a copy that differs from the declaration in one
    /// field, can never be a token's flag; asking a token about one is a caller's mistake that
    /// would otherwise read as a plain `false`.
    fn assert_declares(self, flag: &Flag) {
        debug_assert!(
            self.0.iter().any(|table| table.contains(flag)),
            "{flag:?} is not declared in the grammar these tokens were read with"
        );
    }
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

    /// Whether this token is `flag`, as the grammar that read it declares it. Flags compare by
    /// their spellings, which name one declaration per grammar (`assert_takes_value_table`
    /// rejects a spelling declared twice).
    ///
    /// `flag` must be declared in the grammar the token was read with, as given. A debug build
    /// panics on any other flag, whatever the token; a release build answers `false`, since no
    /// token of that grammar can be that flag. The caller keeps the two together: a predicate
    /// that asks for a flag only some grammars declare either takes tokens whose type names
    /// the grammar, or states which grammar its tokens are read with.
    pub fn is(&self, flag: &Flag) -> bool {
        self.declared.assert_declares(flag);
        self.flag.is_some_and(|own| own.same_declaration(flag))
    }

    /// Whether this token is one of `flags`, as the grammar that read it declares them. Every
    /// flag in `flags` must be declared in that grammar, checked as for [`Token::is`].
    pub fn is_one_of(&self, flags: &[Flag]) -> bool {
        for flag in flags {
            self.declared.assert_declares(flag);
        }
        self.flag
            .is_some_and(|own| flags.iter().any(|flag| own.same_declaration(flag)))
    }

    /// How this token takes its value: its declared flag's [`Flag::value`] for this spelling,
    /// `None` for a boolean or undeclared flag and for a non-flag token.
    pub fn value_spec(&self) -> Option<ValueSpec> {
        self.flag?.value(self.kind)
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

/// True if `name` appears as a `Long` token anywhere in `tokens`. Under [`Dialect::Msbuild`],
/// this matches `-flag`/`--flag`/`/flag` uniformly — correct only for legacy MSBuild.exe
/// passthrough switches (`nologo`, `bl`, `v`); see [`has_double_dash_flag`] for anything else.
///
/// `grammar` is the one `tokens` were read with, and the lookup reads its naming rules: `name`
/// matches a token's text exactly, or folding ASCII case under a [`NameCase::Folded`] dialect,
/// so `name` need not be declared; a switch that takes no value, like MSBuild's `bl`, has
/// nothing for a grammar to record. Under [`Abbreviation::UniquePrefix`] it also matches a
/// `Long` token that the grammar resolved by prefix to a flag spelled `name`, when the typed
/// text abbreviates `name` itself (`--alm` for `almost-all`): an atomic flag's other names
/// match only if the text abbreviates them too, and a whole spelling matches only itself. The
/// same holds for [`has_double_dash_flag`], [`double_dash_flag_value`] and
/// [`double_dash_flag_values`].
pub fn has_flag(tokens: &[Token<'_>], grammar: &Grammar, name: &str) -> bool {
    tokens.iter().any(|t| is_long_named(t, grammar, name))
}

/// Whether `t` is a `Long` token naming `name` under `grammar`'s naming rules (see [`has_flag`]).
fn is_long_named(t: &Token<'_>, grammar: &Grammar, name: &str) -> bool {
    let dialect = grammar.dialect;
    // A token the grammar resolved by prefix: its text is no spelling of its flag, and begins
    // `name`, a spelling of it.
    let abbreviates = || {
        dialect.abbreviation == Abbreviation::UniquePrefix
            && t.flag.is_some_and(|flag| {
                !flag.is_spelled(TokenKind::Long, t.text, dialect)
                    && flag.is_spelled(TokenKind::Long, name, dialect)
                    && name
                        .get(..t.text.len())
                        .is_some_and(|head| flag_name_matches(head, t.text, dialect))
            })
    };
    t.kind == TokenKind::Long && (flag_name_matches(t.text, name, dialect) || abbreviates())
}

/// Like [`double_dash_flag_value`], but only reports presence, not the value; only matches a
/// token written with a literal `--` prefix (`Token::double_dash`), not `-flag`/`/flag` under
/// [`Dialect::Msbuild`]. There, a single-dash or slash spelling of a modern
/// System.CommandLine option (e.g. dotnet's `--logger`) doesn't just get rejected — it gets
/// misparsed as an unrelated legacy MSBuild switch — so use this (not [`has_flag`]) for any
/// option that isn't a genuine legacy MSBuild.exe passthrough switch. `name` matches per
/// `grammar`'s naming rules, see [`has_flag`].
pub fn has_double_dash_flag(tokens: &[Token<'_>], grammar: &Grammar, name: &str) -> bool {
    tokens.iter().any(|t| is_double_dash_flag(t, grammar, name))
}

/// This flag's value, if `name` (matched per `grammar`'s naming rules, see [`has_flag`])
/// appears as a `Long` token written with a literal `--` prefix (`Token::double_dash`) anywhere
/// in `tokens`. See [`has_double_dash_flag`] for why this distinction is load-bearing under
/// [`Dialect::Msbuild`].
pub fn double_dash_flag_value<'a>(
    tokens: &[Token<'a>],
    grammar: &Grammar,
    name: &str,
) -> Option<&'a str> {
    tokens
        .iter()
        .find(|t| is_double_dash_flag(t, grammar, name))
        .and_then(|t| t.value(tokens))
}

/// Every value for `name` (matched per `grammar`'s naming rules, see [`has_flag`]), in order,
/// for a `--`-prefixed flag that can legitimately repeat (e.g. dotnet test's `--logger`,
/// usable more than once) — unlike [`double_dash_flag_value`], which only reports the first
/// match. Occurrences with no value are skipped rather than yielding `None`.
pub fn double_dash_flag_values<'a, 't>(
    tokens: &'t [Token<'a>],
    grammar: &'t Grammar,
    name: &'t str,
) -> impl Iterator<Item = &'a str> + 't {
    tokens
        .iter()
        .filter(move |t| is_double_dash_flag(t, grammar, name))
        .filter_map(|t| t.value(tokens))
}

/// Shared match predicate behind [`has_double_dash_flag`]/[`double_dash_flag_value`]/
/// [`double_dash_flag_values`]: a `Long` token written with a literal `--` prefix, matching
/// `name` per `grammar`'s naming rules.
fn is_double_dash_flag(t: &Token<'_>, grammar: &Grammar, name: &str) -> bool {
    t.double_dash && is_long_named(t, grammar, name)
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

/// Whether a long flag may be typed as a prefix of its spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Abbreviation {
    /// Only the whole spelling. clap (cargo, rg), MSBuild, and commons-cli as RTK reads it.
    Never,
    /// The whole spelling, or a prefix of exactly one declared flag's long spelling, as GNU
    /// `getopt_long` reads it (`ls --sor time` sorts by time). A whole spelling wins over a
    /// longer one it prefixes (`--all` is `--all`, not `--almost-all`), and a prefix of two
    /// flags is neither. Resolved against the grammar's declared flags, so a grammar under
    /// this value declares every long option its tool has, booleans included: a prefix is only
    /// unambiguous against the whole set.
    UniquePrefix,
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
    pub abbreviation: Abbreviation,
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
        abbreviation: Abbreviation::Never,
    };

    /// MSBuild/dotnet-CLI-ish. `-flag`, `--flag` and `/flag` are all one atomic flag name, so
    /// every flag is tagged `TokenKind::Long` and `TokenKind::Short` is never produced here.
    pub const Msbuild: Self = Self {
        single_dash: SingleDash::Atomic,
        attach: Attach::EqualsOrColon,
        dash_dash: DashDashRole::Forwards,
        name_case: NameCase::Folded,
        slash_flags: true,
        abbreviation: Abbreviation::Never,
    };

    /// Apache commons-cli: POSIX, except that a short option is a whole multi-character word
    /// (`-pl`, `-am`, `-gs`, `-emp`), so `-abc` never clusters. Maven is the first consumer.
    ///
    /// Does **not** model commons-cli's Java-property options: `-DskipTests=true` tokenizes as
    /// the flag `DskipTests`, not `D` with the value `skipTests=true`. A caller that reads `-D`
    /// values has to split the name itself; one that only needs "this is not a positional" does
    /// not care.
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

    /// Whether a single-dash multi-character argument is one flag name rather than a cluster.
    const fn single_dash_is_one_word(self) -> bool {
        !matches!(self.single_dash, SingleDash::Cluster)
    }
}

/// How a flag's value may be written. The tokenizer branches on this; a tool states it once,
/// per flag, in its [`Grammar`].
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

impl Attachment {
    /// This attachment for a spelling that is always the whole argument, where `solo_only`
    /// has nothing left to restrict.
    const fn whole_argument(self) -> Self {
        match self {
            Attachment::AttachedOrSeparate { .. } => {
                Attachment::AttachedOrSeparate { solo_only: false }
            }
            Attachment::AttachedOnly => Attachment::AttachedOnly,
        }
    }
}

/// How one flag takes its value ([`Flag::value`]). Held in an `Option`, so "takes no value" is
/// `None` and there is one table per tool rather than one per question asked about the same
/// flag.
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

    /// Whether a `Short` flag takes a separate value only when it is the whole argument.
    const fn is_solo_only(self) -> bool {
        matches!(
            self.attachment,
            Attachment::AttachedOrSeparate { solo_only: true }
        )
    }
}

/// How a [`Flag`] is spelled: bare names, without leading dashes (the [`Token::text`]
/// convention). Which form a grammar may declare is its dialect's [`SingleDash`] axis, checked
/// by [`Grammar::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// A getopt flag: a short spelling read as a `Short` token, a long one read as a `Long`
    /// token, either of which may be absent. The two never match each other's tokens, the way
    /// getopt keeps `-v` and `--v` apart.
    Getopt {
        short: Option<&'static str>,
        long: Option<&'static str>,
    },
    /// An atomic flag: every name is one whole word, read as a `Long` token whatever its
    /// prefix, and all of them are this one flag (`-c` and `--configuration`).
    Atomic(&'static [&'static str]),
}

/// One flag of a [`Grammar`]: its spelling, and how each spelling takes its value.
///
/// The value ([`Flag::value`]) is `None` for a boolean flag, and the [`ValueSpec`] given to
/// [`Flag::takes`] otherwise, except that a spelling that is always the whole argument — a long
/// spelling, or any name of an atomic flag — is never solo-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flag {
    spelling: Spelling,
    short_value: Option<ValueSpec>,
    long_value: Option<ValueSpec>,
}

impl Flag {
    const fn getopt(short: Option<&'static str>, long: Option<&'static str>) -> Self {
        Self {
            spelling: Spelling::Getopt { short, long },
            short_value: None,
            long_value: None,
        }
    }

    /// A boolean getopt flag spelled `-name` only.
    pub const fn short(name: &'static str) -> Self {
        Self::getopt(Some(name), None)
    }

    /// A boolean getopt flag spelled `--name` only.
    pub const fn long(name: &'static str) -> Self {
        Self::getopt(None, Some(name))
    }

    /// A boolean getopt flag spelled both `-short` and `--long`.
    pub const fn pair(short: &'static str, long: &'static str) -> Self {
        Self::getopt(Some(short), Some(long))
    }

    /// A boolean atomic flag: each of `names` is one whole word naming this flag, whichever
    /// prefix it is typed with (`-c`, `--configuration`, `/c`). The only form an atomic dialect
    /// reads. Panics on an empty list, which could never match a token.
    pub const fn atomic(names: &'static [&'static str]) -> Self {
        assert!(!names.is_empty(), "an atomic flag needs at least one name");
        Self {
            spelling: Spelling::Atomic(names),
            short_value: None,
            long_value: None,
        }
    }

    /// This flag, taking a value as `spec` says. A short spelling records `spec` as given; a
    /// spelling that is always the whole argument records it without `solo_only`, which only
    /// restricts a `Short` flag inside a cluster. A solo-only spec on a flag with no short
    /// spelling would restrict nothing, so the call panics: such a flag declares
    /// [`ValueSpec::value`]. Evaluated in a `const` or `static` initializer, as every flag of
    /// this crate is, the panic is a compile error; a call evaluated at run time panics at run
    /// time.
    pub const fn takes(self, spec: ValueSpec) -> Self {
        let (has_short, has_whole) = match self.spelling {
            Spelling::Getopt { short, long } => (short.is_some(), long.is_some()),
            Spelling::Atomic(_) => (false, true),
        };
        assert!(
            has_short || !spec.is_solo_only(),
            "a flag with no short spelling cannot be solo-only: declare ValueSpec::value()"
        );
        Self {
            short_value: if has_short { Some(spec) } else { None },
            long_value: if has_whole {
                Some(ValueSpec {
                    attachment: spec.attachment.whole_argument(),
                    ..spec
                })
            } else {
                None
            },
            ..self
        }
    }

    /// How this flag takes its value when spelled as a `kind` token: `None` for a boolean flag,
    /// for a spelling the flag does not have, and for any kind but `Short` and `Long`.
    pub const fn value(&self, kind: TokenKind) -> Option<ValueSpec> {
        match kind {
            TokenKind::Short => self.short_value,
            TokenKind::Long => self.long_value,
            TokenKind::DashDash | TokenKind::Positional => None,
        }
    }

    /// Whether `other` is this declaration: the same spelling. A grammar declares each spelling
    /// once, so two of its flags never share one.
    fn same_declaration(&self, other: &Flag) -> bool {
        self.spelling == other.spelling
    }

    /// Whether a `kind` token named `name` is this flag, under `dialect`'s naming rules.
    fn is_spelled(&self, kind: TokenKind, name: &str, dialect: Dialect) -> bool {
        match (self.spelling, kind) {
            (Spelling::Getopt { short, .. }, TokenKind::Short) => short == Some(name),
            (Spelling::Getopt { long, .. }, TokenKind::Long) => {
                long.is_some_and(|long| flag_name_matches(name, long, dialect))
            }
            (Spelling::Atomic(names), TokenKind::Long) => names
                .iter()
                .any(|known| flag_name_matches(name, known, dialect)),
            (Spelling::Atomic(_), TokenKind::Short)
            | (_, TokenKind::DashDash | TokenKind::Positional) => false,
        }
    }

    /// Whether `prefix` begins one of this flag's `Long` spellings — a getopt flag's long one,
    /// or any name of an atomic flag — under `dialect`'s naming rules.
    fn has_long_prefix(&self, prefix: &str, dialect: Dialect) -> bool {
        let begins = |spelling: &str| {
            spelling
                .get(..prefix.len())
                .is_some_and(|head| flag_name_matches(head, prefix, dialect))
        };
        match self.spelling {
            Spelling::Getopt { long, .. } => long.is_some_and(begins),
            Spelling::Atomic(names) => names.iter().any(|name| begins(name)),
        }
    }

    /// Whether a grammar under `single_dash` can read this flag's spelling. A getopt flag needs
    /// clustering, where `-x` is a `Short` token; an atomic flag needs the opposite, where every
    /// name is a whole word. One `match` on the axis, so a new axis value has to say which
    /// spellings it reads before anything compiles.
    const fn fits(&self, single_dash: SingleDash) -> bool {
        match single_dash {
            SingleDash::Cluster => matches!(self.spelling, Spelling::Getopt { .. }),
            SingleDash::Atomic | SingleDash::AtomicAliasingLong => {
                matches!(self.spelling, Spelling::Atomic(_))
            }
        }
    }
}

/// One tool's (or one subcommand's) flag grammar, declared once as `const`/`static` data and
/// handed to [`tokenize_grammar`]: a [`Dialect`] and the flags read under it, built through
/// [`Grammar::new`], which holds every flag to a spelling the dialect can read.
///
/// `flags` is a list of flag tables, searched in order, first match wins, and a grammar's
/// table test (`assert_takes_value_table`) holds it to one declaration per spelling. A
/// subcommand whose parent's flags all stay valid after it lists the parent's table next to its
/// own (golangci-lint's `run` lists the global table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grammar {
    dialect: Dialect,
    flags: &'static [&'static [Flag]],
}

impl Grammar {
    /// The grammar of `flags`, read under `dialect`.
    ///
    /// Every flag must be spelled in a form `dialect` can read ([`Flag::fits`]): getopt flags
    /// ([`Flag::short`], [`Flag::long`], [`Flag::pair`]) under a clustering dialect, where `-x`
    /// is a `Short` token; atomic flags ([`Flag::atomic`]) under an atomic one, where every name
    /// is a whole word read as `Long`. Any other flag could never match a token as declared, so
    /// the call panics on it. Evaluated in a `const` or `static` initializer, as every grammar of
    /// this crate is, the panic is a compile error; a call evaluated at run time panics at run
    /// time.
    pub const fn new(dialect: Dialect, flags: &'static [&'static [Flag]]) -> Self {
        let mut table = 0;
        while table < flags.len() {
            let mut flag = 0;
            while flag < flags[table].len() {
                assert!(
                    flags[table][flag].fits(dialect.single_dash),
                    "a clustering dialect reads Flag::short/long/pair, an atomic one Flag::atomic"
                );
                flag += 1;
            }
            table += 1;
        }
        Self { dialect, flags }
    }

    /// The declared flag that a `kind` token named `name` is, if any: a `Short` token matches a
    /// getopt flag's short spelling, a `Long` token a getopt flag's long spelling or any name of
    /// an atomic flag (per this grammar's dialect's naming rules), and no other kind matches
    /// anything. Under [`Abbreviation::UniquePrefix`], a `Long` name no spelling matches whole is
    /// the one flag it is a prefix of, if there is exactly one. A linear search:
    /// [`tokenize_grammar`] runs it once per flag token and keeps the answer in [`Token::flag`],
    /// which is what a caller holding tokens reads.
    pub fn flag(&self, kind: TokenKind, name: &str) -> Option<&'static Flag> {
        let declared = || self.flags.iter().flat_map(|table| table.iter());
        if let Some(flag) = declared().find(|flag| flag.is_spelled(kind, name, self.dialect)) {
            return Some(flag);
        }
        match (self.dialect.abbreviation, kind) {
            // A prefix names a flag only when it begins exactly one; two tables may list the
            // same declaration, which is still one flag.
            (Abbreviation::UniquePrefix, TokenKind::Long) if !name.is_empty() => {
                let mut prefixed =
                    declared().filter(|flag| flag.has_long_prefix(name, self.dialect));
                let first = prefixed.next()?;
                prefixed
                    .all(|other| other.same_declaration(first))
                    .then_some(first)
            }
            (Abbreviation::Never, _)
            | (Abbreviation::UniquePrefix, TokenKind::Short | TokenKind::Long)
            | (_, TokenKind::DashDash | TokenKind::Positional) => None,
        }
    }

    /// How a `kind` flag named `name` takes its value: that flag's [`Flag::value`] for this
    /// spelling, `None` for a boolean or undeclared flag. The question a grammar's table test
    /// asks; a caller holding tokens reads [`Token::value_spec`].
    #[cfg(test)]
    pub fn takes_value(&self, kind: TokenKind, name: &str) -> Option<ValueSpec> {
        self.flag(kind, name)?.value(kind)
    }
}

/// The grammar [`tokenize`] reads under: getopt, with no declared flags.
const STRUCTURAL: Grammar = Grammar::new(Dialect::Posix, &[]);

/// Tokenizes `args` structurally, for a caller asking only which arguments are flags, which are
/// positionals, and where `--` is -- subcommand detection, boundary splitting.
///
/// **No flag takes a value here.** Use [`tokenize_grammar`] for anything that reads a flag's
/// value or counts free positionals: without a grammar, `--grep -p` leaves `-p` looking like a
/// flag of its own and `--filter X` leaves `X` looking like a positional path.
pub fn tokenize<'a, T: AsRef<str>>(args: &'a [T]) -> Vec<Token<'a>> {
    tokenize_grammar(args, &STRUCTURAL)
}

/// Tokenizes `args` under one tool's [`Grammar`]. Never panics: a value-taking flag with nothing
/// left to consume simply gets `attached: None, linked: None`.
///
/// Generic over `T: AsRef<str>`, not `OsStr`/`OsString`: `OsStr` exposes almost no
/// string-manipulation API (no `strip_prefix`, `split_once`), so tokenizing it would mean
/// re-deriving that machinery byte-by-byte the way `clap_lex` does internally.
pub fn tokenize_grammar<'a, T: AsRef<str>>(args: &'a [T], grammar: &Grammar) -> Vec<Token<'a>> {
    let mut scanner = Scanner {
        tokens: Vec::with_capacity(args.len()),
        args,
        i: 0,
        emitted_dash_dash: false,
        grammar,
    };

    while scanner.i < scanner.args.len() {
        let arg = scanner.args[scanner.i].as_ref();

        if scanner.emitted_dash_dash && !scanner.grammar.dialect.classifies_past_dash_dash() {
            let token = scanner.positional(arg, scanner.i);
            scanner.tokens.push(token);
            scanner.i += 1;
            continue;
        }

        if arg == "--" {
            if scanner.emitted_dash_dash {
                // A second (or later) literal "--" is never itself the boundary — it's just
                // ordinary text at this point, under every dialect.
                let token = scanner.positional(arg, scanner.i);
                scanner.tokens.push(token);
            } else {
                let token = scanner.token(TokenKind::DashDash, "", scanner.i, FlagPrefix::Dash);
                scanner.tokens.push(token);
                scanner.emitted_dash_dash = true;
            }
            scanner.i += 1;
            continue;
        }

        if let Some(rest) = arg.strip_prefix("--") {
            scanner.push_atomic_flag(rest, FlagPrefix::DashDash);
            continue;
        }

        if scanner.grammar.dialect.slash_flags
            && let Some(rest) = arg.strip_prefix('/')
        {
            // A real MSBuild switch name never contains another '/' -- without this guard,
            // an absolute Unix path would misclassify as a Long flag (e.g. "tmp/results").
            // KNOWN LIMITATION: a single-segment path (`/app`) is indistinguishable from a
            // genuine switch by structure alone; this pure function has no I/O to resolve it
            // the way real MSBuild does (a filesystem check), but the impact is narrow --
            // only the loose flag lookup ([`has_flag`]) is affected.
            let (name_part, _) = split_attached(rest, scanner.grammar.dialect);
            if !rest.is_empty() && !name_part.contains('/') {
                scanner.push_atomic_flag(rest, FlagPrefix::Slash);
                continue;
            }
        }

        if arg.len() > 1 && arg.starts_with('-') {
            if scanner.grammar.dialect.single_dash_is_one_word() {
                // Under `AtomicAliasingLong` the single-dash spelling *is* the `--` flag, so it
                // has to answer to `has_double_dash_flag` as well.
                let prefix = match scanner.grammar.dialect.single_dash {
                    SingleDash::AtomicAliasingLong => FlagPrefix::DashDash,
                    SingleDash::Cluster | SingleDash::Atomic => FlagPrefix::Dash,
                };
                scanner.push_atomic_flag(&arg[1..], prefix);
                continue;
            }

            let cluster = &arg[1..];

            if is_digit_run(cluster) {
                scanner.tokens.push(Token {
                    flag: scanner.grammar.flag(TokenKind::Short, cluster),
                    ..scanner.token(TokenKind::Short, cluster, scanner.i, FlagPrefix::Dash)
                });
                scanner.i += 1;
                continue;
            }

            let mut consumed_next = false;
            let source_index = scanner.i;

            for (offset, ch) in cluster.char_indices() {
                let char_len = ch.len_utf8();
                let char_text = &cluster[offset..offset + char_len];
                let flag_index = scanner.tokens.len();
                let flag = scanner.grammar.flag(TokenKind::Short, char_text);
                scanner.tokens.push(Token {
                    flag,
                    ..scanner.token(TokenKind::Short, char_text, source_index, FlagPrefix::Dash)
                });

                if let Some(spec) = flag.and_then(|flag| flag.value(TokenKind::Short)) {
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

        let token = scanner.positional(arg, scanner.i);
        scanner.tokens.push(token);
        scanner.i += 1;
    }

    scanner.tokens
}

/// Groups the mutable scan state threaded through [`tokenize_grammar`]'s helper methods
/// (`push_atomic_flag`/`link_next_value`), so a future piece of shared state means adding one
/// field instead of a parameter to every helper and every call site.
struct Scanner<'a, 'g, T> {
    tokens: Vec<Token<'a>>,
    args: &'a [T],
    i: usize,
    emitted_dash_dash: bool,
    grammar: &'g Grammar,
}

impl<'a, 'g, T: AsRef<str>> Scanner<'a, 'g, T> {
    /// Pushes one atomic (non-clustering) `Long` flag token — `--flag` in every dialect, and
    /// `-flag`/`/flag` where the dialect's axes say so. `rest` is the flag text with its prefix
    /// already stripped; `prefix` records which one it was. The value attaches after one of the
    /// dialect's [`Attach`] separators (`--flag=v`, `/flag:v`) or, failing that, is the next
    /// argument. Only the `/flag` spelling is barred from consuming a separate value: an
    /// MSBuild switch attaches its value with `:` (`/bl:x.binlog`), so `/r` (MSBuild's
    /// `restore`) must not swallow the token after it the way dotnet's own `-r <rid>` does.
    fn push_atomic_flag(&mut self, rest: &'a str, prefix: FlagPrefix) {
        let (name, attached) = split_attached(rest, self.grammar.dialect);
        let flag_index = self.tokens.len();
        let source_index = self.i;
        let flag = self.grammar.flag(TokenKind::Long, name);
        self.tokens.push(Token {
            attached,
            flag,
            ..self.token(TokenKind::Long, name, source_index, prefix)
        });
        self.i += 1;

        // `solo_only` cannot apply here: an atomic flag is always the whole argument.
        if attached.is_none()
            && prefix != FlagPrefix::Slash
            && let Some(spec) = flag.and_then(|flag| flag.value(TokenKind::Long))
            && spec.attachment != Attachment::AttachedOnly
            && self.link_next_value(flag_index, self.i, spec)
        {
            self.i += 1;
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
            ..self.positional(next.as_ref(), value_index)
        });
        self.tokens[flag_index].linked = Some(token_index);
        true
    }

    /// Base constructor for a freshly-scanned token of this scan's grammar: `attached`,
    /// `linked` and `flag` default to `None`. Every token-construction site builds on this via
    /// struct-update syntax instead of a full literal.
    fn token(
        &self,
        kind: TokenKind,
        text: &'a str,
        source_index: usize,
        prefix: FlagPrefix,
    ) -> Token<'a> {
        Token {
            kind,
            text,
            attached: None,
            linked: None,
            source_index,
            double_dash: prefix == FlagPrefix::DashDash,
            slash: prefix == FlagPrefix::Slash,
            flag: None,
            declared: Declared(self.grammar.flags),
        }
    }

    fn positional(&self, text: &'a str, source_index: usize) -> Token<'a> {
        self.token(TokenKind::Positional, text, source_index, FlagPrefix::Dash)
    }
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

/// How a flag was spelled. All three can tokenize as `Long`, but they are not interchangeable:
/// MSBuild's `/flag` attaches its value with `:` and never consumes the next argument, while
/// dotnet's own `-flag`/`--flag` do.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FlagPrefix {
    DashDash,
    Dash,
    Slash,
}

/// One row of an [`assert_takes_value_table`] table: every name listed is a declared flag of
/// this kind, taking its value as the spec says, or none for `None` (a boolean flag).
#[cfg(test)]
pub(crate) type TakesValueRow = (TokenKind, &'static [&'static str], Option<ValueSpec>);

/// Asserts that `grammar` answers [`Grammar::flag`] and [`Grammar::takes_value`] exactly as
/// `table` says: for every name the table lists and a few names no table lists, under every
/// token kind, a name listed under a kind is a declared flag taking that row's spec (or no
/// value), and any other is not declared under that spelling — it resolves to no flag, or,
/// under [`Abbreviation::UniquePrefix`], only to the one long flag it abbreviates. Also asserts
/// that `grammar` declares no spelling the table leaves out, so the table is the grammar's full
/// contents, and none twice:
/// the first declaration of a spelling wins, so a second one is dead data that looks live.
/// Spellings compare under the grammar's naming rules, so `NoLogo` and `nologo` are one
/// spelling under a [`NameCase::Folded`] dialect.
#[cfg(test)]
pub(crate) fn assert_takes_value_table(grammar: &Grammar, table: &[TakesValueRow]) {
    // `Some(spec)` for a listed name, `None` for one the table does not list.
    let row = |kind: TokenKind, name: &str| {
        table
            .iter()
            .find(|(k, names, _)| *k == kind && names.contains(&name))
            .map(|(_, _, spec)| *spec)
    };
    let unknown = ["", "-", "x", "zz", "20", "unknown-flag"];
    let names = table
        .iter()
        .flat_map(|(_, names, _)| names.iter().copied())
        .chain(unknown);
    for name in names {
        for kind in [
            TokenKind::Short,
            TokenKind::Long,
            TokenKind::Positional,
            TokenKind::DashDash,
        ] {
            let found = grammar.flag(kind, name);
            match row(kind, name) {
                Some(spec) => {
                    assert!(found.is_some(), "{kind:?} {name:?} declared");
                    assert_eq!(grammar.takes_value(kind, name), spec, "{kind:?} {name:?}");
                }
                None => assert!(
                    found.is_none_or(|flag| grammar.dialect.abbreviation
                        == Abbreviation::UniquePrefix
                        && kind == TokenKind::Long
                        && !flag.is_spelled(kind, name, grammar.dialect)),
                    "{kind:?} {name:?} is declared but not in the table"
                ),
            }
        }
    }
    let mut spellings: Vec<(TokenKind, &str)> = Vec::new();
    for flag in grammar.flags.iter().flat_map(|table| table.iter()) {
        // Every name an atomic flag answers to is a `Long` spelling of it.
        let declared: Vec<(TokenKind, &str)> = match flag.spelling {
            Spelling::Getopt { short, long } => short
                .map(|short| (TokenKind::Short, short))
                .into_iter()
                .chain(long.map(|long| (TokenKind::Long, long)))
                .collect(),
            Spelling::Atomic(names) => names.iter().map(|name| (TokenKind::Long, *name)).collect(),
        };
        for (kind, name) in declared {
            let dashes = if kind == TokenKind::Short { "-" } else { "--" };
            assert!(
                row(kind, name).is_some(),
                "{dashes}{name} is declared but not in the table"
            );
            spellings.push((kind, name));
        }
    }
    if let Some((kind, name)) = spelling_declared_twice(grammar.dialect, &spellings) {
        panic!("{kind:?} {name:?} is declared twice");
    }
}

/// The first `(kind, name)` in `spellings` that names the same flag as an earlier one under
/// `dialect`'s naming rules.
#[cfg(test)]
fn spelling_declared_twice<'s>(
    dialect: Dialect,
    spellings: &[(TokenKind, &'s str)],
) -> Option<(TokenKind, &'s str)> {
    spellings
        .iter()
        .enumerate()
        .find_map(|(index, &(kind, name))| {
            spellings[..index]
                .iter()
                .any(|&(k, n)| k == kind && flag_name_matches(name, n, dialect))
                .then_some((kind, name))
        })
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

    /// No declared flags, MSBuild dialect.
    const MSBUILD_BARE: Grammar = Grammar::new(Dialect::Msbuild, &[]);
    /// git's `--grep <pattern>`.
    const GREP_LONG: Grammar = Grammar::new(
        Dialect::Posix,
        &[&[Flag::long("grep").takes(ValueSpec::value())]],
    );
    /// grep's `-A <n>`.
    const A_SHORT: Grammar = Grammar::new(
        Dialect::Posix,
        &[&[Flag::short("A").takes(ValueSpec::value())]],
    );

    #[test]
    fn attached_only_bars_a_separate_value_in_both_kinds() {
        // Short: git's `-M`/`-U` take an optional attached number and never the next token.
        const M: Flag = Flag::short("M").takes(ValueSpec::attached_only());
        const MIN_PARENTS: Flag = Flag::long("min-parents").takes(ValueSpec::attached_only());
        const SHORT: Grammar = Grammar::new(Dialect::Posix, &[&[M]]);
        const LONG: Grammar = Grammar::new(Dialect::Posix, &[&[MIN_PARENTS]]);
        const BOTH: Grammar = Grammar::new(Dialect::Posix, &[&[M, MIN_PARENTS]]);

        let args = owned(&["-M", "50", "f.txt"]);
        let tokens = tokenize_grammar(&args, &SHORT);
        assert_eq!(tokens[0].text, "M");
        assert_eq!(tokens[0].linked, None, "-M must not claim the 50");
        assert_eq!(tokens[0].value(&tokens), None);
        assert!(tokens[1].is_free_positional());

        // Long: previously unexpressible -- the old API could only bar a Short flag.
        let args = owned(&["--min-parents", "2"]);
        let tokens = tokenize_grammar(&args, &LONG);
        assert_eq!(tokens[0].linked, None);
        assert!(tokens[1].is_free_positional());

        // The attached spelling still works for both.
        let args = owned(&["-M50", "--min-parents=2"]);
        let tokens = tokenize_grammar(&args, &BOTH);
        assert_eq!(tokens[0].value(&tokens), Some("50"));
        assert_eq!(tokens[1].value(&tokens), Some("2"));
    }

    #[test]
    fn solo_only_restricts_a_short_flag_to_the_whole_argument() {
        const N: Grammar = Grammar::new(
            Dialect::Posix,
            &[&[Flag::short("n").takes(ValueSpec::solo_only())]],
        );

        let solo = owned(&["-n", "2"]);
        let tokens = tokenize_grammar(&solo, &N);
        assert_eq!(tokens[0].value(&tokens), Some("2"));

        // Clustered: real git rejects `git log -pn 2`, so the 2 stays a positional.
        let clustered = owned(&["-pn", "2"]);
        let tokens = tokenize_grammar(&clustered, &N);
        let n = tokens.iter().find(|t| t.text == "n").expect("n token");
        assert_eq!(n.value(&tokens), None);
        assert!(tokens.last().expect("positional").is_free_positional());
    }

    #[test]
    fn claiming_dash_dash_is_per_flag_not_global() {
        const E: Grammar = Grammar::new(
            Dialect::Posix,
            &[&[Flag::short("e").takes(ValueSpec::value())]],
        );
        const E_CLAIMING: Grammar = Grammar::new(
            Dialect::Posix,
            &[&[Flag::short("e").takes(ValueSpec::value().claiming_dash_dash())]],
        );
        let args = owned(&["-e", "--", "f.txt"]);

        // Default: `--` is the boundary, so -e gets no value and f.txt is past it.
        let tokens = tokenize_grammar(&args, &E);
        assert_eq!(tokens[0].value(&tokens), None);
        assert_eq!(tokens[1].kind, TokenKind::DashDash);

        // grep/rg: -e claims the literal `--` as its pattern.
        let tokens = tokenize_grammar(&args, &E_CLAIMING);
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
        let tokens = tokenize_grammar(&args, &GREP_LONG);

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
        let tokens = tokenize_grammar(&args, &GREP_LONG);

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
        let tokens = tokenize_grammar(&args, &A_SHORT);

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
        const LOGGER: Grammar = Grammar::new(
            Dialect::Msbuild,
            &[&[Flag::atomic(&["logger"]).takes(ValueSpec::value())]],
        );
        let tokens = tokenize_grammar(&args, &LOGGER);

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
    fn attached_long_value_needs_no_grammar() {
        let args = owned(&["--grep=-p"]);
        // No grammar declares `grep` here: the value comes from the "=" form alone.
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
            const DIFF: Grammar = Grammar::new(
                Dialect::Posix,
                &[&[
                    Flag::long("diff-algorithm").takes(ValueSpec::value()),
                    Flag::long("diff-filter").takes(ValueSpec::value()),
                ]],
            );
            let tokens = tokenize_grammar(&args, &DIFF);

            assert_eq!(tokens[0].linked, Some(1), "--{flag} should link its value");
            assert_eq!(tokens[1].text, "-p");
        }
    }

    #[test]
    fn value_taking_flag_at_end_of_args_degrades_gracefully() {
        let args = owned(&["--grep"]);
        let tokens = tokenize_grammar(&args, &GREP_LONG);

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
        let tokens = tokenize_grammar(&args, &A_SHORT);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].text, "A");
        assert_eq!(tokens[0].attached, Some("3"));
        assert_eq!(tokens[0].linked, None);
    }

    #[test]
    fn short_flag_without_attached_remainder_consumes_next_token() {
        let args = owned(&["-A", "3"]);
        let tokens = tokenize_grammar(&args, &A_SHORT);

        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].linked, Some(1));
        assert_eq!(tokens[1].text, "3");
        assert_eq!(tokens[1].linked, Some(0));
    }

    #[test]
    fn short_cluster_stops_consuming_chars_after_value_taking_one() {
        // "-rA3": r is boolean, A takes the attached "3", nothing after A is scanned.
        let args = owned(&["-rA3"]);
        let tokens = tokenize_grammar(&args, &A_SHORT);

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

    // --- &MSBUILD_BARE ---

    #[test]
    fn msbuild_single_dash_flag_is_atomic_not_a_cluster() {
        // dotnet's "-nologo" is one flag name, not a POSIX cluster of n/o/l/o/g/o.
        let args = owned(&["-nologo"]);
        let tokens = tokenize_grammar(&args, &MSBUILD_BARE);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Long);
        assert_eq!(tokens[0].text, "nologo");
    }

    #[test]
    fn value_is_none_on_a_consumed_positional_and_safe_on_a_slice() {
        let args = owned(&["--grep", "x", "file.rs"]);
        let tokens = tokenize_grammar(&args, &GREP_LONG);

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
        const R: Grammar = Grammar::new(
            Dialect::Msbuild,
            &[&[Flag::atomic(&["r"]).takes(ValueSpec::value())]],
        );
        let slash = owned(&["/r", "-bl:my.binlog"]);
        let tokens = tokenize_grammar(&slash, &R);
        assert_eq!(tokens[0].text, "r");
        assert_eq!(tokens[0].linked, None);
        assert_eq!(tokens[1].kind, TokenKind::Long);
        assert_eq!(tokens[1].text, "bl");
        assert_eq!(tokens[1].attached, Some("my.binlog"));

        // The dash spelling is dotnet's own `-r <rid>`, which does consume the next token.
        let dash = owned(&["-r", "linux-x64"]);
        let tokens = tokenize_grammar(&dash, &R);
        assert_eq!(tokens[0].value(&tokens), Some("linux-x64"));
    }

    #[test]
    fn msbuild_slash_prefix_is_recognized_as_a_flag() {
        let args = owned(&["/nologo"]);
        let tokens = tokenize_grammar(&args, &MSBUILD_BARE);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Long);
        assert_eq!(tokens[0].text, "nologo");
    }

    #[test]
    fn msbuild_slash_alone_is_positional() {
        let args = owned(&["/"]);
        let tokens = tokenize_grammar(&args, &MSBUILD_BARE);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Positional);
        assert_eq!(tokens[0].text, "/");
    }

    #[test]
    fn msbuild_absolute_path_is_positional_not_a_flag() {
        // Real MSBuild never treats a multi-segment "/a/b" as a switch attempt.
        const NOLOGO: Grammar = Grammar::new(
            Dialect::Msbuild,
            &[&[Flag::atomic(&["nologo"]).takes(ValueSpec::value())]],
        );
        let args = owned(&["/tmp/results"]);
        let tokens = tokenize_grammar(&args, &NOLOGO);

        assert_eq!(tokens[0].kind, TokenKind::Positional);
        assert_eq!(tokens[0].text, "/tmp/results");
    }

    #[test]
    fn msbuild_single_segment_slash_flag_is_still_a_flag() {
        // A genuine single-segment MSBuild switch (no internal '/') must still classify as Long,
        // including when it carries an attached value whose own text contains '/'.
        let args = owned(&["/nologo", "/p:OutDir=/tmp/out"]);
        let tokens = tokenize_grammar(&args, &MSBUILD_BARE);

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
            let tokens = tokenize_grammar(&args, &MSBUILD_BARE);

            assert_eq!(tokens[0].text, "logger", "for {arg}");
            assert_eq!(tokens[0].attached, Some("trx"), "for {arg}");
        }
    }

    #[test]
    fn msbuild_separate_token_value_still_works() {
        let args = owned(&["--results-directory", "/tmp/out"]);
        const RESULTS_DIRECTORY: Grammar = Grammar::new(
            Dialect::Msbuild,
            &[&[Flag::atomic(&["results-directory"]).takes(ValueSpec::value())]],
        );
        let tokens = tokenize_grammar(&args, &RESULTS_DIRECTORY);

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
        let tokens = tokenize_grammar(&args, &MSBUILD_BARE);

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
        const RESULTS_DIRECTORY: Grammar = Grammar::new(
            Dialect::Msbuild,
            &[&[Flag::atomic(&["results-directory"]).takes(ValueSpec::value())]],
        );
        let tokens = tokenize_grammar(&args, &RESULTS_DIRECTORY);

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
        let tokens = tokenize_grammar(&args, &MSBUILD_BARE);

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
        let tokens = tokenize_grammar(&args, &MSBUILD_BARE);

        assert!(tokens.iter().all(|t| t.kind != TokenKind::Short));
    }

    #[test]
    fn posix_dialect_unaffected_by_slash_or_colon() {
        // The default (tokenize == &STRUCTURAL) must not gain '/' or ':' handling.
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
        let tokens = tokenize_grammar(&args, &MSBUILD_BARE);

        assert!(has_flag(&tokens, &MSBUILD_BARE, "nologo"));
        assert!(has_flag(&tokens, &MSBUILD_BARE, "NOLOGO"));
    }

    #[test]
    fn posix_has_flag_and_flag_value_are_case_sensitive() {
        // git/cargo/rg/golangci-lint don't fold case; "--Grep" is not "--grep".
        let args = owned(&["--Grep"]);
        let tokens = tokenize(&args);

        assert!(has_flag(&tokens, &STRUCTURAL, "Grep"));
        assert!(!has_flag(&tokens, &STRUCTURAL, "grep"));
    }

    #[test]
    fn has_flag_ignores_short_and_positional_tokens() {
        // A Short "n" or a positional literally spelled "nologo" must not satisfy a Long
        // flag-name lookup for "nologo".
        let args = owned(&["-n", "nologo"]);
        let tokens = tokenize(&args);

        assert!(!has_flag(&tokens, &STRUCTURAL, "nologo"));
        assert!(!has_flag(&tokens, &STRUCTURAL, "n"));
    }

    #[test]
    fn double_dash_flag_value_is_case_insensitive_but_prefix_strict() {
        let args = owned(&["--Logger:trx"]);
        let tokens = tokenize_grammar(&args, &MSBUILD_BARE);

        assert_eq!(
            double_dash_flag_value(&tokens, &MSBUILD_BARE, "logger"),
            Some("trx")
        );
        assert_eq!(
            double_dash_flag_value(&tokens, &MSBUILD_BARE, "LOGGER"),
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
        let tokens = tokenize_grammar(&args, &MSBUILD_BARE);

        assert!(has_flag(&tokens, &MSBUILD_BARE, "results-directory"));
        assert!(!has_double_dash_flag(
            &tokens,
            &MSBUILD_BARE,
            "results-directory"
        ));
        assert_eq!(
            double_dash_flag_value(&tokens, &MSBUILD_BARE, "results-directory"),
            None
        );

        let args = owned(&["/results-directory", "/tmp/out"]);
        let tokens = tokenize_grammar(&args, &MSBUILD_BARE);
        assert!(!has_double_dash_flag(
            &tokens,
            &MSBUILD_BARE,
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
        const LOGGER: Grammar = Grammar::new(
            Dialect::Msbuild,
            &[&[Flag::atomic(&["logger"]).takes(ValueSpec::value())]],
        );
        let tokens = tokenize_grammar(&args, &LOGGER);

        let values: Vec<&str> = double_dash_flag_values(&tokens, &LOGGER, "logger").collect();
        assert_eq!(values, vec!["console;verbosity=normal", "trx"]);
    }

    // --- Dialect::CommonsCli ---

    #[test]
    fn slash_flag_guard_follows_the_attach_axis_not_a_fixed_separator_set() {
        // `slash_flags` and `attach` are independent, so a `/`-prefixed path must stay a path
        // when `:` is not an attach separator -- splitting on a fixed `['=', ':']` would hide
        // the second `/` and promote `/opt:a/b` to a flag.
        let grammar = Grammar::new(
            Dialect {
                slash_flags: true,
                ..Dialect::Posix
            },
            &[],
        );
        let args = owned(&["/opt:a/b"]);
        let tokens = tokenize_grammar(&args, &grammar);
        assert!(tokens[0].is_free_positional());
        assert_eq!(tokens[0].text, "/opt:a/b");
    }

    /// Maven's `-pl`, read as the commons-cli word it is.
    const MVN_PL: Grammar = Grammar::new(
        Dialect::CommonsCli,
        &[&[Flag::atomic(&["pl"]).takes(ValueSpec::value())]],
    );

    #[test]
    fn commons_cli_short_options_are_atomic_words_not_clusters() {
        // `mvn -Bo validate` is an error against the real binary; `-pl` could not exist if
        // single-dash args clustered.
        let args = owned(&["-pl", "core", "test"]);
        // Read as `Long`, not `Short`: a lookup keyed on `Short` would never fire and `-pl`
        // would silently stop claiming its value.
        let tokens = tokenize_grammar(&args, &MVN_PL);

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
        const G: Grammar = Grammar::new(
            Dialect::CommonsCli,
            &[&[Flag::atomic(&["D"]).takes(ValueSpec::value())]],
        );
        let args = owned(&["-DskipTests=true", "test"]);
        let tokens = tokenize_grammar(&args, &G);

        assert_eq!(tokens[0].text, "DskipTests");
        assert_eq!(tokens[0].attached, Some("true"));
        assert!(!has_flag(&tokens, &G, "D"));
        // Still not a positional, which is all goal/task detection needs.
        assert!(!tokens[0].is_free_positional());
        assert!(tokens[1].is_free_positional());
    }

    #[test]
    fn commons_cli_flag_names_are_case_sensitive_unlike_msbuild() {
        const G: Grammar = Grammar::new(Dialect::CommonsCli, &[]);
        let args = owned(&["-B"]);
        let tokens = tokenize_grammar(&args, &G);
        assert!(has_flag(&tokens, &G, "B"));
        assert!(!has_flag(&tokens, &G, "b"));
    }

    #[test]
    fn commons_cli_dashdash_ends_option_parsing() {
        let args = owned(&["--", "-pl", "core"]);
        let tokens = tokenize_grammar(&args, &MVN_PL);

        assert_eq!(tokens[0].kind, TokenKind::DashDash);
        assert!(tokens[1..].iter().all(|t| t.is_free_positional()));
    }

    #[test]
    fn commons_cli_slash_prefixed_path_is_never_a_flag() {
        const G: Grammar = Grammar::new(Dialect::CommonsCli, &[]);
        let args = owned(&["/opt/build"]);
        let tokens = tokenize_grammar(&args, &G);
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
        const G: Grammar =
            Grammar::new(GRADLE, &[&[Flag::short("p").takes(ValueSpec::solo_only())]]);
        let args = owned(&["-qi", "tasks"]);
        let tokens = tokenize_grammar(&args, &G);

        assert_eq!(tokens[0].text, "q");
        assert_eq!(tokens[1].text, "i");
        assert_eq!(tokens[0].source_index, tokens[1].source_index);
        assert_eq!(tokens[2].text, "tasks");
    }

    #[test]
    fn ends_global_options_keeps_parsing_tasks_and_their_options() {
        // Gradle's `--` ends the *global* option region only: `test --tests X` past it is still
        // a task plus its own value-taking option, not three bare positionals.
        const G: Grammar =
            Grammar::new(GRADLE, &[&[Flag::long("tests").takes(ValueSpec::value())]]);
        let args = owned(&["--", "test", "--tests", "com.example.Foo"]);
        let tokens = tokenize_grammar(&args, &G);

        assert_eq!(tokens[1].text, "test");
        assert!(tokens[1].is_free_positional());
        assert_eq!(tokens[2].kind, TokenKind::Long);
        assert_eq!(tokens[2].value(&tokens), Some("com.example.Foo"));
        assert!(!tokens[3].is_free_positional());
    }

    // --- Dialect::GoFlag ---

    #[test]
    fn goflag_single_and_double_dash_are_the_same_flag() {
        const G: Grammar = Grammar::new(
            Dialect::GoFlag,
            &[&[Flag::atomic(&["run"]).takes(ValueSpec::value())]],
        );
        for spelling in ["-run", "--run"] {
            let args = owned(&[spelling, "TestFoo"]);
            let tokens = tokenize_grammar(&args, &G);
            assert_eq!(tokens[0].kind, TokenKind::Long);
            assert!(tokens[0].double_dash, "for {spelling}");
            assert_eq!(
                double_dash_flag_value(&tokens, &G, "run"),
                Some("TestFoo"),
                "for {spelling}"
            );
        }
    }

    #[test]
    fn goflag_single_dash_word_is_one_flag_not_a_cluster() {
        const G: Grammar = Grammar::new(Dialect::GoFlag, &[]);
        let args = owned(&["-count=1"]);
        let tokens = tokenize_grammar(&args, &G);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].text, "count");
        assert_eq!(tokens[0].attached, Some("1"));
    }

    #[test]
    fn goflag_positionals_never_claim_the_double_dash_spelling() {
        const G: Grammar = Grammar::new(Dialect::GoFlag, &[]);
        let args = owned(&["./...", "--", "-v"]);
        let tokens = tokenize_grammar(&args, &G);
        assert!(tokens.iter().all(|t| t.kind != TokenKind::Long));
        assert!(tokens.iter().all(|t| !t.double_dash));
    }

    // --- Grammar ---

    #[test]
    fn grammar_matches_short_and_long_spellings_by_kind() {
        const G: Grammar = Grammar::new(
            Dialect::Posix,
            &[&[Flag::pair("u", "set-upstream-to").takes(ValueSpec::value())]],
        );
        assert_eq!(
            G.takes_value(TokenKind::Short, "u"),
            Some(ValueSpec::value())
        );
        assert_eq!(
            G.takes_value(TokenKind::Long, "set-upstream-to"),
            Some(ValueSpec::value())
        );
        // `--u` is not `-u`, and `-set-upstream-to` is not `--set-upstream-to`.
        assert_eq!(G.takes_value(TokenKind::Long, "u"), None);
        assert_eq!(G.takes_value(TokenKind::Short, "set-upstream-to"), None);
        assert_eq!(G.takes_value(TokenKind::Positional, "u"), None);
        assert_eq!(G.takes_value(TokenKind::DashDash, "u"), None);
    }

    #[test]
    fn grammar_boolean_flag_takes_no_value() {
        const G: Grammar = Grammar::new(Dialect::Posix, &[&[Flag::pair("v", "verbose")]]);
        assert!(G.flag(TokenKind::Short, "v").is_some());
        assert_eq!(G.takes_value(TokenKind::Short, "v"), None);
        assert_eq!(G.takes_value(TokenKind::Long, "verbose"), None);
    }

    #[test]
    fn a_flag_records_solo_only_for_its_short_spelling_only() {
        const G: Grammar = Grammar::new(
            Dialect::Posix,
            &[&[Flag::pair("c", "config").takes(ValueSpec::solo_only())]],
        );
        // `flag()` and `takes_value()` give one answer for each spelling.
        for (kind, name, spec) in [
            (TokenKind::Short, "c", ValueSpec::solo_only()),
            (TokenKind::Long, "config", ValueSpec::value()),
        ] {
            let flag = G.flag(kind, name).expect("declared");
            assert_eq!(flag.value(kind), Some(spec), "{kind:?} {name}");
            assert_eq!(G.takes_value(kind, name), Some(spec), "{kind:?} {name}");
        }
        // A flag has no value under a spelling it does not have.
        const SHORT_ONLY: Flag = Flag::short("n").takes(ValueSpec::solo_only());
        assert_eq!(SHORT_ONLY.value(TokenKind::Long), None);
        assert_eq!(SHORT_ONLY.value(TokenKind::Positional), None);

        // The dash-dash claim is kept for the long spelling.
        const CLAIMING: Flag =
            Flag::pair("e", "regexp").takes(ValueSpec::solo_only().claiming_dash_dash());
        assert_eq!(
            CLAIMING.value(TokenKind::Long),
            Some(ValueSpec::value().claiming_dash_dash())
        );
        // `attached_only` bars the separate form for both spellings.
        const ATTACHED: Flag = Flag::pair("U", "unified").takes(ValueSpec::attached_only());
        assert_eq!(
            ATTACHED.value(TokenKind::Long),
            Some(ValueSpec::attached_only())
        );
        assert_eq!(
            ATTACHED.value(TokenKind::Short),
            Some(ValueSpec::attached_only())
        );
    }

    #[test]
    #[should_panic(expected = "cannot be solo-only")]
    fn a_long_only_flag_cannot_be_solo_only() {
        // In a `const` this is a compile error; called at run time it panics.
        let spec = std::hint::black_box(ValueSpec::solo_only());
        let _ = Flag::long("config").takes(spec);
    }

    #[test]
    #[should_panic(expected = "cannot be solo-only")]
    fn an_atomic_flag_cannot_be_solo_only() {
        // Every name of an atomic flag is the whole argument, so solo-only restricts nothing.
        let spec = std::hint::black_box(ValueSpec::solo_only());
        let _ = Flag::atomic(&["c", "configuration"]).takes(spec);
    }

    #[test]
    #[should_panic(expected = "an atomic one Flag::atomic")]
    fn an_atomic_dialect_refuses_a_getopt_flag() {
        // In a `const` this is a compile error; called at run time it panics. Under MSBuild
        // every name is one `Long` word, so a short spelling could never match a token.
        static TABLES: &[&[Flag]] =
            &[&[Flag::atomic(&["logger"]), Flag::pair("c", "configuration")]];
        let _ = Grammar::new(Dialect::Msbuild, std::hint::black_box(TABLES));
    }

    #[test]
    #[should_panic(expected = "a clustering dialect reads Flag::short/long/pair")]
    fn a_clustering_dialect_refuses_an_atomic_flag() {
        // Under getopt `-pl` is the cluster `-p -l`, so an atomic `pl` could never match.
        static TABLES: &[&[Flag]] = &[&[Flag::atomic(&["pl", "projects"])]];
        let _ = Grammar::new(Dialect::Posix, std::hint::black_box(TABLES));
    }

    #[test]
    fn an_atomic_flag_is_one_flag_under_every_name() {
        const CONFIGURATION: Flag = Flag::atomic(&["c", "configuration"]).takes(ValueSpec::value());
        const G: Grammar = Grammar::new(Dialect::Msbuild, &[&[CONFIGURATION]]);
        for spelling in ["-c", "--configuration", "/c", "--CONFIGURATION"] {
            let args = owned(&[spelling, "Release"]);
            let tokens = tokenize_grammar(&args, &G);
            assert!(tokens[0].is(&CONFIGURATION), "{spelling}");
            assert_eq!(
                tokens[0].value_spec(),
                Some(ValueSpec::value()),
                "{spelling}"
            );
        }
    }

    #[test]
    fn each_flag_token_carries_the_flag_its_grammar_declares() {
        const V: Flag = Flag::pair("v", "verbose");
        const N: Flag = Flag::short("n").takes(ValueSpec::value());
        const G: Grammar = Grammar::new(Dialect::Posix, &[&[V, N]]);
        let args = owned(&["-vn", "3", "--verbose", "-x", "--other", "file"]);
        let tokens = tokenize_grammar(&args, &G);
        let flags: Vec<Option<&Flag>> = tokens.iter().map(|t| t.flag).collect();
        assert_eq!(
            flags,
            [Some(&V), Some(&N), None, Some(&V), None, None, None],
            "{tokens:?}"
        );
        assert!(tokens[0].is(&V) && tokens[1].is_one_of(&[V, N]));
        assert_eq!(tokens[1].value_spec(), Some(ValueSpec::value()));
        assert_eq!(tokens[0].value_spec(), None);
        // The structural tokenizer declares nothing.
        assert!(tokenize(&args).iter().all(|t| t.flag.is_none()));
    }

    /// A copy of a declared flag that differs in one field is not the declaration: asking a
    /// token about it panics in a debug build rather than answering `false` for ever.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "is not declared in the grammar")]
    fn asking_about_a_flag_declared_differently_panics() {
        const N: Flag = Flag::pair("n", "max-count").takes(ValueSpec::solo_only());
        const G: Grammar = Grammar::new(Dialect::Posix, &[&[N]]);
        const N_AS_VALUE: Flag = Flag::pair("n", "max-count").takes(ValueSpec::value());
        let args = owned(&["-n", "2"]);
        let tokens = tokenize_grammar(&args, &G);
        let _ = tokens[0].is(&N_AS_VALUE);
    }

    /// A flag the grammar does not declare at all panics too, whichever token is asked: the
    /// question is wrong before any token answers it.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "is not declared in the grammar")]
    fn asking_about_an_undeclared_flag_panics() {
        const V: Flag = Flag::pair("v", "verbose");
        const G: Grammar = Grammar::new(Dialect::Posix, &[&[V]]);
        let args = owned(&["file"]);
        let tokens = tokenize_grammar(&args, &G);
        let _ = tokens[0].is_one_of(&[V, Flag::short("q")]);
    }

    #[test]
    fn a_spelling_declared_twice_is_reported() {
        assert_eq!(
            spelling_declared_twice(
                Dialect::Posix,
                &[
                    (TokenKind::Short, "c"),
                    (TokenKind::Long, "x"),
                    (TokenKind::Short, "c")
                ],
            ),
            Some((TokenKind::Short, "c"))
        );
        assert_eq!(
            spelling_declared_twice(
                Dialect::Msbuild,
                &[(TokenKind::Long, "nologo"), (TokenKind::Long, "NoLogo")],
            ),
            Some((TokenKind::Long, "NoLogo")),
            "MSBuild folds case"
        );
        // Case matters under getopt, and a short and a long spelling never collide.
        assert_eq!(
            spelling_declared_twice(
                Dialect::Posix,
                &[
                    (TokenKind::Short, "v"),
                    (TokenKind::Short, "V"),
                    (TokenKind::Long, "v"),
                ],
            ),
            None
        );
    }

    #[test]
    fn grammar_tables_are_searched_in_order_first_match_wins() {
        const PARENT: &[Flag] = &[Flag::short("c").takes(ValueSpec::solo_only())];
        const CHILD: &[Flag] = &[
            Flag::short("c").takes(ValueSpec::value()),
            Flag::short("p").takes(ValueSpec::value()),
        ];
        const PARENT_FIRST: Grammar = Grammar::new(Dialect::Posix, &[PARENT, CHILD]);
        const CHILD_FIRST: Grammar = Grammar::new(Dialect::Posix, &[CHILD, PARENT]);

        assert_eq!(
            PARENT_FIRST.takes_value(TokenKind::Short, "c"),
            Some(ValueSpec::solo_only())
        );
        assert_eq!(
            CHILD_FIRST.takes_value(TokenKind::Short, "c"),
            Some(ValueSpec::value())
        );
        // A flag only one table declares is found whichever table declares it.
        assert_eq!(
            PARENT_FIRST.takes_value(TokenKind::Short, "p"),
            Some(ValueSpec::value())
        );
    }

    #[test]
    fn a_folded_dialect_matches_names_case_insensitively() {
        const G: Grammar = Grammar::new(
            Dialect::Msbuild,
            &[&[Flag::atomic(&["results-directory"]).takes(ValueSpec::value())]],
        );
        for name in [
            "results-directory",
            "Results-Directory",
            "RESULTS-DIRECTORY",
        ] {
            assert_eq!(
                G.takes_value(TokenKind::Long, name),
                Some(ValueSpec::value()),
                "{name}"
            );
        }
        // Posix does not fold case.
        const POSIX: Grammar = Grammar::new(
            Dialect::Posix,
            &[&[Flag::long("grep").takes(ValueSpec::value())]],
        );
        assert_eq!(POSIX.takes_value(TokenKind::Long, "Grep"), None);
    }

    /// A grammar resolving abbreviations the way GNU `getopt_long` does, over a table where
    /// one name prefixes another (`all`/`almost-all`) and two share a stem (`ignore`/
    /// `ignore-backups`).
    const ABBREVIATING: Grammar = Grammar::new(
        Dialect {
            abbreviation: Abbreviation::UniquePrefix,
            ..Dialect::Posix
        },
        &[&[
            Flag::long("all"),
            Flag::long("almost-all"),
            Flag::pair("I", "ignore").takes(ValueSpec::value()),
            Flag::long("ignore-backups"),
            Flag::long("sort").takes(ValueSpec::value()),
        ]],
    );

    #[test]
    fn a_unique_prefix_names_the_one_flag_it_begins() {
        let sort = ABBREVIATING.flag(TokenKind::Long, "sort");
        assert!(sort.is_some());
        for prefix in ["s", "so", "sor"] {
            assert_eq!(ABBREVIATING.flag(TokenKind::Long, prefix), sort, "{prefix}");
        }
        assert_eq!(
            ABBREVIATING.flag(TokenKind::Long, "alm"),
            ABBREVIATING.flag(TokenKind::Long, "almost-all")
        );
        let args = owned(&["--sor", "time", "dir"]);
        let tokens = tokenize_grammar(&args, &ABBREVIATING);
        assert_eq!(tokens[0].value(&tokens), Some("time"));
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[2].kind, TokenKind::Positional);
    }

    #[test]
    fn a_whole_spelling_wins_over_a_longer_flag_it_prefixes() {
        assert_eq!(
            ABBREVIATING.flag(TokenKind::Long, "all"),
            Some(&ABBREVIATING.flags[0][0])
        );
        assert_eq!(
            ABBREVIATING.flag(TokenKind::Long, "ignore"),
            Some(&ABBREVIATING.flags[0][2])
        );
    }

    #[test]
    fn a_prefix_of_two_flags_names_neither() {
        for prefix in ["a", "al", "i", "ign", "ignore-"] {
            let expected = (prefix == "ignore-").then(|| &ABBREVIATING.flags[0][3]);
            assert_eq!(
                ABBREVIATING.flag(TokenKind::Long, prefix),
                expected,
                "{prefix}"
            );
        }
        // Unread, `--ign`'s neighbour stays a positional for the tool to reject.
        let args = owned(&["--ign", "beta*"]);
        let tokens = tokenize_grammar(&args, &ABBREVIATING);
        assert_eq!(tokens[1].kind, TokenKind::Positional);
    }

    #[test]
    fn abbreviation_never_applies_to_a_short_flag_or_an_empty_name() {
        assert_eq!(ABBREVIATING.flag(TokenKind::Short, "i"), None);
        assert_eq!(ABBREVIATING.flag(TokenKind::Long, ""), None);
        assert_eq!(ABBREVIATING.flag(TokenKind::Long, "sortx"), None);
    }

    #[test]
    fn the_name_lookups_read_a_resolved_prefix() {
        let args = owned(&["--alm", "--so", "time"]);
        let tokens = tokenize_grammar(&args, &ABBREVIATING);
        assert!(has_flag(&tokens, &ABBREVIATING, "almost-all"));
        assert!(has_double_dash_flag(&tokens, &ABBREVIATING, "sort"));
        assert_eq!(
            double_dash_flag_value(&tokens, &ABBREVIATING, "sort"),
            Some("time")
        );
        assert!(!has_flag(&tokens, &ABBREVIATING, "all"));
        // An atomic flag's other name is not the name typed under a preset.
        const MSBUILD: Grammar = Grammar::new(
            Dialect::Msbuild,
            &[&[Flag::atomic(&["c", "configuration"]).takes(ValueSpec::value())]],
        );
        let args = owned(&["-c", "Release"]);
        let tokens = tokenize_grammar(&args, &MSBUILD);
        assert!(has_flag(&tokens, &MSBUILD, "c"));
        assert!(!has_flag(&tokens, &MSBUILD, "configuration"));
        // Nor under prefix matching: `-c` resolves exactly, and abbreviates nothing.
        const ABBREVIATING_ATOMIC: Grammar = Grammar::new(
            Dialect {
                abbreviation: Abbreviation::UniquePrefix,
                ..Dialect::CommonsCli
            },
            &[&[Flag::atomic(&["c", "configuration"]).takes(ValueSpec::value())]],
        );
        let tokens = tokenize_grammar(&args, &ABBREVIATING_ATOMIC);
        assert!(has_flag(&tokens, &ABBREVIATING_ATOMIC, "c"));
        assert!(!has_flag(&tokens, &ABBREVIATING_ATOMIC, "configuration"));
        let args = owned(&["--conf", "Release"]);
        let tokens = tokenize_grammar(&args, &ABBREVIATING_ATOMIC);
        assert!(has_flag(&tokens, &ABBREVIATING_ATOMIC, "configuration"));
        assert!(!has_flag(&tokens, &ABBREVIATING_ATOMIC, "c"));
    }

    #[test]
    fn the_presets_never_abbreviate() {
        for dialect in [
            Dialect::Posix,
            Dialect::Msbuild,
            Dialect::CommonsCli,
            Dialect::GoFlag,
        ] {
            assert_eq!(dialect.abbreviation, Abbreviation::Never, "{dialect:?}");
        }
        const EXACT: Grammar = Grammar::new(
            Dialect::Posix,
            &[&[Flag::long("sort").takes(ValueSpec::value())]],
        );
        assert_eq!(EXACT.flag(TokenKind::Long, "sor"), None);
    }

    #[test]
    fn assert_takes_value_table_accepts_an_exact_table() {
        const G: Grammar = Grammar::new(
            Dialect::Posix,
            &[&[
                Flag::pair("n", "max-count").takes(ValueSpec::solo_only()),
                Flag::long("grep").takes(ValueSpec::value()),
            ]],
        );
        assert_takes_value_table(
            &G,
            &[
                (TokenKind::Short, &["n"], Some(ValueSpec::solo_only())),
                (
                    TokenKind::Long,
                    &["max-count", "grep"],
                    Some(ValueSpec::value()),
                ),
            ],
        );
    }

    #[test]
    fn assert_takes_value_table_checks_boolean_flags() {
        const G: Grammar = Grammar::new(
            Dialect::Posix,
            &[&[
                Flag::pair("v", "verbose"),
                Flag::pair("n", "max-count").takes(ValueSpec::solo_only()),
            ]],
        );
        assert_takes_value_table(
            &G,
            &[
                (TokenKind::Short, &["v"], None),
                (TokenKind::Long, &["verbose"], None),
                (TokenKind::Short, &["n"], Some(ValueSpec::solo_only())),
                (TokenKind::Long, &["max-count"], Some(ValueSpec::value())),
            ],
        );
    }

    #[test]
    #[should_panic(expected = "declared")]
    fn assert_takes_value_table_rejects_a_boolean_listed_as_undeclared() {
        // The table lists `-v` as a boolean flag the grammar does not declare.
        const G: Grammar = Grammar::new(Dialect::Posix, &[&[Flag::long("verbose")]]);
        assert_takes_value_table(
            &G,
            &[
                (TokenKind::Short, &["v"], None),
                (TokenKind::Long, &["verbose"], None),
            ],
        );
    }

    #[test]
    #[should_panic(expected = "declared but not in the table")]
    fn assert_takes_value_table_rejects_a_declared_flag_the_table_omits() {
        const G: Grammar = Grammar::new(
            Dialect::Posix,
            &[&[
                Flag::long("grep").takes(ValueSpec::value()),
                Flag::long("author").takes(ValueSpec::value()),
            ]],
        );
        assert_takes_value_table(
            &G,
            &[(TokenKind::Long, &["grep"], Some(ValueSpec::value()))],
        );
    }
}
