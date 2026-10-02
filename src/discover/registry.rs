//! Matches shell commands against known RTK rewrite rules to decide how to handle them.

use crate::cmds::system::search::{Engine, is_bare_file_list};
use crate::core::toml_filter::{command_matches_filter, is_rtk_reserved_command, toml_disabled};
use crate::core::utils::composer_bin_dirs;
use regex::{Regex, RegexSet};
use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::iter::once;
use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use super::families::{self, AFTER_USER_PREFIX, LayerKind};
use super::rules::{IGNORED_EXACT, IGNORED_PREFIXES, RULES, RtkRule};
use crate::core::arg_tokenizer::{self, TokenKind as ArgKind};
use crate::core::cmdline::bash_grammar::{ReservedWord, reserved_word};
use crate::core::cmdline::edit::{Edit, apply_edits};
use crate::core::cmdline::lexer::{
    CommandStart, CompoundSpan, Nesting, PipeKind, QuoteScan, Reading, SubstitutionDepth, Token,
    TokenKind, Word, ansi_c_quote_defeats_lexer, assignment_value, content_bounds, is_ifs,
    quotes_balanced, read_nesting, redirect_has_file_target, resolve_word_text, resolve_words,
    split_for_classify, split_ifs, squeeze_blanks, starts_with_grammar, tokenize, tokenize_trimmed,
    trim_ifs, trim_ifs_end, trim_ifs_start, words,
};
use crate::core::cmdline::rtk::rtk_invocation;
use crate::core::cmdline::walker::{self, Line, Peel, Position};
use crate::core::deferred;

const PHP_TOOL_NAMES: [&str; 6] = ["phpunit", "phpstan", "ecs", "pest", "paratest", "pint"];

/// Result of classifying a command.
#[derive(Debug, PartialEq)]
pub enum Classification {
    Supported {
        rtk_equivalent: &'static str,
        category: &'static str,
        estimated_savings_pct: f64,
        status: super::report::RtkStatus,
    },
    Unsupported {
        base_command: String,
    },
    Ignored,
}

/// Average token counts per category for estimation when no output_len available.
pub fn category_avg_tokens(category: &str, subcmd: &str) -> usize {
    match category {
        "Git" => match subcmd {
            "log" | "diff" | "show" => 200,
            _ => 40,
        },
        "Cargo" => match subcmd {
            "test" => 500,
            _ => 150,
        },
        "Tests" => 800,
        "Files" => 100,
        "Build" => 300,
        "Infra" => 120,
        "Network" => 150,
        "GitHub" => 200,
        "GitLab" => 200,
        "PackageManager" => 150,
        _ => 150,
    }
}

static REGEX_SET: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new(RULES.iter().map(|r| r.pattern)).expect("invalid regex patterns")
});
static COMPILED: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    RULES
        .iter()
        .map(|r| Regex::new(r.pattern).expect("invalid regex"))
        .collect()
});
// Git global options that appear before the subcommand: -C <path>, -c <key=val>,
// --git-dir <dir>, --work-tree <dir>, and flag-only options (#163)
static GIT_GLOBAL_OPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?:-C[ \t\n]+[^ \t\n]+|-c[ \t\n]+[^ \t\n]+|--git-dir(?:=[^ \t\n]+|[ \t\n]+[^ \t\n]+)|--work-tree(?:=[^ \t\n]+|[ \t\n]+[^ \t\n]+)|--no-pager|--no-optional-locks|--bare|--literal-pathspecs)[ \t\n]+)+").unwrap()
});
// Strip pnpm global options that precede the subcommand so `pnpm -r install`,
// `pnpm --filter @app install`, `pnpm -w list` route to the same rules as their
// bare forms. Only a fixed, known set is stripped — never an unknown `-x`, so a
// non-install flag-first command can't be mis-rewritten into a filter with savings.
static PNPM_GLOBAL_OPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?:-r|--recursive|-w|--workspace-root|--filter(?:=[^ \t\n]+|[ \t\n]+[^ \t\n]+)|-F(?:=[^ \t\n]+|[ \t\n]+[^ \t\n]+))[ \t\n]+)+").unwrap()
});
// Issue #1362: each capture expects a SINGLE file argument (`[^ \t\n]+$`). Multi-file
// invocations like `head -3 a b c` fail to match so the segment is passed through
// to the native `head`/`tail` binary — which already handles multi-file with
// `==> name <==` banners that a single `rtk read` window cannot reproduce.
static HEAD_N: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^head[ \t\n]+-(\d+)[ \t\n]+([^ \t\n]+)$").unwrap());
static HEAD_LINES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^head[ \t\n]+--lines=(\d+)[ \t\n]+([^ \t\n]+)$").unwrap());
// `-n N` and `--lines N` are the spellings `tail` already accepted below; `head`
// silently fell through to no rewrite at all without them.
static HEAD_N_SPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^head[ \t\n]+-n[ \t\n]+(\d+)[ \t\n]+([^ \t\n]+)$").unwrap());
static HEAD_LINES_SPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^head[ \t\n]+--lines[ \t\n]+(\d+)[ \t\n]+([^ \t\n]+)$").unwrap());
// Bare `head FILE` means ten lines, not the whole file. Restricted to a single
// operand: multi-file and optioned forms stay with the native binary.
static HEAD_BARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^head[ \t\n]+([^ \t\n]+)$").unwrap());

/// A rewritten head/tail command, with the space the trailing redirect kept
/// after it needs.
///
/// `head -n 1 f>out` leaves a redirect with no blank before it, and these
/// rewrites end in a bare number, so plain concatenation yields
/// `--head-lines 1>out` — which the shell reads as an fd-1 redirect, leaving
/// `--head-lines` with no value. A separating space keeps the flag intact.
///
/// Deliberately not applied to the shared prefix rewrites: those keep the
/// original argument/redirect boundary, where inserting a space could split a
/// descriptor-duplication form such as `1>&2` and demote the descriptor number
/// into an argument.
fn space_before_redirect(mut rewritten: String, redirect_suffix: &str) -> String {
    if !redirect_suffix.is_empty() && !redirect_suffix.starts_with(is_ifs) {
        rewritten.push(' ');
    }
    rewritten
}

/// Whether an operand is safe to hand to `rtk read` as one concrete file.
///
/// One whitespace-delimited token is not one shell operand: `*.rs`, `{a,b}` and
/// `$FILES` each expand to several at execution time, and `rtk read`
/// concatenates files where `head`/`tail` print `==> name <==` banners.
///
/// The token is raw shell source, not the argument the callee receives, so
/// character-class blocklists kept missing cases: `'--'` and `\--help` hide an
/// option behind a quote or backslash, and a leading `#` opens a comment that
/// swallows the flags this rewrite appends. Hence an allowlist: only characters
/// that cannot change the operand's word count or turn it into an option are
/// accepted, and anything else is left to the native binary. `~` is the one
/// shell-active character kept, because tilde expansion yields exactly one word
/// and the operand is passed through unchanged. Unicode alphanumerics stay
/// eligible so non-ASCII filenames still route.
///
/// The cost is deliberate: a quoted literal like `'literal*.txt'` also stays
/// native even though the quoting would prevent expansion. Losing a rewrite is
/// recoverable; changing what the user's command does is not.
fn is_single_file_operand(operand: &str) -> bool {
    if operand.is_empty() || operand.starts_with('-') {
        return false;
    }
    operand.chars().all(|c| {
        c.is_alphanumeric()
            || matches!(c, '.' | '_' | '/' | '-' | '~' | '+' | '@' | ':' | ',' | '=')
    })
}
static TAIL_N: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^tail[ \t\n]+-(\d+)[ \t\n]+([^ \t\n]+)$").unwrap());
static TAIL_N_SPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^tail[ \t\n]+-n[ \t\n]+(\d+)[ \t\n]+([^ \t\n]+)$").unwrap());
static TAIL_LINES_EQ: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^tail[ \t\n]+--lines=(\d+)[ \t\n]+([^ \t\n]+)$").unwrap());
static TAIL_LINES_SPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^tail[ \t\n]+--lines[ \t\n]+(\d+)[ \t\n]+([^ \t\n]+)$").unwrap());

const GOLANGCI_GLOBAL_OPT_WITH_VALUE: &[&str] = &[
    "-c",
    "--color",
    "--config",
    "--cpu-profile-path",
    "--mem-profile-path",
    "--trace-path",
];

#[derive(Debug, Clone, Copy)]
struct GolangciRunParts<'a> {
    global_segment: &'a str,
    run_segment: &'a str,
}

/// Classify a single (already-split) command, one that starts where a
/// pipeline does.
pub fn classify_command(cmd: &str) -> Classification {
    classify_command_at(cmd, CommandStart::Pipeline)
}

/// [`classify_command`] for a command that starts at `start`: a stage after
/// `|` reads `time` as the program `time`.
pub(crate) fn classify_command_at(cmd: &str, start: CommandStart) -> Classification {
    let tokens = tokenize(cmd);
    classify_words(cmd, &tokens, &words(cmd, &tokens), start)
}

/// Classify a single (already-split) command that a walk reached at `pos`:
/// behind an assignment, `time` is the program `time`.
///
/// The command is read past the reserved words bash runs a pipeline after
/// (`time` and its options, `then`, `do`, ...) and past its assignments and
/// `env` words, as the rewrite's walk reads them, so `time cargo test` is
/// `cargo test`.
fn classify_command_from(cmd: &str, mut pos: Position) -> Classification {
    let tokens = tokenize(cmd);
    let words = words(cmd, &tokens);
    let line = Line {
        text: cmd,
        words: &words,
        tokens: &tokens,
    };
    let mut at = 0;
    while let Some(peel) = walker::peel_reserved(&line, at, pos, LayerKind::ReservedWord)
        .or_else(|| walker::peel_assignments(&line, at, families::ALL, pos, LayerKind::Assignments))
    {
        at = peel.end;
        pos = peel.next;
    }
    classify_command_words(cmd, &tokens, &words[at..], pos)
}

/// Where a command that the walk reads at `pos` starts.
fn command_start(pos: Position) -> CommandStart {
    if pos.pipeline {
        CommandStart::Pipeline
    } else {
        CommandStart::PipeStage
    }
}

/// Whether `cmd` is one of the commands the report leaves out.
fn is_ignored(cmd: &str) -> bool {
    IGNORED_EXACT.contains(&cmd)
        || IGNORED_PREFIXES
            .iter()
            .any(|prefix| cmd.starts_with(prefix))
}

/// [`classify_command_at`] for the command made of `words`, spans of `text`,
/// lexed as `tokens`. It runs from the first word to the end of the last, so
/// an escaped blank that ends the last word stays in it.
fn classify_words(
    text: &str,
    tokens: &[Token<'_>],
    words: &[Word<'_>],
    start: CommandStart,
) -> Classification {
    let (Some(first), Some(last)) = (words.first(), words.last()) else {
        return Classification::Ignored;
    };
    let trimmed = &text[first.start..last.end];

    // A command that runs rtk is not an opportunity: the report counts it as
    // already rtk, unless it is `rtk proxy`.
    if rtk_invocation(words).is_some() {
        return Classification::Ignored;
    }

    // A segment that starts with a reserved word is grammar around commands,
    // not a command: `rtk until …` or `rtk esac` would break the construct.
    // Nor is one that starts with the `((` of an arithmetic command, which
    // runs no command. `read_grammar`'s rules decide both, so `[[(-f a)]]`
    // starts with `[[`, `"if"` is an ordinary word, `((ls) )` is two
    // subshells, and after `|` `time` is a program.
    if starts_with_grammar(tokens, start) {
        return Classification::Ignored;
    }

    if is_ignored(trimmed) {
        return Classification::Ignored;
    }

    // Strip env prefixes (env VAR=val, VAR=val); sudo is left untouched (#146)
    let cmd_clean = env_run_end(words).map_or(trimmed, |end| &text[end..last.end]);
    if cmd_clean.is_empty() {
        return Classification::Ignored;
    }

    classify_clean(cmd_clean)
}

/// [`classify_command_from`] for the command made of `words`, read past its
/// reserved words and assignments already, which stands at `pos`: the
/// rewrite's walk has done that reading when it decides about a command.
fn classify_command_words(
    text: &str,
    tokens: &[Token<'_>],
    words: &[Word<'_>],
    pos: Position,
) -> Classification {
    let (Some(first), Some(last)) = (words.first(), words.last()) else {
        return Classification::Ignored;
    };
    let cmd_clean = &text[first.start..last.end];

    // A command that runs rtk is not an opportunity.
    if rtk_invocation(words).is_some() {
        return Classification::Ignored;
    }

    // A command that starts with a reserved word the walk does not step over
    // is grammar around commands, not a command: `rtk until …` or `rtk esac`
    // would break the construct. Nor is one that starts with the `((` of an
    // arithmetic command, which runs no command. `read_grammar`'s rules
    // decide both, so `[[(-f a)]]` starts with `[[`, a quoted `"if"` is an
    // ordinary word, `((ls) )` is two subshells, and after `|` `time` is a
    // program. Behind an assignment no word is reserved. A command word that
    // is an option (`time -f %e cmd` runs a command named `-f`) is none.
    let from = tokens.partition_point(|tok| tok.offset < first.start);
    if (pos.simple_command && starts_with_grammar(&tokens[from..], command_start(pos)))
        || head_is_an_option(first.text)
    {
        return Classification::Ignored;
    }

    if is_ignored(cmd_clean) {
        return Classification::Ignored;
    }

    classify_clean(cmd_clean)
}

/// Classifies `cmd_clean`, a command read past what precedes its program,
/// against the rules.
fn classify_clean(cmd_clean: &str) -> Classification {
    // Normalize absolute binary paths: /usr/bin/grep → grep (#485)
    let cmd_normalized = strip_absolute_path(cmd_clean);
    // Strip git global options: git -C /tmp status → git status (#163)
    let cmd_normalized = strip_git_global_opts(&cmd_normalized);
    // Normalize PHP tool paths: vendor/bin/phpunit, bin/phpunit, or composer
    // custom bin-dir → phpunit (so one rule matches every Composer layout).
    let cmd_normalized = normalize_php_tool_command(&cmd_normalized);
    // Strip golangci-lint global options before `run` so classify/rewrite stays
    // aligned with the runtime wrapper behavior.
    let cmd_normalized = strip_golangci_global_opts(&cmd_normalized);
    // Strip pnpm global options (-r, --filter, -w) before the subcommand so
    // `pnpm -r install` classifies like `pnpm install` — but only adopt the
    // stripped form when it routes to the `rtk pnpm` rule itself. For the tool
    // rules reachable via `pnpm exec`/`pnpm run` (`pnpm -r exec vitest`,
    // `pnpm -r lint`, …) the rewrite matches the original flag-first text and
    // never fires, so classifying the stripped form there would report a
    // Supported saving the hook can't deliver — misleading `rtk discover` and
    // `rtk session`, which count `Supported` as covered. See #3275.
    let cmd_pnpm_stripped = strip_pnpm_global_opts(&cmd_normalized);
    let cmd_normalized =
        if cmd_pnpm_stripped != cmd_normalized && matches_pnpm_rule(&cmd_pnpm_stripped) {
            cmd_pnpm_stripped
        } else {
            cmd_normalized
        };
    let cmd_clean = cmd_normalized.as_str();

    // Exclude cat/head/tail with redirect operators — these are writes, not reads (#315)
    if matches!(split_ifs(cmd_clean).next(), Some("cat" | "head" | "tail")) {
        let has_redirect = split_ifs(cmd_clean)
            .skip(1)
            .any(|t| t.starts_with('>') || t == "<" || t.starts_with(">>"));
        if has_redirect {
            return Classification::Unsupported {
                base_command: split_ifs(cmd_clean).next().unwrap_or("cat").to_string(),
            };
        }
    }

    // Fast check with RegexSet — take the last (most specific) match
    let matches: Vec<usize> = REGEX_SET.matches(cmd_clean).into_iter().collect();
    if let Some(&idx) = matches.last() {
        let rule = &RULES[idx];

        // Extract subcommand for savings override and status detection
        let (savings, status) = if let Some(caps) = COMPILED[idx].captures(cmd_clean) {
            if let Some(sub) = caps.get(1) {
                // One space between words, so a two-word capture ("pm  ls")
                // still matches its single-spaced key in the tables below.
                let subcmd_owned = squeeze_blanks(sub.as_str());
                let subcmd = subcmd_owned.as_str();
                // Check if this subcommand has a special status
                let status = rule
                    .subcmd_status
                    .iter()
                    .find(|(s, _)| *s == subcmd)
                    .map(|(_, st)| *st)
                    .unwrap_or(super::report::RtkStatus::Existing);

                // A passthrough subcommand runs unfiltered, so it cannot save
                // anything. Deriving that from the status keeps the two from
                // drifting: a rule that marks a subcommand passthrough without
                // also zeroing its entry in `subcmd_savings` would otherwise
                // inherit the rule's headline percentage.
                let savings = if status == super::report::RtkStatus::Passthrough {
                    0.0
                } else {
                    rule.subcmd_savings
                        .iter()
                        .find(|(s, _)| *s == subcmd)
                        .map(|(_, pct)| *pct)
                        .unwrap_or(rule.savings_pct)
                };

                (savings, status)
            } else {
                (rule.savings_pct, super::report::RtkStatus::Existing)
            }
        } else {
            (rule.savings_pct, super::report::RtkStatus::Existing)
        };

        Classification::Supported {
            rtk_equivalent: rule.rtk_cmd,
            category: rule.category,
            estimated_savings_pct: savings,
            status,
        }
    } else {
        // Extract base command for unsupported
        let base = extract_base_command(cmd_clean);
        if base.is_empty() {
            Classification::Ignored
        } else {
            Classification::Unsupported {
                base_command: base.to_string(),
            }
        }
    }
}

/// Extract the base command (first word, or first two if it looks like a subcommand pattern).
fn extract_base_command(cmd: &str) -> &str {
    let parts: Vec<&str> = cmd.splitn(3, is_ifs).collect();
    match parts.len() {
        0 => "",
        1 => parts[0],
        _ => {
            let second = parts[1];
            // If the second token looks like a subcommand (no leading -)
            if !second.starts_with('-') && !second.contains('/') && !second.contains('.') {
                // Return "cmd subcmd"
                let end = cmd
                    .find(is_ifs)
                    .and_then(|i| {
                        let rest = &cmd[i..];
                        let trimmed = trim_ifs_start(rest);
                        trimmed
                            .find(is_ifs)
                            .map(|j| i + (rest.len() - trimmed.len()) + j)
                    })
                    .unwrap_or(cmd.len());
                &cmd[..end]
            } else {
                parts[0]
            }
        }
    }
}

/// Quote-aware heredoc detection — `<<` inside quotes is not a heredoc.
pub fn has_heredoc(cmd: &str) -> bool {
    tokens_have_heredoc(&tokenize(cmd))
}

fn tokens_have_heredoc(tokens: &[Token<'_>]) -> bool {
    tokens
        .iter()
        .any(|t| t.kind == TokenKind::Redirect && t.value.starts_with("<<"))
}

/// One command in a chain, with what a report needs to know about where it sat.
pub struct ChainPart<'a> {
    pub text: &'a str,
    /// Whether a `|` follows, so this command's output is consumed by the next
    /// one rather than read by the person who ran it.
    pub feeds_pipe: bool,
}

pub fn split_command_chain_parts(cmd: &str) -> Vec<ChainPart<'_>> {
    let trimmed = trim_ifs(cmd);
    if trimmed.is_empty() {
        return vec![];
    }

    // Lexer-based for `<<`; string-based for `$((` (lexer splits it across tokens).
    if has_heredoc(trimmed) || trimmed.contains("$((") {
        return vec![ChainPart {
            text: trimmed,
            feeds_pipe: false,
        }];
    }

    // Every stage, not just the one before the first `|`. A pipeline's later
    // stages are commands the agent ran, and stopping at the pipe made them
    // invisible to classification — `rtk discover` could not report what it
    // never looked at (#3683).
    split_for_classify(trimmed)
        .into_iter()
        .map(|s| ChainPart {
            text: s.text,
            feeds_pipe: s.feeds_pipe,
        })
        .collect()
}

pub fn split_command_chain(cmd: &str) -> Vec<&str> {
    split_command_chain_parts(cmd)
        .into_iter()
        .map(|p| p.text)
        .collect()
}

/// Which commands of `full` the rewriter acts on, one flag per
/// [`split_command_chain`] part.
///
/// Put to the rewriter rather than re-derived from `PipelineSafety`, so the two
/// cannot drift: a second copy of the rule is a second thing to keep in step.
/// Answered for the whole line at once, because what happens to one command
/// depends on the ones around it.
///
/// Position decides it. `tail -1` alone becomes `rtk read`, but in
/// `cargo test 2>&1 | tail -1` the rewriter takes the producer and leaves the
/// consumer alone — RTK filters what `cargo` writes, and rewriting the stage
/// that reads it would change what the pipeline prints. In
/// `cargo test | grep FAILED` it is the other way round, because `rtk grep` is
/// one of the rules that are safe to run at the end of a pipeline. A command
/// the rewriter passes over offers nothing to save.
pub(crate) fn stages_the_rewriter_reaches(
    full: &str,
    excluded: &[ExcludePattern],
    normalized_prefixes: &[String],
) -> Vec<bool> {
    let before = split_command_chain(full);
    let Some(rewritten) = rewrite_command_precompiled(full, excluded, normalized_prefixes) else {
        return vec![false; before.len()];
    };
    let after = split_command_chain(&rewritten);
    // A rewrite substitutes commands and keeps every separator, so the chains
    // line up command for command. If they ever do not, nothing can be located
    // and claiming a saving would be a guess.
    if before.len() != after.len() {
        return vec![false; before.len()];
    }
    before.iter().zip(after).map(|(b, a)| *b != a).collect()
}

fn normalize_php_tool_command(cmd: &str) -> String {
    normalize_php_tool_command_with_dirs(cmd, &composer_bin_dirs())
}

/// Peel a leading `php` interpreter wrapper off a Composer-tool invocation
/// (`php vendor/bin/phpunit …` → `vendor/bin/phpunit …`) so the tool path
/// normalizes to its bare name. Only meaningful for the resolved tools, where
/// a `php` prefix is always the interpreter (never `php artisan`/`run-tests.php`).
fn strip_php_wrapper(cmd: &str) -> &str {
    cmd.strip_prefix("php ").map_or(cmd, trim_ifs_start)
}

fn normalize_php_tool_command_with_dirs(cmd: &str, bin_dirs: &[std::path::PathBuf]) -> String {
    let first_space = cmd.find(is_ifs);
    let first_word = match first_space {
        Some(pos) => &cmd[..pos],
        None => cmd,
    };

    let Some(tool) = normalize_php_tool_word(first_word, bin_dirs) else {
        return cmd.to_string();
    };

    match first_space {
        Some(pos) => format!("{}{}", tool, &cmd[pos..]),
        None => tool.to_string(),
    }
}

fn normalize_php_tool_word<'a>(word: &str, bin_dirs: &'a [std::path::PathBuf]) -> Option<&'a str> {
    let normalized_word = normalize_php_tool_path(word);

    for tool in PHP_TOOL_NAMES {
        if normalized_word == tool {
            return Some(tool);
        }

        if bin_dirs
            .iter()
            .any(|bin_dir| matches_php_tool_path(&normalized_word, bin_dir, tool))
        {
            return Some(tool);
        }
    }

    None
}

fn matches_php_tool_path(word: &str, bin_dir: &Path, tool: &str) -> bool {
    let normalized_dir = normalize_php_tool_path(&bin_dir.to_string_lossy());
    let candidate = format!("{normalized_dir}/{tool}");
    word == candidate || word.ends_with(&format!("/{candidate}"))
}

fn normalize_php_tool_path(path: &str) -> String {
    let mut normalized = trim_ifs(path).replace('\\', "/");
    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }

    if let Some((stem, ext)) = normalized.rsplit_once('.')
        && ["bat", "cmd", "exe", "ps1"]
            .iter()
            .any(|candidate| ext.eq_ignore_ascii_case(candidate))
    {
        normalized = stem.to_string();
    }

    normalized
}

/// Strip git global options before the subcommand (#163).
/// `git -C /tmp status` → `git status`, preserving the rest.
/// Returns the original string unchanged if not a git command.
fn strip_git_global_opts(cmd: &str) -> String {
    // Only applies to commands starting with "git "
    if !cmd.starts_with("git ") {
        return cmd.to_string();
    }
    let after_git = &cmd[4..]; // skip "git "
    let stripped = GIT_GLOBAL_OPT.replace(after_git, "");
    format!("git {}", trim_ifs(&stripped))
}

/// Strip pnpm global options before the subcommand (mirror of `strip_git_global_opts`).
/// `pnpm -r install` → `pnpm install`; `pnpm --filter @app list` → `pnpm list`.
/// Classification only — the rewrite re-emits the ORIGINAL command, so the stripped
/// flags are preserved (e.g. `pnpm -r install` → `rtk pnpm -r install`).
/// Returns the original string unchanged if not a pnpm command.
fn strip_pnpm_global_opts(cmd: &str) -> String {
    // Gate on a literal `pnpm ` so that the form classify adopts is one the
    // rewrite also matches: `pnpm\t-r install` is left alone here and stays
    // Unsupported on both sides, rather than being classified `rtk pnpm` on a
    // shape the rewrite declines (#3275). Extra spaces are still tolerated via
    // `trim_ifs_start` (`pnpm  -r  install`), since `PNPM_GLOBAL_OPT` is
    // `^`-anchored and a leading space would skip the strip.
    if !cmd.starts_with("pnpm ") {
        return cmd.to_string();
    }
    let after_pnpm = trim_ifs_start(&cmd[5..]); // skip "pnpm ", then any extra blanks
    let stripped = PNPM_GLOBAL_OPT.replace(after_pnpm, "");
    format!("pnpm {}", trim_ifs(&stripped))
}

/// True when `cmd` (already normalized) routes to the `rtk pnpm` rule rather than
/// a tool rule reachable through `pnpm exec`/`pnpm run`. Gates the pnpm
/// global-option strip in `classify_command`: adopting the stripped form for a
/// tool rule would diverge from the rewrite, which matches the original
/// flag-first text and never fires there. See #3275.
fn matches_pnpm_rule(cmd: &str) -> bool {
    REGEX_SET
        .matches(cmd)
        .into_iter()
        .next_back()
        .is_some_and(|idx| RULES[idx].rtk_cmd == "rtk pnpm")
}

/// Strip golangci-lint global options before the `run` subcommand.
/// `golangci-lint --color never run ./...` → `golangci-lint run ./...`
/// Returns the original string unchanged if this is not a supported compact `run` invocation.
fn strip_golangci_global_opts(cmd: &str) -> String {
    match parse_golangci_run_parts(cmd) {
        Some(parts) => format!("golangci-lint {}", parts.run_segment),
        None => cmd.to_string(),
    }
}

/// Parse supported golangci-lint invocations with optional global flags before `run`.
fn parse_golangci_run_parts(cmd: &str) -> Option<GolangciRunParts<'_>> {
    golangci_run_parts(cmd, &words(cmd, &tokenize(cmd)))
}

/// [`parse_golangci_run_parts`] over the words of `text`, already found. It
/// reads words, not tokens: an unquoted glob like `*.yml` is one word however
/// many tokens it spans.
fn golangci_run_parts<'a>(text: &'a str, words: &[Word<'a>]) -> Option<GolangciRunParts<'a>> {
    let first = words.first()?.text;
    if first != "golangci-lint" && first != "golangci" {
        return None;
    }

    let mut i = 1;
    while i < words.len() {
        let word = words[i].text;

        if word == "--" {
            return None;
        }

        if !word.starts_with('-') {
            if word == "run" {
                // Each segment ends where its last word does, so an escaped
                // blank there stays in it.
                let global_segment = if i > 1 {
                    &text[words[1].start..words[i - 1].end]
                } else {
                    ""
                };
                let run_segment = &text[words[i].start..words[words.len() - 1].end];
                return Some(GolangciRunParts {
                    global_segment,
                    run_segment,
                });
            }
            return None;
        }

        if let Some(flag) = split_golangci_flag_name(word)
            && golangci_flag_takes_separate_value(word, flag)
        {
            i += 1;
        }

        i += 1;
    }

    None
}

fn split_golangci_flag_name(arg: &str) -> Option<&str> {
    if arg.starts_with("--") {
        return Some(arg.split_once('=').map(|(flag, _)| flag).unwrap_or(arg));
    }

    if arg.starts_with('-') {
        return Some(arg);
    }

    None
}

fn golangci_flag_takes_separate_value(arg: &str, flag: &str) -> bool {
    if !GOLANGCI_GLOBAL_OPT_WITH_VALUE.contains(&flag) {
        return false;
    }

    if arg.starts_with("--") && arg.contains('=') {
        return false;
    }

    true
}

/// Normalize absolute binary paths: `/usr/bin/grep -rn foo` → `grep -rn foo` (#485)
/// Only strips if the first word contains a `/` (Unix path).
fn strip_absolute_path(cmd: &str) -> String {
    let first_space = cmd.find(is_ifs);
    let first_word = match first_space {
        Some(pos) => &cmd[..pos],
        None => cmd,
    };
    if first_word.contains('/') {
        // Extract basename
        let basename = first_word.rsplit('/').next().unwrap_or(first_word);
        if basename.is_empty() {
            return cmd.to_string();
        }
        match first_space {
            Some(pos) => format!("{}{}", basename, &cmd[pos..]),
            None => basename.to_string(),
        }
    } else {
        cmd.to_string()
    }
}

/// What an assignment spells to bypass the rewrite (#345).
const RTK_DISABLED_MARKER: &str = "RTK_DISABLED=";
/// The same assignment spelled as an append.
const RTK_DISABLED_APPEND_MARKER: &str = "RTK_DISABLED+=";

/// Whether one of `words`, a run of assignments and `env` words, assigns
/// `RTK_DISABLED`, by that exact name: `XRTK_DISABLED=1` and
/// `MSG='RTK_DISABLED=1'` are not the bypass.
fn words_set_rtk_disabled(words: &[Word<'_>]) -> bool {
    // Behind a run word (`env`) the words are that program's operands: it sets
    // a variable named `RTK_DISABLED+`, and does not append.
    //
    // The words are read as written, where a wrapper's head is read with its
    // quotes removed: `env 'RTK_DISABLED=1' git status` really sets the
    // variable, but the walk ends at `env` (a quoted operand is no assignment
    // word to it), the line is left as written and no warning is printed.
    // Reading the operands unquoted would peel `env` and rewrite the command
    // behind it, which is left as written today, so the asymmetry stays.
    let mut shell_assignments = true;
    for word in words {
        if word.text.starts_with(RTK_DISABLED_MARKER)
            || (shell_assignments && word.text.starts_with(RTK_DISABLED_APPEND_MARKER))
        {
            return true;
        }
        shell_assignments &= walker::is_assignment_word(word.text);
    }
    false
}

pub fn prefix_contains_rtk_disabled(prefix_part: &str) -> bool {
    prefix_part.contains(RTK_DISABLED_MARKER)
}

/// Check if a command has RTK_DISABLED= prefix in its env prefix portion.
pub fn cmd_has_rtk_disabled_prefix(cmd: &str) -> bool {
    let (prefix_part, _) = split_env_prefix(cmd);
    prefix_contains_rtk_disabled(prefix_part)
}

/// Split the leading `env` and `NAME=value` assignments off a command, as
/// `(prefix, command)`. [`env_run_end`] decides where they end.
///
/// `sudo` is deliberately not stripped. `sudo rtk docker ps` fails at runtime
/// because `rtk` is not on root's `secure_path`, and where it is, it would run
/// rtk as root (#146).
pub fn split_env_prefix(cmd: &str) -> (&str, &str) {
    let words = words(cmd, &tokenize(cmd));
    let (Some(first), Some(last)) = (words.first(), words.last()) else {
        return ("", "");
    };
    // Both halves end where a word does, so a trailing escaped blank (`f\ `)
    // stays in the command.
    match env_run_end(&words) {
        // Up to where the command starts, not where the last assignment ends, so
        // that whoever puts the two back together need not know what separated them.
        Some(end) => (&cmd[first.start..end], &cmd[end..last.end]),
        None => ("", &cmd[first.start..last.end]),
    }
}

/// Whether `word` is `env` or an assignment word, in bash's own sense (see
/// [`assignment_value`]). The value is whatever the rest of the word is,
/// since [`words`] has already decided where the word ends, quotes included.
fn is_env_word(word: &str) -> bool {
    word == "env" || assignment_value(word).is_some()
}

/// Where the run of `env` and `NAME=value` words that opens `words` ends: at
/// the start of the first word after it, or at the end of the last word. `None`
/// when `words` does not open with one.
///
/// By words, because a quoted value is one word: `D='# shellcheck disable=SC2034'`
/// is an assignment whole, and the `shellcheck` inside it is not a command. A
/// pattern cannot hold that line — its alternation for the value backtracks into
/// the quotes as soon as the quoted form is not followed by a blank, which is
/// what happens at the end of a line or before a `;`, and the rewrite then edits
/// inside the literal (#3262).
fn env_run_end(words: &[Word<'_>]) -> Option<usize> {
    let (first, rest) = words.split_first()?;
    if !is_env_word(first.text) {
        return None;
    }
    Some(match rest.iter().find(|word| !is_env_word(word.text)) {
        Some(word) => word.start,
        None => words[words.len() - 1].end,
    })
}

/// Matches a bash line-continuation: a backslash immediately followed by
/// `\n` or `\r\n`, *plus* any horizontal whitespace on the line before AND
/// after the break. This is what bash already collapses to a single space
/// before executing the command — rtk's hook matcher needs to do the same
/// so commands authored across multiple lines still hit the rewrite rules.
/// Consuming the trailing whitespace prevents double spaces in cases like
/// `git diff \<NL>HEAD~1`.
static LINE_CONTINUATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)[ \t\x0B\x0C]*\\\r?\n[ \t\x0B\x0C]*").unwrap());

static BASH_JOIN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\\r?\n").unwrap());

/// Replace every bash line continuation with a single space, mirroring what
/// bash does before dispatching the command. Returns a borrowed `&str` when the
/// input contains no continuations, so the common fast path allocates nothing.
fn collapse_line_continuations(s: &str) -> Cow<'_, str> {
    LINE_CONTINUATION_RE.replace_all(s, " ")
}

/// Returns `None` if the command is unsupported or ignored (hook should pass through).
///
/// Handles compound commands (`&&`, `||`, `;`) by rewriting each segment independently.
/// For pipelines, preserves intermediate stages and only rewrites a pipeline-safe final stage,
/// then continues rewriting segments after subsequent `&&`/`||`/`;` operators.
/// Also peels user-configured transparent wrapper prefixes
/// (`[hooks].transparent_prefixes` in `config.toml`) before routing.
///
/// A transparent prefix is a wrapper command that doesn't change *what* is
/// being run, only *how* it's run — e.g. `docker exec mycontainer`,
/// `direnv exec .`, `poetry run`, or `bundle exec`. Peeling it lets the inner
/// command match a filter, and only that inner command is edited, so the
/// prefix stays as written. The built-in wrapper [`families`] are always
/// applied in addition to user-configured prefixes.
///
/// Matching is strict: a configured prefix `"foo bar"` matches a command whose
/// words start with `foo bar` (or that strictly equals it), not anything else.
/// Matching is literal, not pattern-based: configure the exact concrete prefix
/// you use.
///
/// What it says on stderr comes in this order: #508's `RTK_DISABLED` warning,
/// printed exactly when a walk of the rewrite stopped at the prefix and the
/// command was refused, then the configuration's own warnings, then whatever
/// the filter registry says about trust. Those are held back while the rewrite
/// runs ([`deferred::capture`]), since the first needs the rewrite's answer.
/// `rewrite_command_precompiled` also serves `rtk discover`, which asks about
/// commands from months of history and prints none of it.
pub fn rewrite_command(
    cmd: &str,
    excluded: &[String],
    transparent_prefixes: &[String],
) -> Option<String> {
    let (answer, held) = deferred::capture(|| {
        let compiled = compile_exclude_patterns(excluded);
        let normalized_prefixes = normalize_transparent_prefixes(transparent_prefixes);
        rewrite_line(cmd, &compiled, &normalized_prefixes)
    });
    if answer.disabled {
        eprintln!(
            "[rtk] warning: RTK_DISABLED=1 detected — skipping filter for this command. \
             Remove RTK_DISABLED=1 to restore token savings."
        );
    }
    for warning in held {
        eprintln!("{warning}");
    }
    answer.text
}

/// What the rewrite of a line answers: the rewritten text, if any, and
/// whether a walk that decided about one of its commands stopped at
/// `RTK_DISABLED=` and the command was refused (#345), which #508 warns
/// about.
#[derive(Default)]
struct Answer {
    text: Option<String>,
    disabled: bool,
}

/// Core of `rewrite_command`, taking already-compiled exclude patterns and
/// already-normalized transparent prefixes so a caller checking many commands
/// against the same config in a loop can compile once and reuse — instead of
/// recompiling `exclude_commands` regexes on every single call. `rewrite_command`
/// itself is the right entry point for a one-off check (real hook invocations,
/// `rtk rewrite`, tests); this exists for `rtk discover`'s estimate-coverage
/// fallback, which calls this once per historical command scanned (the same
/// compile-once-per-run pattern this PR already applies to permission rules —
/// see `discover::PermissionRules` — and hook-install status).
pub(crate) fn rewrite_command_precompiled(
    cmd: &str,
    compiled: &[ExcludePattern],
    normalized_prefixes: &[String],
) -> Option<String> {
    rewrite_line(cmd, compiled, normalized_prefixes).text
}

/// The refusals every rewrite starts with, then the rewrite of the line.
fn rewrite_line(cmd: &str, compiled: &[ExcludePattern], normalized_prefixes: &[String]) -> Answer {
    // Bash joins `\<NL>` with nothing, so `<<` or `$((` can arrive split across
    // a continuation; the space-join below would erase them (#3188 review).
    if let Cow::Owned(joined) = BASH_JOIN_RE.replace_all(cmd, "")
        && (has_heredoc(&joined) || joined.contains("$(("))
    {
        return Answer::default();
    }

    // The pre-pass runs before the blanks at either end are left out, so
    // nothing it leaves sits in front of the command, where it would hide the
    // command from every rule (#1564).
    let normalized = collapse_line_continuations(cmd);
    let line = CompoundLex::new(&normalized);
    if line.text.is_empty() || line.has_heredoc() || line.text.contains("$((") {
        return Answer::default();
    }

    let mut rewriter = Rewriter::new(compiled, normalized_prefixes);
    let text = if line.text.contains('\n') {
        rewrite_multiline_block(&line, &mut rewriter)
    } else {
        rewrite_single(&line, &mut rewriter)
    };
    Answer {
        text,
        disabled: rewriter.disabled,
    }
}

/// What the rewrite of one line carries from command to command: the
/// configuration it runs under, the edits made so far, and whether a command
/// was refused on `RTK_DISABLED=` ([`Answer::disabled`]).
struct Rewriter<'c> {
    excluded: &'c [ExcludePattern],
    prefixes: &'c [String],
    edits: Vec<Edit>,
    disabled: bool,
}

impl<'c> Rewriter<'c> {
    fn new(excluded: &'c [ExcludePattern], prefixes: &'c [String]) -> Self {
        Self {
            excluded,
            prefixes,
            edits: Vec::new(),
            disabled: false,
        }
    }

    /// The command in `line.tokens[range]`, rewritten in `context`: its edit,
    /// if any, is recorded, and so is a refusal on `RTK_DISABLED=`. Says
    /// whether it was rewritten.
    ///
    /// A command inside a compound command whose parts are command lists, in
    /// any of them, writes the compound's output, or reads its input: when a
    /// pipe joins the compound to another command, the command is left as
    /// written. `kept` says that for each token of the line
    /// ([`kept_across_lines`]).
    fn segment(
        &mut self,
        line: &Slice<'_, '_>,
        range: Range<usize>,
        context: RewriteContext,
        kept: &[bool],
    ) -> bool {
        if kept
            .get(range.clone())
            .is_some_and(|kept| kept.contains(&true))
        {
            return false;
        }
        let Some(slice) = Slice::new(line.text, &line.tokens[range]) else {
            return false;
        };
        let outcome = rewrite_segment(slice, self.excluded, self.prefixes, context);
        self.disabled |= outcome.disabled;
        match outcome.edit {
            Some(edit) => {
                self.edits.push(edit);
                true
            }
            None => false,
        }
    }

    /// The text the recorded edits make of `text`, if they make any change.
    fn apply(&self, text: &str) -> Option<String> {
        (!self.edits.is_empty())
            .then(|| apply_edits(text, &self.edits))
            .flatten()
    }
}

/// Whether `line` is one simple command that runs rtk: [`rtk_invocation`]
/// reads its words, and no unquoted `&&`, `||`, `;`, `|` or `&` joins another
/// command to it. A compound line that starts with `rtk`
/// (`rtk git add . && cargo test`) is not: its other commands get rewritten.
fn already_rtk(line: &Slice<'_, '_>) -> bool {
    let has_compound = line.tokens.iter().any(|token| match token.kind {
        TokenKind::Operator | TokenKind::Pipe(_) => true,
        TokenKind::Shellism => token.value == "&",
        _ => false,
    });
    !has_compound && rtk_invocation(&line.words()).is_some()
}

/// Rewrite one logical command line (no unquoted newlines). A line that
/// already runs through rtk comes back as it is.
fn rewrite_single(line: &CompoundLex<'_>, rewriter: &mut Rewriter<'_>) -> Option<String> {
    let whole = line.whole()?;
    if already_rtk(&whole) {
        return Some(line.text.to_string());
    }
    rewrite_compound(whole, rewriter);
    rewriter.apply(line.text)
}

/// Byte offset where an unquoted `#` at the start of a word begins a trailing
/// comment, if any. The lexer has no comment state, so the independence checks
/// must ignore comment text themselves: `git log | # keep pipeline` continues
/// the pipeline across the newline even though the line ends in comment text.
fn comment_start(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    // `#` starts a comment at any word start, incl. after an operator
    // byte — but not after `{`: `${#var}` is an expansion (#3188 review).
    QuoteScan::new(line).significant().find_map(|c| {
        (c.byte == b'#'
            && !c.in_single
            && !c.in_double
            && (c.index == 0
                || is_ifs(char::from(bytes[c.index - 1]))
                || matches!(bytes[c.index - 1], b'|' | b'&' | b';' | b'(' | b')')))
        .then_some(c.index)
    })
}

/// Unquoted `(`/`)` or `{`/`}` that don't balance within the line: an array
/// literal (`arr=(one`), function body (`foo() {`), or group spans lines, so
/// the lines around it are not independent commands.
fn line_has_unbalanced_grouping(code: &str) -> bool {
    let mut paren = 0i32;
    let mut brace = 0i32;
    for c in QuoteScan::new(code).significant() {
        if c.in_single || c.in_double {
            continue;
        }
        match c.byte {
            b'(' => paren += 1,
            b')' => paren -= 1,
            b'{' => brace += 1,
            b'}' => brace -= 1,
            _ => {}
        }
        if paren < 0 || brace < 0 {
            return true;
        }
    }
    paren != 0 || brace != 0
}

/// Unquoted `[[` / `]]` words that don't balance within the line: bash allows
/// a conditional expression to span lines (`[[ -f a &&` / `-f b ]]`), so the
/// surrounding lines are not independent commands.
fn line_has_unbalanced_test_brackets(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut depth = 0i32;
    for c in QuoteScan::new(code).significant() {
        if c.in_single || c.in_double || !matches!(c.byte, b'[' | b']') {
            continue;
        }
        let i = c.index;
        let word_start = i == 0 || is_ifs(char::from(bytes[i - 1]));
        let word_end = bytes.get(i + 2).is_none_or(|&b| is_ifs(char::from(b)));
        if bytes.get(i + 1) == Some(&c.byte) && word_start && word_end {
            depth += if c.byte == b'[' { 1 } else { -1 };
            if depth < 0 {
                return true;
            }
        }
    }
    depth != 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineRole {
    Passive,
    Independent,
    ContinuesNext,
    Unsafe,
}

fn classify_line(line: &str) -> LineRole {
    if line.is_empty() || line.starts_with('#') {
        return LineRole::Passive;
    }
    let comment = comment_start(line);
    let code = comment.map_or(line, |i| trim_ifs_end(&line[..i]));
    let first = split_ifs(code).next().unwrap_or("");
    // A line inside a loop, conditional, case arm, function body, group or
    // subshell is not an independent command, so the whole block passes
    // through untouched.
    if matches!(first, "(" | ")")
        || reserved_word(first).is_some_and(ReservedWord::delimits_command_list)
    {
        return LineRole::Unsafe;
    }
    const CONTINUATION_OPS: [&str; 4] = ["&&", "||", "|&", "|"];
    if CONTINUATION_OPS.iter().any(|op| code.starts_with(op))
        || code.starts_with("((")
        || code.ends_with("))")
        || line_has_unbalanced_grouping(code)
        || line_has_unbalanced_test_brackets(code)
    {
        return LineRole::Unsafe;
    }
    if CONTINUATION_OPS.iter().any(|op| code.ends_with(op)) {
        // An operator behind a trailing comment can't be joined textually:
        // the comment-blind tokenizer would read the comment as command words.
        return if comment.is_some() {
            LineRole::Unsafe
        } else {
            LineRole::ContinuesNext
        };
    }
    LineRole::Independent
}

/// Rewrite each line of a multi-line block independently (issue #1243).
///
/// Split points are the newline tokens the quote-aware lexer emits, so a
/// newline inside a quoted string (e.g. a multi-line commit message) never
/// becomes a boundary. Lines continued by a trailing `&&`/`||`/`|`/`|&` are
/// joined and rewritten as one logical command, the newlines and blank lines
/// inside it kept as written; any line [`classify_line`] marks unsafe passes
/// the whole block through. Blank lines are preserved verbatim, as is
/// indentation. A line ends at its `\n` only: the `\r` of a CRLF is the last
/// byte of the line's last word, as bash reads it.
///
/// A compound command that one line opens and a later one closes holds the
/// lines between as part of its lists: when a pipe joins it to another
/// command, before its opening word or after its closing one, the commands
/// on all of those lines are left as written, as on one line
/// ([`Rewriter::segment`]).
///
/// If any newline byte was swallowed by quote state, the block passes through
/// untouched. The lexer has no comment awareness, so an apostrophe in a `#`
/// comment opens quote state and hides the rest of the block — rewriting (or
/// prefixing) such a block would act on lines no permission verdict was
/// computed for. Passthrough hands the original command to the agent's native
/// permission handling instead. Genuine quoted newlines (multi-line commit
/// messages) also land here; forgoing that rewrite is the safe trade.
fn rewrite_multiline_block(block: &CompoundLex<'_>, rewriter: &mut Rewriter<'_>) -> Option<String> {
    let cmd = block.text;
    let tokens = &block.tokens;
    if ansi_c_quote_defeats_lexer(cmd) {
        return None;
    }

    let newlines: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, tok)| tok.kind == TokenKind::Newline)
        .map(|(i, _)| i)
        .collect();

    // The lexer emits a `Newline` for each `\n` it reads as syntax, so a
    // difference in count is a newline swallowed by quote state or an escape.
    let raw_breaks = cmd.bytes().filter(|&b| b == b'\n').count();
    if raw_breaks != newlines.len() {
        // Every newline swallowed by quote state with quotes balanced at EOF
        // is one logical command (a multi-line commit message), not a hidden
        // extra line; rewrite it whole, as develop always did (#3319 fuzz).
        if newlines.is_empty() && quotes_balanced(cmd) {
            return rewrite_single(block, rewriter);
        }
        return None;
    }

    // Each line is the run of tokens between the newlines around it.
    let lines: Vec<Range<usize>> = once(0)
        .chain(newlines.iter().map(|&i| i + 1))
        .zip(newlines.iter().copied().chain(once(tokens.len())))
        .map(|(from, to)| from..to)
        .collect();
    let line_text = |line: &Range<usize>| {
        if line.is_empty() {
            ""
        } else {
            &cmd[tokens[line.start].offset..tokens[line.end - 1].end()]
        }
    };

    let roles: Vec<LineRole> = lines
        .iter()
        .map(|line| classify_line(trim_ifs(line_text(line))))
        .collect();
    if roles.contains(&LineRole::Unsafe) {
        return None;
    }

    // Each unit, a line with the lines its operators join to it, is read
    // first: a compound command it opens or closes may span others.
    let mut units: Vec<(Range<usize>, LineRead)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if roles[i] == LineRole::Passive {
            i += 1;
            continue;
        }

        let mut end = i;
        while roles[end] == LineRole::ContinuesNext {
            let mut next = end + 1;
            while next < lines.len() && trim_ifs(line_text(&lines[next])).is_empty() {
                next += 1;
            }
            if next >= lines.len() {
                break;
            }
            if roles[next] == LineRole::Passive {
                // Comment line inside a continuation: the comment-blind
                // tokenizer would join it as command words (#3188 review).
                return None;
            }
            end = next;
        }

        let range = lines[i].start..lines[end].end;
        if let Some((first, last)) = content_bounds(&tokens[range.clone()]) {
            let unit = range.start + first..range.start + last + 1;
            if let Some(slice) = Slice::new(cmd, &tokens[unit.clone()])
                && !already_rtk(&slice)
            {
                units.push((unit, LineRead::new(&slice)));
            }
        }
        i = end + 1;
    }

    let kept = kept_across_lines(tokens.len(), &units);
    for (unit, read) in &units {
        if let Some(slice) = Slice::new(cmd, &tokens[unit.clone()]) {
            rewrite_read(slice, read, &kept[unit.clone()], rewriter);
        }
    }
    rewriter.apply(cmd)
}

/// For each of the `len` tokens of a line or a block, whether it sits in a
/// compound command that is a pipeline's stage, from the opening word to the
/// closing one, given each of its `units` (their token ranges; a line is one)
/// and how it reads: such a compound writes its output into a pipe, or reads
/// its input from one, and a command in any of its lists (a `case` arm's
/// included) is left as written. A pipeline inside one of its lists is no
/// reason: it is read like any other. A compound command a unit leaves open
/// is closed by the first unit after it that closes one it did not open, as
/// bash pairs them; one left open at the end, or closed with none open,
/// counts as joined by a pipe when any unit holds one.
fn kept_across_lines(len: usize, units: &[(Range<usize>, LineRead)]) -> Vec<bool> {
    let piped = units.iter().any(|(_, read)| read.piped);
    let mut kept = vec![false; len];
    let mut mark = |from: usize, to: usize, joined: bool| {
        if joined {
            kept[from..to].iter_mut().for_each(|k| *k = true);
        }
    };
    // The compound commands left open so far: where each opens, in the
    // block, and whether a pipe comes before it.
    let mut open: Vec<(usize, bool)> = Vec::new();
    for (unit, read) in units {
        let at = |i: usize| unit.start + i;
        for span in &read.compounds {
            match (span.open, span.close) {
                (Some(o), Some(c)) => mark(at(o), at(c), span.piped),
                (None, Some(c)) => match open.pop() {
                    Some((o, before)) => mark(o, at(c), before || span.piped),
                    None => mark(0, at(c), span.piped || piped),
                },
                (Some(o), None) => open.push((at(o), span.piped)),
                (None, None) => {}
            }
        }
    }
    for (o, before) in open {
        mark(o, len, before || piped);
    }
    kept
}

/// Where a pipeline's stages are, as indices into its line's tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PipelineAnalysis {
    /// Where the pipeline ends: at `next_clause`, or at the end of the line.
    end: usize,
    /// The operator or background `&` that follows the pipeline.
    next_clause: Option<usize>,
    /// Where the last stage starts, when every stage holds a command and no
    /// `|&` joins two of them.
    final_stage_start: Option<usize>,
    all_consumers_safe: bool,
}

fn analyze_pipeline(
    line: &Slice<'_, '_>,
    readings: &[Reading],
    segment_start: usize,
    first_pipe: usize,
) -> PipelineAnalysis {
    let tokens = line.tokens;
    // A stage ends where its last word does, which keeps an escaped blank:
    // `head\ ` is the program `head␠`, not a safe consumer.
    let stage = |range: Range<usize>| Slice::new(line.text, &tokens[range]);
    // Only a token read as command text joins or ends a stage: the `||` of a
    // `[[ ]]` expression is part of that stage's command.
    let in_commands = |i: usize| readings[i] == Reading::Commands;
    let next_clause = tokens
        .iter()
        .enumerate()
        .skip(first_pipe + 1)
        .find(|&(i, tok)| {
            in_commands(i)
                && (tok.kind == TokenKind::Operator
                    || (tok.kind == TokenKind::Shellism && tok.value == "&"))
        })
        .map(|(i, _)| i);
    let end = next_clause.unwrap_or(tokens.len());

    let mut stage_start = segment_start;
    let mut final_stage_start = None;
    let mut has_supported_structure = true;
    let mut consumers_all_safe = true;

    for (i, tok) in tokens.iter().enumerate().take(end).skip(first_pipe) {
        if !in_commands(i) {
            continue;
        }
        if tok.kind == TokenKind::Redirect {
            if redirect_has_file_target(tokens, i) {
                consumers_all_safe = false;
            }
            continue;
        }
        let TokenKind::Pipe(kind) = tok.kind else {
            continue;
        };

        let current = stage(stage_start..i);
        if current.is_none() || kind == PipeKind::StdoutAndStderr {
            has_supported_structure = false;
        }
        if i > first_pipe && !current.is_some_and(|stage| is_safe_pipe_consumer(&stage)) {
            consumers_all_safe = false;
        }

        stage_start = i + 1;
        final_stage_start = Some(stage_start);
    }

    match stage(stage_start..end) {
        None => has_supported_structure = false,
        Some(last) if !is_safe_pipe_consumer(&last) => consumers_all_safe = false,
        Some(_) => {}
    }

    PipelineAnalysis {
        end,
        next_clause,
        final_stage_start: if has_supported_structure {
            final_stage_start
        } else {
            None
        },
        all_consumers_safe: has_supported_structure && consumers_all_safe,
    }
}

/// The edit a pipeline gets: its last stage's, when that stage is safe to
/// rewrite; else, when every later stage only displays what it reads, its
/// first stage's (#3171).
fn rewrite_pipeline(
    line: &Slice<'_, '_>,
    segment_start: usize,
    first_pipe: usize,
    analysis: PipelineAnalysis,
    kept: &[bool],
    rewriter: &mut Rewriter<'_>,
) {
    let rewritten = analysis.final_stage_start.is_some_and(|start| {
        rewriter.segment(
            line,
            start..analysis.end,
            RewriteContext::PipelineFinal,
            kept,
        )
    });
    if !rewritten && analysis.all_consumers_safe {
        rewriter.segment(
            line,
            segment_start..first_pipe,
            RewriteContext::PipelineProducer,
            kept,
        );
    }
}

/// Rewrites each command of `line`, recording one edit per rewritten command
/// in `rewriter`. Third of three compound-command segmenters — see the
/// comparison table on [`split_for_permissions`]. Less conservative than that
/// gate: a redirect stays part of its command rather than truncating it. It
/// also reads bash's grammar through [`read_grammar`]: a `[[ … ]]` expression
/// is part of the one command `[[` starts, word text (an extglob group, an
/// array literal, a `${ }`) part of the command its word belongs to, and a
/// `case` pattern belongs to no command.
///
/// The blanks and operators between two commands belong to no edit, so they
/// are emitted as written: an operator's own spacing is nobody's to normalise,
/// which is what keeps `echo 1;;esac` and `a  &&  b` intact.
///
/// A word behind a subshell's closing `)` in the same simple command, glued
/// to it or not, is not a command bash reads there (a subshell is followed by
/// a separator or a redirect), and putting `rtk` in front of it would only
/// move the error: that command is left as written. A `case` pattern's `)`
/// is not one, since an arm's command may follow it directly.
///
/// A command inside a compound command that a pipe joins to another
/// command is left as written ([`Rewriter::segment`]).
///
/// [`read_grammar`]: crate::core::cmdline::lexer::read_grammar
fn rewrite_compound(line: Slice<'_, '_>, rewriter: &mut Rewriter<'_>) {
    let len = line.tokens.len();
    let units = [(0..len, LineRead::new(&line))];
    let kept = kept_across_lines(len, &units);
    rewrite_read(line, &units[0].1, &kept, rewriter);
}

/// A line of commands as the rewrite reads it once: how bash reads each
/// token, its compound commands whose parts are command lists
/// ([`read_nesting`]), and whether it holds a pipe.
struct LineRead {
    readings: Vec<Reading>,
    compounds: Vec<CompoundSpan>,
    piped: bool,
}

impl LineRead {
    fn new(line: &Slice<'_, '_>) -> Self {
        let Nesting {
            readings,
            compounds,
        } = read_nesting(line.text, line.tokens);
        // A `case` pattern's `|` and a `[[ ]]` regex's belong to no pipeline.
        let piped = line.tokens.iter().zip(&readings).any(|(tok, reading)| {
            matches!(reading, Reading::Commands | Reading::WordText)
                && matches!(tok.kind, TokenKind::Pipe(_))
        });
        Self {
            readings,
            compounds,
            piped,
        }
    }

    /// Whether the rewrite leaves the whole line as written: one that pipes
    /// and holds a bracket. A `case` pattern's `|` and brackets, and a
    /// `[[ ]]` regex's, belong to no pipeline. Word text's count here, as
    /// they do for the permission gate's segmenter, so a line holding them
    /// next to a pipe is left as written.
    fn left_whole(&self, tokens: &[Token<'_>]) -> bool {
        self.piped
            && tokens
                .iter()
                .zip(&self.readings)
                .filter(|(_, reading)| matches!(reading, Reading::Commands | Reading::WordText))
                .any(|(tok, _)| {
                    tok.kind == TokenKind::Shellism && matches!(tok.value, "(" | ")" | "{" | "}")
                })
    }
}

/// [`rewrite_compound`] for a line already read, with the tokens where a
/// command is left as written.
fn rewrite_read(line: Slice<'_, '_>, read: &LineRead, kept: &[bool], rewriter: &mut Rewriter<'_>) {
    let tokens = line.tokens;
    let readings = &read.readings;
    if read.left_whole(tokens) {
        return;
    }

    // Where the last subshell's closing `)` ended, as a token index.
    let mut subshell_end = None;
    let segment =
        |range: Range<usize>, subshell_end: Option<usize>, rewriter: &mut Rewriter<'_>| {
            if subshell_end != Some(range.start) {
                rewriter.segment(&line, range, RewriteContext::Normal, kept);
            }
        };
    let mut seg_start = 0;
    let mut substitution = SubstitutionDepth::default();

    for (i, tok) in tokens.iter().enumerate() {
        // A blank ends nothing here: a newline that reaches this loop follows
        // an operator that joined two lines, so it only separates words.
        if i < seg_start || tok.is_blank() {
            continue;
        }
        match readings[i] {
            // A `case` pattern runs nothing, so it belongs to no command: what
            // precedes it ends, and the arm's commands start after its `)`.
            Reading::Pattern => {
                segment(seg_start..i, subshell_end, rewriter);
                seg_start = i + 1;
                continue;
            }
            // A `[[ ]]` expression is part of the command `[[` starts, and
            // word text part of the command its word belongs to: nothing
            // inside either ends that command.
            Reading::Expression | Reading::WordText => continue,
            Reading::Commands => {}
        }
        // `$( )`, `<( )` and `>( )` all run a command in service of the outer
        // one — as text it is built from, or as a file it reads. Filtering that
        // output would change what the outer command parses rather than what
        // reaches anyone, so nothing inside one is a boundary and nothing
        // inside one is rewritten.
        if substitution.absorbs(line.text, tok) || substitution.is_inside() {
            continue;
        }
        let boundary = match tok.kind {
            TokenKind::Operator => true,
            TokenKind::Pipe(_) => {
                let analysis = analyze_pipeline(&line, readings, seg_start, i);
                rewrite_pipeline(&line, seg_start, i, analysis, kept, rewriter);
                match analysis.next_clause {
                    Some(next_clause) => {
                        seg_start = next_clause;
                        continue;
                    }
                    None => return,
                }
            }
            TokenKind::Shellism => matches!(tok.value, "&" | "(" | ")"),
            _ => false,
        };
        if boundary {
            segment(seg_start..i, subshell_end, rewriter);
            seg_start = i + 1;
            if tok.value == ")" {
                subshell_end = Some(seg_start);
            }
        }
    }

    segment(seg_start..tokens.len(), subshell_end, rewriter);
}

fn rewrite_line_range(cmd: &str) -> Option<String> {
    for re in [&*HEAD_N, &*HEAD_LINES, &*HEAD_N_SPACE, &*HEAD_LINES_SPACE] {
        if let Some(caps) = re.captures(cmd) {
            let n = caps.get(1)?.as_str();
            let file = caps.get(2)?.as_str();
            if is_single_file_operand(file) {
                return Some(format!("rtk read {} --head-lines {}", file, n));
            }
            return None;
        }
    }
    if let Some(caps) = HEAD_BARE.captures(cmd) {
        let file = caps.get(1)?.as_str();
        if is_single_file_operand(file) {
            return Some(format!("rtk read {} --head-lines 10", file));
        }
    }
    if cmd.starts_with("head") {
        return None;
    }
    for re in [
        &*TAIL_N,
        &*TAIL_N_SPACE,
        &*TAIL_LINES_EQ,
        &*TAIL_LINES_SPACE,
    ] {
        if let Some(caps) = re.captures(cmd) {
            let n = caps.get(1)?.as_str();
            let file = caps.get(2)?.as_str();
            if is_single_file_operand(file) {
                return Some(format!("rtk read {} --tail-lines {}", file, n));
            }
            return None;
        }
    }
    None
}

struct SafePipeConsumer {
    name: &'static str,
    unsafe_flags: &'static [&'static str],
    unsafe_flag_chars: &'static [char],
}

const SAFE_PIPE_CONSUMERS: &[SafePipeConsumer] = &[
    SafePipeConsumer {
        name: "cat",
        unsafe_flags: &[],
        unsafe_flag_chars: &[],
    },
    SafePipeConsumer {
        name: "head",
        unsafe_flags: &[],
        unsafe_flag_chars: &[],
    },
    // #3171: only non-following tail is display-only
    SafePipeConsumer {
        name: "tail",
        unsafe_flags: &["--follow"],
        unsafe_flag_chars: &['f', 'F'],
    },
];

fn arg_matches_unsafe_flag(consumer: &SafePipeConsumer, arg: &str) -> bool {
    if let Some(rest) = arg.strip_prefix("--") {
        let name = rest.split_once('=').map_or(rest, |(name, _)| name);
        return !name.is_empty()
            && consumer.unsafe_flags.iter().any(|flag| {
                flag.strip_prefix("--")
                    .is_some_and(|full| full.starts_with(name))
            });
    }
    arg.strip_prefix('-').is_some_and(|rest| {
        rest.chars()
            .any(|c| consumer.unsafe_flag_chars.contains(&c))
    })
}

fn is_safe_pipe_consumer(stage: &Slice<'_, '_>) -> bool {
    let words = stage.argv();
    let mut words = words.iter();
    let Some(head) = words.next() else {
        return false;
    };
    let Some(consumer) = SAFE_PIPE_CONSUMERS.iter().find(|c| c.name == head.as_str()) else {
        return false;
    };
    !words.any(|arg| arg_matches_unsafe_flag(consumer, arg))
}

/// How many layers a walk unwraps before giving up. A command nesting this
/// many transparent prefixes is either pathological or an attempt to make the
/// rewriter spend unbounded time on it.
const MAX_PREFIX_DEPTH: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RewriteContext {
    Normal,
    PipelineFinal,
    PipelineProducer,
}

impl RewriteContext {
    /// Where a command rewritten in this context starts. Every stage of a
    /// pipeline starts its own simple command, but only its first is where a
    /// pipeline starts, which is where bash reads `time` as its reserved
    /// word.
    fn position(self) -> Position {
        match self {
            RewriteContext::PipelineFinal => Position::STAGE,
            RewriteContext::Normal | RewriteContext::PipelineProducer => Position::START,
        }
    }
}

/// Checks whether grep or rg reads patterns from a file, given its argv.
fn search_uses_pattern_file(argv: &[String]) -> bool {
    argv.iter()
        .skip(1)
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| {
            arg == "--file"
                || arg.starts_with("--file=")
                || arg
                    .strip_prefix('-')
                    .filter(|flags| !flags.starts_with('-'))
                    .is_some_and(|flags| flags.contains('f'))
        })
}

/// `argv` gives the command's argv, and is only called for grep and rg.
fn pipeline_command_is_safe<'w>(rtk_cmd: &str, argv: impl FnOnce() -> &'w [String]) -> bool {
    !matches!(rtk_cmd, "rtk grep" | "rtk rg") || !search_uses_pattern_file(argv())
}

/// A folded file list (`-l`/`-L`/`--files`) carries its shared prefix in a header line, so a
/// display consumer that keeps only some lines (`tail`) would return tails with no prefix.
/// `argv` gives the command's argv, and is only called for grep and rg.
fn producer_output_is_line_faithful<'w>(
    rtk_cmd: &str,
    argv: impl FnOnce() -> &'w [String],
) -> bool {
    let engine = match rtk_cmd {
        "rtk grep" => Engine::Grep,
        "rtk rg" => Engine::Rg,
        _ => return true,
    };
    !is_bare_file_list(engine, argv().get(1..).unwrap_or_default())
}

pub(crate) enum ExcludePattern {
    Regex(Regex),
    Prefix(String),
}

pub(crate) fn compile_exclude_patterns(patterns: &[String]) -> Vec<ExcludePattern> {
    patterns
        .iter()
        .filter_map(|pattern| {
            let trimmed = pattern.trim();
            if trimmed.is_empty() || trimmed == "^" {
                deferred::warn(format_args!(
                    "[rtk] warning: ignoring trivial exclude_commands pattern '{}'",
                    pattern
                ));
                return None;
            }
            let anchored = if trimmed.starts_with('^') {
                trimmed.to_string()
            } else {
                format!(r"^{}($|[ \t\n])", regex::escape(trimmed))
            };
            Some(match Regex::new(&anchored) {
                Ok(re) => ExcludePattern::Regex(re),
                Err(e) => {
                    deferred::warn(format_args!(
                        "[rtk] warning: invalid exclude_commands pattern '{}': {}",
                        pattern, e
                    ));
                    ExcludePattern::Prefix(trimmed.to_string())
                }
            })
        })
        .collect()
}

pub(crate) use super::families::user::normalize_transparent_prefixes;

fn is_excluded(cmd: &str, excluded: &[ExcludePattern]) -> bool {
    excluded.iter().any(|pat| match pat {
        ExcludePattern::Regex(re) => re.is_match(cmd),
        ExcludePattern::Prefix(prefix) => cmd.starts_with(prefix.as_str()),
    })
}

/// A command line and its tokens, lexed once. Every line of a block, segment
/// and pipeline stage the rewrite reads is a [`Slice`] of these tokens, and
/// every edit it makes is a span of `text`.
struct CompoundLex<'a> {
    text: &'a str,
    tokens: Vec<Token<'a>>,
}

impl<'a> CompoundLex<'a> {
    /// `line` without the blanks at either end, and its tokens.
    fn new(line: &'a str) -> Self {
        let (text, tokens) = tokenize_trimmed(line);
        Self { text, tokens }
    }

    /// The whole line; `None` when it holds nothing but blanks.
    fn whole(&self) -> Option<Slice<'a, '_>> {
        Slice::new(self.text, &self.tokens)
    }

    fn has_heredoc(&self) -> bool {
        tokens_have_heredoc(&self.tokens)
    }
}

/// The tokens of one command, or of one line of commands, from its first word
/// to the end of its last. A command ends where its last word does, and an
/// escaped or quoted blank belongs to that word (`head\ ` names the program
/// `head␠`). Everything outside the slice is emitted as written.
#[derive(Clone, Copy)]
struct Slice<'a, 't> {
    /// The whole text the tokens were lexed from; offsets index into it.
    text: &'a str,
    /// Neither the first nor the last is a blank.
    tokens: &'t [Token<'a>],
}

impl<'a, 't> Slice<'a, 't> {
    /// The command in `tokens`, without the blanks at either end; `None` when
    /// they are all blanks.
    fn new(text: &'a str, tokens: &'t [Token<'a>]) -> Option<Self> {
        let (first, last) = content_bounds(tokens)?;
        Some(Self {
            text,
            tokens: &tokens[first..=last],
        })
    }

    fn start(&self) -> usize {
        self.tokens[0].offset
    }

    fn end(&self) -> usize {
        self.tokens[self.tokens.len() - 1].end()
    }

    fn as_str(&self) -> &'a str {
        &self.text[self.start()..self.end()]
    }

    /// The tokens that are not blanks.
    fn toks(&self) -> impl DoubleEndedIterator<Item = &'t Token<'a>> {
        self.tokens.iter().filter(|tok| !tok.is_blank())
    }

    fn words(&self) -> Vec<Word<'a>> {
        words(self.text, self.tokens)
    }

    /// The words with their quotes and escapes resolved.
    fn argv(&self) -> Vec<String> {
        resolve_words(&self.words())
    }

    /// Where the run of redirects that ends the command starts, each redirect
    /// taken with the operand that follows it, if any.
    fn trailing_redirect_start(&self) -> Option<usize> {
        let mut toks = self.toks().rev().peekable();
        let mut start = None;
        while let Some(tok) = toks.next() {
            match tok.kind {
                TokenKind::Redirect => start = Some(tok.offset),
                TokenKind::Arg => match toks.next_if(|prev| prev.kind == TokenKind::Redirect) {
                    Some(redirect) => start = Some(redirect.offset),
                    None => break,
                },
                _ => break,
            }
        }
        start
    }

    /// The command up to the token at `offset`, that token excluded; `None`
    /// when nothing but blanks comes before it.
    fn before(&self, offset: usize) -> Option<Self> {
        let count = self.tokens.partition_point(|tok| tok.offset < offset);
        Self::new(self.text, &self.tokens[..count])
    }
}

/// The words of the tokens before `end`, which falls on a token boundary,
/// given the words of a command that runs on past it: a word the boundary
/// cuts, as in `status>out`, ends there.
fn words_before<'a>(text: &'a str, words: &[Word<'a>], end: usize) -> Vec<Word<'a>> {
    words
        .iter()
        .take_while(|word| word.start < end)
        .map(|word| Word {
            text: &text[word.start..word.end.min(end)],
            end: word.end.min(end),
            ..*word
        })
        .collect()
}

/// One transparent layer a walk peeled off the front of a command: the words
/// it covers in the command's own lex, its kind, and where the walk stood
/// behind it. Everything a rewrite does with a layer is a span of the
/// command's text.
#[derive(Debug, Clone, Copy)]
struct Layer {
    /// Index of the layer's first word.
    word: usize,
    /// Index of the first word after it, where what it wraps starts.
    end: usize,
    kind: LayerKind,
    /// Where the walk stands behind this layer.
    next: Position,
}

/// Why a walk ended where it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// At the command word, or past every word.
    Command,
    /// At an assignment run that sets `RTK_DISABLED` and has a command behind
    /// it: the rewrite is refused (#345).
    Disabled,
    /// At a layer with nothing after it.
    Nothing,
    /// [`MAX_PREFIX_DEPTH`] layers deep, with the command unseen.
    Depth,
}

/// A command's transparent layers, peeled left to right in one pass over the
/// words and tokens of its one lex, and the word the command under them
/// starts at (it runs to the end of the line). `command` is `None` when the
/// walk stopped with nothing to decide about; a `Some` past the last word
/// means the layers took every word.
struct Walk {
    layers: Vec<Layer>,
    command: Option<usize>,
    stop: Stop,
}

impl Walk {
    /// Peels `line`'s layers from word `from`, each read at the position the
    /// one before it left ([`Position`]), starting at `pos`. Stops at a
    /// command word, at a layer that [`ends_the_walk`], or at
    /// [`MAX_PREFIX_DEPTH`] layers counted from `depth`.
    fn run(
        line: &Line<'_>,
        prefixes: &[String],
        from: usize,
        mut pos: Position,
        mut depth: usize,
    ) -> Self {
        let mut layers = Vec::new();
        let mut at = from;
        let stopped = |layers, stop| Walk {
            layers,
            command: None,
            stop,
        };
        loop {
            if depth >= MAX_PREFIX_DEPTH {
                return stopped(layers, Stop::Depth);
            }
            let Some(peel) = peel_at(line, at, pos, prefixes) else {
                break;
            };
            let layer = Layer {
                word: at,
                end: peel.end,
                kind: peel.kind,
                next: peel.next,
            };
            if let Some(stop) = ends_the_walk(line, &layer) {
                return stopped(layers, stop);
            }
            layers.push(layer);
            at = peel.end;
            pos = peel.next;
            depth += 1;
        }
        Walk {
            layers,
            command: Some(at),
            stop: Stop::Command,
        }
    }
}

/// The layer `at` opens, if any: an assignment run, a reserved word bash runs
/// a pipeline after, a keyword, a wrapper or a user-configured prefix, tried
/// in that order. The built-in peel wins whenever it succeeds, and an entry
/// of `transparent_prefixes` that starts with a reserved word is refused where
/// the configuration is read ([`families::user`]), so a reserved word is never
/// a user prefix's to claim.
///
/// A wrapper with a literal `rtk` word among its own flags and operands
/// (#2375) is no layer: `/usr/bin/time -o rtk cmd` names its output file `rtk`. Words
/// are read, not the raw text, so a quoted value that holds ` rtk `
/// (`/usr/bin/time -f "%e rtk %U" cmd`) is one word and does not count. A
/// wrapped command that runs rtk is left alone by [`decide`], which keeps it.
fn peel_at(
    line: &Line<'_>,
    at: usize,
    pos: Position,
    prefixes: &[String],
) -> Option<Peel<LayerKind>> {
    walker::peel_tiers(
        line,
        at,
        families::ALL,
        prefixes,
        pos,
        (
            LayerKind::Assignments,
            LayerKind::ReservedWord,
            LayerKind::UserPrefix,
        ),
        AFTER_USER_PREFIX,
    )
}

/// Whether `layer` ends the walk instead of becoming one of its layers: a
/// layer with nothing after it, which is no more the command than a wrapper
/// of one, or an assignment run that sets `RTK_DISABLED` (#345). An
/// assignment run with no command behind it (`RTK_DISABLED=1; git status`)
/// is a statement of its own, and disables nothing.
fn ends_the_walk(line: &Line<'_>, layer: &Layer) -> Option<Stop> {
    if layer.end >= line.words.len() {
        return Some(Stop::Nothing);
    }
    (layer.kind == LayerKind::Assignments
        && words_set_rtk_disabled(&line.words[layer.word..layer.end]))
    .then_some(Stop::Disabled)
}

/// What deciding about one command found.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// The command already runs through rtk.
    Keep,
    Rewrite(Edit),
    /// The command is one `exclude_commands` names, as written or as the tool
    /// a rule routes it to.
    Excluded,
}

/// The rewrite of one command: its edit, if any, and whether a walk stopped
/// at `RTK_DISABLED=` without anything else deciding about the command.
#[derive(Debug, Default)]
struct Outcome {
    edit: Option<Edit>,
    disabled: bool,
    /// The walks the rewrite ran (test builds only).
    #[cfg(test)]
    walks: usize,
}

/// The rewrite of the command `cmd` in `context`: its edit, if any.
///
/// The command's layers are walked once ([`Walk::run`]) over one [`Line`],
/// the command under them is decided about, and when that decides nothing the
/// layers are read again ([`Search::fall_back`]). Every rewrite is an edit
/// against the text the command's tokens index.
fn rewrite_segment(
    cmd: Slice<'_, '_>,
    excluded: &[ExcludePattern],
    transparent_prefixes: &[String],
    context: RewriteContext,
) -> Outcome {
    let words = cmd.words();
    let line = Line {
        text: cmd.text,
        words: &words,
        tokens: cmd.tokens,
    };
    let walk = Walk::run(&line, transparent_prefixes, 0, context.position(), 0);
    let decided = walk
        .command
        .and_then(|at| decide(&line, at, excluded, context));
    let edit = |decision| match decision {
        Some(Decision::Rewrite(edit)) => Some(edit),
        _ => None,
    };
    if let Some(decision @ (Decision::Rewrite(_) | Decision::Keep)) = decided {
        return Outcome {
            edit: edit(Some(decision)),
            disabled: false,
            #[cfg(test)]
            walks: 1,
        };
    }
    let mut search = Search {
        line: &line,
        excluded,
        prefixes: transparent_prefixes,
        context,
        visited: HashSet::new(),
        refusals: HashMap::new(),
        disabled: false,
        #[cfg(test)]
        walks: 1,
    };
    let stopped = walk.stop == Stop::Disabled;
    let decision = search.fall_back(walk.layers);
    // A fall-through that wins draws no warning.
    let disabled = decision.is_none() && (stopped || search.disabled);
    Outcome {
        edit: edit(decision),
        disabled,
        #[cfg(test)]
        walks: search.walks,
    }
}

/// What a search for a fall-through concluded about what a routable wrapper
/// wraps, from the weakest answer to the strongest: when several walks
/// answer, the strongest holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Refusal {
    Allowed,
    /// The walk stopped at an option under `exclude_commands`, which leaves
    /// the command behind it unread: the wrapper is not matched whole, and a
    /// user prefix that starts where it does is tried.
    Unread,
    /// Refused: something wrapped is excluded, or out of sight.
    Refused,
    /// Refused because a walk stopped at `RTK_DISABLED=`.
    Disabled,
}

/// One walk still to search for a fall-through: its layers, `layers[..upto]`
/// left to try, innermost first, and the depth `layers[0]` was peeled at, so
/// that `layers[i]` sits `depth + i` layers deep.
struct Frame {
    layers: Vec<Layer>,
    depth: usize,
    upto: usize,
}

/// The state of one search for a fall-through over the layers of one command.
struct Search<'l, 'a> {
    line: &'l Line<'a>,
    excluded: &'l [ExcludePattern],
    prefixes: &'l [String],
    context: RewriteContext,
    /// `(word, depth)` pairs already tried: a retry depends on nothing else.
    visited: HashSet<(usize, usize)>,
    /// [`Search::walk_refuses`]' answers, by where a walk started, how deep,
    /// and the position it read from.
    refusals: HashMap<(usize, usize, Position), Refusal>,
    /// Whether a walk of the search stopped at `RTK_DISABLED=`.
    disabled: bool,
    #[cfg(test)]
    walks: usize,
}

impl Search<'_, '_> {
    fn walk(&mut self, from: usize, pos: Position, depth: usize) -> Walk {
        #[cfg(test)]
        {
            self.walks += 1;
        }
        let walk = Walk::run(self.line, self.prefixes, from, pos, depth);
        self.disabled |= walk.stop == Stop::Disabled;
        walk
    }

    fn decide(&self, word: usize) -> Option<Decision> {
        decide(self.line, word, self.excluded, self.context)
    }

    /// #2768: when nothing is decided about the command, each layer's own
    /// text is read again at that layer's position and depth, innermost
    /// first.
    ///
    /// A user prefix that starts at the layer is read first, whatever the
    /// layer's kind: `transparent_prefixes = ["nice -n 19 ionice"]` rewrites
    /// `nice -n 19 ionice git status` although the command under `nice`'s own
    /// peel (`ionice git status`) has no rewrite. That prefix is the answer:
    /// its own walk is searched the same way, and the layer is not also tried
    /// whole. When no prefix matches, only a routable layer (`uv run`) may be
    /// matched whole by a rule, and only when [`Search::fallthrough_refused`]
    /// allows it; any other layer runs its command without changing which
    /// program runs, so a command with no rewrite of its own leaves it as
    /// written. A reserved word is bash's, and no prefix takes it.
    ///
    /// Every layer any walk produces is a candidate. The layers of a walk made
    /// here are tried before the shallower layers of the walk that led to it,
    /// and each walk's layers are kept apart, so a branch that leads nowhere
    /// adds nothing to one that decides. `(word, depth)` is recorded once its
    /// prefix has been tried (a frame that skips it because the prefix ends
    /// inside one of its own layers leaves it for another frame), and reached
    /// a second time it is skipped, which keeps a `uv run uv run … uv run` chain linear in its depth.
    fn fall_back(&mut self, layers: Vec<Layer>) -> Option<Decision> {
        let words = self.line.words;
        let upto = layers.len();
        let mut stack = vec![Frame {
            layers,
            depth: 0,
            upto,
        }];
        while let Some(mut frame) = stack.pop() {
            while frame.upto > 0 {
                frame.upto -= 1;
                let idx = frame.upto;
                let layer = frame.layers[idx];
                if matches!(layer.kind, LayerKind::UserPrefix | LayerKind::ReservedWord) {
                    continue;
                }
                let depth = frame.depth + idx;
                let mut routable = layer.kind == LayerKind::RoutableWrapper;
                if routable {
                    match self.fallthrough_refused(&layer, depth) {
                        Refusal::Allowed => {}
                        Refusal::Unread => routable = false,
                        Refusal::Refused | Refusal::Disabled => continue,
                    }
                }
                if self.visited.contains(&(layer.word, depth)) {
                    continue;
                }
                let Some(user) = walker::peel_literal_prefix(
                    self.line,
                    layer.word,
                    self.prefixes,
                    LayerKind::UserPrefix,
                    AFTER_USER_PREFIX,
                ) else {
                    self.visited.insert((layer.word, depth));
                    if routable && let Some(decision) = decided(self.decide(layer.word)) {
                        return Some(decision);
                    }
                    continue;
                };
                // A prefix that ends inside the words of a layer it covers
                // (`nice` inside `nice -5`, `nice timeout` inside `nice
                // timeout 5`) would leave that layer's own flags as the
                // command. One with nothing behind it leaves nothing to
                // decide about.
                if prefix_ends_inside_a_layer(user.end, &frame.layers[idx..])
                    || user.end >= words.len()
                {
                    continue;
                }
                self.visited.insert((layer.word, depth));
                let alt = self.walk(user.end, user.next, depth + 1);
                if let Some(decision) = decided(alt.command.and_then(|at| self.decide(at))) {
                    return Some(decision);
                }
                let mut layers = vec![Layer {
                    word: layer.word,
                    end: user.end,
                    kind: user.kind,
                    next: user.next,
                }];
                layers.extend(alt.layers);
                let upto = layers.len();
                // This frame's shallower layers resume after the new walk's.
                stack.push(frame);
                stack.push(Frame {
                    layers,
                    depth,
                    upto,
                });
                break;
            }
        }
        None
    }

    /// Whether a routable layer's own text must not be matched whole: the
    /// command it wraps was left for a reason a whole-string match would undo.
    ///
    /// - A `NAME=value` word right behind `uv run` is a program's name, since
    ///   `uv run` is a program and nothing behind it starts a simple command.
    ///   Matching the whole string would hand that word to `uv run` as its
    ///   command (`uv run RTK_DISABLED=1 git status`). `env` is a program too,
    ///   and falls through (`uv run env python x.py`).
    /// - Everything else is read by walking what the wrapper wraps as the
    ///   rewrite walks it ([`Search::walk_refuses`]), from `depth`: the same
    ///   peels, the same user-prefix retries, the same depth budget. A walk
    ///   that stops at `RTK_DISABLED=` refuses, and so does one whose command
    ///   `exclude_commands` covers, as [`decide`] reads it, on its words as
    ///   bash runs them ([`command_is_excluded`]): under `exclude_commands =
    ///   ["pytest"]`, `uv run python -m pytest`, `uv run timeout 5 pytest`,
    ///   `uv run nice python -m pytest` and `uv run 'pytest'` are refused as
    ///   `uv run pytest` is. What the wrapper wraps,
    ///   as written and past an assignment run, is checked too (`uv run env`
    ///   under an excluded `env`). A walk that runs out of depth hides its
    ///   command, and counts as excluded.
    /// - Under exclusions, a walk that stops at an option (`uv run --with x
    ///   pytest`), which is the wrapper's own and leaves the command behind it
    ///   unread, keeps the wrapper from being matched whole; a user prefix
    ///   that starts where the wrapper does is tried
    ///   ([`Refusal::Unread`]). A `--` right behind the wrapper is read
    ///   through: what follows it is the command (`uv run -- pytest`).
    fn fallthrough_refused(&mut self, layer: &Layer, depth: usize) -> Refusal {
        let words = self.line.words;
        let mut from = layer.end;
        if !self.excluded.is_empty() && words.get(from).is_some_and(|w| is_dash_dash(w.text)) {
            from += 1;
        }
        let Some(inner) = words.get(from) else {
            return Refusal::Refused;
        };
        if walker::is_assignment_word(inner.text) {
            return Refusal::Refused;
        }
        if !self.excluded.is_empty() {
            let behind_env = walker::peel_assignments(
                self.line,
                from,
                families::ALL,
                layer.next,
                LayerKind::Assignments,
            )
            .map_or(from, |run| run.end);
            let excluded = |at| self.decide(at) == Some(Decision::Excluded);
            if excluded(from) || (behind_env != from && excluded(behind_env)) {
                return Refusal::Refused;
            }
        }
        self.walk_refuses(from, layer.next, depth + 1, Some(from))
    }

    /// What a walk from `(from, pos, depth)`, the start of what a routable
    /// wrapper wraps, means for its fall-through ([`Search::fallthrough_refused`]):
    /// its own stop, and those of the walks a user prefix starting at one of
    /// its layers reads out of the same words. Remembered per start, so the
    /// candidates of one search share the work.
    ///
    /// `not_excluded` is a word the caller has already found `exclude_commands`
    /// does not cover, which the walk does not decide about again.
    fn walk_refuses(
        &mut self,
        from: usize,
        pos: Position,
        depth: usize,
        not_excluded: Option<usize>,
    ) -> Refusal {
        let key = (from, depth, pos);
        if let Some(&known) = self.refusals.get(&key) {
            return known;
        }
        let walk = self.walk(from, pos, depth);
        let excluding = !self.excluded.is_empty();
        let mut refused = match walk.stop {
            Stop::Disabled => Refusal::Disabled,
            Stop::Depth if excluding => Refusal::Refused,
            Stop::Command => match walk.command {
                Some(at)
                    if excluding
                        && not_excluded != Some(at)
                        && self.decide(at) == Some(Decision::Excluded) =>
                {
                    Refusal::Refused
                }
                // A command word that is an option is a wrapper's own, which
                // hides the command behind it.
                Some(at)
                    if excluding
                        && self
                            .line
                            .words
                            .get(at)
                            .is_some_and(|w| head_is_an_option(w.text)) =>
                {
                    Refusal::Unread
                }
                // A program spelled by a path that a layer's word names hides
                // the command behind it.
                Some(at)
                    if excluding && walker::is_path_spelled_head(self.line, at, families::ALL) =>
                {
                    Refusal::Refused
                }
                _ => Refusal::Allowed,
            },
            Stop::Depth | Stop::Nothing => Refusal::Allowed,
        };
        for (idx, walked) in walk.layers.iter().enumerate() {
            if refused == Refusal::Disabled {
                break;
            }
            if matches!(walked.kind, LayerKind::UserPrefix | LayerKind::ReservedWord) {
                continue;
            }
            let Some(user) = walker::peel_literal_prefix(
                self.line,
                walked.word,
                self.prefixes,
                LayerKind::UserPrefix,
                AFTER_USER_PREFIX,
            ) else {
                continue;
            };
            if user.end >= self.line.words.len()
                || prefix_ends_inside_a_layer(user.end, &walk.layers[idx..])
            {
                continue;
            }
            let other = self.walk_refuses(user.end, user.next, depth + idx + 1, None);
            refused = refused.max(other);
        }
        self.refusals.insert(key, refused);
        refused
    }
}

/// A decision that settles the command: a rewrite, or a command left as it is
/// because it already runs rtk.
fn decided(decision: Option<Decision>) -> Option<Decision> {
    decision.filter(|d| matches!(d, Decision::Rewrite(_) | Decision::Keep))
}

/// How [`arg_tokenizer::tokenize`] reads `word`, as the shell resolves it
/// (`"-s"` is `-s`), as one argument: an option, `--` or a positional.
fn resolved_arg_kind(word: &str) -> Option<ArgKind> {
    let resolved = [resolve_word_text(word)];
    arg_tokenizer::tokenize(&resolved)
        .first()
        .map(|token| token.kind)
}

/// Whether `word`, as the shell resolves it, is `--`.
fn is_dash_dash(word: &str) -> bool {
    resolved_arg_kind(word) == Some(ArgKind::DashDash)
}

/// Whether a prefix ending at word `end` ends inside the words of one of
/// `layers` (a built-in layer's own flags and operands), which would leave the
/// rest of those words as the command.
fn prefix_ends_inside_a_layer(end: usize, layers: &[Layer]) -> bool {
    layers
        .iter()
        .any(|l| l.kind != LayerKind::UserPrefix && l.word < end && end < l.end)
}

/// Whether the word a command starts with, as the shell resolves it (`"-s"` is
/// `-s`), is an option or `--` rather than a program's name: a wrapper's own
/// flag it does not declare, or `--`. No command is named that.
fn head_is_an_option(word: &str) -> bool {
    resolved_arg_kind(word).is_some_and(|kind| kind != ArgKind::Positional)
}

/// Whether `exclude_commands` covers the command, up to where it ends (the
/// redirections behind it are no part of it). A pattern covers it when it
/// matches the command either as typed, `raw`, or as bash runs it: the words
/// `argv`, their quotes and escapes removed, with the first word's absolute
/// path left out, or in the spelling of the tool a rule routes it to
/// ([`tool_form`]: `python -m pytest` is `pytest`). The words are read in one
/// spelling ([`spell_words`]), so `'pytest' -x`, `\pytest -x` and
/// `'/opt/my tools/pytest' -x` are excluded with `pytest` and so with
/// `^'pytest'`, and `'pytest x'`, which runs a program named `pytest x`, is
/// not excluded with `pytest`.
fn command_is_excluded(
    raw: &str,
    spelled: &str,
    argv: &[String],
    classification: &Classification,
    excluded: &[ExcludePattern],
    position: Position,
) -> bool {
    if excluded.is_empty() {
        return false;
    }
    let Some((program, args)) = argv.split_first() else {
        return false;
    };
    let args = args.iter().map(String::as_str);
    let as_run = spell_words(std::iter::once(program.as_str()).chain(args.clone()));
    // The program's basename, read off its one word as it is resolved.
    let named = match program.rsplit_once('/') {
        Some((_, base)) if !base.is_empty() => spell_words(std::iter::once(base).chain(args)),
        _ => as_run.clone(),
    };
    // The command as typed, with the directory of its program left out. The
    // first word is cut at its last `/` only where it is written plain; a
    // quoted or escaped one may hold a blank and a `/` in one name, and the
    // words bash runs ([`named`]) read it right.
    let typed: Cow<'_, str> = match raw.strip_prefix(spelled) {
        Some(rest)
            if !spelled.contains(['\'', '"', '\\'])
                && !walker::basename(spelled).is_empty()
                && walker::basename(spelled) != spelled =>
        {
            Cow::Owned(format!("{}{rest}", walker::basename(spelled)))
        }
        _ => Cow::Borrowed(raw),
    };
    if [raw, typed.as_ref(), as_run.as_str(), named.as_str()]
        .into_iter()
        .any(|text| is_excluded(text, excluded))
    {
        return true;
    }
    // The tool a rule routes the command to, read off the command as typed
    // and off the words bash runs: `npx "playwright" test` is `npx playwright
    // test`. The words are classified again only when they spell something
    // other than what was classified.
    let as_words = (named != typed || !matches!(classification, Classification::Supported { .. }))
        .then(|| classify_command_from(&named, position));
    [Some(classification), as_words.as_ref()]
        .into_iter()
        .flatten()
        .any(|class| {
            matches!(class, Classification::Supported { rtk_equivalent, .. }
                if [typed.as_ref(), named.as_str()]
                    .into_iter()
                    .enumerate()
                    .any(|(i, text)| (i == 0 || text != typed)
                        && is_excluded(&tool_form(text, rtk_equivalent), excluded)))
        })
}

/// `words` as the one text a pattern of `exclude_commands` reads: one space
/// between two words, and a blank inside a word behind a backslash, as bash
/// reads it, so that a blank of the text separates two words exactly where
/// bash separates them. Every other character is the word's own.
fn spell_words<'w>(words: impl IntoIterator<Item = &'w str>) -> String {
    let mut text = String::new();
    for (index, word) in words.into_iter().enumerate() {
        if index > 0 {
            text.push(' ');
        }
        for c in word.chars() {
            if is_ifs(c) {
                text.push('\\');
            }
            text.push(c);
        }
    }
    text
}

/// Decides about the command that starts at word `word` of `line`, which a
/// walk found under its layers or which is a routable layer's own text.
///
/// The command is classified where its segment starts (`context`), not where
/// the walk stands behind its layers. The two differ only in whether the
/// first word is grammar, and the segment's start is the stricter reading: a
/// `[[` or `((` behind a wrapper or an assignment (`a=1 [[ -f a ]]`) is left
/// as written, as the rewrite has always left it, rather than read as a
/// program that takes arguments.
///
/// A command whose first word is an assignment is a program named like one:
/// the walk peels an assignment run wherever bash reads one, so one left here
/// is a `NAME=value` behind `timeout` or `exec`, which bash runs as a command.
/// A first word that is an option is a wrapper's own, a flag it does not
/// declare or `--`, and no command is named that. Neither is rewritten.
///
/// Exclusion is decided before anything that depends on the context, so a
/// command `exclude_commands` names is [`Decision::Excluded`] wherever it
/// sits.
fn decide(
    line: &Line<'_>,
    word: usize,
    excluded: &[ExcludePattern],
    context: RewriteContext,
) -> Option<Decision> {
    let first = line.words.get(word)?;
    if walker::is_assignment_word(first.text)
        || head_is_an_option(first.text)
        || walker::is_path_spelled_head(line, word, families::ALL)
    {
        return None;
    }
    let text = line.text;
    let from = line.tokens.partition_point(|tok| tok.offset < first.start);
    let slice = Slice::new(text, &line.tokens[from..])?;
    let all_words = &line.words[word..];
    // Trailing stderr/stdout redirects are left out of the match and kept as
    // written (#530): `git status 2>&1` matches `git status`.
    let redirect_start = slice.trailing_redirect_start();
    let part = match redirect_start {
        Some(start) => slice.before(start)?,
        None => slice,
    };
    let words: Cow<'_, [Word<'_>]> = match redirect_start {
        Some(start) => Cow::Owned(words_before(text, all_words, start)),
        None => Cow::Borrowed(all_words),
    };
    let cmd_part = part.as_str();
    let redirect_suffix = &text[part.end()..slice.end()];
    let replace = |text: String| {
        Some(Decision::Rewrite(Edit::Replace {
            span: part.start()..part.end(),
            text,
        }))
    };
    let argv_cell = OnceCell::new();
    let argv = || argv_cell.get_or_init(|| resolve_words(&words)).as_slice();
    // The program, as bash ends its name: at a space, a tab or a newline.
    let program = words.first().map_or("", |word| word.text);

    // Already RTK — pass through unchanged
    if rtk_invocation(&words).is_some() {
        return Some(Decision::Keep);
    }

    let classification = classify_command_words(text, part.tokens, &words, context.position());
    if !excluded.is_empty()
        && command_is_excluded(
            cmd_part,
            first.text,
            argv(),
            &classification,
            excluded,
            context.position(),
        )
    {
        return Some(Decision::Excluded);
    }

    // A bare `head` or `tail` reads its input and has no line range to map, so
    // only one with arguments takes this branch.
    if context == RewriteContext::Normal && words.len() > 1 && matches!(program, "head" | "tail") {
        return replace(space_before_redirect(
            rewrite_line_range(cmd_part)?,
            redirect_suffix,
        ));
    }

    // A bare `cat` has no options to check and goes on to the filters.
    if program == "cat" && words.len() > 1 && !cat_options_map_to_read(&argv()[1..]) {
        return None;
    }

    let rtk_equivalent = match classification {
        Classification::Supported { rtk_equivalent, .. } => rtk_equivalent,
        // TOML-only commands: consult the registry so the hook filters them too (#2179).
        Classification::Unsupported { .. } => {
            if context != RewriteContext::Normal || toml_disabled() {
                return None;
            }
            let normalized = strip_absolute_path(cmd_part);
            let base = split_ifs(&normalized).next().unwrap_or("");
            if is_rtk_reserved_command(base) || !command_matches_filter(&normalized) {
                return None;
            }
            return Some(Decision::Rewrite(Edit::Insert {
                at: part.start(),
                text: "rtk ".to_string(),
            }));
        }
        Classification::Ignored => return None,
    };

    // Find the matching rule (rtk_cmd values are unique across all rules)
    let rule = RULES.iter().find(|r| r.rtk_cmd == rtk_equivalent)?;
    if context == RewriteContext::PipelineFinal
        && (!rule.pipeline_safety.final_safe() || !pipeline_command_is_safe(rule.rtk_cmd, argv))
    {
        return None;
    }
    // #3171
    if context == RewriteContext::PipelineProducer
        && (!rule.pipeline_safety.producer_safe()
            || !pipeline_command_is_safe(rule.rtk_cmd, argv)
            || !producer_output_is_line_faithful(rule.rtk_cmd, argv))
    {
        return None;
    }

    if let Some(parts) = golangci_run_parts(text, &words) {
        let text = if parts.global_segment.is_empty() {
            format!("rtk golangci-lint {}", parts.run_segment)
        } else {
            format!(
                "rtk golangci-lint {} {}",
                parts.global_segment, parts.run_segment
            )
        };
        // The span runs to the end of the command, trailing redirects included,
        // and the text ends with `run_segment`, which stops at the last word
        // before them: unlike every other rule, a trailing redirect is dropped
        // from the output (`golangci-lint run ./... 2>&1` becomes
        // `rtk golangci-lint run ./...`).
        return Some(Decision::Rewrite(Edit::Replace {
            span: part.start()..slice.end(),
            text,
        }));
    }

    // #196: gh with --json/--jq/--template produces structured output that
    // rtk gh would corrupt — skip rewrite so the caller gets raw JSON.
    if rule.rtk_cmd == "rtk gh" {
        let args_lower = cmd_part.to_lowercase();
        if args_lower.contains("--json")
            || args_lower.contains("--jq")
            || args_lower.contains("--template")
        {
            return None;
        }
    }

    // For the Composer-resolved php tools, normalize the leading invocation
    // (php wrapper + ini flags, ./, vendor/bin, composer bin-dir) exactly as
    // classify_command does, so a small canonical prefix list matches every
    // invocation form instead of enumerating each literal spelling.
    let php_normalized = php_tool_form(cmd_part, rule.rtk_cmd);
    let strip_target = php_normalized.as_deref().unwrap_or(cmd_part);

    // Try each rewrite prefix (longest first) with word-boundary check
    for &prefix in rule.rewrite_prefixes {
        if let Some(rest) = strip_word_prefix(strip_target, prefix) {
            return replace(if rest.is_empty() {
                rule.rtk_cmd.to_string()
            } else {
                format!("{} {}", rule.rtk_cmd, rest)
            });
        }
    }

    None
}

/// Whether `cat`, given `args`, maps onto `rtk read`: it names at least one
/// file, and every option it is given has an `rtk read` equivalent. Only `-n`
/// (line numbers) does: most others (`-v`, `-A`, `-e`, `-t`, `-s`, `-b`,
/// `--show-all`, …) mean something `rtk read` does not do, or nothing it
/// accepts, and a `--` refuses too. Without a file `cat` reads its input,
/// which `rtk read -n` does not. `args` are read as `cat` receives them, so a
/// quoted `'-A'` is an option.
fn cat_options_map_to_read(args: &[String]) -> bool {
    let parsed = arg_tokenizer::tokenize(args);
    parsed.iter().any(|arg| arg.kind == ArgKind::Positional)
        && parsed.iter().all(|arg| match arg.kind {
            ArgKind::Positional => true,
            ArgKind::Short => {
                arg.text == "n" && args.get(arg.source_index).is_some_and(|a| a == "-n")
            }
            ArgKind::Long | ArgKind::DashDash => false,
        })
}

/// The tool-name portion of a matched rewrite prefix: the shortest token-suffix of
/// `prefix` that is itself a rewrite prefix of the same rule. That peels the wrapper
/// (`npx`, `pnpm exec`, `python3 -m`, `bundle exec`) while keeping a subcommand the
/// rule treats as part of the tool, so `golangci-lint run` and `next build` survive
/// intact instead of collapsing to `run` and `build`.
fn tool_portion(prefix: &'static str, rule: &RtkRule) -> &'static str {
    let mut best = prefix;
    let mut rest = prefix;
    while let Some(pos) = rest.find(' ') {
        rest = &rest[pos + 1..];
        if rule.rewrite_prefixes.contains(&rest) {
            best = rest;
        }
    }
    best
}

/// Rewrite a command into the spelling `exclude_commands` is written against.
///
/// An entry names a tool, but the command may spell it with a wrapper
/// (`npx playwright test`), an interpreter (`python3 -m pytest tests/`) or a path
/// (`vendor/bin/phpunit tests/`). Peeling that spelling down to the tool lets one entry
/// cover every form. The arguments are kept, so an anchored pattern still means what it
/// says: `"^ls$"` excludes a bare `ls` without swallowing `ls -la`.
/// Canonical `<tool> <args>` form of a Composer-resolved PHP tool invocation, peeling
/// the `php` wrapper and its ini flags, a leading `./`, and a vendor/composer bin dir.
/// `None` when `rtk_cmd` is not one of those tools.
///
/// `normalize_php_tool_command` only strips `./` for paths that resolve to a Composer
/// tool, so a plain `./bin/<tool>` would otherwise survive and miss the prefix match.
fn php_tool_form(cmd: &str, rtk_cmd: &str) -> Option<String> {
    rtk_cmd
        .strip_prefix("rtk ")
        .filter(|t| PHP_TOOL_NAMES.contains(t))?;
    let unwrapped = strip_php_wrapper(cmd);
    let unwrapped = unwrapped.strip_prefix("./").unwrap_or(unwrapped);
    Some(normalize_php_tool_command(unwrapped))
}

fn tool_form(cmd_clean: &str, rtk_equivalent: &str) -> String {
    // Same normalization the rewrite path applies, so the exclusion sees the tool
    // whichever way it was spelled — including `php vendor/bin/phpunit`.
    let unwrapped =
        php_tool_form(cmd_clean, rtk_equivalent).unwrap_or_else(|| cmd_clean.to_string());
    // A quoted first word may hold a blank and a `/` in one name, which a cut
    // at the first blank would split; the caller reads it from its words.
    let first_word = unwrapped.split(is_ifs).next().unwrap_or_default();
    let normalized = if first_word.contains(['\'', '"', '\\']) {
        unwrapped
    } else {
        strip_absolute_path(&unwrapped)
    };
    RULES
        .iter()
        .find(|r| r.rtk_cmd == rtk_equivalent)
        .and_then(|rule| {
            rule.rewrite_prefixes.iter().find_map(|&prefix| {
                // What follows the prefix is kept as spelled, an empty
                // argument included, so `pytest ''` is not `pytest`.
                let tail = normalized.strip_prefix(prefix)?;
                if !tail.is_empty() && !tail.starts_with(is_ifs) {
                    return None;
                }
                // No rewrite prefix carries a path outside its first token, and that
                // token is already a basename here, so `tool_portion` needs no strip.
                let tool = tool_portion(prefix, rule);
                Some(format!("{tool}{tail}"))
            })
        })
        .unwrap_or(normalized)
}

/// Strip a command prefix with word-boundary check.
/// Returns the remainder of the command after the prefix, or `None` if no match.
///
/// Bash separates words on space, tab and newline alike, and the rule
/// patterns match `[ \t\n]+`, so the boundary here is all three.
///
/// A newline is also a command terminator, so accepting one here would be
/// wrong if it could arrive joining two commands. It cannot, by two separate
/// routes, and both have to hold:
///
/// - `rewrite_command_precompiled` hands anything containing a newline to
///   `rewrite_multiline_block`, which splits on the lexer's unquoted newline
///   tokens. A newline surviving that is inside an open quote span, where the
///   byte after a matching prefix is the quote rather than whitespace.
/// - A line ending in an operator is rejoined with the next into one unit,
///   which does carry an unquoted newline. `rewrite_compound` then re-splits
///   on that operator, and the newline is a blank before the following
///   segment's first word, where that segment starts.
fn strip_word_prefix<'a>(cmd: &'a str, prefix: &str) -> Option<&'a str> {
    if cmd == prefix {
        Some("")
    } else if cmd.len() > prefix.len()
        && cmd.starts_with(prefix)
        && cmd[prefix.len()..].starts_with(is_ifs)
    {
        Some(trim_ifs_start(&cmd[prefix.len()..]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::report::RtkStatus;
    use super::*;
    use crate::core::cmdline::family::Form;
    use crate::core::cmdline::lexer::{read_grammar, shell_split};
    use crate::core::test_isolation;

    fn rewrite_command_no_prefixes(cmd: &str, excluded: &[String]) -> Option<String> {
        super::rewrite_command(cmd, excluded, &[])
    }

    // Three consumers read the same command line for different purposes: the
    // permission gate (`split_for_permissions`), classification
    // (`split_for_classify`/`split_command_chain`), and the rewrite
    // (`rewrite_compound`'s token walk). They used to disagree about what a
    // command even was, and each disagreement was a bug waiting: a command the
    // gate never saw, a command the report never counted, a command the rewrite
    // glued to a bracket and then failed to match.
    //
    // These tests put all three side by side on one input. Where they agree,
    // that agreement is the point and is asserted. Where they still differ, the
    // difference is named and its reason given — a divergence nobody can state
    // a reason for is a bug.
    mod segmenter_agreement {
        use super::{rewrite_command_no_prefixes, split_command_chain};
        use crate::core::cmdline::lexer::split_for_permissions;

        /// `&` ends a command as surely as `;` does.
        #[test]
        fn background_ampersand_ends_a_command_for_everyone() {
            let cmd = "git status & rm -rf ~";
            assert_eq!(split_for_permissions(cmd), vec!["git status", "rm -rf ~"]);
            assert_eq!(split_command_chain(cmd), vec!["git status", "rm -rf ~"]);
            // `rm -rf ~` has no rtk equivalent, so it is left alone because no
            // rule matches it — not because it was never segmented.
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some("rtk git status & rm -rf ~".into())
            );
        }

        /// A subshell runs the commands it wraps, so all three see those
        /// commands. Gluing the bracket to the text beside it used to lose the
        /// first one: `(git status` matched no rule while `cargo build)` did,
        /// so one command of a pair was filtered and the other silently was not.
        #[test]
        fn a_subshell_is_a_boundary_for_everyone() {
            let cmd = "(git status; cargo build)";
            assert_eq!(
                split_for_permissions(cmd),
                vec!["git status", "cargo build"]
            );
            assert_eq!(split_command_chain(cmd), vec!["git status", "cargo build"]);
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some("(rtk git status; rtk cargo build)".into())
            );
            // A lone command in a subshell is still a command.
            assert_eq!(
                rewrite_command_no_prefixes("(git status)", &[]),
                Some("(rtk git status)".into())
            );
        }

        /// The one `(` that opens no subshell. A `case` pattern may be written
        /// `(ls)` as readily as `ls)`, and treating that bracket as a subshell
        /// makes the pattern a command position: the rewrite then turns a
        /// one-word pattern into two words, which bash refuses to parse — the
        /// same way `;;` split into `; ;` did in #3197.
        #[test]
        fn a_case_pattern_is_not_a_subshell() {
            for cmd in [
                "case $x in (ls) echo 1;; esac",
                // Every arm re-enters pattern position, not just the first.
                "case $x in a) echo 1;; (ls) echo 2;; esac",
                "case $x in a) echo 1;;& (ls) echo 2;; esac",
                "case $x in a) echo 1;& (ls) echo 2;; esac",
            ] {
                assert_eq!(
                    rewrite_command_no_prefixes(cmd, &[]),
                    None,
                    "a case pattern was rewritten as if it were a subshell: {cmd}"
                );
            }

            // The `)` still ends the pattern, so the arm's own commands are
            // rewritten whether or not the pattern was bracketed.
            assert_eq!(
                rewrite_command_no_prefixes("case a in (a) git status;; esac", &[]),
                Some("case a in (a) rtk git status;; esac".into())
            );
            assert_eq!(
                rewrite_command_no_prefixes("case a in a) git status;; esac", &[]),
                Some("case a in a) rtk git status;; esac".into())
            );

            // `case` is the keyword only in command position, and a real
            // subshell is still a subshell.
            assert_eq!(
                rewrite_command_no_prefixes("echo case; (git status)", &[]),
                Some("echo case; (rtk git status)".into())
            );

            // Nor does the gate or analytics read a subshell there: the gate
            // keeps the pattern in the segment before it, and analytics leaves
            // it out of every segment, so neither reports an `ls` that no shell
            // ever runs.
            let bracketed = "case $x in (ls) echo 1;; esac";
            assert_eq!(
                split_for_permissions(bracketed),
                vec!["case $x in (ls", "echo 1", "esac"]
            );
            assert_eq!(
                split_command_chain(bracketed),
                vec!["case $x in", "echo 1", "esac"]
            );
        }

        /// Every stage of a pipeline is a command that ran, and all three see
        /// all of them. What the rewrite then does with a stage is a per-rule
        /// question (`PipelineSafety`) applied after segmentation, not a
        /// disagreement about where the commands are: here only `grep` is safe
        /// to run at the end of a pipe, so only it is taken.
        #[test]
        fn every_pipeline_stage_is_a_command_for_everyone() {
            let cmd = "git status | grep x && cargo build";
            let stages = vec!["git status", "grep x", "cargo build"];
            assert_eq!(split_for_permissions(cmd), stages);
            assert_eq!(split_command_chain(cmd), stages);
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some("git status | rtk grep x && rtk cargo build".into())
            );
        }

        /// Divergence, with a reason. The gate cuts a segment at its first
        /// redirect so nothing can ride in behind one; the other two keep it,
        /// because the rewritten line has to reproduce the command's real shape
        /// and the report has to show what was really run. All three still
        /// agree on where one command ends and the next begins.
        #[test]
        fn a_redirect_is_dropped_only_by_the_gate() {
            let cmd = "git status 2>&1 && cargo build";
            assert_eq!(
                split_for_permissions(cmd),
                vec!["git status", "cargo build"]
            );
            assert_eq!(
                split_command_chain(cmd),
                vec!["git status 2>&1", "cargo build"]
            );
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some("rtk git status 2>&1 && rtk cargo build".into())
            );
        }

        /// Divergence, with a reason. Classification and the rewrite read a
        /// `[[ … ]]` expression, an arithmetic command and a `case` pattern as
        /// bash does, as no command at all: an `rtk` there breaks the line, and
        /// a report that counts one promises a saving the hook never takes. The
        /// gate keeps its own splitting at every `&&`, `||` and `)`, which can
        /// only check a segment that runs nothing, never leave one unchecked.
        #[test]
        fn the_gate_alone_splits_expressions_and_patterns() {
            let test = "[[ -f a || ls ]] && git status";
            assert_eq!(
                split_for_permissions(test),
                vec!["[[ -f a", "ls ]]", "git status"]
            );
            assert_eq!(
                split_command_chain(test),
                vec!["[[ -f a || ls ]]", "git status"]
            );
            assert_eq!(
                rewrite_command_no_prefixes(test, &[]),
                Some("[[ -f a || ls ]] && rtk git status".into())
            );

            let arithmetic = "(( a || ls )) && git status";
            assert_eq!(
                split_for_permissions(arithmetic),
                vec!["a", "ls", "git status"]
            );
            assert_eq!(
                split_command_chain(arithmetic),
                vec!["(( a || ls ))", "git status"]
            );
            assert_eq!(
                rewrite_command_no_prefixes(arithmetic, &[]),
                Some("(( a || ls )) && rtk git status".into())
            );

            let case = "case $x in a) echo esac;; ls) ls;; esac";
            assert_eq!(
                split_for_permissions(case),
                vec!["case $x in a", "echo esac", "ls", "ls", "esac"]
            );
            assert_eq!(
                split_command_chain(case),
                vec!["case $x in", "echo esac", "ls", "esac"]
            );
            assert_eq!(
                rewrite_command_no_prefixes(case, &[]),
                Some("case $x in a) echo esac;; ls) rtk ls;; esac".into())
            );
        }

        /// No piece that classification finds inside an expression or a
        /// pattern is a supported command, and an ordinary chain still yields
        /// one supported piece per command.
        #[test]
        fn no_supported_piece_inside_expressions_and_patterns() {
            use super::super::{Classification, classify_command};

            fn supported(cmd: &str) -> Vec<&str> {
                split_command_chain(cmd)
                    .into_iter()
                    .filter(|part| {
                        matches!(classify_command(part), Classification::Supported { .. })
                    })
                    .collect()
            }
            for cmd in [
                "[[ -f a || ls ]]",
                "[[ -f a && git status ]]",
                "(( a || b ))",
                "(( a || ls ))",
                "case $x in ls) echo 1;; git|ls) echo 2;; esac",
                "case $x in (ls) echo 1;; esac",
            ] {
                assert_eq!(supported(cmd), Vec::<&str>::new(), "{cmd:?}");
            }
            assert_eq!(supported("ls && git status"), vec!["ls", "git status"]);
        }

        /// Divergence, with a reason. A command inside `$( )` runs, so the gate
        /// descends into it and holds it to the deny rules. The other two stay
        /// out: the text around a substitution is not a command of its own, so
        /// descending would invent a `git log $` nobody ran, and what the
        /// substitution captures is a string the outer command is built from —
        /// filtering it would change that string rather than what reaches
        /// anyone.
        #[test]
        fn only_the_gate_descends_into_a_substitution() {
            let cmd = "git log $(git rev-parse HEAD~1)";
            assert!(
                split_for_permissions(cmd).contains(&"git rev-parse HEAD~1"),
                "the gate has to see a command that runs, wherever it sits"
            );
            assert_eq!(split_command_chain(cmd), vec![cmd]);
            // The outer command is rewritten and the captured one is left
            // exactly as written, which is the whole point of staying out.
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some("rtk git log $(git rev-parse HEAD~1)".into())
            );
        }

        /// An operator inside a substitution separates commands whose output
        /// becomes one word of the command being built. Reading it as a
        /// boundary out here rewrites into the captured string, so `echo` would
        /// print a command line nobody wrote.
        #[test]
        fn an_operator_inside_a_substitution_is_not_a_boundary() {
            for cmd in [
                "echo $(git status && git log)",
                "echo $(git status; git log)",
                "echo $(git status || git log)",
                "echo $(git status | grep x)",
            ] {
                assert_eq!(
                    rewrite_command_no_prefixes(cmd, &[]),
                    None,
                    "nothing in {cmd:?} is rewritable from outside the substitution"
                );
                assert_eq!(split_command_chain(cmd), vec![cmd], "{cmd}");
            }
        }

        /// `<( )` and `>( )` serve the outer command the same way `$( )` does,
        /// handing it a file to read instead of text to build with. Reading
        /// only `$` left `diff <(a) <(b)` classified as four commands, two of
        /// them the fragments `diff <` and `<`, and filtered the output that
        /// `diff` was about to compare.
        #[test]
        fn process_substitution_is_a_substitution_too() {
            let cmd = "diff <(git status) <(git log)";
            assert_eq!(split_command_chain(cmd), vec![cmd]);
            // The outer command is taken; what it is about to compare is not.
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some("rtk diff <(git status) <(git log)".into())
            );

            let writing = "tee >(cargo build) < in.txt";
            assert_eq!(split_command_chain(writing), vec![writing]);
            assert_eq!(rewrite_command_no_prefixes(writing, &[]), None);
        }

        /// Brackets nest. Counting only the `(` that follows a `$` closed the
        /// substitution one bracket early, so the real closing `)` read as a
        /// boundary and went missing from the segment.
        #[test]
        fn a_bracket_nested_in_a_substitution_still_closes_it() {
            let cmd = "$(cd /tmp && (git status; git log))";
            assert_eq!(split_command_chain(cmd), vec![cmd]);
            assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None);

            // The same nesting, with a command after it that must still be seen.
            let chained = "$(cd /tmp && (git status)) && cargo build";
            assert_eq!(
                split_command_chain(chained),
                vec!["$(cd /tmp && (git status))", "cargo build"]
            );
            assert_eq!(
                rewrite_command_no_prefixes(chained, &[]),
                Some("$(cd /tmp && (git status)) && rtk cargo build".into())
            );

            // A rewritable command after the inner `)` but still inside the
            // substitution. Miscount the nesting and the `&&` reads as a
            // boundary out here, so `git status` is rewritten into the captured
            // text — the only shape where the rewrite side of the count shows.
            assert_eq!(
                rewrite_command_no_prefixes("$( (ls) && git status )", &[]),
                None
            );
        }

        /// Divergence, with a reason. `{` opens a group only at a command
        /// position; anywhere else it is brace expansion, and `ls {a,b}.txt` is
        /// one command. `(` carries no such ambiguity, which is why it became a
        /// boundary and `{` did not. The gate copes by stripping the bracket
        /// before it matches rules, so nothing hides behind one.
        #[test]
        fn a_brace_group_is_not_a_boundary_outside_the_gate() {
            assert_eq!(
                rewrite_command_no_prefixes("ls {a,b}.txt", &[]),
                Some("rtk ls {a,b}.txt".into()),
                "brace expansion is part of the command, not a boundary"
            );
            let cmd = "{ git status; cargo build; }";
            assert_eq!(
                split_command_chain(cmd),
                vec!["{ git status", "cargo build", "}"]
            );
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some("{ git status; rtk cargo build; }".into())
            );
        }
    }

    mod multiline_blocks {
        use super::rewrite_command_no_prefixes;

        #[test]
        fn test_rewrites_each_line() {
            assert_eq!(
                rewrite_command_no_prefixes("git status\ngit log --oneline -3", &[]),
                Some("rtk git status\nrtk git log --oneline -3".into())
            );
        }

        #[test]
        fn test_preserves_blank_lines_comments_and_indentation() {
            assert_eq!(
                rewrite_command_no_prefixes("git status\n\n# check history\n  git log -3", &[]),
                Some("rtk git status\n\n# check history\n  rtk git log -3".into())
            );
        }

        #[test]
        fn test_compound_line_inside_block() {
            assert_eq!(
                rewrite_command_no_prefixes("cd /tmp && git status\ngrep -rn foo src", &[]),
                Some("cd /tmp && rtk git status\nrtk grep -rn foo src".into())
            );
        }

        #[test]
        fn test_crlf_line_keeps_its_cr_in_the_last_word() {
            // Bash runs `git status\r`: the `\r` is the last byte of `status\r`,
            // a subcommand git does not have, so that line is left alone.
            // Every separator byte is kept.
            assert_eq!(
                rewrite_command_no_prefixes("git status\r\ngit log -3", &[]),
                Some("git status\r\nrtk git log -3".into())
            );
        }

        #[test]
        fn test_newline_inside_quotes_rewrites_as_one_command() {
            // The quoted body is never treated as a command line of its own;
            // the whole thing is one logical command and gets one prefix.
            assert_eq!(
                rewrite_command_no_prefixes("git commit -m \"subject\ngit status in body\"", &[]),
                Some("rtk git commit -m \"subject\ngit status in body\"".into())
            );
            assert_eq!(
                rewrite_command_no_prefixes("git commit -m 'multi\nline\nmessage'", &[]),
                Some("rtk git commit -m 'multi\nline\nmessage'".into())
            );
        }

        #[test]
        fn test_lone_cr_inside_quotes_rewrites_as_one_command() {
            // A `\r` inside quotes is part of the argument, not a line break,
            // so the block is one logical command with a single prefix.
            assert_eq!(
                rewrite_command_no_prefixes("git commit -m 'subject\rin body'", &[]),
                Some("rtk git commit -m 'subject\rin body'".into())
            );
        }

        #[test]
        fn test_lone_cr_is_a_word_byte() {
            // A bare `\r` is not a line break: bash keeps `git` glued to the
            // preceding word, so the first line runs git with a `status\rgit`
            // subcommand and is left alone. Only the `\n` starts a new line.
            assert_eq!(
                rewrite_command_no_prefixes("git status\rgit log\ngit diff", &[]),
                Some("git status\rgit log\nrtk git diff".into())
            );
        }

        #[test]
        fn test_quoted_lone_cr_does_not_bail_out_the_block() {
            // The raw-break parity check counts `\n` only. Counting a quoted
            // lone `\r` too would make the block look like it hid a line from
            // the lexer and send it through unrewritten.
            assert_eq!(
                rewrite_command_no_prefixes("echo 'a\rb'\ngit log -3", &[]),
                Some("echo 'a\rb'\nrtk git log -3".into())
            );
        }

        #[test]
        fn test_unbalanced_swallowed_newline_passes_through() {
            assert_eq!(
                rewrite_command_no_prefixes("git commit -m \"subject\ngit status", &[]),
                None
            );
        }

        #[test]
        fn test_comment_apostrophe_swallowing_newline_passes_through() {
            // The lexer has no comment state: the apostrophe in `don't` opens
            // a quote that swallows the newline and hides the next line. The
            // block must pass through so native permission handling sees the
            // original command — never a partially rewritten one.
            assert_eq!(
                rewrite_command_no_prefixes("git status # don't\nrm -rf /tmp/x", &[]),
                None
            );
        }

        #[test]
        fn test_comment_apostrophe_hidden_in_later_segment_passes_through() {
            // Same hazard when a clean split point precedes the contaminated
            // line: the swallowed-newline check is global, not per-segment.
            assert_eq!(
                rewrite_command_no_prefixes("git log -3\ngit status # don't\nrm -rf /tmp/x", &[]),
                None
            );
        }

        #[test]
        fn test_comment_with_balanced_quotes_still_rewrites() {
            // Both apostrophes close before the newline, so the split is safe
            // and the trailing comment rides along untouched.
            assert_eq!(
                rewrite_command_no_prefixes(
                    "git status # isn't it what's expected\ngit log -3",
                    &[]
                ),
                Some("rtk git status # isn't it what's expected\nrtk git log -3".into())
            );
        }

        #[test]
        fn test_arithmetic_spanning_lines_passes_through() {
            // `(( x = ls ))` is arithmetic evaluation; injecting `rtk` before
            // `ls` would splice a command into arithmetic context.
            assert_eq!(rewrite_command_no_prefixes("(( x =\nls ))", &[]), None);
        }

        #[test]
        fn test_array_assignment_spanning_lines_passes_through() {
            // The inner line is an array element, not a command; rewriting it
            // would mutate the array's contents.
            assert_eq!(
                rewrite_command_no_prefixes("arr=(one\ngit status\ntwo)", &[]),
                None
            );
        }

        #[test]
        fn test_function_definition_spanning_lines_passes_through() {
            assert_eq!(
                rewrite_command_no_prefixes("foo() {\n  git status\n}", &[]),
                None
            );
        }

        #[test]
        fn test_continuation_operator_behind_comment_passes_through() {
            // Bash continues the pipeline across the newline even though the
            // line ends in comment text; the next line is a pipeline stage,
            // not an independent command.
            assert_eq!(
                rewrite_command_no_prefixes("git log | # keep pipeline\ngrep -f patterns.txt", &[]),
                None
            );
            assert_eq!(
                rewrite_command_no_prefixes("git status && # continue\ngit log -3", &[]),
                None
            );
        }

        #[test]
        fn test_ansi_c_escaped_quote_passes_through() {
            // Inside $'...' bash treats \' as a literal quote that does not
            // close the string, so the second line is string content — the
            // lexer can't see that, so the block forgoes the rewrite.
            assert_eq!(
                rewrite_command_no_prefixes("x=$'foo\\'\ngit status\n'", &[]),
                None
            );
        }

        #[test]
        fn test_ansi_c_without_escaped_quote_still_rewrites() {
            assert_eq!(
                rewrite_command_no_prefixes("echo $'a\\tb'\ngit status", &[]),
                Some("echo $'a\\tb'\nrtk git status".into())
            );
        }

        #[test]
        fn test_balanced_grouping_within_a_line_still_rewrites() {
            // `${HOME}` braces (quoted or not) must not trip the
            // unbalanced-grouping bail.
            assert_eq!(
                rewrite_command_no_prefixes("echo ${HOME}\ngit status", &[]),
                Some("echo ${HOME}\nrtk git status".into())
            );
            assert_eq!(
                rewrite_command_no_prefixes("echo \"${HOME}\"\ngit status", &[]),
                Some("echo \"${HOME}\"\nrtk git status".into())
            );
        }

        #[test]
        fn test_no_rewritable_line_passes_through() {
            assert_eq!(rewrite_command_no_prefixes("echo one\necho two", &[]), None);
        }

        #[test]
        fn test_already_rtk_lines_count_as_unchanged() {
            assert_eq!(
                rewrite_command_no_prefixes("rtk git status\necho done", &[]),
                None
            );
        }

        #[test]
        fn test_mixed_rtk_and_rewritable_line() {
            assert_eq!(
                rewrite_command_no_prefixes("rtk git status\ngit log -3", &[]),
                Some("rtk git status\nrtk git log -3".into())
            );
        }

        #[test]
        fn test_for_loop_block_passes_through() {
            assert_eq!(
                rewrite_command_no_prefixes("for f in a b; do\n  grep -n foo $f\ndone", &[]),
                None
            );
        }

        #[test]
        fn test_if_block_passes_through() {
            assert_eq!(
                rewrite_command_no_prefixes("if [ -d src ]; then\n  git status\nfi", &[]),
                None
            );
        }

        #[test]
        fn test_cross_line_and_list_joins_and_rewrites() {
            assert_eq!(
                rewrite_command_no_prefixes("git status &&\ngit log -3", &[]),
                Some("rtk git status &&\nrtk git log -3".into())
            );
        }

        #[test]
        fn test_cross_line_pipeline_joins_and_rewrites() {
            assert_eq!(
                rewrite_command_no_prefixes("git log |\ngrep feat", &[]),
                Some("git log |\nrtk grep feat".into())
            );
            assert_eq!(
                rewrite_command_no_prefixes("cargo test |&\ngrep FAILED", &[]),
                None
            );
        }

        #[test]
        fn test_cross_line_pipeline_unsafe_final_stage_passes_through() {
            assert_eq!(
                rewrite_command_no_prefixes("git log |\ngrep -f patterns.txt", &[]),
                None
            );
        }

        #[test]
        fn test_mixed_independent_and_continued_lines() {
            assert_eq!(
                rewrite_command_no_prefixes("grep -rn foo src\ngit status &&\ngit log -3", &[]),
                Some("rtk grep -rn foo src\nrtk git status &&\nrtk git log -3".into())
            );
        }

        #[test]
        fn test_blank_line_inside_continuation_joins() {
            assert_eq!(
                rewrite_command_no_prefixes("git status &&\n\ngit log -3", &[]),
                Some("rtk git status &&\n\nrtk git log -3".into())
            );
        }

        #[test]
        fn test_comment_line_inside_continuation_passes_through() {
            assert_eq!(
                rewrite_command_no_prefixes("git status &&\n# note\ngit log -3", &[]),
                None
            );
        }

        #[test]
        fn test_comment_directly_after_operator_passes_through() {
            assert_eq!(
                rewrite_command_no_prefixes("git log |# keep pipeline\ngrep -f patterns.txt", &[]),
                None
            );
        }

        #[test]
        fn test_conditional_expression_spanning_lines_passes_through() {
            assert_eq!(
                rewrite_command_no_prefixes("[[ -f a &&\n-f b ]]\ngit status", &[]),
                None
            );
            assert_eq!(
                rewrite_command_no_prefixes("git status\n[[\n-f a ]]", &[]),
                None
            );
        }

        #[test]
        fn test_balanced_conditional_line_still_rewrites() {
            assert_eq!(
                rewrite_command_no_prefixes("[[ -x foo ]] &&\ngit status", &[]),
                Some("[[ -x foo ]] &&\nrtk git status".into())
            );
        }

        #[test]
        fn test_subshell_spanning_lines_passes_through() {
            assert_eq!(rewrite_command_no_prefixes("(\n  git status\n)", &[]), None);
        }

        #[test]
        fn test_group_spanning_lines_passes_through() {
            assert_eq!(rewrite_command_no_prefixes("{\n  git status\n}", &[]), None);
        }

        #[test]
        fn test_heredoc_block_passes_through() {
            assert_eq!(
                rewrite_command_no_prefixes("git status\ncat <<EOF\nhello\nEOF", &[]),
                None
            );
        }

        #[test]
        fn test_heredoc_split_by_line_continuation_passes_through() {
            assert_eq!(
                rewrite_command_no_prefixes("cat <\\\n<EOF\ngit status\nEOF", &[]),
                None
            );
        }

        #[test]
        fn test_arithmetic_split_by_line_continuation_passes_through() {
            assert_eq!(
                rewrite_command_no_prefixes("echo $(\\\n(1+2))\ngit status", &[]),
                None
            );
        }
    }

    /// [`PipelineAnalysis`] with its token indices turned into byte offsets.
    struct PipelineOffsets {
        end_offset: usize,
        next_clause_offset: Option<usize>,
        final_stage_start: Option<usize>,
        all_consumers_safe: bool,
    }

    fn analyze_test_pipeline(cmd: &str) -> PipelineOffsets {
        let line = CompoundLex::new(cmd);
        let whole = line.whole().expect("test command must hold a command");
        let first_pipe = whole
            .tokens
            .iter()
            .position(|token| matches!(token.kind, TokenKind::Pipe(_)))
            .expect("test command must contain a pipe");
        let readings = read_grammar(whole.text, whole.tokens);
        let analysis = analyze_pipeline(&whole, &readings, 0, first_pipe);
        let offset = |i: usize| whole.tokens.get(i).map_or(cmd.len(), |t| t.offset);
        PipelineOffsets {
            end_offset: offset(analysis.end),
            next_clause_offset: analysis.next_clause.map(offset),
            final_stage_start: analysis.final_stage_start.map(offset),
            all_consumers_safe: analysis.all_consumers_safe,
        }
    }

    #[test]
    fn test_analyze_pipeline_finds_final_stage() {
        let cmd = "git log | grep feat | wc -l";
        let analysis = analyze_test_pipeline(cmd);

        assert_eq!(analysis.end_offset, cmd.len());
        assert_eq!(analysis.next_clause_offset, None);
        assert_eq!(
            cmd[analysis.final_stage_start.unwrap()..analysis.end_offset].trim(),
            "wc -l"
        );
    }

    #[test]
    fn test_analyze_pipeline_rejects_stderr_pipe() {
        let analysis = analyze_test_pipeline("cargo test |& grep FAILED");

        assert_eq!(analysis.final_stage_start, None);
    }

    #[test]
    fn test_analyze_pipeline_rejects_empty_stage() {
        let analysis = analyze_test_pipeline("cargo test | | grep FAILED");

        assert_eq!(analysis.final_stage_start, None);
    }

    #[test]
    fn test_analyze_pipeline_stops_at_next_clause() {
        let cmd = "cargo test | grep FAILED && git status";
        let analysis = analyze_test_pipeline(cmd);
        let next_clause_offset = cmd.find("&&").unwrap();

        assert_eq!(analysis.end_offset, next_clause_offset);
        assert_eq!(analysis.next_clause_offset, Some(next_clause_offset));
        assert_eq!(
            cmd[analysis.final_stage_start.unwrap()..analysis.end_offset].trim(),
            "grep FAILED"
        );
    }

    #[test]
    fn test_analyze_pipeline_all_consumers_safe() {
        for cmd in ["git log | tail -5", "git log | head | cat"] {
            assert!(analyze_test_pipeline(cmd).all_consumers_safe, "{cmd}");
        }
        for cmd in [
            "git log | wc -l",
            "git log | tail > f",
            "cargo test |& tail",
            "git log | FOO=1 tail",
        ] {
            assert!(!analyze_test_pipeline(cmd).all_consumers_safe, "{cmd}");
        }
    }

    #[test]
    fn test_pipeline_producer_safe_rule_set() {
        let mut safe_rules: Vec<_> = RULES
            .iter()
            .filter(|rule| rule.pipeline_safety.producer_safe())
            .map(|rule| rule.rtk_cmd)
            .collect();
        safe_rules.sort_unstable();
        safe_rules.dedup();

        assert_eq!(
            safe_rules,
            vec![
                "rtk ast-grep",
                "rtk brew",
                "rtk bundle",
                "rtk cargo",
                "rtk composer",
                "rtk df",
                "rtk diff",
                "rtk dotnet",
                "rtk du",
                "rtk ecs",
                "rtk find",
                "rtk git",
                "rtk go",
                "rtk golangci-lint run",
                "rtk grep",
                "rtk hadolint",
                "rtk helm",
                "rtk iptables",
                "rtk lint",
                "rtk liquibase",
                "rtk ls",
                "rtk markdownlint",
                "rtk mix",
                "rtk mvn",
                "rtk mypy",
                "rtk next",
                "rtk paratest",
                "rtk pest",
                "rtk phpstan",
                "rtk phpunit",
                "rtk pint",
                "rtk pio",
                "rtk pip",
                "rtk poetry",
                "rtk pre-commit",
                "rtk prettier",
                "rtk ps",
                "rtk pytest",
                "rtk quarto",
                "rtk rake",
                "rtk rg",
                "rtk rspec",
                "rtk rubocop",
                "rtk ruff",
                "rtk shellcheck",
                "rtk shopify",
                "rtk swift",
                "rtk systemctl",
                "rtk terraform",
                "rtk tofu",
                "rtk tree",
                "rtk trunk",
                "rtk wc",
                "rtk yamllint",
            ]
        );
    }

    #[test]
    fn test_pipeline_final_safe_rule_set() {
        let safe_rules: Vec<_> = RULES
            .iter()
            .filter(|rule| rule.pipeline_safety.final_safe())
            .map(|rule| rule.rtk_cmd)
            .collect();

        assert_eq!(safe_rules, vec!["rtk grep", "rtk rg"]);
    }

    #[test]
    fn test_pipeline_final_search_pattern_file_is_unsafe() {
        for command in [
            "grep -f patterns.txt input.txt",
            "grep -rfpatterns.txt input",
            "grep --file patterns.txt input.txt",
            "grep --file=patterns.txt input.txt",
            "rg -f patterns.txt input.txt",
            "rg --file=patterns.txt input.txt",
        ] {
            assert!(search_uses_pattern_file(&shell_split(command)), "{command}");
        }

        assert!(!search_uses_pattern_file(&shell_split("grep -- -f")));
        assert!(!search_uses_pattern_file(&shell_split("grep -F pattern")));
    }

    #[test]
    fn test_classify_git_status() {
        assert_eq!(
            classify_command("git status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_yadm_status() {
        assert_eq!(
            classify_command("yadm status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_yadm_diff() {
        assert_eq!(
            classify_command("yadm diff"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_rewrite_yadm_status() {
        assert_eq!(
            rewrite_command_no_prefixes("yadm status", &[]),
            Some("rtk git status".to_string())
        );
    }

    #[test]
    fn test_classify_git_diff_cached() {
        assert_eq!(
            classify_command("git diff --cached"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cargo_test_filter() {
        assert_eq!(
            classify_command("cargo test filter::"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 90.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_npx_tsc() {
        assert_eq!(
            classify_command("npx tsc --noEmit"),
            Classification::Supported {
                rtk_equivalent: "rtk tsc",
                category: "Build",
                estimated_savings_pct: 83.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cat_file() {
        assert_eq!(
            classify_command("cat src/main.rs"),
            Classification::Supported {
                rtk_equivalent: "rtk read",
                category: "Files",
                estimated_savings_pct: 60.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cat_redirect_not_supported() {
        // cat > file and cat >> file are writes, not reads — should not be classified as supported
        let write_commands = [
            "cat > /tmp/output.txt",
            "cat >> /tmp/output.txt",
            "cat file.txt > output.txt",
            "cat -n file.txt >> log.txt",
            "head -10 README.md > output.txt",
            "tail -f app.log > /dev/null",
        ];
        for cmd in &write_commands {
            if let Classification::Supported { .. } = classify_command(cmd) {
                panic!("{} should NOT be classified as Supported", cmd)
            }
            // Unsupported or Ignored is fine
        }
    }

    #[test]
    fn test_classify_cd_ignored() {
        assert_eq!(classify_command("cd /tmp"), Classification::Ignored);
    }

    #[test]
    fn test_classify_rtk_already() {
        for cmd in [
            "rtk git status",
            "rtk",
            "rtk\tgit status",
            "'rtk' git status",
            "\\rtk git status",
            "rtk proxy git status",
        ] {
            assert_eq!(classify_command(cmd), Classification::Ignored, "{cmd:?}");
        }
    }

    #[test]
    fn test_classify_echo_ignored() {
        assert_eq!(
            classify_command("echo hello world"),
            Classification::Ignored
        );
    }

    #[test]
    fn test_classify_htop_unsupported() {
        match classify_command("htop -d 10") {
            Classification::Unsupported { base_command } => {
                assert_eq!(base_command, "htop");
            }
            other => panic!("expected Unsupported, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_env_prefix_stripped() {
        assert_eq!(
            classify_command("GIT_SSH_COMMAND=ssh git push"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_sudo_not_stripped() {
        // sudo is intentionally not stripped: sudo commands stay unclassified so
        // they pass through unchanged rather than rewriting to a broken `sudo rtk`.
        match classify_command("sudo docker ps") {
            Classification::Unsupported { base_command } => {
                // sudo is not peeled off, so the command is seen as-is (not `docker`).
                assert_eq!(base_command, "sudo docker");
            }
            other => panic!("expected Unsupported, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_cargo_check() {
        assert_eq!(
            classify_command("cargo check"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cargo_check_all_targets() {
        assert_eq!(
            classify_command("cargo check --all-targets"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cargo_fmt_passthrough() {
        // Passthrough: `cargo fmt` runs unfiltered, so it saves nothing even
        // though the rule's other subcommands do.
        assert_eq!(
            classify_command("cargo fmt"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 0.0,
                status: RtkStatus::Passthrough,
            }
        );
    }

    #[test]
    fn test_classify_cargo_clippy_savings() {
        assert_eq!(
            classify_command("cargo clippy --all-targets"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_registry_covers_all_cargo_subcommands() {
        // Verify that every CargoCommand variant (Build, Test, Clippy, Check, Fmt)
        // except Other has a matching pattern in the registry
        for subcmd in ["build", "test", "clippy", "check", "fmt"] {
            let cmd = format!("cargo {subcmd}");
            match classify_command(&cmd) {
                Classification::Supported { .. } => {}
                other => panic!("cargo {subcmd} should be Supported, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_registry_covers_all_git_subcommands() {
        // Verify that every GitCommand subcommand has a matching pattern
        for subcmd in [
            "status", "log", "diff", "show", "add", "commit", "push", "pull", "branch", "fetch",
            "stash", "worktree",
        ] {
            let cmd = format!("git {subcmd}");
            match classify_command(&cmd) {
                Classification::Supported { .. } => {}
                other => panic!("git {subcmd} should be Supported, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_classify_find_not_blocked_by_fi() {
        // Regression: "fi" in IGNORED_PREFIXES used to shadow "find" commands
        // because "find".starts_with("fi") is true. "fi" should only match exactly.
        assert_eq!(
            classify_command("find . -name foo"),
            Classification::Supported {
                rtk_equivalent: "rtk find",
                category: "Files",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_fi_still_ignored_exact() {
        // Bare "fi" (shell keyword) should still be ignored
        assert_eq!(classify_command("fi"), Classification::Ignored);
    }

    #[test]
    fn test_done_still_ignored_exact() {
        // Bare "done" (shell keyword) should still be ignored
        assert_eq!(classify_command("done"), Classification::Ignored);
    }

    /// A segment whose first word is a bash reserved word is grammar around
    /// commands, never a command of that name, whatever follows the word.
    #[test]
    fn test_reserved_word_segments_are_ignored() {
        for cmd in [
            "if git status",
            "then\tgit log",
            "elif true",
            "else",
            "fi >/dev/null",
            "case $x in",
            "in a b",
            "esac",
            "for f in a b",
            "select x in a",
            "while true",
            "until false",
            "do git status",
            "done < f",
            "function f",
            "coproc git status",
            "time cargo test",
            "[[ -f x ]]",
            "[[(-f x)]]",
            "]]",
            "(( x++ ))",
            "((x>1))",
            "((x || y",
        ] {
            assert_eq!(classify_command(cmd), Classification::Ignored, "{cmd:?}");
        }
        // Quoted, escaped or glued to more text, the word is an ordinary one.
        for cmd in [
            "\"if\" x",
            "\\if x",
            "donex",
            "\"[[\"(x)",
            "\\[[(x)",
            "\"((\" x",
            "\\((x",
            // Two subshells, one inside the other, not an arithmetic command.
            "((ls) )",
        ] {
            assert_ne!(classify_command(cmd), Classification::Ignored, "{cmd:?}");
        }
        // After `|`, `time` is the program `time`, and other reserved words
        // read as they do where a pipeline starts.
        for (cmd, ignored) in [
            ("time cargo test", false),
            ("time -p git status", false),
            ("if git status", true),
            ("[[ -f x ]]", true),
            ("(( x++ ))", true),
        ] {
            assert_eq!(
                classify_command_at(cmd, CommandStart::PipeStage) == Classification::Ignored,
                ignored,
                "{cmd:?}"
            );
        }
    }

    #[test]
    fn test_split_chain_and() {
        assert_eq!(split_command_chain("a && b"), vec!["a", "b"]);
    }

    #[test]
    fn test_split_chain_semicolon() {
        assert_eq!(split_command_chain("a ; b"), vec!["a", "b"]);
    }

    /// Every stage is a command the agent ran, so classification sees them
    /// all. Stopping at the first `|` hid the rest from the report (#3683).
    #[test]
    fn test_split_chain_covers_every_pipeline_stage() {
        assert_eq!(split_command_chain("a | b"), vec!["a", "b"]);
        assert_eq!(split_command_chain("a | b | c"), vec!["a", "b", "c"]);
        assert_eq!(
            split_command_chain("a | b && c"),
            vec!["a", "b", "c"],
            "a pipeline followed by another clause"
        );
        // A heredoc or arithmetic expansion is still handed over whole.
        assert_eq!(
            split_command_chain("cat <<EOF | wc -l"),
            vec!["cat <<EOF | wc -l"]
        );
    }

    /// Seeing a stage is not the same as being able to rewrite it, and the
    /// report must not promise a saving the hook declines to take.
    #[test]
    fn test_only_the_stages_the_rewriter_takes_are_reached() {
        for (cmd, expected) in [
            // The producer is rewritten; `tail` reads what RTK filtered, so
            // rewriting it too would change what the pipeline prints.
            ("cargo test 2>&1 | tail -1", vec![true, false]),
            // `rtk grep` is pipeline-final safe, so here the consumer is the
            // stage that gets taken and `cargo test` is left feeding it.
            ("cargo test | grep FAILED", vec![false, true]),
            // Separate clauses, each rewritten on its own.
            ("git status && cargo build", vec![true, true]),
            // Nothing RTK handles anywhere in the line.
            ("frobnicate | wibble", vec![false, false]),
            // Already routed through RTK, so there is nothing left to take.
            ("cargo test | rtk grep foo", vec![false, false]),
            // The bypass stops the command carrying it, and the pipeline it
            // feeds goes with it.
            (
                "cargo test | RTK_DISABLED=1 grep FAILED",
                vec![false, false],
            ),
        ] {
            assert_eq!(
                stages_the_rewriter_reaches(cmd, &[], &[]),
                expected,
                "{cmd:?} rewrites to {:?}",
                rewrite_command_no_prefixes(cmd, &[])
            );
        }
    }

    /// A command that feeds a pipe writes for another command to consume, and
    /// the report treats that differently from one whose output came back — so
    /// the flag has to follow the `|`, not the position in the chain.
    #[test]
    fn test_only_a_command_before_a_pipe_feeds_one() {
        for (cmd, expected) in [
            ("a | b | c", vec![true, true, false]),
            ("a && b | c ; d", vec![false, true, false, false]),
            ("a || b", vec![false, false]),
            ("a", vec![false]),
            ("cat <<EOF | wc -l", vec![false]),
        ] {
            let feeds: Vec<bool> = split_command_chain_parts(cmd)
                .iter()
                .map(|p| p.feeds_pipe)
                .collect();
            assert_eq!(feeds, expected, "wrong pipe-feeding for {cmd:?}");
        }
    }

    /// One flag per part, whatever the line looks like — a caller indexes this
    /// by part, so a short answer would silently point at the wrong command.
    #[test]
    fn test_every_part_gets_a_reachability_flag() {
        for cmd in [
            "cargo test | grep FAILED",
            "a && b || c ; d | e",
            "cat <<EOF | wc -l",
            "echo $((1 + 2))",
            "git status",
        ] {
            assert_eq!(
                stages_the_rewriter_reaches(cmd, &[], &[]).len(),
                split_command_chain(cmd).len(),
                "flag count does not match part count for {cmd:?}"
            );
        }
    }

    /// Whether the rewrite of `cmd` refused a command on `RTK_DISABLED=`,
    /// which is when #508's warning is printed.
    fn refused_on_disabled(cmd: &str) -> bool {
        rewrite_line(cmd, &[], &[]).disabled
    }

    /// The bypass is per command, so the prefix can sit on any of them — the
    /// warning must not depend on it being the first thing on the line.
    #[test]
    fn test_the_refusal_on_rtk_disabled_is_found_on_any_command() {
        for cmd in [
            "RTK_DISABLED=1 git status",
            "cargo test | RTK_DISABLED=1 grep FAILED",
            "git status && RTK_DISABLED=1 cargo build",
            "git status\nRTK_DISABLED=1 cargo build",
            "timeout 5 env RTK_DISABLED=1 git status",
            "nice -n 5 env RTK_DISABLED=1 git status",
            "RTK_DISABLED+=1 git status",
            "RTK_DISABLED=1 frobnicate --all",
        ] {
            assert!(refused_on_disabled(cmd), "{cmd:?}");
        }
        for (cmd, why) in [
            ("git status", "no marker"),
            ("SOME_VAR=1 git status", "another name"),
            ("XRTK_DISABLED=1 git status", "another name"),
            ("NOTE='set RTK_DISABLED=1 x' git status", "a value"),
            ("env MSG=RTK_DISABLED=1 git status", "a value"),
            ("echo RTK_DISABLED=1", "an argument is not a prefix"),
            (
                "echo \"a\nRTK_DISABLED=1 b\"",
                "a newline inside quotes does not start a command",
            ),
            (
                "cat <<EOF\nRTK_DISABLED=1 b\nEOF",
                "a heredoc body is data, and the line is refused before any walk",
            ),
            ("RTK_DISABLED=1; git status", "a statement of its own"),
            ("timeout 5 RTK_DISABLED=1 git status", "a program's name"),
            ("noglob RTK_DISABLED=1 git status", "a program's name"),
            ("uv run RTK_DISABLED=1 git status", "a program's name"),
            ("RTK_DISABLED=1 git status | wc -l", "no walk reads it"),
        ] {
            assert!(!refused_on_disabled(cmd), "{cmd:?}: {why}");
        }
    }

    /// The bypass stops the command that carries it, not the clause beside it.
    #[test]
    fn test_a_bypass_does_not_stop_a_sibling_clause() {
        assert_eq!(
            stages_the_rewriter_reaches("RTK_DISABLED=1 cargo test && git status", &[], &[]),
            vec![false, true]
        );
    }

    #[test]
    fn test_split_single() {
        assert_eq!(split_command_chain("git status"), vec!["git status"]);
    }

    #[test]
    fn test_split_quoted_and() {
        assert_eq!(
            split_command_chain(r#"echo "a && b""#),
            vec![r#"echo "a && b""#]
        );
    }

    #[test]
    fn test_split_heredoc_no_split() {
        let cmd = "cat <<'EOF'\nhello && world\nEOF";
        assert_eq!(split_command_chain(cmd), vec![cmd]);
    }

    #[test]
    fn test_classify_mypy() {
        assert_eq!(
            classify_command("mypy src/"),
            Classification::Supported {
                rtk_equivalent: "rtk mypy",
                category: "Build",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_python_m_mypy() {
        assert_eq!(
            classify_command("python3 -m mypy --strict"),
            Classification::Supported {
                rtk_equivalent: "rtk mypy",
                category: "Build",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    // --- rewrite_command tests ---

    #[test]
    fn test_rewrite_git_status() {
        assert_eq!(
            rewrite_command_no_prefixes("git status", &[]),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_git_checkout() {
        assert_eq!(
            rewrite_command_no_prefixes("git checkout main", &[]),
            Some("rtk git checkout main".into())
        );
    }

    #[test]
    fn test_rewrite_git_log() {
        assert_eq!(
            rewrite_command_no_prefixes("git log -10", &[]),
            Some("rtk git log -10".into())
        );
    }

    // --- git -C <path> support (#555) ---

    #[test]
    fn test_rewrite_git_dash_c_status() {
        assert_eq!(
            rewrite_command_no_prefixes("git -C /path/to/repo status", &[]),
            Some("rtk git -C /path/to/repo status".into())
        );
    }

    #[test]
    fn test_rewrite_git_dash_c_log() {
        assert_eq!(
            rewrite_command_no_prefixes("git -C /tmp/myrepo log --oneline -5", &[]),
            Some("rtk git -C /tmp/myrepo log --oneline -5".into())
        );
    }

    #[test]
    fn test_rewrite_git_dash_c_diff() {
        assert_eq!(
            rewrite_command_no_prefixes("git -C /home/user/project diff --name-only", &[]),
            Some("rtk git -C /home/user/project diff --name-only".into())
        );
    }

    #[test]
    fn test_classify_git_dash_c() {
        let result = classify_command("git -C /tmp status");
        assert!(
            matches!(
                result,
                Classification::Supported {
                    rtk_equivalent: "rtk git",
                    ..
                }
            ),
            "git -C should be classified as supported, got: {:?}",
            result
        );
    }

    // --- pnpm global option stripping (-r / --filter / -w) ---

    #[test]
    fn test_rewrite_pnpm_recursive_install() {
        assert_eq!(
            rewrite_command_no_prefixes("pnpm -r install", &[]),
            Some("rtk pnpm -r install".into())
        );
    }

    #[test]
    fn test_rewrite_pnpm_filter_install() {
        assert_eq!(
            rewrite_command_no_prefixes("pnpm --filter @app install", &[]),
            Some("rtk pnpm --filter @app install".into())
        );
    }

    #[test]
    fn test_rewrite_pnpm_filter_short_install() {
        assert_eq!(
            rewrite_command_no_prefixes("pnpm -F @app install", &[]),
            Some("rtk pnpm -F @app install".into())
        );
    }

    #[test]
    fn test_rewrite_pnpm_filter_eq_install() {
        assert_eq!(
            rewrite_command_no_prefixes("pnpm --filter=@app install", &[]),
            Some("rtk pnpm --filter=@app install".into())
        );
    }

    #[test]
    fn test_rewrite_pnpm_workspace_root_install() {
        assert_eq!(
            rewrite_command_no_prefixes("pnpm -w install", &[]),
            Some("rtk pnpm -w install".into())
        );
    }

    #[test]
    fn test_rewrite_pnpm_recursive_filter_combo() {
        assert_eq!(
            rewrite_command_no_prefixes("pnpm -r --filter @app list", &[]),
            Some("rtk pnpm -r --filter @app list".into())
        );
    }

    // No-regression: bare forms behave exactly as before.
    #[test]
    fn test_rewrite_pnpm_bare_install_unchanged() {
        assert_eq!(
            rewrite_command_no_prefixes("pnpm install", &[]),
            Some("rtk pnpm install".into())
        );
    }

    #[test]
    fn test_rewrite_pnpm_run_build_unchanged() {
        assert_eq!(
            rewrite_command_no_prefixes("pnpm run build", &[]),
            Some("rtk pnpm run build".into())
        );
    }

    // Bare `pnpm build` is still NOT rewritten: it would only hit the passthrough
    // (no output parser), so rewriting it would add false-positive surface for zero
    // savings. Stripping global opts must not change this.
    #[test]
    fn test_rewrite_pnpm_bare_build_none() {
        assert_eq!(rewrite_command_no_prefixes("pnpm build", &[]), None);
    }

    // False-positive guards.
    #[test]
    fn test_rewrite_pnpm_filter_no_subcommand_none() {
        // A filter with no subcommand must not be rewritten.
        assert_eq!(rewrite_command_no_prefixes("pnpm --filter @app", &[]), None);
    }

    #[test]
    fn test_rewrite_pnpm_unknown_flag_not_stripped() {
        // `-x` is not a known global opt → not stripped → no subcommand → None.
        assert_eq!(rewrite_command_no_prefixes("pnpm -x build", &[]), None);
        // Load-bearing case for the fixed-set design: `install` IS a routed
        // subcommand, so if `-x` were stripped this would rewrite to
        // `rtk pnpm -x install`, which reaches clap and dies. Only the fixed
        // allowlist keeps it a safe passthrough (None).
        assert_eq!(rewrite_command_no_prefixes("pnpm -x install", &[]), None);
    }

    #[test]
    fn test_rewrite_pnpm_recursive_lint_safe_noop() {
        // `pnpm lint` classifies as Supported, but the ORIGINAL `pnpm -r lint`
        // matches no lint rewrite-prefix → safe no-op (never a malformed rewrite).
        assert_eq!(rewrite_command_no_prefixes("pnpm -r lint", &[]), None);
    }

    #[test]
    fn test_classify_pnpm_flag_first_tool_stays_unsupported() {
        // #3275 blocker: the strip must not make a tool rule reachable via
        // `pnpm exec`/`pnpm run` classify as Supported — its rewrite matches the
        // original flag-first text and never fires, so a Supported verdict would
        // advertise savings `rtk discover`/`rtk session` can never deliver. These
        // must classify exactly as on develop: Unsupported(pnpm).
        for cmd in [
            "pnpm -r lint",
            "pnpm -r exec eslint .",
            "pnpm --filter @app exec vitest run",
            "pnpm -F web exec playwright test",
            "pnpm -r exec tsc --noEmit",
            "pnpm -w exec next build",
        ] {
            assert!(
                matches!(classify_command(cmd), Classification::Unsupported { .. }),
                "{cmd} must stay Unsupported (rewrite can't fire), got: {:?}",
                classify_command(cmd)
            );
        }
    }

    #[test]
    fn test_classify_pnpm_flag_first_install_still_supported() {
        // The gate keeps everything the PR claims: flag-first forms that route to
        // the `rtk pnpm` rule stay Supported.
        for cmd in [
            "pnpm -r install",
            "pnpm --filter @app list",
            "pnpm -w install",
            "pnpm -r outdated",
        ] {
            assert!(
                matches!(
                    classify_command(cmd),
                    Classification::Supported {
                        rtk_equivalent: "rtk pnpm",
                        ..
                    }
                ),
                "{cmd} must classify as rtk pnpm, got: {:?}",
                classify_command(cmd)
            );
        }
    }

    #[test]
    fn test_rewrite_pnpm_extra_whitespace() {
        // Extra spaces before the global flag must not skip the strip
        // (`PNPM_GLOBAL_OPT` is `^`-anchored, so the slice is trimmed first).
        assert_eq!(
            rewrite_command_no_prefixes("pnpm  -r  install", &[]),
            Some("rtk pnpm -r  install".into())
        );
    }

    #[test]
    fn test_pnpm_tab_separator_no_classify_rewrite_divergence() {
        // The global-option strip gates on a literal `pnpm `, so a tab-separated
        // `pnpm` carrying one is never classified into a shape the rewrite
        // declines — both sides say no, which is what #3275 is about.
        let cmd = "pnpm\t-r install";
        assert!(
            matches!(classify_command(cmd), Classification::Unsupported { .. }),
            "tab-separated pnpm must stay Unsupported, got: {:?}",
            classify_command(cmd)
        );
        assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None);

        // With no global option to strip there is nothing to disagree about, and
        // a tab separates a command from its prefix like any other blank.
        assert_eq!(
            rewrite_command_no_prefixes("pnpm\tinstall", &[]),
            Some("rtk pnpm install".into())
        );
    }

    #[test]
    fn test_rewrite_cargo_test() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo test", &[]),
            Some("rtk cargo test".into())
        );
    }

    #[test]
    fn test_classify_ctest() {
        assert_eq!(
            classify_command("ctest -R smoke --output-on-failure"),
            Classification::Supported {
                rtk_equivalent: "rtk ctest",
                category: "Tests",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_rewrite_ctest() {
        assert_eq!(
            rewrite_command_no_prefixes("ctest -R smoke --output-on-failure", &[]),
            Some("rtk ctest -R smoke --output-on-failure".into())
        );
    }

    #[test]
    fn test_rewrite_compound_and() {
        assert_eq!(
            rewrite_command_no_prefixes("git add . && cargo test", &[]),
            Some("rtk git add . && rtk cargo test".into())
        );
    }

    #[test]
    fn test_rewrite_compound_three_segments() {
        assert_eq!(
            rewrite_command_no_prefixes(
                "cargo fmt --all && cargo clippy --all-targets && cargo test",
                &[]
            ),
            Some("rtk cargo fmt --all && rtk cargo clippy --all-targets && rtk cargo test".into())
        );
    }

    #[test]
    fn test_rewrite_already_rtk() {
        assert_eq!(
            rewrite_command_no_prefixes("rtk git status", &[]),
            Some("rtk git status".into())
        );
    }

    /// `rtk` is recognised as the first word, however bash ends that word and
    /// quotes or escapes it, and only an unquoted operator makes the line more
    /// than that one command.
    #[test]
    fn test_rewrite_already_rtk_reads_words_and_operators() {
        for cmd in [
            "rtk",
            "rtk\tls",
            "rtk  git status",
            "rtk\tgit\tstatus",
            "rtk grep 'a|b' src",
            "rtk git commit -m \"a && b; c & d\"",
            "rtk grep a\\|b src",
            "'rtk' git status",
            "\"rtk\" git status",
            "\\rtk git status",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some(cmd.to_string()),
                "{cmd:?}"
            );
        }
        for cmd in ["rtkx ls", "rtk\x0bls", "rtk\rls"] {
            assert_ne!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some(cmd.to_string()),
                "{cmd:?}"
            );
        }
    }

    /// A command joined to a leading `rtk` command is rewritten whatever blanks
    /// sit around the operator: tabs, several spaces, or none.
    #[test]
    fn test_rewrite_after_rtk_for_every_operator_spacing() {
        for gap in ["", " ", "  ", "\t", " \t"] {
            for op in ["&&", "||", ";", "&"] {
                let cmd = format!("rtk git status{gap}{op}{gap}git log");
                assert_eq!(
                    rewrite_command_no_prefixes(&cmd, &[]),
                    Some(format!("rtk git status{gap}{op}{gap}rtk git log")),
                    "{cmd:?}"
                );
            }
            let cmd = format!("rtk ls{gap}|{gap}grep x");
            assert_eq!(
                rewrite_command_no_prefixes(&cmd, &[]),
                Some(format!("rtk ls{gap}|{gap}rtk grep x")),
                "{cmd:?}"
            );
        }
        for (cmd, expected) in [
            ("rtk git status &git log", "rtk git status &rtk git log"),
            ("rtk git status& git log", "rtk git status& rtk git log"),
            (
                "\\rtk git status && git log",
                "\\rtk git status && rtk git log",
            ),
            (
                "git log && 'rtk' git status",
                "rtk git log && 'rtk' git status",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some(expected.to_string()),
                "{cmd:?}"
            );
        }
    }

    #[test]
    fn test_rewrite_background_single_amp() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo test & git status", &[]),
            Some("rtk cargo test & rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_background_unsupported_right() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo test & htop", &[]),
            Some("rtk cargo test & htop".into())
        );
    }

    #[test]
    fn test_rewrite_background_does_not_affect_double_amp() {
        // `&&` must still work after adding `&` support
        assert_eq!(
            rewrite_command_no_prefixes("cargo test && git status", &[]),
            Some("rtk cargo test && rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_unsupported_returns_none() {
        assert_eq!(rewrite_command_no_prefixes("htop", &[]), None);
    }

    #[test]
    fn test_rewrite_ignored_cd() {
        assert_eq!(rewrite_command_no_prefixes("cd /tmp", &[]), None);
    }

    #[test]
    fn test_rewrite_toml_orphan_jj() {
        assert_eq!(
            rewrite_command_no_prefixes("jj log", &[]),
            Some("rtk jj log".into())
        );
    }

    #[test]
    fn test_rewrite_toml_orphan_jq() {
        assert_eq!(
            rewrite_command_no_prefixes("jq .", &[]),
            Some("rtk jq .".into())
        );
    }

    #[test]
    fn test_rewrite_toml_orphan_just() {
        assert_eq!(
            rewrite_command_no_prefixes("just build", &[]),
            Some("rtk just build".into())
        );
    }

    #[test]
    fn test_rewrite_toml_absolute_path() {
        assert_eq!(
            rewrite_command_no_prefixes("/usr/bin/jj log", &[]),
            Some("rtk /usr/bin/jj log".into())
        );
    }

    #[test]
    fn test_rewrite_toml_redirect_suffix_preserved() {
        assert_eq!(
            rewrite_command_no_prefixes("jj log 2>&1", &[]),
            Some("rtk jj log 2>&1".into())
        );
    }

    #[test]
    fn test_rewrite_toml_pipe_rewrites_only_safe_final() {
        assert_eq!(
            rewrite_command_no_prefixes("jj log | grep change", &[]),
            Some("jj log | rtk grep change".into())
        );
    }

    #[test]
    fn test_rewrite_toml_compound() {
        assert_eq!(
            rewrite_command_no_prefixes("jj diff && jq .", &[]),
            Some("rtk jj diff && rtk jq .".into())
        );
    }

    #[test]
    fn test_rewrite_toml_env_prefix() {
        assert_eq!(
            rewrite_command_no_prefixes("FOO=bar jj log", &[]),
            Some("FOO=bar rtk jj log".into())
        );
    }

    #[test]
    fn test_rewrite_toml_respects_exclude() {
        let excluded = vec!["jj".to_string()];
        assert_eq!(rewrite_command_no_prefixes("jj log", &excluded), None);
    }

    #[test]
    fn test_rewrite_toml_exclude_matches_absolute_path() {
        let excluded = vec!["jj".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("/usr/bin/jj log", &excluded),
            None
        );
    }

    #[test]
    fn test_rewrite_toml_unknown_command_still_none() {
        assert_eq!(rewrite_command_no_prefixes("frobnicate xyz", &[]), None);
    }

    #[test]
    fn test_rewrite_with_env_prefix() {
        assert_eq!(
            rewrite_command_no_prefixes("GIT_SSH_COMMAND=ssh git push", &[]),
            Some("GIT_SSH_COMMAND=ssh rtk git push".into())
        );
    }

    #[test]
    fn test_rewrite_tsc() {
        let commands = vec![
            "npm exec tsc",
            "npm rum tsc",
            "npm run tsc",
            "npm run-script tsc",
            "npm urn tsc",
            "npm x tsc",
            "pnpm dlx tsc",
            "pnpm exec tsc",
            "pnpm run tsc",
            "pnpm run-script tsc",
            "npm tsc",
            "npx tsc",
            "pnpm tsc",
            "pnpx tsc",
            "tsc",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command_no_prefixes(&format!("{command} --noEmit"), &[]),
                Some("rtk tsc --noEmit".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_cat_file() {
        assert_eq!(
            rewrite_command_no_prefixes("cat src/main.rs", &[]),
            Some("rtk read src/main.rs".into())
        );
    }

    #[test]
    fn test_rewrite_cat_with_incompatible_flags_skipped() {
        // cat flags with different semantics than rtk read — skip rewrite
        assert_eq!(rewrite_command_no_prefixes("cat -A file.cpp", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("cat -v file.txt", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("cat -e file.txt", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("cat -t file.txt", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("cat -s file.txt", &[]), None);
        assert_eq!(
            rewrite_command_no_prefixes("cat --show-all file.txt", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_cat_with_compatible_flags() {
        // cat -n (line numbers) maps to rtk read -n — allow rewrite
        assert_eq!(
            rewrite_command_no_prefixes("cat -n file.txt", &[]),
            Some("rtk read -n file.txt".into())
        );
    }

    /// `cat`'s options are read as `cat` receives them: wherever they sit, however
    /// they are quoted, and after a tab as after a space.
    #[test]
    fn test_rewrite_cat_options_are_read_as_cat_receives_them() {
        for cmd in [
            "cat\t-A f",
            "cat f -A",
            "cat '-A' f",
            "cat -nA f",
            "cat -- f",
            "cat --number f",
            "cat\t>out f",
            "cat -n",
        ] {
            assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None, "{cmd:?}");
        }
        for (cmd, expected) in [
            ("cat\t-n f", "rtk read -n f"),
            ("cat -n\tf", "rtk read -n\tf"),
            ("cat f -n", "rtk read f -n"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "{cmd:?}"
            );
        }
    }

    /// `head` and `tail` are recognised by their first word, which a tab ends as
    /// a space does, so the line-range rewrite applies to both spellings.
    #[test]
    fn test_rewrite_head_tail_after_a_tab() {
        for (cmd, expected) in [
            ("tail\t-n 5 f", Some("rtk read f --tail-lines 5")),
            (
                "head\t-3 file.txt",
                Some("rtk read file.txt --head-lines 3"),
            ),
            ("head\tfile.txt", Some("rtk read file.txt --head-lines 10")),
            ("tail\t-n 5 a b", None),
            ("tail\t-f log", None),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                expected,
                "{cmd:?}"
            );
        }
    }

    /// A command ends where its last word does, and an escaped blank is part of
    /// that word: bash reads `cat f\ ` as the file `f␠` and `head\ ` as the
    /// program `head␠`. The rewrite keeps the blank, and a stage whose program is
    /// `head␠` is no known consumer, so the producer in front of it stays too.
    #[test]
    fn test_an_escaped_trailing_blank_stays_in_its_word() {
        for (cmd, expected) in [
            ("cat f\\ ", Some("rtk read f\\ ")),
            ("cat f\\\t", Some("rtk read f\\\t")),
            ("cat f\\  ", Some("rtk read f\\ ")),
            ("FOO=1 cat f\\ ", Some("FOO=1 rtk read f\\ ")),
            ("nice cat f\\ ", Some("nice rtk read f\\ ")),
            (
                "git status && cat f\\ ",
                Some("rtk git status && rtk read f\\ "),
            ),
            ("git log | grep x\\ ", Some("git log | rtk grep x\\ ")),
            ("ls\n  cat f\\ ", Some("rtk ls\n  rtk read f\\ ")),
            ("git log | head\\ ", None),
            ("golangci-lint run f\\ ", Some("rtk golangci-lint run f\\ ")),
            ("cat 'f '", Some("rtk read 'f '")),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                expected,
                "{cmd:?}"
            );
        }
    }

    #[test]
    fn test_rewrite_rg_pattern() {
        assert_eq!(
            rewrite_command_no_prefixes("rg \"fn main\"", &[]),
            Some("rtk rg \"fn main\"".into())
        );
    }

    /// `;;`, `;&` and `;;&` terminate a `case` arm. The rewrite rebuilds the
    /// text around each operator it splits on, so a terminator that lexes as
    /// two operators comes back as `; ;` — which bash rejects — or as `; &`,
    /// which runs the arm in the background instead of falling through.
    #[test]
    fn test_case_terminators_survive_a_rewrite() {
        for (cmd, expected) in [
            (
                "ls /tmp; case x in a) echo 1;; *) echo 2;; esac",
                "rtk ls /tmp; case x in a) echo 1;; *) echo 2;; esac",
            ),
            (
                "ls /tmp; case x in a) echo 1;& *) echo 2;; esac",
                "rtk ls /tmp; case x in a) echo 1;& *) echo 2;; esac",
            ),
            (
                "ls /tmp; case x in a) echo 1;;& *) echo 2;; esac",
                "rtk ls /tmp; case x in a) echo 1;;& *) echo 2;; esac",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "case terminator was not preserved: {cmd}"
            );
        }
    }

    /// A `case` reached through a block keyword is the shape this was reported
    /// from, and it is not the shape above: there the rewritable command comes
    /// first and the `case` trails it, here the `case` is passed over on the
    /// way to the command. Both have to keep their terminators, so both are
    /// pinned, and `while`/`until` are here because they broke the same way.
    #[test]
    fn test_case_terminators_survive_inside_a_block() {
        for (cmd, expected) in [
            (
                "for f in a.mjs b.test.mjs; do case \"$f\" in *.test.mjs) continue;; esac; grep -q x README.md || echo miss; done",
                "for f in a.mjs b.test.mjs; do case \"$f\" in *.test.mjs) continue;; esac; rtk grep -q x README.md || echo miss; done",
            ),
            (
                "case x in x) echo A;; esac; ls /tmp",
                "case x in x) echo A;; esac; rtk ls /tmp",
            ),
            (
                "if true; then case x in x) echo a;; esac; ls /tmp; fi",
                "if true; then case x in x) echo a;; esac; rtk ls /tmp; fi",
            ),
            (
                "while read -r l; do case \"$l\" in a) continue;; esac; ls /tmp; done",
                "while read -r l; do case \"$l\" in a) continue;; esac; rtk ls /tmp; done",
            ),
            (
                "until false; do case x in a) break;; esac; ls /tmp; done",
                "until false; do case x in a) break;; esac; rtk ls /tmp; done",
            ),
            // A function body is a block too, and `f()` is the one place a
            // `()` pair defines rather than groups.
            (
                "f() { case x in x) echo a;; esac; }; f; ls /tmp",
                "f() { case x in x) echo a;; esac; }; f; rtk ls /tmp",
            ),
            (
                "f () { case x in x) echo a;; esac; }; f; ls /tmp",
                "f () { case x in x) echo a;; esac; }; f; rtk ls /tmp",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "case terminator was not preserved: {cmd}"
            );
        }
    }

    /// Inside `[[ … ]]` nothing is a command: `&&`, `||`, `(` and `)` there
    /// are operators of the expression, so the rewrite never puts `rtk`
    /// inside one, wherever the `[[` sits and whatever the expression holds.
    #[test]
    fn test_rewrite_reads_a_test_expression_as_one_command() {
        for (cmd, expected) in [
            ("[[ -f a || ls ]]", None),
            ("[[ -f a || ls ]] && ls", Some("[[ -f a || ls ]] && rtk ls")),
            (
                "[[ ( -f a || ls ) && -e c ]] || git status",
                Some("[[ ( -f a || ls ) && -e c ]] || rtk git status"),
            ),
            ("if [[ -f a || ls ]]; then :; fi", None),
            ("while [[ -f a && ls ]]; do :; done", None),
            (
                "ls && [[ -f a || ls ]] && git status",
                Some("rtk ls && [[ -f a || ls ]] && rtk git status"),
            ),
            (
                "time [[ -f a || ls ]] && ls",
                Some("time [[ -f a || ls ]] && rtk ls"),
            ),
            // `time`'s options `-p` and `--` leave `[[` in command position.
            (
                "time -p [[ -f a || ls ]] && ls",
                Some("time -p [[ -f a || ls ]] && rtk ls"),
            ),
            ("time -- [[ -f a || ls ]]", None),
            (
                "time -p -- [[ -f a || ls ]] && ls",
                Some("time -p -- [[ -f a || ls ]] && rtk ls"),
            ),
            // Anything else after `time` is a command, and `[[` its argument.
            (
                "time -- -p [[ -f a || ls ]]",
                Some("time -- -p [[ -f a || rtk ls ]]"),
            ),
            (
                "time \"-p\" [[ -f a || ls ]]",
                Some("time \"-p\" [[ -f a || rtk ls ]]"),
            ),
            (
                "[[ $x =~ a||ls ]] && ls",
                Some("[[ $x =~ a||ls ]] && rtk ls"),
            ),
            (
                "[[ $x =~ ^(ls)+$ ]] || ls",
                Some("[[ $x =~ ^(ls)+$ ]] || rtk ls"),
            ),
            (
                "ls | [[ -f a || ls ]] && git status",
                Some("ls | [[ -f a || ls ]] && rtk git status"),
            ),
            // `*]]` is one word, so it does not close the expression.
            (
                "[[ $x == *]] || ls ]] && ls",
                Some("[[ $x == *]] || ls ]] && rtk ls"),
            ),
            // With no `]]`, the rest of the line is the expression.
            ("[[ -f a || ls", None),
            ("[[ -f a && ls; git status", None),
            // Where `[[` is not in command position it is an ordinary word,
            // and `||` separates commands.
            ("echo [[ -f a || ls ]]", Some("echo [[ -f a || rtk ls ]]")),
            ("a=1 [[ -f a || ls ]]", Some("a=1 [[ -f a || rtk ls ]]")),
            ("\"[[\" -f a || ls ]]", Some("\"[[\" -f a || rtk ls ]]")),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                expected,
                "{cmd:?}"
            );
        }
    }

    /// An arithmetic command `(( … ))` runs no command: `||`, `&&`, `;` and
    /// the brackets inside it belong to its expression, so the rewrite never
    /// puts `rtk` inside one.
    #[test]
    fn test_rewrite_reads_an_arithmetic_command_as_one_command() {
        for (cmd, expected) in [
            ("(( ls || x ))", None),
            ("((ls||x)) && ls", Some("((ls||x)) && rtk ls")),
            (
                "ls && (( ls++ )) || git status",
                Some("rtk ls && (( ls++ )) || rtk git status"),
            ),
            ("time -p (( ls )) && ls", Some("time -p (( ls )) && rtk ls")),
            ("if (( ls || x )); then :; fi", None),
            (
                "case $x in a) (( ls )) || ls;; esac",
                Some("case $x in a) (( ls )) || rtk ls;; esac"),
            ),
            ("( (( ls )) && ls )", Some("( (( ls )) && rtk ls )")),
            (
                "(( ( ls ) || ( x ) )) && ls",
                Some("(( ( ls ) || ( x ) )) && rtk ls"),
            ),
            (
                "(( ls = \"x)\" )) || ls",
                Some("(( ls = \"x)\" )) || rtk ls"),
            ),
            ("coproc (( ls ))", None),
            // No `)` right after the one that closes the second `(`: two
            // subshells, whose commands are rewritten.
            ("((ls) )", Some("((rtk ls) )")),
            ("((ls); (git status))", Some("((rtk ls); (rtk git status))")),
            ("((ls) || (ls))", Some("((rtk ls) || (rtk ls))")),
            // With no `)` to close the second `(`, the rest of the line is the
            // expression.
            ("(( ls || x", None),
            ("(( ls || x; git status", None),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                expected,
                "{cmd:?}"
            );
        }
    }

    /// After a compound command's last word, and after the name of a `for`,
    /// `select` or `coproc`, bash reads a reserved word, so the `then` or `do`
    /// there is one and the `[[ ]]` or `(( ))` behind it runs no command.
    #[test]
    fn test_rewrite_reads_a_reserved_word_after_a_closer() {
        for (cmd, expected) in [
            ("for x do [[ -f a || ls ]]; done", None),
            ("select x do [[ -f a || ls ]]; done", None),
            ("if (true) then [[ -f a || ls ]]; fi", None),
            ("if [[ b ]] then [[ -f a || ls ]]; fi", None),
            ("while (( 0 )) do [[ -f a || ls ]]; done", None),
            ("coproc NAME [[ -f a || ls ]]", None),
            ("for x do (( ls || x )); done", None),
            ("if (true) then (( ls || x )); fi", None),
            (
                "for x do [[ -f a || ls ]] && ls; done",
                Some("for x do [[ -f a || ls ]] && rtk ls; done"),
            ),
            (
                "if (true) then [[ -f a || ls ]] && git status; fi",
                Some("if (true) then [[ -f a || ls ]] && rtk git status; fi"),
            ),
            (
                "while (( 0 )) do [[ -f a || ls ]]; done; git status",
                Some("while (( 0 )) do [[ -f a || ls ]]; done; rtk git status"),
            ),
            (
                "case x in a) (ls) esac; [[ -f a || ls ]] || git status",
                Some("case x in a) (rtk ls) esac; [[ -f a || ls ]] || rtk git status"),
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                expected,
                "{cmd:?}"
            );
        }
    }

    /// A `case` pattern is never a command: after `in` and after each `;;`,
    /// `;&` or `;;&`, the words up to the pattern's `)` are patterns, and only
    /// an `esac` there or in command position closes the `case`.
    #[test]
    fn test_rewrite_never_reads_a_case_pattern_as_a_command() {
        for (cmd, expected) in [
            (
                "case $x in a) echo hi ;; ls) ls ;; esac",
                Some("case $x in a) echo hi ;; ls) rtk ls ;; esac"),
            ),
            (
                "case $x in a) echo esac ;; (ls) ls ;; esac",
                Some("case $x in a) echo esac ;; (ls) rtk ls ;; esac"),
            ),
            (
                "case $x in a) ls;;& ls) ls;& (ls) ls;; esac",
                Some("case $x in a) rtk ls;;& ls) rtk ls;& (ls) rtk ls;; esac"),
            ),
            (
                "case $x in ls) echo esac; ls;; git) git status;; esac",
                Some("case $x in ls) echo esac; rtk ls;; git) rtk git status;; esac"),
            ),
            // An `esac` in command position ends the last arm.
            (
                "case $x in a) echo hi; esac; ls",
                Some("case $x in a) echo hi; esac; rtk ls"),
            ),
            (
                "case $x in a) case $y in ls) ls;; esac;; ls) ls;; esac; ls",
                Some("case $x in a) case $y in ls) rtk ls;; esac;; ls) rtk ls;; esac; rtk ls"),
            ),
            (
                "case $x in a) [[ -f a || ls ]] || ls;; esac",
                Some("case $x in a) [[ -f a || ls ]] || rtk ls;; esac"),
            ),
            (
                "(case $x in ls) ls;; esac) && git status",
                Some("(case $x in ls) rtk ls;; esac) && rtk git status"),
            ),
            (
                "case in in in) ls;; esac",
                Some("case in in in) rtk ls;; esac"),
            ),
            // Glued to more text, `esac` is a pattern word.
            (
                "case $x in a) ls;; esac*) ls;; esac",
                Some("case $x in a) rtk ls;; esac*) rtk ls;; esac"),
            ),
            ("case $x in esac; ls", Some("case $x in esac; rtk ls")),
            // A pattern with no `)` runs to the end of the line.
            ("case $x in a) echo;; ls; git status", None),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                expected,
                "{cmd:?}"
            );
        }
    }

    #[test]
    fn test_case_terminators_lex_as_one_operator() {
        for (input, expected) in [(";;", ";;"), (";&", ";&"), (";;&", ";;&"), (";", ";")] {
            let tokens = tokenize(input);
            assert_eq!(tokens.len(), 1, "{input} should lex as one token");
            assert_eq!(tokens[0].kind, TokenKind::Operator);
            assert_eq!(tokens[0].value, expected);
            assert_eq!(tokens[0].offset, 0);
        }

        // No gap before `esac` does not glue it into the operator: bash reads
        // `;;esac` as two words, and so must the lexer, or the terminator
        // swallows the keyword that closes the statement.
        let tokens = tokenize(";;esac");
        assert_eq!(tokens.len(), 2, ";;esac should lex as two tokens");
        assert_eq!(
            (tokens[0].kind, tokens[0].value),
            (TokenKind::Operator, ";;")
        );
        assert_eq!((tokens[1].kind, tokens[1].value), (TokenKind::Arg, "esac"));
        assert_eq!(tokens[1].offset, 2);

        // A separator followed by a real background operator is still two
        // tokens, since the `&` is not glued to the `;`.
        let tokens = tokenize("a ; & b");
        assert_eq!(
            tokens
                .iter()
                .filter(|t| matches!(t.kind, TokenKind::Operator | TokenKind::Shellism))
                .count(),
            2
        );
    }

    #[test]
    fn test_subcommand_rules_require_token_boundaries() {
        // The pnpm case is covered separately. The sbt rule is included here
        // because it is already boundary-safe and guards the full issue family
        // against future regressions.
        let false_positives = [
            "git branchless status",
            "gh prs",
            "glab mrs",
            "cargo builder",
            "prettierish",
            "next builder",
            "playwrighting",
            "prismax",
            "docker psql",
            "kubectl getall",
            "oc status-check",
            "ruff checker",
            "sqlfluff linting",
            "pip installer",
            "uv pip installer",
            "go vetting",
            "sbt tester",
            "rake tester",
            "rails tester",
            "pio runner",
            "quarto renderer",
            "shopify themepark",
            "terraform planner",
            "trunk builder",
        ];

        for command in false_positives {
            assert!(
                matches!(
                    classify_command(command),
                    Classification::Unsupported { .. }
                ),
                "{command} must not classify as a supported command"
            );
            assert_eq!(
                rewrite_command_no_prefixes(command, &[]),
                None,
                "{command} must not be rewritten"
            );
        }

        let valid_commands = [
            ("git branch status", "rtk git"),
            ("gh pr list", "rtk gh"),
            ("glab mr list", "rtk glab"),
            ("cargo build --release", "rtk cargo"),
            ("prettier --check .", "rtk prettier"),
            ("next build --turbo", "rtk next"),
            ("playwright test", "rtk playwright"),
            ("prisma migrate status", "rtk prisma"),
            ("docker ps", "rtk docker"),
            ("kubectl get pods", "rtk kubectl"),
            ("oc status", "rtk oc"),
            ("ruff check .", "rtk ruff"),
            ("sqlfluff lint .", "rtk sqlfluff"),
            ("pip install flask", "rtk pip"),
            ("uv pip install flask", "rtk uv"),
            ("go test ./...", "rtk go"),
            ("sbt test", "rtk sbt"),
            ("rake test", "rtk rake"),
            ("rake test:unit", "rtk rake"),
            ("rails test:system", "rtk rake"),
            ("bundle exec rake test:models", "rtk rake"),
            ("bin/rails test:integration", "rtk rake"),
            ("pio run", "rtk pio"),
            ("quarto render docs", "rtk quarto"),
            ("shopify theme push", "rtk shopify"),
            ("terraform plan", "rtk terraform"),
            ("trunk build", "rtk trunk"),
        ];

        for (command, expected_rtk_command) in valid_commands {
            match classify_command(command) {
                Classification::Supported { rtk_equivalent, .. } => {
                    assert_eq!(rtk_equivalent, expected_rtk_command, "{command}");
                }
                classification => panic!("{command} classified as {classification:?}"),
            }
        }
    }

    #[test]
    fn test_rewrite_playwright() {
        let commands = vec![
            "npm exec playwright",
            "npm rum playwright",
            "npm run playwright",
            "npm run-script playwright",
            "npm urn playwright",
            "npm x playwright",
            "pnpm dlx playwright",
            "pnpm exec playwright",
            "pnpm run playwright",
            "pnpm run-script playwright",
            "npm playwright",
            "npx playwright",
            "pnpm playwright",
            "pnpx playwright",
            "playwright",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command_no_prefixes(&format!("{command} test"), &[]),
                Some("rtk playwright test".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_next_build() {
        let commands = vec![
            "npm exec next build",
            "npm rum next build",
            "npm run next build",
            "npm run-script next build",
            "npm urn next build",
            "npm x next build",
            "pnpm dlx next build",
            "pnpm exec next build",
            "pnpm run next build",
            "pnpm run-script next build",
            "npm next build",
            "npx next build",
            "pnpm next build",
            "pnpx next build",
            "next build",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command_no_prefixes(&format!("{command} --turbo"), &[]),
                Some("rtk next --turbo".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_pipe_final_safe_stage_only() {
        assert_eq!(
            rewrite_command_no_prefixes("git log -10 | grep feat", &[]),
            Some("git log -10 | rtk grep feat".into())
        );
    }

    #[test]
    fn test_rewrite_find_pipe_skipped() {
        // find in a pipe should NOT be rewritten — rtk find output format
        // is incompatible with pipe consumers like xargs (#439)
        assert_eq!(
            rewrite_command_no_prefixes("find . -name '*.rs' | xargs grep 'fn run'", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_find_pipe_wc_stays_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("find src -type f | wc -l", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_multi_pipe_with_wc_final_stays_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("git log | grep feat | wc -l", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_pipe_unsafe_final_stage_stays_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("find . | xargs grep TODO", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes(
                "printf 'src/main.rs\\n' | grep -f /dev/null src/main.rs",
                &[]
            ),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes(
                "printf 'src/main.rs\\n' | rg --file=/dev/null src/main.rs",
                &[]
            ),
            None
        );
    }

    #[test]
    fn test_rewrite_malformed_pipeline_stays_raw() {
        assert_eq!(rewrite_command_no_prefixes("| grep FAILED", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("cargo test |", &[]), None);
        assert_eq!(
            rewrite_command_no_prefixes("cargo test | | grep FAILED", &[]),
            None
        );
    }

    // --- Safe pipe consumers: producer rewrite ---

    #[test]
    fn test_rewrite_pipe_safe_consumers_producer_rewritten() {
        assert_eq!(
            rewrite_command_no_prefixes("git log | tail -5", &[]),
            Some("rtk git log | tail -5".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("cargo test | tail -50", &[]),
            Some("rtk cargo test | tail -50".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("git diff | cat", &[]),
            Some("rtk git diff | cat".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("RUST_BACKTRACE=1 cargo test 2>&1 | tail -50", &[]),
            Some("RUST_BACKTRACE=1 rtk cargo test 2>&1 | tail -50".into())
        );
    }

    #[test]
    fn test_rewrite_multi_pipe_all_safe_consumers() {
        assert_eq!(
            rewrite_command_no_prefixes("git log | head -20 | tail -5", &[]),
            Some("rtk git log | head -20 | tail -5".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_safe_consumer_with_next_clause() {
        assert_eq!(
            rewrite_command_no_prefixes("git log | tail -5 && git status", &[]),
            Some("rtk git log | tail -5 && rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_mixed_consumers_stay_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("git log | head | wc -l", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("git log | tail | xargs echo", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("git log | grep feat | wc -l", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_pipe_producer_no_rule_or_excluded_stays_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("unknowncmd | tail -5", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("git log | tail -5", &["git log".into()]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("rtk git log | tail -5", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_pipe_consumer_decorations_stay_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("git log | FOO=1 tail -5", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("git log | /usr/bin/tail -5", &[]),
            None
        );
        assert_eq!(rewrite_command_no_prefixes("git log |& tail -5", &[]), None);
    }

    #[test]
    fn test_rewrite_pipe_consumer_fd_dup_redirect_rewritten() {
        assert_eq!(
            rewrite_command_no_prefixes("git log | tail -5 2>&1", &[]),
            Some("rtk git log | tail -5 2>&1".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("git log | tail -5 2>/dev/null", &[]),
            Some("rtk git log | tail -5 2>/dev/null".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_consumer_redirect_stays_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("git log | tail -5 > out.txt", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("git log | cat > file.txt", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_pipe_read_producer_stays_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("head -20 file.txt | tail -5", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("cat file.txt | tail -5", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("tail -20 file.txt | head -5", &[]),
            None
        );
    }

    fn assert_consumer_flag_blocks_rewrite(consumer: &str, spelling: &str) {
        let cmd = format!("git log | {consumer} {spelling}");
        assert_eq!(rewrite_command_no_prefixes(&cmd, &[]), None, "{cmd}");
    }

    /// Every shell spelling of a consumer's unsafe flag must keep the producer raw.
    /// Driven off `SAFE_PIPE_CONSUMERS` so a consumer added later is covered on arrival:
    /// `getopt_long` accepts any unambiguous prefix of a long option, and the shell strips
    /// quotes and backslashes before the flag ever reaches the consumer.
    #[test]
    fn test_unsafe_consumer_flag_spellings_stay_raw() {
        for consumer in SAFE_PIPE_CONSUMERS {
            for flag in consumer.unsafe_flags {
                let name = flag
                    .strip_prefix("--")
                    .expect("unsafe_flags entries are long options");
                for len in 1..=name.len() {
                    let abbrev = &name[..len];
                    for spelling in [
                        format!("--{abbrev}"),
                        format!("\"--{abbrev}\""),
                        format!("'--{abbrev}'"),
                        format!("\\-\\-{abbrev}"),
                        format!("--{abbrev}=x"),
                    ] {
                        assert_consumer_flag_blocks_rewrite(consumer.name, &spelling);
                    }
                }
            }

            for ch in consumer.unsafe_flag_chars {
                for spelling in [
                    format!("-{ch}"),
                    format!("\"-{ch}\""),
                    format!("'-{ch}'"),
                    format!("\\-{ch}"),
                    format!("-{ch}q"),
                    format!("-q{ch}"),
                    format!("-{ch}n20"),
                ] {
                    assert_consumer_flag_blocks_rewrite(consumer.name, &spelling);
                }
            }
        }
    }

    /// Guards the test above against passing vacuously if the consumer table empties.
    #[test]
    fn test_safe_consumer_spellings_still_rewrite() {
        for cmd in [
            "git log | cat",
            "git log | head -20",
            "git log | tail -20",
            "git log | tail -n 20",
        ] {
            assert!(
                rewrite_command_no_prefixes(cmd, &[]).is_some(),
                "{cmd} should rewrite"
            );
        }
    }

    #[test]
    fn test_rewrite_pipe_following_tail_stays_raw() {
        for cmd in [
            "git log | tail -f",
            "git log | tail -F",
            "git log | tail --follow",
            "git log | tail --follow=name",
            "git log | tail --foll",
            "git log | tail --f",
            "git log | tail -fn20",
            "git log | tail \"-f\"",
            "git log | tail \\-f",
        ] {
            assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None, "{cmd}");
        }
        assert_eq!(
            rewrite_command_no_prefixes("git log | tail -n 20", &[]),
            Some("rtk git log | tail -n 20".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_producer_unsafe_rules_stay_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("ping 127.0.0.1 | head -5", &[]),
            None
        );
        assert_eq!(rewrite_command_no_prefixes("vitest | head", &[]), None);
        assert_eq!(
            rewrite_command_no_prefixes("npm run dev | head -5", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("docker logs app | tail -20", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_pipe_producer_pattern_file_stays_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("grep -f patterns.txt input.txt | cat", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("rg --file=patterns.txt input.txt | cat", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("grep foo src/main.rs | head -5", &[]),
            Some("rtk grep foo src/main.rs | head -5".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_producer_file_list_stays_raw() {
        // A folded list keeps its prefix in the first line, which `tail` drops.
        for cmd in [
            "grep -rl foo src | tail -3",
            "grep -rL foo . | head -5",
            "rg -l foo | tail",
            "rg --files src | cat",
        ] {
            assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None, "{cmd}");
        }
        // `-c` with `-l` is not folded, and `-e -l` makes `-l` the pattern.
        assert_eq!(
            rewrite_command_no_prefixes("grep -rlc foo src | tail -3", &[]),
            Some("rtk grep -rlc foo src | tail -3".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("grep -e -l src | tail -3", &[]),
            Some("rtk grep -e -l src | tail -3".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_producer_batch_rules_rewritten() {
        assert_eq!(
            rewrite_command_no_prefixes("pytest | tail -20", &[]),
            Some("rtk pytest | tail -20".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("terraform plan | head -40", &[]),
            Some("rtk terraform plan | head -40".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_final_grep_beats_producer_path() {
        assert_eq!(
            rewrite_command_no_prefixes("git log | grep feat", &[]),
            Some("git log | rtk grep feat".into())
        );
    }

    #[test]
    fn test_rewrite_find_no_pipe_still_rewritten() {
        // find WITHOUT a pipe should still be rewritten
        assert_eq!(
            rewrite_command_no_prefixes("find . -name '*.rs'", &[]),
            Some("rtk find . -name '*.rs'".into())
        );
    }

    #[test]
    fn test_rewrite_heredoc_returns_none() {
        assert_eq!(
            rewrite_command_no_prefixes("cat <<'EOF'\nfoo\nEOF", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_empty_returns_none() {
        assert_eq!(rewrite_command_no_prefixes("", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("   ", &[]), None);
    }

    #[test]
    fn test_rewrite_mixed_compound_partial() {
        // First segment already RTK, second gets rewritten
        assert_eq!(
            rewrite_command_no_prefixes("rtk git add . && cargo test", &[]),
            Some("rtk git add . && rtk cargo test".into())
        );
    }

    // --- #345: RTK_DISABLED ---

    #[test]
    fn test_rewrite_rtk_disabled_curl() {
        assert_eq!(
            rewrite_command_no_prefixes("RTK_DISABLED=1 curl https://example.com", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_rtk_disabled_git_status() {
        assert_eq!(
            rewrite_command_no_prefixes("RTK_DISABLED=1 git status", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_rtk_disabled_multi_env() {
        assert_eq!(
            rewrite_command_no_prefixes("FOO=1 RTK_DISABLED=1 git status", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_rtk_disabled_warns_on_stderr() {
        assert_eq!(
            rewrite_command_no_prefixes("RTK_DISABLED=1 git status", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_rtk_disabled_subprocess_warns() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }

        let output = test_isolation::rtk_command()
            .args(["rewrite", "RTK_DISABLED=1 git status"])
            .output()
            .expect("Failed to run rtk");

        assert!(
            !output.status.success(),
            "Should exit non-zero (no rewrite)"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("RTK_DISABLED=1 detected"),
            "Should warn on stderr, got: {}",
            stderr
        );
    }

    #[test]
    fn test_rewrite_non_rtk_disabled_env_still_rewrites() {
        assert_eq!(
            rewrite_command_no_prefixes("SOME_VAR=1 git status", &[]),
            Some("SOME_VAR=1 rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_env_quoted_value_with_spaces() {
        assert_eq!(
            rewrite_command_no_prefixes(
                r#"GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=no" git push"#,
                &[]
            ),
            Some(r#"GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=no" rtk git push"#.into())
        );
    }

    #[test]
    fn test_rewrite_env_single_quoted_value_with_spaces() {
        assert_eq!(
            rewrite_command_no_prefixes("EDITOR='vim -u NONE' git commit", &[]),
            Some("EDITOR='vim -u NONE' rtk git commit".into())
        );
    }

    #[test]
    fn test_rewrite_env_quoted_plus_unquoted() {
        assert_eq!(
            rewrite_command_no_prefixes(r#"FOO="bar baz" BAR=1 git status"#, &[]),
            Some(r#"FOO="bar baz" BAR=1 rtk git status"#.into())
        );
    }

    #[test]
    fn test_rewrite_env_escaped_quotes_in_value() {
        assert_eq!(
            rewrite_command_no_prefixes(r#"FOO="he said \"hello\"" git status"#, &[]),
            Some(r#"FOO="he said \"hello\"" rtk git status"#.into())
        );
    }

    #[test]
    fn test_classify_env_quoted_value_stripped() {
        assert_eq!(
            classify_command(r#"GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=no" git push"#),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    // --- #346: 2>&1 and &> redirect detection ---

    #[test]
    fn test_rewrite_redirect_2_gt_amp_1_with_pipe() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo test 2>&1 | grep FAILED", &[]),
            Some("cargo test 2>&1 | rtk grep FAILED".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_2_gt_amp_1_trailing() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo test 2>&1", &[]),
            Some("rtk cargo test 2>&1".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_plain_2_devnull() {
        // 2>/dev/null has no `&`, never broken — non-regression
        assert_eq!(
            rewrite_command_no_prefixes("git status 2>/dev/null", &[]),
            Some("rtk git status 2>/dev/null".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_2_gt_amp_1_with_and() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo test 2>&1 && echo done", &[]),
            Some("rtk cargo test 2>&1 && echo done".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_amp_gt_devnull() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo test &>/dev/null", &[]),
            Some("rtk cargo test &>/dev/null".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_double() {
        // Double redirect: only last one stripped, but full command rewrites correctly
        assert_eq!(
            rewrite_command_no_prefixes("git status 2>&1 >/dev/null", &[]),
            Some("rtk git status 2>&1 >/dev/null".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_fd_close() {
        // 2>&- (close stderr fd)
        assert_eq!(
            rewrite_command_no_prefixes("git status 2>&-", &[]),
            Some("rtk git status 2>&-".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_quotes_not_stripped() {
        // Redirect-like chars inside quotes should NOT be stripped
        // Known limitation: apostrophes cause conservative no-strip (safe fallback)
        let result = rewrite_command_no_prefixes("git commit -m \"it's fixed\" 2>&1", &[]);
        assert!(
            result.is_some(),
            "Should still rewrite even with apostrophe"
        );
    }

    #[test]
    fn test_rewrite_background_amp_non_regression() {
        // background `&` must still work after redirect fix
        assert_eq!(
            rewrite_command_no_prefixes("cargo test & git status", &[]),
            Some("rtk cargo test & rtk git status".into())
        );
    }

    // --- P0.2: head -N rewrite ---

    #[test]
    fn test_head_tail_honour_exclude_commands() {
        // head/tail rewrite to `rtk read`; excluding them must suppress that.
        let excluded = vec!["head".to_string(), "tail".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("head -20 src/main.rs", &excluded),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("tail -20 src/main.rs", &excluded),
            None
        );
        // An env prefix is a layer the walk peels before `decide` reaches this
        // branch, so the exclusion still applies to the wrapped head/tail.
        assert_eq!(
            rewrite_command_no_prefixes("RUST_LOG=debug tail -20 src/main.rs", &excluded),
            None
        );
        // ...and must not affect unrelated commands.
        assert_eq!(
            rewrite_command_no_prefixes("git status", &excluded),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_routable_wrapper_honours_exclude_commands() {
        // `uv run` is a routable wrapper: when the inner rewrite is dropped it
        // falls through and re-tests `uv run <cmd>` as a `uv` invocation. That
        // fall-through must not resurrect a command the user excluded.
        let excluded = vec!["head".to_string(), "tail".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("uv run head -20 src/main.rs", &excluded),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("uv run cat src/main.rs", &["cat".to_string()]),
            None
        );
        // A non-excluded inner command still rewrites through the wrapper.
        assert_eq!(
            rewrite_command_no_prefixes("uv run head -20 src/main.rs", &["cat".to_string()]),
            Some("uv run rtk read src/main.rs --head-lines 20".into())
        );
    }

    #[test]
    fn test_head_tail_rewrite_when_not_excluded() {
        assert_eq!(
            rewrite_command_no_prefixes("head -20 src/main.rs", &["cat".to_string()]),
            Some("rtk read src/main.rs --head-lines 20".into())
        );
    }

    #[test]
    fn test_rewrite_head_numeric_flag() {
        // head -20 file → rtk read file --head-lines 20 (not rtk read -20 file)
        assert_eq!(
            rewrite_command_no_prefixes("head -20 src/main.rs", &[]),
            Some("rtk read src/main.rs --head-lines 20".into())
        );
    }

    /// A single token that the shell expands into several operands must stay
    /// native: `rtk read` concatenates files, losing `head`'s `==> name <==`
    /// banners. Covers the newly added `-n N` / `--lines N` routes, which had
    /// no matcher at all before and so were native by accident.
    #[test]
    fn test_rewrite_head_expanding_operand_stays_native() {
        for cmd in [
            "head src/core/*.rs",
            "head -1 src/core/*.rs",
            "head -n 1 src/core/*.rs",
            "head --lines 1 src/core/*.rs",
            "head --lines=1 src/core/*.rs",
            "head -n 1 src/core/{a,b}.rs",
            "head -n 1 $FILES",
            "head -n 1 src/core/?.rs",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                None,
                "expanding operand must stay native: {}",
                cmd
            );
        }
    }

    /// `head -n 1 --help` must reach `head`, not print `rtk read`'s help. The
    /// quoted and escaped spellings matter as much as the bare one: the matcher
    /// sees shell source, so `'--'` and `\--help` slip past a leading-`-` test
    /// while still reaching the tool as options.
    #[test]
    fn test_rewrite_head_option_operand_stays_native() {
        for cmd in [
            "head -n 1 --help",
            "head --lines 1 --version",
            "head --help",
            "head -n 1 '--'",
            "head -n 1 \"--\"",
            "head -n 1 '--help'",
            "head -n 1 \"--help\"",
            "head -n 1 \\--help",
            "head '--'",
            "head -n 1 #comment",
            "head #comment",
            "head -n 1 a|b",
            "head -n 1 (x)",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                None,
                "option-like operand must stay native: {}",
                cmd
            );
        }
    }

    /// The same expansion hazard applied to `tail`, which shared the code path.
    #[test]
    fn test_rewrite_tail_expanding_operand_stays_native() {
        for cmd in [
            "tail -20 src/core/*.rs",
            "tail -n 20 src/core/*.rs",
            "tail --lines 20 $FILES",
            "tail -n 1 '--'",
            "tail -n 1 \\--help",
            "tail -n 1 #comment",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                None,
                "expanding operand must stay native: {}",
                cmd
            );
        }
    }

    /// `;` and `&` are operators the lexer splits on before the operand is ever
    /// examined, so the trailing text is a separate command and the real operand
    /// is just `f`. These rewrite, and should: the allowlist only has to judge
    /// characters that survive segmentation.
    #[test]
    fn test_rewrite_head_operand_after_operator_split() {
        assert_eq!(
            rewrite_command_no_prefixes("head -n 1 f;rm", &[]),
            Some("rtk read f --head-lines 1;rm".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("head -n 1 a&b", &[]),
            Some("rtk read a --head-lines 1&b".into())
        );
    }

    /// Bash's `$IFS` is space, tab and newline, and the rule patterns match
    /// `[ \t\n]+`, so a tab-separated command classifies as Supported. The rewrite
    /// then has to agree, or the hook reports coverage it does not deliver
    /// and the command streams raw (#4100).
    #[test]
    fn test_a_tab_separates_a_command_from_its_prefix() {
        for (cmd, expected) in [
            ("ls\t-la", "rtk ls -la"),
            ("cargo\tbuild", "rtk cargo build"),
            // Wrapper prefixes peel on a tab as they do on a space, and the
            // tab itself is the author's text.
            ("noglob\tls -la", "noglob\trtk ls -la"),
            ("command\tls -la", "command\trtk ls -la"),
            ("exec\tls -la", "exec\trtk ls -la"),
            ("uv run\tls -la", "uv run\trtk ls -la"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "{cmd:?} classifies as Supported, so it has to rewrite"
            );
        }
    }

    /// The two sides have to agree about what a command is: anything
    /// `classify_command` calls Supported must produce a rewrite, or
    /// `rtk discover` counts coverage the hook never delivers.
    #[test]
    fn test_supported_classification_implies_a_rewrite() {
        const SEPARATORS: &[&str] = &[" ", "\t", "  ", " \t "];
        // Shapes a rule pattern matches directly, where "did it rewrite" is
        // the whole question. Wrappers are deliberately absent. `noglob` and
        // the other shell keywords because no rule names them, so
        // `classify_command` never calls them Supported; `uv run` because a
        // broken wrapper peel still returns `Some` — the wrong text, routed to
        // `rtk uv` instead of peeling — and presence alone cannot see that.
        // Both are pinned by the exact-output table above.
        const COMMANDS: &[&str] = &["ls|-la", "cargo|build", "git|status", "grep|-rn foo"];

        let mut silent: Vec<&str> = Vec::new();
        for shape in COMMANDS {
            let mut checked = 0;
            for separator in SEPARATORS {
                let cmd = shape.replace('|', separator);
                if !matches!(classify_command(&cmd), Classification::Supported { .. }) {
                    continue;
                }
                checked += 1;
                assert!(
                    rewrite_command_no_prefixes(&cmd, &[]).is_some(),
                    "{cmd:?} classifies as Supported but does not rewrite"
                );
            }
            // Per shape, not in total: a shape that never classifies Supported
            // contributes nothing, and a total is large enough to hide it.
            if checked < SEPARATORS.len() {
                silent.push(shape);
            }
        }
        assert!(
            silent.is_empty(),
            "these shapes do not classify as Supported for every separator, so \
             they assert nothing here: {silent:#?}"
        );
    }

    /// The rewrite replaces commands, so everything that is not a command must
    /// come through untouched. Removing each inserted `rtk ` has to give back
    /// the input exactly — no operator respaced, no newline collapsed, no
    /// separator invented.
    ///
    /// Stated as a property because what it catches is spacing, and nobody
    /// writes a case for the spacing they did not think of.
    #[test]
    fn test_rewrite_changes_commands_and_nothing_else() {
        const COMMANDS: &[&str] = &["ls", "ls -la", "cargo build", "echo hi", "nosuchtool x"];
        const GAPS: &[&str] = &["", " ", "  ", "\t"];
        const JOINERS: &[&str] = &[
            " && ", "&&", "  &&  ", " || ", "||", "; ", ";", "  ;  ", " & ", "&", " | ", "|",
            " ;; ", ";;", ";&", ";;&", "\n", " \n ", "\n\n", "\t&&\t",
        ];

        let mut corpus: Vec<String> = Vec::new();
        for left in COMMANDS {
            for joiner in JOINERS {
                for right in COMMANDS {
                    corpus.push(format!("{left}{joiner}{right}"));
                }
            }
        }
        // Derived from the tables the production code reads, not listed by
        // hand: a wrapper or consumer added later is covered without anyone
        // remembering to add it here. Every one of these rejoins text around
        // a command, and every one of them collapsed the gap at some point.
        let mut groups: Vec<(&str, Vec<String>)> = Vec::new();

        let mut wrapped = Vec::new();
        for family in families::ALL
            .iter()
            .filter(|f| !matches!(f.form, Form::RunWord))
        {
            let wrapper = family.head;
            // `timeout` takes a duration before the command it wraps.
            let operand = match family.form {
                Form::Wrapper(spec) => "5 ".repeat(spec.operands),
                _ => String::new(),
            };
            for gap in GAPS {
                wrapped.push(format!("{wrapper} {operand}{gap}ls -la"));
                wrapped.push(format!("{wrapper} {operand}{gap}ls -la && ls"));
            }
        }
        groups.push(("wrapper prefixes", wrapped));

        let mut piped = Vec::new();
        for consumer in SAFE_PIPE_CONSUMERS {
            for gap in GAPS {
                for pipe_gap in GAPS {
                    piped.push(format!("ls -la{gap}|{pipe_gap}{} -3", consumer.name));
                }
            }
        }
        groups.push(("safe pipe consumers", piped));

        // A `case` arm's terminator may sit flush against `esac`. The
        // terminator is one operator and `esac` the word after it, so the gap
        // between them — including no gap at all — is text the emitter has
        // to leave exactly as written, or `;;esac` comes back as `;; esac`.
        let mut cased = Vec::new();
        for terminator in [";;", ";&", ";;&"] {
            for gap in GAPS {
                cased.push(format!("case x in x) echo 1{terminator}{gap}esac; ls -la"));
                cased.push(format!("ls -la; case x in x) echo 1{terminator}{gap}esac"));
            }
        }
        groups.push(("case terminators against esac", cased));

        for (label, group) in &groups {
            let rewritten = group
                .iter()
                .filter(|c| rewrite_command_no_prefixes(c, &[]).is_some())
                .count();
            // A group that stops rewriting asserts nothing. That is how the
            // pipe cases sat here proving nothing while a real bug lived
            // behind them.
            assert!(
                rewritten * 2 >= group.len(),
                "{label}: only {rewritten} of {} inputs rewrite, so this group \
                 no longer exercises the emitter",
                group.len()
            );
            corpus.extend(group.iter().cloned());
        }

        for cmd in &corpus {
            let Some(out) = rewrite_command_no_prefixes(cmd, &[]) else {
                continue;
            };
            // The whole command is trimmed on the way in, which is separate
            // from what the emitter owns: the text between one command and
            // the next.
            let restored = out.replace("rtk ", "");
            assert_eq!(
                restored,
                cmd.trim().replace("rtk ", ""),
                "rewrite altered text outside a command\n  in:  {cmd:?}\n  out: {out:?}"
            );
        }
    }

    /// A trailing redirect is stripped before the operand is judged, so `a>b`
    /// rewrites with operand `a` — but the suffix must not fuse onto the numeric
    /// flag value, or the shell reads `1>b` as an fd-1 redirect and
    /// `--head-lines` loses its argument.
    #[test]
    fn test_rewrite_head_redirect_suffix_keeps_flag_value() {
        assert_eq!(
            rewrite_command_no_prefixes("head -n 1 a>b", &[]),
            Some("rtk read a --head-lines 1 >b".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("tail -n 1 a>b", &[]),
            Some("rtk read a --tail-lines 1 >b".into())
        );
        // An already-spaced suffix must not gain a second space.
        assert_eq!(
            rewrite_command_no_prefixes("head -n 1 a > b", &[]),
            Some("rtk read a --head-lines 1 > b".into())
        );
    }

    /// Shapes the allowlist must keep accepting, including a non-ASCII name.
    #[test]
    fn test_rewrite_head_accepts_ordinary_paths() {
        for (cmd, want) in [
            (
                "head -10 /tmp/seq.txt",
                "rtk read /tmp/seq.txt --head-lines 10",
            ),
            ("head ./a-b_c.txt", "rtk read ./a-b_c.txt --head-lines 10"),
            ("head ~/x.txt", "rtk read ~/x.txt --head-lines 10"),
            (
                "head /tmp/a.b.c-d_e.txt",
                "rtk read /tmp/a.b.c-d_e.txt --head-lines 10",
            ),
            (
                "head src/日本語.rs",
                "rtk read src/日本語.rs --head-lines 10",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some(want.into()),
                "must still rewrite: {}",
                cmd
            );
        }
    }

    #[test]
    fn test_rewrite_tail_plain_operand_still_rewrites() {
        assert_eq!(
            rewrite_command_no_prefixes("tail -20 src/main.rs", &[]),
            Some("rtk read src/main.rs --tail-lines 20".into())
        );
    }

    #[test]
    fn test_rewrite_head_n_space_flag() {
        assert_eq!(
            rewrite_command_no_prefixes("head -n 50 src/lib.rs", &[]),
            Some("rtk read src/lib.rs --head-lines 50".into())
        );
    }

    #[test]
    fn test_rewrite_head_lines_space_flag() {
        assert_eq!(
            rewrite_command_no_prefixes("head --lines 50 src/lib.rs", &[]),
            Some("rtk read src/lib.rs --head-lines 50".into())
        );
    }

    /// Multi-file and optioned bare forms stay native: the rewrite emits one
    /// path (see `is_single_file_operand`), and `head` prints `==> name <==`
    /// banners for several files, which a concatenated read cannot reproduce.
    #[test]
    fn test_rewrite_head_bare_multifile_stays_native() {
        assert_eq!(rewrite_command_no_prefixes("head a.txt b.txt", &[]), None);
    }

    #[test]
    fn test_rewrite_head_unsupported_option_stays_native() {
        assert_eq!(rewrite_command_no_prefixes("head -c 10 f.bin", &[]), None);
    }

    #[test]
    fn test_rewrite_head_lines_long_flag() {
        assert_eq!(
            rewrite_command_no_prefixes("head --lines=50 src/lib.rs", &[]),
            Some("rtk read src/lib.rs --head-lines 50".into())
        );
    }

    #[test]
    fn test_rewrite_head_no_flag_still_rewrites() {
        // plain `head file` means ten lines, so the rewrite must bound the read
        // rather than dumping the whole file.
        assert_eq!(
            rewrite_command_no_prefixes("head src/main.rs", &[]),
            Some("rtk read src/main.rs --head-lines 10".into())
        );
    }

    #[test]
    fn test_rewrite_head_other_flag_skipped() {
        // head -c 100 file: unsupported flag, skip rewriting
        assert_eq!(
            rewrite_command_no_prefixes("head -c 100 src/main.rs", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_tail_numeric_flag() {
        assert_eq!(
            rewrite_command_no_prefixes("tail -20 src/main.rs", &[]),
            Some("rtk read src/main.rs --tail-lines 20".into())
        );
    }

    #[test]
    fn test_rewrite_tail_n_space_flag() {
        assert_eq!(
            rewrite_command_no_prefixes("tail -n 12 src/lib.rs", &[]),
            Some("rtk read src/lib.rs --tail-lines 12".into())
        );
    }

    #[test]
    fn test_rewrite_tail_lines_long_flag() {
        assert_eq!(
            rewrite_command_no_prefixes("tail --lines=7 src/lib.rs", &[]),
            Some("rtk read src/lib.rs --tail-lines 7".into())
        );
    }

    #[test]
    fn test_rewrite_tail_lines_space_flag() {
        assert_eq!(
            rewrite_command_no_prefixes("tail --lines 7 src/lib.rs", &[]),
            Some("rtk read src/lib.rs --tail-lines 7".into())
        );
    }

    #[test]
    fn test_rewrite_tail_other_flag_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("tail -c 100 src/main.rs", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_tail_plain_file_skipped() {
        assert_eq!(rewrite_command_no_prefixes("tail src/main.rs", &[]), None);
    }

    // --- Issue #1362: head/tail with multiple files falls back to native command ---
    //
    // The head/tail rewrite emits a single positional path (see
    // `is_single_file_operand`), even though `rtk read` itself accepts several, in
    // a shape that maps cleanly to `head -N`. Rewriting `head -N a b c` to
    // `rtk read a b c --max-lines N` previously produced a command where `rtk read`
    // would concatenate the files without the `==> name <==` banners that native
    // `head` emits, so the fix is to skip the rewrite and let the shell run the
    // real `head`/`tail` binary.

    #[test]
    fn test_rewrite_head_numeric_flag_multi_file_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("head -3 /tmp/a /tmp/b /tmp/c", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_head_lines_long_flag_multi_file_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("head --lines=50 src/main.rs src/lib.rs", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_tail_numeric_flag_multi_file_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("tail -20 a.log b.log", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_tail_n_space_flag_multi_file_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("tail -n 12 a.log b.log c.log", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_tail_lines_eq_multi_file_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("tail --lines=7 a.log b.log", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_tail_lines_space_multi_file_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("tail --lines 7 a.log b.log", &[]),
            None
        );
    }

    // --- New registry entries ---

    #[test]
    fn test_classify_gh_release() {
        assert!(matches!(
            classify_command("gh release list"),
            Classification::Supported {
                rtk_equivalent: "rtk gh",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_glab_mr() {
        assert!(matches!(
            classify_command("glab mr list"),
            Classification::Supported {
                rtk_equivalent: "rtk glab",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_glab_ci() {
        assert!(matches!(
            classify_command("glab ci list"),
            Classification::Supported {
                rtk_equivalent: "rtk glab",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_glab_release() {
        assert!(matches!(
            classify_command("glab release list"),
            Classification::Supported {
                rtk_equivalent: "rtk glab",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_glab_mr_list() {
        assert_eq!(
            rewrite_command_no_prefixes("glab mr list", &[]),
            Some("rtk glab mr list".into())
        );
    }

    #[test]
    fn test_rewrite_glab_ci_status() {
        assert_eq!(
            rewrite_command_no_prefixes("glab ci status", &[]),
            Some("rtk glab ci status".into())
        );
    }

    #[test]
    fn test_classify_cargo_install() {
        assert!(matches!(
            classify_command("cargo install rtk"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_docker_run() {
        assert!(matches!(
            classify_command("docker run --rm ubuntu bash"),
            Classification::Supported {
                rtk_equivalent: "rtk docker",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_docker_exec() {
        assert!(matches!(
            classify_command("docker exec -it mycontainer bash"),
            Classification::Supported {
                rtk_equivalent: "rtk docker",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_docker_build() {
        assert!(matches!(
            classify_command("docker build -t myimage ."),
            Classification::Supported {
                rtk_equivalent: "rtk docker",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_kubectl_describe() {
        assert!(matches!(
            classify_command("kubectl describe pod mypod"),
            Classification::Supported {
                rtk_equivalent: "rtk kubectl",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_kubectl_apply() {
        assert!(matches!(
            classify_command("kubectl apply -f deploy.yaml"),
            Classification::Supported {
                rtk_equivalent: "rtk kubectl",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_tree() {
        assert!(matches!(
            classify_command("tree src/"),
            Classification::Supported {
                rtk_equivalent: "rtk tree",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_diff() {
        assert!(matches!(
            classify_command("diff file1.txt file2.txt"),
            Classification::Supported {
                rtk_equivalent: "rtk diff",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_tree() {
        assert_eq!(
            rewrite_command_no_prefixes("tree src/", &[]),
            Some("rtk tree src/".into())
        );
    }

    #[test]
    fn test_rewrite_diff() {
        assert_eq!(
            rewrite_command_no_prefixes("diff file1.txt file2.txt", &[]),
            Some("rtk diff file1.txt file2.txt".into())
        );
    }

    #[test]
    fn test_rewrite_gh_release() {
        assert_eq!(
            rewrite_command_no_prefixes("gh release list", &[]),
            Some("rtk gh release list".into())
        );
    }

    #[test]
    fn test_rewrite_cargo_install() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo install rtk", &[]),
            Some("rtk cargo install rtk".into())
        );
    }

    #[test]
    fn test_rewrite_kubectl_describe() {
        assert_eq!(
            rewrite_command_no_prefixes("kubectl describe pod mypod", &[]),
            Some("rtk kubectl describe pod mypod".into())
        );
    }

    #[test]
    fn test_rewrite_docker_run() {
        assert_eq!(
            rewrite_command_no_prefixes("docker run --rm ubuntu bash", &[]),
            Some("rtk docker run --rm ubuntu bash".into())
        );
    }

    #[test]
    fn test_rewrite_bun_x_space_form() {
        assert_eq!(
            rewrite_command_no_prefixes("bun x tsc --noEmit", &[]),
            Some("rtk bun x tsc --noEmit".into())
        );
    }

    /// Status and savings a rule assigns to a command, for the passthrough
    /// accounting tests below.
    fn status_and_savings(cmd: &str) -> (RtkStatus, f64) {
        match classify_command(cmd) {
            Classification::Supported {
                status,
                estimated_savings_pct,
                ..
            } => (status, estimated_savings_pct),
            other => panic!("expected Supported for {cmd}, got {other:?}"),
        }
    }

    #[test]
    fn test_deno_pattern_does_not_match_subcommand_prefixes() {
        // Without a terminator, "deno taskfoo" matches the "task" alternative.
        assert_eq!(rewrite_command_no_prefixes("deno taskfoo", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("deno testify", &[]), None);
        assert_eq!(
            rewrite_command_no_prefixes("deno task build", &[]),
            Some("rtk deno task build".into())
        );
    }

    /// A rule's word ends at a space, a tab, a newline, the end of the line or
    /// a metacharacter glued to it, never at a `\r`, a vertical tab, a form
    /// feed, a non-breaking space or punctuation inside the word. The built-in
    /// TOML filter a rule mirrors ends its words the same way, so neither
    /// takes the command.
    #[test]
    fn test_rule_words_end_at_ifs() {
        for (cmd, expected) in [
            ("brew install\r", None),
            ("brew install foo\r", Some("rtk brew install foo\r")),
            ("make\r", None),
            ("make\x0b-j4", None),
            ("du\u{a0}-sh", None),
            ("helm-docs", None),
            ("ansible-playbook-grapher site.yml", None),
            ("dotnet build-server shutdown", None),
            ("bun install\x0c", None),
            ("deno test\r", None),
            ("mvn install:install-file -Dfile=a.jar", None),
            ("mvn test-compile", Some("rtk mvn test-compile")),
            (
                "mvnd clean test-compile",
                Some("rtk mvnd clean test-compile"),
            ),
            ("make>/dev/null", Some("rtk make>/dev/null")),
            ("helm list", Some("rtk helm list")),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                expected.map(String::from),
                "{cmd:?}"
            );
        }

        // Classification reads a segment with its redirects kept.
        for cmd in ["make>build.log", "brew install jq 2>&1", "du -sh>/dev/null"] {
            assert!(
                matches!(classify_command(cmd), Classification::Supported { .. }),
                "{cmd:?}: {:?}",
                classify_command(cmd)
            );
        }
        for cmd in ["make\r", "helm-docs", "brew install\u{a0}"] {
            assert!(
                !matches!(classify_command(cmd), Classification::Supported { .. }),
                "{cmd:?}"
            );
        }
    }

    #[test]
    fn test_passthrough_subcommands_claim_no_savings() {
        // These run unfiltered, so discover must not credit them with the
        // rule's headline savings. Asserting the percentage matters as much as
        // the status: the two are separate fields and only the percentage
        // reaches the projection.
        for cmd in [
            "deno install npm:cowsay",
            "deno run main.ts",
            "deno task build",
            "bun pm cache rm",
            "bun run dev",
            "bun build ./index.ts",
            "deno compile m.ts",
            "cargo fmt",
        ] {
            let (status, savings) = status_and_savings(cmd);
            assert_eq!(status, RtkStatus::Passthrough, "{cmd}");
            assert_eq!(savings, 0.0, "{cmd}");
        }

        // The filtered forms are still credited.
        let (status, savings) = status_and_savings("bun pm ls");
        assert_eq!(status, RtkStatus::Existing);
        assert_eq!(savings, 70.0);
        let (status, savings) = status_and_savings("deno test");
        assert_eq!(status, RtkStatus::Existing);
        assert_eq!(savings, 90.0);
    }

    #[test]
    fn test_rewrite_bun_unknown_subcommand_untouched() {
        assert_eq!(rewrite_command_no_prefixes("bun xtask build", &[]), None);
    }

    #[test]
    fn test_classify_swift_test() {
        assert!(matches!(
            classify_command("swift test"),
            Classification::Supported {
                rtk_equivalent: "rtk swift",
                category: "Build",
                estimated_savings_pct: 90.0,
                status: RtkStatus::Existing,
            }
        ));
    }

    #[test]
    fn test_rewrite_swift_test() {
        assert_eq!(
            rewrite_command_no_prefixes("swift test --parallel", &[]),
            Some("rtk swift test --parallel".into())
        );
    }

    // --- #336: docker compose supported subcommands rewritten, unsupported skipped ---

    #[test]
    fn test_rewrite_docker_compose_ps() {
        assert_eq!(
            rewrite_command_no_prefixes("docker compose ps", &[]),
            Some("rtk docker compose ps".into())
        );
    }

    #[test]
    fn test_rewrite_docker_compose_logs() {
        assert_eq!(
            rewrite_command_no_prefixes("docker compose logs web", &[]),
            Some("rtk docker compose logs web".into())
        );
    }

    #[test]
    fn test_rewrite_docker_compose_build() {
        assert_eq!(
            rewrite_command_no_prefixes("docker compose build", &[]),
            Some("rtk docker compose build".into())
        );
    }

    #[test]
    fn test_rewrite_docker_compose_up_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("docker compose up -d", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_docker_compose_down_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("docker compose down", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_docker_compose_config_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("docker compose -f foo.yaml config --services", &[]),
            None
        );
    }

    // --- AWS / psql (PR #216) ---

    #[test]
    fn test_classify_aws() {
        assert!(matches!(
            classify_command("aws s3 ls"),
            Classification::Supported {
                rtk_equivalent: "rtk aws",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_aws_ec2() {
        assert!(matches!(
            classify_command("aws ec2 describe-instances"),
            Classification::Supported {
                rtk_equivalent: "rtk aws",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_psql() {
        assert!(matches!(
            classify_command("psql -U postgres"),
            Classification::Supported {
                rtk_equivalent: "rtk psql",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_psql_url() {
        assert!(matches!(
            classify_command("psql postgres://localhost/mydb"),
            Classification::Supported {
                rtk_equivalent: "rtk psql",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_aws() {
        assert_eq!(
            rewrite_command_no_prefixes("aws s3 ls", &[]),
            Some("rtk aws s3 ls".into())
        );
    }

    #[test]
    fn test_rewrite_aws_ec2() {
        assert_eq!(
            rewrite_command_no_prefixes("aws ec2 describe-instances --region us-east-1", &[]),
            Some("rtk aws ec2 describe-instances --region us-east-1".into())
        );
    }

    #[test]
    fn test_rewrite_psql() {
        assert_eq!(
            rewrite_command_no_prefixes("psql -U postgres -d mydb", &[]),
            Some("rtk psql -U postgres -d mydb".into())
        );
    }

    // --- Python tooling ---

    #[test]
    fn test_classify_ruff_check() {
        assert!(matches!(
            classify_command("ruff check ."),
            Classification::Supported {
                rtk_equivalent: "rtk ruff",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_ruff_format() {
        assert!(matches!(
            classify_command("ruff format src/"),
            Classification::Supported {
                rtk_equivalent: "rtk ruff",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_sqlfluff_lint() {
        assert!(matches!(
            classify_command("sqlfluff lint models/"),
            Classification::Supported {
                rtk_equivalent: "rtk sqlfluff",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_pytest() {
        assert!(matches!(
            classify_command("pytest tests/"),
            Classification::Supported {
                rtk_equivalent: "rtk pytest",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_python_m_pytest() {
        assert!(matches!(
            classify_command("python -m pytest tests/"),
            Classification::Supported {
                rtk_equivalent: "rtk pytest",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_pip_list() {
        assert!(matches!(
            classify_command("pip list"),
            Classification::Supported {
                rtk_equivalent: "rtk pip",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_uv_pip_list() {
        assert!(matches!(
            classify_command("uv pip list"),
            Classification::Supported {
                rtk_equivalent: "rtk pip",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_ruff_check() {
        assert_eq!(
            rewrite_command_no_prefixes("ruff check .", &[]),
            Some("rtk ruff check .".into())
        );
    }

    #[test]
    fn test_rewrite_ruff_format() {
        assert_eq!(
            rewrite_command_no_prefixes("ruff format src/", &[]),
            Some("rtk ruff format src/".into())
        );
    }

    #[test]
    fn test_rewrite_sqlfluff_lint() {
        assert_eq!(
            rewrite_command_no_prefixes("sqlfluff lint models/", &[]),
            Some("rtk sqlfluff lint models/".into())
        );
    }

    #[test]
    fn test_rewrite_pytest() {
        assert_eq!(
            rewrite_command_no_prefixes("pytest tests/", &[]),
            Some("rtk pytest tests/".into())
        );
    }

    #[test]
    fn test_rewrite_python_m_pytest() {
        assert_eq!(
            rewrite_command_no_prefixes("python -m pytest -x tests/", &[]),
            Some("rtk pytest -x tests/".into())
        );
    }

    #[test]
    fn test_rewrite_uv_run_pytest() {
        assert_eq!(
            rewrite_command_no_prefixes("uv run pytest tests/", &[]),
            Some("uv run rtk pytest tests/".into())
        );
    }

    #[test]
    fn test_rewrite_env_uv_run_pytest() {
        assert_eq!(
            rewrite_command_no_prefixes("PYTHONPATH=. uv run pytest tests/", &[]),
            Some("PYTHONPATH=. uv run rtk pytest tests/".into())
        );
    }

    #[test]
    fn test_rewrite_uv_run_python_m_pytest() {
        assert_eq!(
            rewrite_command_no_prefixes("uv run python -m pytest -q", &[]),
            Some("uv run rtk pytest -q".into())
        );
    }

    #[test]
    fn test_rewrite_uv_run_supported_inner_command() {
        assert_eq!(
            rewrite_command_no_prefixes("uv run ruff check .", &[]),
            Some("uv run rtk ruff check .".into())
        );
    }

    #[test]
    fn test_rewrite_uv_run_options_are_passed_through() {
        assert_eq!(
            rewrite_command_no_prefixes("uv run --unknown pytest tests/", &[]),
            Some("rtk uv run --unknown pytest tests/".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("uv run -m pytest -q", &[]),
            Some("rtk uv run -m pytest -q".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("uv run --module pytest -q", &[]),
            Some("rtk uv run --module pytest -q".into())
        );
    }

    #[test]
    fn test_rewrite_pip_list() {
        assert_eq!(
            rewrite_command_no_prefixes("pip list", &[]),
            Some("rtk pip list".into())
        );
    }

    #[test]
    fn test_rewrite_pip_outdated() {
        assert_eq!(
            rewrite_command_no_prefixes("pip outdated", &[]),
            Some("rtk pip outdated".into())
        );
    }

    #[test]
    fn test_rewrite_uv_pip_list() {
        assert_eq!(
            rewrite_command_no_prefixes("uv pip list", &[]),
            Some("rtk pip list".into())
        );
    }

    #[test]
    fn test_classify_uv_run() {
        let commands = vec![
            "uv run python script.py",
            "uv run pytest",
            "uv run ruff check",
            "uv run --project backend --extra dev python script.py",
        ];

        for command in commands {
            assert!(
                matches!(
                    classify_command(command),
                    Classification::Supported {
                        rtk_equivalent: "rtk uv",
                        ..
                    }
                ),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_shell_keyword_prefix_does_not_fall_through_to_whole_string() {
        // `rtk exec date` would be unspawnable, so an unfiltered inner command
        // must drop the rewrite rather than re-test the prefixed string.
        for cmd in [
            "exec somethingunfiltered",
            "noglob somethingunfiltered",
            "command somethingunfiltered",
            "builtin somethingunfiltered",
            "nocorrect somethingunfiltered",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                None,
                "Failed for command: {}",
                cmd
            );
        }
    }

    #[test]
    fn test_rewrite_uv_run() {
        let cases = vec![
            ("uv run pytest", "uv run rtk pytest"),
            ("uv run ruff check", "uv run rtk ruff check"),
            ("uv run python script.py", "rtk uv run python script.py"),
            (
                "uv run --project backend --extra dev python script.py",
                "rtk uv run --project backend --extra dev python script.py",
            ),
        ];

        for (command, expected) in cases {
            assert_eq!(
                rewrite_command_no_prefixes(command, &[]),
                Some(expected.to_string()),
                "Failed for command: {}",
                command
            );
        }
    }

    // --- Go tooling ---

    #[test]
    fn test_classify_go_test() {
        assert!(matches!(
            classify_command("go test ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk go",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_go_build() {
        assert!(matches!(
            classify_command("go build ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk go",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_go_vet() {
        assert!(matches!(
            classify_command("go vet ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk go",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint() {
        assert!(matches!(
            classify_command("golangci-lint run"),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_with_flag_before_run() {
        assert!(matches!(
            classify_command("golangci-lint -v run ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_with_value_flag_before_run() {
        assert!(matches!(
            classify_command("golangci-lint --color never run ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_with_inline_value_flag_before_run() {
        assert!(matches!(
            classify_command("golangci-lint --color=never run ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_with_quoted_value_flag_before_run() {
        // A quoted global-flag value containing a space (`--config "a path/x.yml"`)
        // The space inside the quotes does not split the value: `--config` and
        // `"a path/x.yml"` are two words, so `run` is found after them.
        assert!(matches!(
            classify_command(r#"golangci-lint --config "a path/x.yml" run ./..."#),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_with_unquoted_glob_value_flag_before_run() {
        // An UNQUOTED global-flag value containing a shell metacharacter
        // (`--config *.yml`) must also stay one word. Routing this through the
        // full shell tokenize() (rather than a quote-aware but syntax-blind
        // word splitter) regressed this: tokenize() treats `*` as its own
        // Shellism token even outside quotes, splitting "*.yml" into "*" and
        // ".yml" and desyncing the flag-value-skip loop, which then reads
        // ".yml" where it expects "run" and misclassifies the whole command as
        // Unsupported.
        assert!(matches!(
            classify_command("golangci-lint --config *.yml run ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_with_inline_config_flag_before_run() {
        assert!(matches!(
            classify_command("golangci-lint --config=foo.yml run ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_bare_is_not_compact_wrapper() {
        assert!(!matches!(
            classify_command("golangci-lint"),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_other_subcommand_is_not_compact_wrapper() {
        assert!(!matches!(
            classify_command("golangci-lint version"),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_go_test() {
        assert_eq!(
            rewrite_command_no_prefixes("go test ./...", &[]),
            Some("rtk go test ./...".into())
        );
    }

    #[test]
    fn test_rewrite_go_build() {
        assert_eq!(
            rewrite_command_no_prefixes("go build ./...", &[]),
            Some("rtk go build ./...".into())
        );
    }

    #[test]
    fn test_rewrite_go_vet() {
        assert_eq!(
            rewrite_command_no_prefixes("go vet ./...", &[]),
            Some("rtk go vet ./...".into())
        );
    }

    #[test]
    fn test_rewrite_golangci_lint() {
        assert_eq!(
            rewrite_command_no_prefixes("golangci-lint run ./...", &[]),
            Some("rtk golangci-lint run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_golangci_lint_with_flag_before_run() {
        assert_eq!(
            rewrite_command_no_prefixes("golangci-lint -v run ./...", &[]),
            Some("rtk golangci-lint -v run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_golangci_lint_with_value_flag_before_run() {
        assert_eq!(
            rewrite_command_no_prefixes("golangci-lint --color never run ./...", &[]),
            Some("rtk golangci-lint --color never run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_golangci_lint_with_inline_value_flag_before_run() {
        assert_eq!(
            rewrite_command_no_prefixes("golangci-lint --color=never run ./...", &[]),
            Some("rtk golangci-lint --color=never run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_golangci_lint_with_inline_config_flag_before_run() {
        assert_eq!(
            rewrite_command_no_prefixes("golangci-lint --config=foo.yml run ./...", &[]),
            Some("rtk golangci-lint --config=foo.yml run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_env_prefixed_golangci_lint_with_value_flag_before_run() {
        assert_eq!(
            rewrite_command_no_prefixes("FOO=1 golangci-lint --color never run ./...", &[]),
            Some("FOO=1 rtk golangci-lint --color never run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_env_prefixed_golangci_lint_with_inline_value_flag_before_run() {
        assert_eq!(
            rewrite_command_no_prefixes("FOO=1 golangci-lint --color=never run ./...", &[]),
            Some("FOO=1 rtk golangci-lint --color=never run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_bare_golangci_lint_skips_compact_wrapper() {
        assert_eq!(rewrite_command_no_prefixes("golangci-lint", &[]), None);
    }

    #[test]
    fn test_rewrite_other_golangci_lint_subcommand_skips_compact_wrapper() {
        assert_eq!(
            rewrite_command_no_prefixes("golangci-lint version", &[]),
            None
        );
    }

    // --- JS/TS tooling ---

    #[test]
    fn test_classify_lint() {
        let commands = vec![
            "npm exec biome",
            "npm exec eslint",
            "npm rum biome",
            "npm rum eslint",
            "npm rum lint",
            "npm run biome",
            "npm run eslint",
            "npm run lint",
            "npm run-script biome",
            "npm run-script eslint",
            "npm run-script lint",
            "npm urn biome",
            "npm urn eslint",
            "npm urn lint",
            "npm x biome",
            "npm x eslint",
            "pnpm dlx biome",
            "pnpm dlx eslint",
            "pnpm exec biome",
            "pnpm exec eslint",
            "pnpm run biome",
            "pnpm run eslint",
            "pnpm run lint",
            "pnpm run-script biome",
            "pnpm run-script eslint",
            "pnpm run-script lint",
            "npm biome",
            "npm eslint",
            "npm lint",
            "npx biome",
            "npx eslint",
            "npx lint",
            "pnpm biome",
            "pnpm eslint",
            "pnpm lint",
            "pnpx biome",
            "pnpx eslint",
            "pnpx lint",
            "biome",
            "eslint",
            "lint",
        ];
        for command in commands {
            assert!(
                matches!(
                    classify_command(command),
                    Classification::Supported {
                        rtk_equivalent: "rtk lint",
                        ..
                    }
                ),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_lint() {
        let commands = vec![
            "npm exec biome",
            "npm exec eslint",
            "npm rum biome",
            "npm rum eslint",
            "npm rum lint",
            "npm run biome",
            "npm run eslint",
            "npm run lint",
            "npm run-script biome",
            "npm run-script eslint",
            "npm run-script lint",
            "npm urn biome",
            "npm urn eslint",
            "npm urn lint",
            "npm x biome",
            "npm x eslint",
            "pnpm dlx biome",
            "pnpm dlx eslint",
            "pnpm exec biome",
            "pnpm exec eslint",
            "pnpm run biome",
            "pnpm run eslint",
            "pnpm run lint",
            "pnpm run-script biome",
            "pnpm run-script eslint",
            "pnpm run-script lint",
            "npm biome",
            "npm eslint",
            "npm lint",
            "npx biome",
            "npx eslint",
            "npx lint",
            "pnpm biome",
            "pnpm eslint",
            "pnpm lint",
            "pnpx biome",
            "pnpx eslint",
            "pnpx lint",
            "biome",
            "eslint",
            "lint",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command_no_prefixes(command, &[]),
                Some("rtk lint".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_classify_jest() {
        let commands = vec![
            "jest run",
            "jest",
            "npm exec jest run",
            "npm exec jest",
            "npm jest run",
            "npm jest",
            "npm rum jest run",
            "npm rum jest",
            "npm run jest run",
            "npm run jest",
            "npm run-script jest run",
            "npm run-script jest",
            "npm urn jest run",
            "npm urn jest",
            "npm x jest run",
            "npm x jest",
            "npx jest run",
            "npx jest",
            "pnpm dlx jest run",
            "pnpm dlx jest",
            "pnpm exec jest run",
            "pnpm exec jest",
            "pnpm jest run",
            "pnpm jest",
            "pnpm run jest run",
            "pnpm run jest",
            "pnpm run-script jest run",
            "pnpm run-script jest",
            "pnpx jest run",
            "pnpx jest",
        ];
        for command in commands {
            assert!(
                matches!(
                    classify_command(command),
                    Classification::Supported {
                        rtk_equivalent: "rtk jest",
                        ..
                    }
                ),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_jest() {
        let commands = vec![
            "jest run",
            "jest",
            "npm exec jest run",
            "npm exec jest",
            "npm jest run",
            "npm jest",
            "npm rum jest run",
            "npm rum jest",
            "npm run jest run",
            "npm run jest",
            "npm run-script jest run",
            "npm run-script jest",
            "npm urn jest run",
            "npm urn jest",
            "npm x jest run",
            "npm x jest",
            "npx jest run",
            "npx jest",
            "pnpm dlx jest run",
            "pnpm dlx jest",
            "pnpm exec jest run",
            "pnpm exec jest",
            "pnpm jest run",
            "pnpm jest",
            "pnpm run jest run",
            "pnpm run jest",
            "pnpm run-script jest run",
            "pnpm run-script jest",
            "pnpx jest run",
            "pnpx jest",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command_no_prefixes(command, &[]),
                Some("rtk jest".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_classify_vitest() {
        let commands = vec![
            "npm exec vitest run",
            "npm exec vitest",
            "npm rum vitest run",
            "npm rum vitest",
            "npm run vitest run",
            "npm run vitest",
            "npm run-script vitest run",
            "npm run-script vitest",
            "npm urn vitest run",
            "npm urn vitest",
            "npm vitest run",
            "npm vitest",
            "npm x vitest run",
            "npm x vitest",
            "npx vitest run",
            "npx vitest",
            "pnpm dlx vitest run",
            "pnpm dlx vitest",
            "pnpm exec vitest run",
            "pnpm exec vitest",
            "pnpm run vitest run",
            "pnpm run vitest",
            "pnpm run-script vitest run",
            "pnpm run-script vitest",
            "pnpm vitest run",
            "pnpm vitest",
            "pnpx vitest run",
            "pnpx vitest",
            "vitest run",
            "vitest",
        ];
        for command in commands {
            assert!(
                matches!(
                    classify_command(command),
                    Classification::Supported {
                        rtk_equivalent: "rtk vitest",
                        ..
                    }
                ),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_vitest() {
        let commands = vec![
            "npm exec vitest run",
            "npm exec vitest",
            "npm rum vitest run",
            "npm rum vitest",
            "npm run vitest run",
            "npm run vitest",
            "npm run-script vitest run",
            "npm run-script vitest",
            "npm urn vitest run",
            "npm urn vitest",
            "npm vitest run",
            "npm vitest",
            "npm x vitest run",
            "npm x vitest",
            "npx vitest run",
            "npx vitest",
            "pnpm dlx vitest run",
            "pnpm dlx vitest",
            "pnpm exec vitest run",
            "pnpm exec vitest",
            "pnpm run vitest run",
            "pnpm run vitest",
            "pnpm run-script vitest run",
            "pnpm run-script vitest",
            "pnpm vitest run",
            "pnpm vitest",
            "pnpx vitest run",
            "pnpx vitest",
            "vitest run",
            "vitest",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command_no_prefixes(command, &[]),
                Some("rtk vitest".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_classify_prisma() {
        let commands = vec![
            "npm exec prisma",
            "npm rum prisma",
            "npm run prisma",
            "npm run-script prisma",
            "npm urn prisma",
            "npm x prisma",
            "pnpm dlx prisma",
            "pnpm exec prisma",
            "pnpm run prisma",
            "pnpm run-script prisma",
            "npm prisma",
            "npx prisma",
            "pnpm prisma",
            "pnpx prisma",
            "prisma",
        ];
        for command in commands {
            assert!(
                matches!(
                    classify_command(format!("{command} migrate dev").as_str()),
                    Classification::Supported {
                        rtk_equivalent: "rtk prisma",
                        ..
                    }
                ),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_prisma() {
        let commands = vec![
            "npm exec prisma",
            "npm rum prisma",
            "npm run prisma",
            "npm run-script prisma",
            "npm urn prisma",
            "npm x prisma",
            "pnpm dlx prisma",
            "pnpm exec prisma",
            "pnpm run prisma",
            "pnpm run-script prisma",
            "npm prisma",
            "npx prisma",
            "pnpm prisma",
            "pnpx prisma",
            "prisma",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command_no_prefixes(format!("{command} migrate dev").as_str(), &[]),
                Some("rtk prisma migrate dev".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_prettier() {
        let commands = vec![
            "npm exec prettier",
            "npm rum prettier",
            "npm run prettier",
            "npm run-script prettier",
            "npm urn prettier",
            "npm x prettier",
            "pnpm dlx prettier",
            "pnpm exec prettier",
            "pnpm run prettier",
            "pnpm run-script prettier",
            "npm prettier",
            "npx prettier",
            "pnpm prettier",
            "pnpx prettier",
            "prettier",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command_no_prefixes(format!("{command} --check src/").as_str(), &[]),
                Some("rtk prettier --check src/".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_pnpm_command() {
        let commands = vec![
            "exec",
            "i",
            "install",
            "list",
            "ls",
            "outdated",
            "run",
            "run-script",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command_no_prefixes(format!("pnpm {command}").as_str(), &[]),
                Some(format!("rtk pnpm {command}")),
                "Failed for command: pnpm {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_npm_bare_subcommand() {
        let commands = vec!["exec", "run", "run-script", "x"];
        for command in commands {
            assert_eq!(
                rewrite_command_no_prefixes(format!("npm {command}").as_str(), &[]),
                Some(format!("rtk npm {command}")),
                "Failed for bare command: npm {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_npm_with_args() {
        assert_eq!(
            rewrite_command_no_prefixes("npm run test", &[]),
            Some("rtk npm run test".to_string()),
        );
        assert_eq!(
            rewrite_command_no_prefixes("npm exec vitest", &[]),
            Some("rtk vitest".to_string()),
        );
    }

    #[test]
    fn test_rewrite_npx() {
        assert_eq!(
            rewrite_command_no_prefixes("npx svgo", &[]),
            Some("rtk npx svgo".to_string()),
        );
    }

    // --- Gradle ---

    #[test]
    fn test_classify_gradlew() {
        assert!(matches!(
            classify_command("./gradlew assembleDebug"),
            Classification::Supported {
                rtk_equivalent: "rtk gradlew",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_gradlew_no_dot_slash() {
        assert!(matches!(
            classify_command("gradlew build"),
            Classification::Supported {
                rtk_equivalent: "rtk gradlew",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_gradlew_bat() {
        assert!(matches!(
            classify_command("gradlew.bat clean"),
            Classification::Supported {
                rtk_equivalent: "rtk gradlew",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_gradle() {
        assert!(matches!(
            classify_command("gradle build"),
            Classification::Supported {
                rtk_equivalent: "rtk gradlew",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_gradlew() {
        assert_eq!(
            rewrite_command_no_prefixes("./gradlew assembleDebug", &[]),
            Some("rtk gradlew assembleDebug".into())
        );
    }

    #[test]
    fn test_rewrite_gradlew_no_dot_slash() {
        assert_eq!(
            rewrite_command_no_prefixes("gradlew build", &[]),
            Some("rtk gradlew build".into())
        );
    }

    #[test]
    fn test_rewrite_gradlew_bat() {
        assert_eq!(
            rewrite_command_no_prefixes("gradlew.bat clean", &[]),
            Some("rtk gradlew clean".into())
        );
    }

    #[test]
    fn test_rewrite_gradle() {
        assert_eq!(
            rewrite_command_no_prefixes("gradle build", &[]),
            Some("rtk gradlew build".into())
        );
    }

    #[test]
    fn test_rewrite_gradlew_test_savings() {
        assert_eq!(
            classify_command("./gradlew test"),
            Classification::Supported {
                rtk_equivalent: "rtk gradlew",
                category: "Build",
                estimated_savings_pct: 90.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_rewrite_sbt_test_only() {
        assert_eq!(
            rewrite_command_no_prefixes("sbt testOnly com.example.MySpec", &[]),
            Some("rtk sbt testOnly com.example.MySpec".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes(r#"sbt "testOnly com.example.MySpec""#, &[]),
            Some(r#"rtk sbt "testOnly com.example.MySpec""#.into())
        );
        assert_eq!(
            rewrite_command_no_prefixes(r#"sbt "testOnly *MySpec -- -z foo""#, &[]),
            Some(r#"rtk sbt "testOnly *MySpec -- -z foo""#.into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("sbt testQuick", &[]),
            Some("rtk sbt testQuick".into())
        );
    }

    #[test]
    fn test_rewrite_sbt_does_not_match_unrelated_tasks() {
        assert_eq!(rewrite_command_no_prefixes("sbt testify", &[]), None);
        assert_eq!(
            rewrite_command_no_prefixes(r#"sbt "test:compile""#, &[]),
            None
        );
    }

    // --- Maven ---

    #[test]
    fn test_classify_mvn_test() {
        assert!(matches!(
            classify_command("mvn test"),
            Classification::Supported {
                rtk_equivalent: "rtk mvn",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_mvn_integration_test() {
        assert!(matches!(
            classify_command("mvn integration-test"),
            Classification::Supported {
                rtk_equivalent: "rtk mvn",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_mvn_flags_before_goal() {
        assert!(matches!(
            classify_command("mvn -B -DskipTests=false clean install"),
            Classification::Supported {
                rtk_equivalent: "rtk mvn",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_mvnw_wrapper() {
        assert!(matches!(
            classify_command("./mvnw verify"),
            Classification::Supported {
                rtk_equivalent: "rtk mvn",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_mvnw_cmd_wrapper() {
        assert!(matches!(
            classify_command("mvnw.cmd package"),
            Classification::Supported {
                rtk_equivalent: "rtk mvn",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_mvn_clean_bypassed() {
        // `clean` deliberately excluded from the alternation to avoid 0-overhead fork.
        assert!(!matches!(
            classify_command("mvn clean"),
            Classification::Supported {
                rtk_equivalent: "rtk mvn",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_mvn_site_bypassed() {
        assert!(!matches!(
            classify_command("mvn site"),
            Classification::Supported {
                rtk_equivalent: "rtk mvn",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_mvn_plugin_goal_bypassed() {
        assert!(!matches!(
            classify_command("mvn dependency:tree"),
            Classification::Supported {
                rtk_equivalent: "rtk mvn",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_mvn_bare_bypassed() {
        assert!(!matches!(
            classify_command("mvn"),
            Classification::Supported {
                rtk_equivalent: "rtk mvn",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_mvn_version_bypassed() {
        assert!(!matches!(
            classify_command("mvn --version"),
            Classification::Supported {
                rtk_equivalent: "rtk mvn",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_mvn_clean_install() {
        assert_eq!(
            rewrite_command_no_prefixes("mvn -B clean install", &[]),
            Some("rtk mvn -B clean install".into())
        );
    }

    #[test]
    fn test_rewrite_mvnw_test() {
        assert_eq!(
            rewrite_command_no_prefixes("./mvnw test", &[]),
            Some("rtk mvn test".into())
        );
    }

    /// rtk-ai/rtk#3184 — `mvnd` must route to `rtk mvnd`, never `rtk mvn`,
    /// so the daemon binary is the one that actually runs.
    #[test]
    fn test_rewrite_mvnd_clean_install() {
        assert_eq!(
            rewrite_command_no_prefixes("mvnd clean install", &[]),
            Some("rtk mvnd clean install".into())
        );
    }

    #[test]
    fn test_classify_mvnd_test() {
        assert!(matches!(
            classify_command("mvnd test"),
            Classification::Supported {
                rtk_equivalent: "rtk mvnd",
                ..
            }
        ));
    }

    /// Upstream PR #3199 review, finding 5 — `mvnd.cmd` (mvnd's Windows
    /// wrapper) must classify and rewrite to `rtk mvnd`, mirroring how the
    /// mvn rule handles `mvnw.cmd`. `mvnd` alone stops at the `.`, where
    /// `[ \t\n]+(compile|...)` cannot follow, so the pattern names
    /// `mvnd.cmd` itself.
    #[test]
    fn test_classify_mvnd_cmd_wrapper() {
        assert!(matches!(
            classify_command("mvnd.cmd package"),
            Classification::Supported {
                rtk_equivalent: "rtk mvnd",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_mvnd_cmd_clean_install() {
        assert_eq!(
            rewrite_command_no_prefixes("mvnd.cmd clean install", &[]),
            Some("rtk mvnd clean install".into())
        );
    }

    // --- Compound operator edge cases ---

    #[test]
    fn test_rewrite_compound_or() {
        // `||` fallback: left rewritten, right rewritten
        assert_eq!(
            rewrite_command_no_prefixes("cargo test || cargo build", &[]),
            Some("rtk cargo test || rtk cargo build".into())
        );
    }

    #[test]
    fn test_rewrite_compound_semicolon() {
        assert_eq!(
            rewrite_command_no_prefixes("git status; cargo test", &[]),
            Some("rtk git status; rtk cargo test".into())
        );
    }

    #[test]
    fn test_rewrite_compound_pipe_raw_filter() {
        // Producers stay raw; only a pipeline-safe final stage is rewritten.
        assert_eq!(
            rewrite_command_no_prefixes("cargo test | grep FAILED", &[]),
            Some("cargo test | rtk grep FAILED".into())
        );
    }

    #[test]
    fn test_rewrite_compound_pipe_git_grep() {
        assert_eq!(
            rewrite_command_no_prefixes("git log -10 | grep feat", &[]),
            Some("git log -10 | rtk grep feat".into())
        );
    }

    #[test]
    fn test_rewrite_compound_four_segments() {
        assert_eq!(
            rewrite_command_no_prefixes(
                "cargo fmt --all && cargo clippy && cargo test && git status",
                &[]
            ),
            Some(
                "rtk cargo fmt --all && rtk cargo clippy && rtk cargo test && rtk git status"
                    .into()
            )
        );
    }

    #[test]
    fn test_rewrite_compound_mixed_supported_unsupported() {
        // unsupported segments stay raw
        assert_eq!(
            rewrite_command_no_prefixes("cargo test && htop", &[]),
            Some("rtk cargo test && htop".into())
        );
    }

    #[test]
    fn test_rewrite_compound_all_unsupported_returns_none() {
        // No rewrite at all: returns None
        assert_eq!(rewrite_command_no_prefixes("htop && top", &[]), None);
    }

    // --- sudo / env prefix + rewrite ---

    #[test]
    fn test_rewrite_sudo_passthrough() {
        // sudo commands are not rewritten (#146): `sudo rtk …` would fail under
        // root's secure_path / run rtk as root. They pass through unchanged.
        assert_eq!(rewrite_command_no_prefixes("sudo docker ps", &[]), None);
        assert_eq!(
            rewrite_command_no_prefixes("sudo -u root docker ps", &[]),
            None
        );
        assert_eq!(rewrite_command_no_prefixes("sudo git status", &[]), None);
        // The passthrough must also survive an env prefix in front of sudo, a bare
        // `sudo`, and must not catch `sudoedit` (#3569's motivating cases).
        assert_eq!(
            rewrite_command_no_prefixes("FOO=1 sudo docker ps", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("env FOO=1 sudo docker ps", &[]),
            None
        );
        assert_eq!(rewrite_command_no_prefixes("sudo", &[]), None);
        assert_eq!(
            rewrite_command_no_prefixes("sudoedit /etc/hosts", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_env_var_prefix() {
        assert_eq!(
            rewrite_command_no_prefixes("GIT_SSH_COMMAND=ssh git push origin main", &[]),
            Some("GIT_SSH_COMMAND=ssh rtk git push origin main".into())
        );
    }

    // --- find with native flags ---

    #[test]
    fn test_rewrite_find_with_flags() {
        assert_eq!(
            rewrite_command_no_prefixes("find . -name '*.rs' -type f", &[]),
            Some("rtk find . -name '*.rs' -type f".into())
        );
    }

    #[test]
    fn test_all_rules_are_complete() {
        for rule in RULES {
            assert!(
                !rule.pattern.is_empty(),
                "Rule '{}' has empty pattern",
                rule.rtk_cmd
            );
            assert!(!rule.rtk_cmd.is_empty(), "Rule with empty rtk_cmd found");
            assert!(
                rule.rtk_cmd.starts_with("rtk "),
                "rtk_cmd '{}' must start with 'rtk '",
                rule.rtk_cmd
            );
            assert!(
                !rule.rewrite_prefixes.is_empty(),
                "Rule '{}' has no rewrite_prefixes",
                rule.rtk_cmd
            );
        }
    }

    // --- exclude_commands (#243) ---

    #[test]
    fn test_rewrite_excludes_curl() {
        let excluded = vec!["curl".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("curl https://api.example.com/health", &excluded),
            None
        );
    }

    #[test]
    fn test_rewrite_exclude_does_not_affect_other_commands() {
        let excluded = vec!["curl".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("git status", &excluded),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_empty_excludes_rewrites_curl() {
        let excluded: Vec<String> = vec![];
        assert!(rewrite_command_no_prefixes("curl https://api.example.com", &excluded).is_some());
    }

    #[test]
    fn test_rewrite_compound_partial_exclude() {
        // curl excluded but git still rewrites
        let excluded = vec!["curl".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("git status && curl https://api.example.com", &excluded),
            Some("rtk git status && curl https://api.example.com".into())
        );
    }

    #[test]
    fn test_exclude_env_prefixed_command() {
        let excluded = vec!["psql".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("PGPASSWORD=postgres psql -h localhost", &excluded),
            None
        );
    }

    #[test]
    fn test_exclude_subcommand_pattern() {
        let excluded = vec!["git push".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("git push origin main", &excluded),
            None
        );
    }

    #[test]
    fn test_exclude_regex_pattern() {
        let excluded = vec!["^curl".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("curl http://example.com", &excluded),
            None
        );
    }

    #[test]
    fn test_exclude_invalid_regex_fallback() {
        let excluded = vec!["curl[".to_string()];
        assert!(rewrite_command_no_prefixes("curl http://example.com", &excluded).is_some());
    }

    #[test]
    fn test_exclude_covers_php_wrapper_forms() {
        // The rewrite path normalizes `php` + ini flags, `./`, and vendor/composer
        // bin dirs; the exclusion must see the same canonical form.
        for (pattern, cmd) in [
            ("phpunit", "vendor/bin/phpunit tests/"),
            ("phpunit", "php vendor/bin/phpunit tests/"),
            ("phpunit", "php bin/phpunit"),
            ("phpstan", "php vendor/bin/phpstan analyse src"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[pattern.to_string()]),
                None,
                "expected `{}` to be excluded by `{}`",
                cmd,
                pattern
            );
        }
        // A different PHP tool is untouched.
        assert!(
            rewrite_command_no_prefixes(
                "php vendor/bin/phpstan analyse src",
                &["phpunit".to_string()]
            )
            .is_some()
        );
    }

    #[test]
    fn test_exclude_matches_wrapper_invoked_form() {
        // #243: the README example, across every form that reaches `rtk playwright`.
        let excluded = vec!["playwright".to_string()];
        for cmd in [
            "playwright test",
            "npx playwright test",
            "pnpm exec playwright test",
            "pnpm dlx playwright test",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &excluded),
                None,
                "expected `{}` to be excluded",
                cmd
            );
        }
    }

    #[test]
    fn test_exclude_covers_interpreter_and_path_forms() {
        // #3035: the interpreter form, and the path forms noted in #1053.
        for (pattern, cmd) in [
            ("pytest", "python3 -m pytest tests/ -q"),
            ("pytest", "python -m pytest tests/"),
            ("mypy", "python -m mypy ."),
            ("gradlew", "./gradlew assembleDebug"),
            ("phpunit", "vendor/bin/phpunit tests/"),
            ("rspec", "bundle exec rspec"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[pattern.to_string()]),
                None,
                "expected `{}` to be excluded by `{}`",
                cmd,
                pattern
            );
        }
    }

    #[test]
    fn test_exclude_covers_wrapper_when_tool_name_differs_from_target() {
        // `eslint` and `biome` both resolve to `rtk lint`, so matching the resolved
        // target instead of the peeled command would miss the wrapper form entirely.
        let excluded = vec!["eslint".to_string()];
        assert_eq!(rewrite_command_no_prefixes("eslint .", &excluded), None);
        assert_eq!(rewrite_command_no_prefixes("npx eslint .", &excluded), None);
        // ...and does not reach the other tool sharing that target.
        assert!(rewrite_command_no_prefixes("npx biome check .", &excluded).is_some());
    }

    #[test]
    fn test_exclude_keeps_arguments_so_anchored_regex_still_narrows() {
        // An end-anchored entry exists to exclude the bare invocation only. Peeling
        // must not drop the arguments, or `^ls$` would swallow every `ls`.
        let excluded = vec!["^ls$".to_string()];
        assert_eq!(rewrite_command_no_prefixes("ls", &excluded), None);
        assert!(rewrite_command_no_prefixes("ls -la", &excluded).is_some());

        // The same anchoring works through a wrapper.
        let excluded = vec!["^pytest ".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("python3 -m pytest tests/", &excluded),
            None
        );
    }

    #[test]
    fn test_exclude_does_not_widen_across_tools_sharing_a_target() {
        // Entries name the tool the user types, not rtk's internal command, so an
        // entry must not leak to every tool routed to the same filter.
        for (pattern, cmd) in [
            ("read", "cat foo.txt"),
            ("lint", "eslint ."),
            ("lint", "biome check ."),
            ("git", "yadm status"),
        ] {
            assert!(
                rewrite_command_no_prefixes(cmd, &[pattern.to_string()]).is_some(),
                "`{}` must not be excluded by `{}`",
                cmd,
                pattern
            );
        }
    }

    #[test]
    fn test_exclude_peeled_form_is_exact_token() {
        let excluded = vec!["go".to_string()];
        assert!(rewrite_command_no_prefixes("golangci-lint run ./...", &excluded).is_some());
        assert_eq!(
            rewrite_command_no_prefixes("go build ./...", &excluded),
            None
        );
        // A rule whose prefix carries a subcommand keeps it, so `golangci-lint run`
        // does not collapse to `run`.
        assert_eq!(
            rewrite_command_no_prefixes("golangci-lint run ./...", &["golangci-lint".to_string()]),
            None
        );
    }

    #[test]
    fn test_exclude_subcommand_pattern_stays_narrow() {
        let excluded = vec!["git push".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("git push origin main", &excluded),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("git status", &excluded),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_exclude_does_not_substring_match() {
        let excluded = vec!["go".to_string()];
        assert!(rewrite_command_no_prefixes("golangci-lint run ./...", &excluded).is_some());
    }

    #[test]
    fn test_exclude_does_not_match_hyphenated_command() {
        let excluded = vec!["golangci".to_string()];
        assert!(rewrite_command_no_prefixes("golangci-lint run ./...", &excluded).is_some());
    }

    #[test]
    fn test_exclude_empty_pattern_ignored() {
        let excluded = vec!["".to_string()];
        assert!(rewrite_command_no_prefixes("git status", &excluded).is_some());
    }

    #[test]
    fn test_exclude_bare_anchor_ignored() {
        let excluded = vec!["^".to_string()];
        assert!(rewrite_command_no_prefixes("git status", &excluded).is_some());
    }

    /// A rule matches against a whole command line, so a pattern that ends in
    /// a bare alternation matches any command whose subcommand merely *starts*
    /// with a listed one — `git branchless` routes into `rtk git` (#4009).
    #[test]
    fn test_every_rule_pattern_is_anchored_at_both_ends() {
        // The spellings in use. `[ \t\n]+` is the form used by rules that take
        // a whole command rather than a subcommand alternation. `\b`, `\s` and
        // `\S` are not among them: they end a word at `\r`, a vertical tab, a
        // form feed or a non-breaking space (and `\b` at any punctuation), where
        // bash keeps reading it.
        const APPROVED_SUFFIXES: &[&str] = &[
            r"(?:[ \t\n]|$|[;|&()<>])",
            // `:` ends the word too where a namespaced subcommand is the point:
            // `rake test:unit` and `rails test:system` are the task, not a
            // command that merely starts with `test`.
            r"(?:[ \t\n:]|$|[;|&()<>])",
            r#"(?:[ \t\n"']|$)"#,
            r"(?:[ \t\n]|$)",
            r"([ \t\n]|$)",
            r"[ \t\n]+",
            r"$",
        ];

        // Pending #3676, which is what would give pnpm's bare script forms a
        // terminator. An entry that no longer matches any rule fails the check
        // below, so this cannot rot into a permanent exemption.
        const PENDING: &[&str] =
            &[r"^pnpm[ \t\n]+(exec|i|install|list|ls|outdated|run|run-script)"];

        let mut unanchored_start = Vec::new();
        let mut unicode_word_end = Vec::new();
        let mut unterminated = Vec::new();
        for rule in RULES {
            if !rule.pattern.starts_with('^') {
                unanchored_start.push(rule.pattern);
            }
            if [r"\b", r"\s", r"\S"]
                .iter()
                .any(|escape| rule.pattern.contains(escape))
            {
                unicode_word_end.push(rule.pattern);
            }
            if PENDING.contains(&rule.pattern) {
                continue;
            }
            if !APPROVED_SUFFIXES.iter().any(|s| rule.pattern.ends_with(s)) {
                unterminated.push(rule.pattern);
            }
        }

        assert!(
            unanchored_start.is_empty(),
            "patterns match against a whole command line, so one that does not \
             start with `^` fires on a path component or a wrapper argument: \
             {unanchored_start:#?}"
        );
        assert!(
            unicode_word_end.is_empty(),
            "`\\b`, `\\s` and `\\S` end a word at `\\r`, a vertical tab, a form \
             feed or a non-breaking space, which bash reads as word bytes; spell \
             a separator `[ \\t\\n]` and end the word with \
             `(?:[ \\t\\n]|$|[;|&()<>])` instead: {unicode_word_end:#?}"
        );
        assert!(
            unterminated.is_empty(),
            "these patterns do not end at a word boundary, so any command whose \
             subcommand merely starts with a listed one is rewritten (#4009): \
             {unterminated:#?}"
        );

        let stale: Vec<&str> = PENDING
            .iter()
            .copied()
            .filter(|p| !RULES.iter().any(|r| r.pattern == *p))
            .collect();
        assert!(
            stale.is_empty(),
            "PENDING names patterns no longer in RULES — drop them rather than \
             leave a standing exemption: {stale:#?}"
        );
    }

    /// `classify_command` picks a rule by index, then `decide` looks one up
    /// again by `rtk_cmd` with `find` — which takes the first of a duplicate
    /// pair, not necessarily the one that classified.
    ///
    /// `rtk php` and `rtk uv` are each two rules, so that lookup already
    /// returns a sibling. It is harmless only because every field it reads
    /// matches across the pair; an edit to one sibling's `pipeline_safety` or
    /// `rewrite_prefixes` would silently apply to the other's commands.
    #[test]
    fn test_duplicate_rtk_cmds_agree_on_what_the_rewrite_reads() {
        for rule in RULES {
            let siblings: Vec<&RtkRule> =
                RULES.iter().filter(|r| r.rtk_cmd == rule.rtk_cmd).collect();
            let first = siblings[0];
            for sibling in &siblings[1..] {
                assert_eq!(
                    sibling.rewrite_prefixes, first.rewrite_prefixes,
                    "{} is several rules with different rewrite_prefixes, and the \
                     rewrite reads whichever comes first",
                    rule.rtk_cmd
                );
                assert_eq!(
                    sibling.pipeline_safety, first.pipeline_safety,
                    "{} is several rules with different pipeline_safety, and the \
                     rewrite reads whichever comes first",
                    rule.rtk_cmd
                );
            }
        }
    }

    #[test]
    fn test_all_patterns_are_valid_regex() {
        use regex::Regex;
        for (i, rule) in RULES.iter().enumerate() {
            assert!(
                Regex::new(rule.pattern).is_ok(),
                "RULES[{i}] ({}) has invalid pattern '{}'",
                rule.rtk_cmd,
                rule.pattern
            );
        }
    }

    // --- #196: gh --json/--jq/--template passthrough ---

    #[test]
    fn test_rewrite_gh_json_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("gh pr list --json number,title", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_gh_jq_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("gh pr list --json number --jq '.[].number'", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_gh_template_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("gh pr view 42 --template '{{.title}}'", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_gh_api_json_skipped() {
        assert_eq!(
            rewrite_command_no_prefixes("gh api repos/owner/repo --jq '.name'", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_gh_without_json_still_works() {
        assert_eq!(
            rewrite_command_no_prefixes("gh pr list", &[]),
            Some("rtk gh pr list".into())
        );
    }

    // --- #508: RTK_DISABLED detection helpers ---

    #[test]
    fn test_cmd_has_rtk_disabled_prefix() {
        assert!(cmd_has_rtk_disabled_prefix("RTK_DISABLED=1 git status"));
        assert!(cmd_has_rtk_disabled_prefix(
            "FOO=1 RTK_DISABLED=1 cargo test"
        ));
        assert!(cmd_has_rtk_disabled_prefix(
            "RTK_DISABLED=true git log --oneline"
        ));
        assert!(!cmd_has_rtk_disabled_prefix("git status"));
        assert!(!cmd_has_rtk_disabled_prefix("rtk git status"));
        assert!(!cmd_has_rtk_disabled_prefix("SOME_VAR=1 git status"));
    }

    #[test]
    fn test_split_env_prefix_keeps_the_bypass_visible() {
        assert_eq!(
            split_env_prefix("RTK_DISABLED=1 git status"),
            ("RTK_DISABLED=1 ", "git status")
        );
        assert_eq!(
            split_env_prefix("FOO=1 RTK_DISABLED=1 cargo test"),
            ("FOO=1 RTK_DISABLED=1 ", "cargo test")
        );
        assert_eq!(split_env_prefix("git status"), ("", "git status"));
    }

    /// A quoted value is one word however many blanks it holds. Reading it with
    /// a pattern let the value alternation backtrack into the quotes whenever
    /// the quoted form was not followed by a blank — at the end of a line, or
    /// before a `;` — so `D='# shellcheck disable=SC2034'` was read as the
    /// assignment `D='# ` and a command `shellcheck`, and the rewrite edited
    /// inside the literal (#3262).
    #[test]
    fn test_a_quoted_value_is_one_word() {
        for (cmd, prefix, rest) in [
            // The three shapes of the same backtrack: before a `;`, at the end
            // of the line, either quote.
            (
                "D='# shellcheck disable=SC2034'",
                "D='# shellcheck disable=SC2034'",
                "",
            ),
            ("D=\"x y\"", "D=\"x y\"", ""),
            ("D='no trailing space'", "D='no trailing space'", ""),
            // And the forms that always worked, which must keep working.
            ("FOO=bar git status", "FOO=bar ", "git status"),
            (
                "GIT_SSH_COMMAND='ssh -o X=no' git push",
                "GIT_SSH_COMMAND='ssh -o X=no' ",
                "git push",
            ),
            ("env FOO=1 cargo test", "env FOO=1 ", "cargo test"),
            ("FOO=a\\ b git status", "FOO=a\\ b ", "git status"),
            // A backslash escapes inside double quotes, not inside single ones.
            (
                "FOO=\"he said \\\"hi\\\"\" git status",
                "FOO=\"he said \\\"hi\\\"\" ",
                "git status",
            ),
            // An assignment is bash's: any case, `+=`, and a quoted value
            // that ends the line is still one word.
            ("foo=bar git status", "foo=bar ", "git status"),
            ("Foo_1=bar git status", "Foo_1=bar ", "git status"),
            ("_x=1 git status", "_x=1 ", "git status"),
            ("foo+=x git status", "foo+=x ", "git status"),
            ("a=1 B=2 git status", "a=1 B=2 ", "git status"),
            ("env foo=1 cargo test", "env foo=1 ", "cargo test"),
            (
                "d='# shellcheck disable=SC2034'",
                "d='# shellcheck disable=SC2034'",
                "",
            ),
            // And what bash runs as a command rather than assigning.
            ("1a=b git status", "", "1a=b git status"),
            ("=x git status", "", "=x git status"),
            ("foo-bar=x git status", "", "foo-bar=x git status"),
            ("\"foo\"=x git status", "", "\"foo\"=x git status"),
            // A prefix with nothing after it is all prefix. There is no command
            // to classify, which is the same answer as before by a shorter road.
            ("env", "env", ""),
            ("FOO=bar", "FOO=bar", ""),
            // A word of nothing but escapes ends the prefix like any other word
            // that is not an assignment. Lose it and the assignment behind it is
            // swallowed too, and `BAZ=1` stops being the env var it is.
            (
                "FOO=bar \\' BAZ=1 git status",
                "FOO=bar ",
                "\\' BAZ=1 git status",
            ),
            // `sudo` is never stripped (#146).
            ("sudo docker ps", "", "sudo docker ps"),
            // Both halves end where a word does: bare blanks around the line
            // go, an escaped one stays with its word.
            (" \tFOO=1  ls \t", "FOO=1  ", "ls"),
            ("FOO=1 cat f\\ ", "FOO=1 ", "cat f\\ "),
            ("FOO=a\\ ", "FOO=a\\ ", ""),
            (" \t", "", ""),
        ] {
            assert_eq!(split_env_prefix(cmd), (prefix, rest), "{cmd}");
        }
    }

    /// The value belongs to the assignment, so a command inside it is not a
    /// command — but the real one after it still is.
    #[test]
    fn test_a_command_after_a_quoted_assignment_is_still_rewritten() {
        assert_eq!(
            rewrite_command_no_prefixes("D='# shellcheck disable=SC2034'; git status", &[]),
            Some("D='# shellcheck disable=SC2034'; rtk git status".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("D='x shellcheck y'; echo hi", &[]),
            None,
            "nothing here is a command RTK handles"
        );
        // The same holds for a lowercase name, which bash assigns just the same.
        assert_eq!(
            rewrite_command_no_prefixes("d='# shellcheck disable=SC2034'; git status", &[]),
            Some("d='# shellcheck disable=SC2034'; rtk git status".into())
        );
    }

    /// An assignment prefix is bash's, whatever the case of its name: the
    /// command after it is the command, and the assignment is carried over
    /// byte for byte. A word bash runs rather than assigns is left alone.
    #[test]
    fn test_any_assignment_bash_accepts_is_looked_past() {
        for (cmd, expected) in [
            ("foo=bar git status", Some("foo=bar rtk git status")),
            ("foo+=x git status", Some("foo+=x rtk git status")),
            ("a=1 B=2 git status", Some("a=1 B=2 rtk git status")),
            ("foo='a b' git status", Some("foo='a b' rtk git status")),
            ("1a=b git status", None),
            ("foo-bar=x git status", None),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                expected,
                "{cmd}"
            );
        }
    }

    // --- #485: absolute path normalization ---

    #[test]
    fn test_classify_absolute_path_grep() {
        assert_eq!(
            classify_command("/usr/bin/grep -rni pattern"),
            Classification::Supported {
                rtk_equivalent: "rtk grep",
                category: "Files",
                estimated_savings_pct: 75.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_absolute_path_ls() {
        assert_eq!(
            classify_command("/bin/ls -la"),
            Classification::Supported {
                rtk_equivalent: "rtk ls",
                category: "Files",
                estimated_savings_pct: 65.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_absolute_path_git() {
        assert_eq!(
            classify_command("/usr/local/bin/git status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_absolute_path_no_args() {
        // /usr/bin/find alone → still classified
        assert_eq!(
            classify_command("/usr/bin/find ."),
            Classification::Supported {
                rtk_equivalent: "rtk find",
                category: "Files",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_strip_absolute_path_helper() {
        assert_eq!(strip_absolute_path("/usr/bin/grep -rn foo"), "grep -rn foo");
        assert_eq!(strip_absolute_path("/bin/ls -la"), "ls -la");
        assert_eq!(strip_absolute_path("grep -rn foo"), "grep -rn foo");
        assert_eq!(strip_absolute_path("/usr/local/bin/git"), "git");
    }

    // --- #163: git global options ---

    #[test]
    fn test_classify_git_with_dash_c_path() {
        assert_eq!(
            classify_command("git -C /tmp status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_git_no_pager_log() {
        assert_eq!(
            classify_command("git --no-pager log -5"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_git_git_dir() {
        assert_eq!(
            classify_command("git --git-dir /tmp/.git status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_rewrite_git_dash_c() {
        assert_eq!(
            rewrite_command_no_prefixes("git -C /tmp status", &[]),
            Some("rtk git -C /tmp status".to_string())
        );
    }

    #[test]
    fn test_rewrite_git_no_pager() {
        assert_eq!(
            rewrite_command_no_prefixes("git --no-pager log -5", &[]),
            Some("rtk git --no-pager log -5".to_string())
        );
    }

    #[test]
    fn test_strip_git_global_opts_helper() {
        assert_eq!(strip_git_global_opts("git -C /tmp status"), "git status");
        assert_eq!(strip_git_global_opts("git --no-pager log"), "git log");
        assert_eq!(strip_git_global_opts("git status"), "git status");
        assert_eq!(strip_git_global_opts("cargo test"), "cargo test");
    }

    #[test]
    fn test_strip_golangci_global_opts_helper() {
        assert_eq!(
            strip_golangci_global_opts("golangci-lint -v run ./..."),
            "golangci-lint run ./..."
        );
        assert_eq!(
            strip_golangci_global_opts("golangci-lint --color never run ./..."),
            "golangci-lint run ./..."
        );
        assert_eq!(
            strip_golangci_global_opts("golangci-lint --color=never run ./..."),
            "golangci-lint run ./..."
        );
        assert_eq!(
            strip_golangci_global_opts("golangci-lint --config=foo.yml run ./..."),
            "golangci-lint run ./..."
        );
        assert_eq!(
            strip_golangci_global_opts("golangci-lint version"),
            "golangci-lint version"
        );
        assert_eq!(strip_golangci_global_opts("cargo test"), "cargo test");
    }

    // --- #wc: wc filter was silently ignored by the hook ---

    #[test]
    fn test_classify_wc_supported() {
        // BUG: "wc " was in IGNORED_PREFIXES despite wc_cmd.rs having a full filter.
        // This test documents the bug: it must FAIL before the fix and PASS after.
        assert_eq!(
            classify_command("wc -l src/main.rs"),
            Classification::Supported {
                rtk_equivalent: "rtk wc",
                category: "Files",
                estimated_savings_pct: 60.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_wc_multi_file() {
        assert_eq!(
            classify_command("wc src/*.rs"),
            Classification::Supported {
                rtk_equivalent: "rtk wc",
                category: "Files",
                estimated_savings_pct: 60.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_rewrite_wc() {
        assert_eq!(
            rewrite_command_no_prefixes("wc -l src/main.rs", &[]),
            Some("rtk wc -l src/main.rs".into())
        );
    }

    #[test]
    fn test_rewrite_wc_multi_file() {
        assert_eq!(
            rewrite_command_no_prefixes("wc src/*.rs", &[]),
            Some("rtk wc src/*.rs".into())
        );
    }

    #[test]
    fn test_classify_command_substitution_passthrough() {
        assert_eq!(
            classify_command("git log $(git rev-parse HEAD~1)"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_rewrite_command_substitution_passthrough() {
        assert_eq!(
            rewrite_command_no_prefixes("git log $(git rev-parse HEAD~1)", &[]),
            Some("rtk git log $(git rev-parse HEAD~1)".into())
        );
    }

    #[test]
    fn test_split_command_substitution_no_split() {
        assert_eq!(
            split_command_chain("git log $(git rev-parse HEAD~1)"),
            vec!["git log $(git rev-parse HEAD~1)"]
        );
    }

    #[test]
    fn test_shell_prefix_noglob() {
        assert_eq!(
            rewrite_command_no_prefixes("noglob git status", &[]),
            Some("noglob rtk git status".into())
        );
    }

    #[test]
    fn test_shell_prefix_command() {
        assert_eq!(
            rewrite_command_no_prefixes("command git status", &[]),
            Some("command rtk git status".into())
        );
    }

    #[test]
    fn test_shell_prefix_builtin_exec_nocorrect() {
        assert_eq!(
            rewrite_command_no_prefixes("builtin git status", &[]),
            Some("builtin rtk git status".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("exec git status", &[]),
            Some("exec rtk git status".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("nocorrect git status", &[]),
            Some("nocorrect rtk git status".into())
        );
    }

    #[test]
    fn test_shell_prefix_unknown_inner() {
        assert_eq!(
            rewrite_command_no_prefixes("noglob unknown_cmd --flag", &[]),
            None
        );
    }

    // --- transparent_prefixes tests ---

    #[test]
    fn test_transparent_prefix_strips_and_reprepends() {
        let prefixes = vec!["shadowenv exec --".to_string()];
        assert_eq!(
            super::rewrite_command("shadowenv exec -- git status", &[], &prefixes),
            Some("shadowenv exec -- rtk git status".into())
        );
    }

    #[test]
    fn test_transparent_prefix_with_test_runner() {
        let prefixes = vec!["shadowenv exec --".to_string()];
        assert_eq!(
            super::rewrite_command("shadowenv exec -- cargo test", &[], &prefixes),
            Some("shadowenv exec -- rtk cargo test".into())
        );
    }

    #[test]
    fn test_transparent_prefix_unknown_inner_returns_none() {
        let prefixes = vec!["shadowenv exec --".to_string()];
        assert_eq!(
            super::rewrite_command("shadowenv exec -- htop", &[], &prefixes),
            None
        );
    }

    #[test]
    fn test_transparent_prefix_not_matched_is_passthrough() {
        // Without the prefix configured, the wrapper breaks routing.
        assert_eq!(
            super::rewrite_command("shadowenv exec -- git status", &[], &[]),
            None
        );
    }

    #[test]
    fn test_transparent_prefix_composed_with_builtin() {
        // `noglob shadowenv exec -- git status` — builtin layer strips noglob,
        // user layer strips shadowenv exec --, inner `git status` routes.
        let prefixes = vec!["shadowenv exec --".to_string()];
        assert_eq!(
            super::rewrite_command("noglob shadowenv exec -- git status", &[], &prefixes),
            Some("noglob shadowenv exec -- rtk git status".into())
        );
    }

    #[test]
    fn test_transparent_prefix_composed_with_env_prefix() {
        let prefixes = vec!["bundle exec".to_string()];
        assert_eq!(
            super::rewrite_command("RAILS_ENV=test bundle exec git status", &[], &prefixes),
            Some("RAILS_ENV=test bundle exec rtk git status".into())
        );
    }

    /// A prefix that does not lex into whole words (a quote left open, a
    /// trailing backslash) is refused where the configuration is read, so it
    /// matches nothing and the line is left as written.
    #[test]
    fn test_a_prefix_that_does_not_lex_into_whole_words_is_refused() {
        for (prefix, cmd) in [
            ("x 'a", "x 'a ls -la & "),
            ("sh -c \"", "sh -c \" git status\t"),
            ("sh -c \"", "sh -c \" timeout 5 git status"),
            ("x\\", "x\\ git status"),
        ] {
            assert_eq!(
                super::rewrite_command(cmd, &[], &[prefix.to_string()]),
                None,
                "{cmd:?}"
            );
        }
        // The other entries are kept.
        assert_eq!(
            super::rewrite_command(
                "docker exec c git status",
                &[],
                &["x 'a".to_string(), "docker exec c".to_string()]
            ),
            Some("docker exec c rtk git status".to_string())
        );
    }

    #[test]
    fn test_env_prefix_composed_with_builtin() {
        assert_eq!(
            rewrite_command_no_prefixes("FOO=bar noglob git status", &[]),
            Some("FOO=bar noglob rtk git status".into())
        );
    }

    /// #2768's fall-through retries a user's own `transparent_prefixes` against
    /// the built-in wrapper's text, not just the plain decision: a user prefix
    /// that starts with a routable wrapper's words (`"uv run --frozen"` starts
    /// with `"uv run"`) only matches on that second reading, because the first
    /// peels `"uv run"` alone and leaves `"--frozen …"`, which nothing matches.
    #[test]
    fn test_routable_fallback_retries_a_user_prefix_not_just_the_decision() {
        let prefixes = vec!["uv run --frozen".to_string()];
        assert_eq!(
            super::rewrite_command("uv run --frozen git status", &[], &prefixes),
            Some("uv run --frozen rtk git status".into())
        );
        assert_eq!(
            super::rewrite_command("uv run --frozen pytest", &[], &prefixes),
            Some("uv run --frozen rtk pytest".into())
        );
        // No filter for the inner command either way: no rewrite, and not
        // `rtk uv run --frozen python x.py` either.
        assert_eq!(
            super::rewrite_command("uv run --frozen python x.py", &[], &prefixes),
            None
        );
    }

    /// A retry's own walk can turn up another routable layer: here the user
    /// prefix `"uv run --frozen"` peels alone and leaves a built-in `"uv run"`
    /// whose inner command nothing rewrites either. That layer gets the same
    /// retry, so the search goes on into a retry's own layers.
    #[test]
    fn test_fallback_search_continues_into_a_fallback_walks_own_layers() {
        let prefixes = vec!["uv run --frozen".to_string()];
        for (input, expected) in [
            (
                "uv run --frozen uv run foo",
                "uv run --frozen rtk uv run foo",
            ),
            (
                "uv run --frozen uv run xyz --flag",
                "uv run --frozen rtk uv run xyz --flag",
            ),
            (
                "uv run --frozen uv run python x.py",
                "uv run --frozen rtk uv run python x.py",
            ),
            (
                "uv run --frozen noglob uv run foo",
                "uv run --frozen noglob rtk uv run foo",
            ),
            (
                "uv run --frozen timeout 5 uv run foo",
                "uv run --frozen timeout 5 rtk uv run foo",
            ),
        ] {
            assert_eq!(
                super::rewrite_command(input, &[], &prefixes),
                Some(expected.into()),
                "{input:?}"
            );
        }
        // Without the user prefix, only the built-in `uv run` falls through,
        // at the front of the line.
        assert_eq!(
            super::rewrite_command("uv run uv run foo", &[], &[]),
            Some("uv run rtk uv run foo".into())
        );
    }

    /// A retry starts at the depth its layer was peeled at, so a nested
    /// fall-through shares the segment's [`MAX_PREFIX_DEPTH`] budget rather
    /// than getting a fresh one.
    #[test]
    fn test_nested_fallback_keeps_the_originating_layers_own_depth_budget() {
        let prefixes = vec![
            "docker exec c".to_string(),
            "uv run --frozen".to_string(),
            "sudo -u bob".to_string(),
            "timeout 5".to_string(),
            "poetry run".to_string(),
        ];
        let excluded = vec!["pytest".to_string()];
        assert_eq!(
            super::rewrite_command(
                "noglob noglob noglob uv run --frozen noglob uv run --frozen noglob noglob noglob noglob git status",
                &excluded,
                &prefixes
            ),
            None
        );
    }

    /// A chain of routable wrappers that a user prefix also matches at every
    /// position gives every layer two readings. Skipping a (position, depth)
    /// pair already tried keeps the search linear in the chain's depth: the
    /// calls are counted, since a timeout could only gesture at that.
    #[test]
    fn test_a_deep_all_routable_chain_stays_bounded() {
        let prefixes = vec!["uv run".to_string()];
        let chain = "uv run ".repeat(20) + "xyz";
        let line = CompoundLex::new(&chain);
        let segment = line.whole().expect("a command");
        let outcome = rewrite_segment(segment, &[], &prefixes, RewriteContext::Normal);
        let walks = outcome.walks;
        // The chain is deeper than `MAX_PREFIX_DEPTH`, so no walk reaches `xyz`
        // and nothing is decided. An exponential search over it would run into
        // the thousands of walks.
        assert!(walks < 50, "expected O(depth) walks, got {walks}");
        assert_eq!(outcome.edit, None);
        assert_eq!(super::rewrite_command(&chain, &[], &prefixes), None);
    }

    /// A trailing redirect can cut a command down to one env word (`env>f`
    /// leaves `env`, `FOO=1>f` leaves `FOO=1`). That is nothing to rewrite,
    /// whatever `exclude_commands` holds — including a pattern that would
    /// match the empty command left after the prefix.
    #[test]
    fn test_a_command_cut_down_to_an_env_word_is_left_alone() {
        for excluded in [vec![], vec!["^$".to_string()], vec!["env".to_string()]] {
            for cmd in ["env>f", "FOO=1>f", "env\x0b>f"] {
                assert_eq!(
                    super::rewrite_command(cmd, &excluded, &[]),
                    None,
                    "{cmd:?} with {excluded:?}"
                );
            }
        }
        // Only the cut segment is left alone; its neighbour is rewritten.
        assert_eq!(
            super::rewrite_command("git status; env>f", &[], &[]),
            Some("rtk git status; env>f".to_string())
        );
        assert_eq!(
            super::rewrite_command("FOO=1 git status>f", &[], &[]),
            Some("FOO=1 rtk git status>f".to_string())
        );
    }

    /// Several layers peeled, and only the decided command's own span edited:
    /// the quoted assignment, the wrapper and the redirect stay as written.
    #[test]
    fn test_emitter_keeps_quotes_and_redirect_untouched_around_a_peeled_wrapper() {
        assert_eq!(
            rewrite_command_no_prefixes(r#"GIT_SSH_COMMAND="ssh -v" timeout 5 git push 2>&1"#, &[]),
            Some(r#"GIT_SSH_COMMAND="ssh -v" timeout 5 rtk git push 2>&1"#.into())
        );
    }

    /// The layers of a walk over the whole of `text`, as [`rewrite_segment`]
    /// starts one, and the command under them.
    fn walk_text(text: &str, prefixes: &[String]) -> (Vec<(LayerKind, String)>, Option<String>) {
        let tokens = tokenize(text);
        let words = words(text, &tokens);
        let line = Line {
            text,
            words: &words,
            tokens: &tokens,
        };
        let walk = Walk::run(&line, prefixes, 0, Position::START, 0);
        let span = |from: usize, to: usize| text[words[from].start..words[to - 1].end].to_string();
        let layers = walk
            .layers
            .iter()
            .map(|layer| (layer.kind, span(layer.word, layer.end)))
            .collect();
        let command = walk
            .command
            .filter(|&at| at < words.len())
            .map(|at| span(at, words.len()));
        (layers, command)
    }

    /// Every layer is a range of the words of the segment's one lex, each
    /// read at the position the one before it left.
    #[test]
    fn test_the_walk_peels_layers_as_ranges_of_the_segments_words() {
        let (layers, command) = walk_text("FOO=1 noglob timeout 5 uv run git status", &[]);
        assert_eq!(
            layers,
            vec![
                (LayerKind::Assignments, "FOO=1".to_string()),
                (LayerKind::Precommand, "noglob".to_string()),
                (LayerKind::ProcessWrapper, "timeout 5".to_string()),
                (LayerKind::RoutableWrapper, "uv run".to_string()),
            ]
        );
        assert_eq!(command.as_deref(), Some("git status"));

        let (layers, command) = walk_text("time -p then A=1 env B=2 nice -5 git status", &[]);
        assert_eq!(
            layers,
            vec![
                (LayerKind::ReservedWord, "time -p".to_string()),
                (LayerKind::ReservedWord, "then".to_string()),
                (LayerKind::Assignments, "A=1 env B=2".to_string()),
                (LayerKind::ProcessWrapper, "nice -5".to_string()),
            ]
        );
        assert_eq!(command.as_deref(), Some("git status"));

        let prefixes = vec!["docker exec c".to_string()];
        let (layers, command) = walk_text("docker exec c A=1 git status", &prefixes);
        assert_eq!(
            layers,
            vec![
                (LayerKind::UserPrefix, "docker exec c".to_string()),
                (LayerKind::Assignments, "A=1".to_string()),
            ]
        );
        assert_eq!(command.as_deref(), Some("git status"));
    }

    /// A command ends where its last word does: a space or tab the last token
    /// holds, escaped or inside an unclosed quote, belongs to the command, and
    /// only the bare blanks after it are outside. Every reading of the command
    /// sees what a lex of that text alone gives.
    #[test]
    fn test_a_slice_ends_where_its_last_word_does() {
        for (text, expected) in [
            ("git status\\ ", "git status\\ "),
            ("git status 'abc  ", "git status 'abc  "),
            ("a\\\t  b\\  \t", "a\\\t  b\\ "),
            ("  git status \t\n", "git status"),
        ] {
            let tokens = tokenize(text);
            let slice = Slice::new(text, &tokens).expect("a command");
            let (trimmed, fresh) = tokenize_trimmed(text);
            assert_eq!(slice.as_str(), expected, "{text:?}");
            assert_eq!(trimmed, expected, "{text:?}");
            assert_eq!(slice.argv(), shell_split(trimmed), "{text:?}");
            assert_eq!(
                slice.toks().map(|t| t.value).collect::<Vec<_>>(),
                fresh
                    .iter()
                    .filter(|t| !t.is_blank())
                    .map(|t| t.value)
                    .collect::<Vec<_>>(),
                "{text:?}"
            );
        }
        assert!(Slice::new(" \t\n ", &tokenize(" \t\n ")).is_none());
    }

    #[test]
    fn test_trailing_redirects_start_at_the_first_of_the_trailing_run() {
        for (text, expected) in [
            ("git status 2>&1", Some("2>&1")),
            ("git status >out 2>&1", Some(">out 2>&1")),
            ("git status > out", Some("> out")),
            ("git status", None),
            ("git >out status", None),
        ] {
            let tokens = tokenize(text);
            let slice = Slice::new(text, &tokens).expect("a command");
            assert_eq!(
                slice.trailing_redirect_start().map(|at| &text[at..]),
                expected,
                "{text:?}"
            );
        }
    }

    #[test]
    fn test_sudo_with_builtin_not_rewritten() {
        // A leading sudo blocks the rewrite even when a transparent builtin follows.
        assert_eq!(
            rewrite_command_no_prefixes("sudo noglob git status", &[]),
            None
        );
    }

    #[test]
    fn test_process_wrapper_rewrites_inner_command() {
        for (input, expected) in [
            ("timeout 300 cargo test", "timeout 300 rtk cargo test"),
            ("time cargo build", "time rtk cargo build"),
            ("nice -n 10 cargo test", "nice -n 10 rtk cargo test"),
            ("nohup cargo build", "nohup rtk cargo build"),
            (
                "/usr/bin/timeout 300 cargo test",
                "/usr/bin/timeout 300 rtk cargo test",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(input, &[]),
                Some(expected.into()),
                "{}",
                input
            );
        }
    }

    #[test]
    fn test_process_wrapper_option_forms() {
        for (input, expected) in [
            (
                "timeout -k 5s 300 cargo test",
                "timeout -k 5s 300 rtk cargo test",
            ),
            (
                "timeout -k5s 300 cargo test",
                "timeout -k5s 300 rtk cargo test",
            ),
            (
                "timeout --kill-after=5s 300 cargo test",
                "timeout --kill-after=5s 300 rtk cargo test",
            ),
            (
                "timeout --preserve-status 300 cargo test",
                "timeout --preserve-status 300 rtk cargo test",
            ),
            ("timeout -- 300 cargo test", "timeout -- 300 rtk cargo test"),
            ("time -p cargo build", "time -p rtk cargo build"),
            (
                "/usr/bin/time -f %e cargo build",
                "/usr/bin/time -f %e rtk cargo build",
            ),
            ("nice -n10 cargo test", "nice -n10 rtk cargo test"),
            ("nice -10 cargo test", "nice -10 rtk cargo test"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(input, &[]),
                Some(expected.into()),
                "{}",
                input
            );
        }
    }

    #[test]
    fn test_process_wrapper_unknown_option_is_passthrough() {
        assert_eq!(
            rewrite_command_no_prefixes("timeout --unknown 300 cargo test", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("nice --unknown cargo test", &[]),
            None
        );
    }

    #[test]
    fn test_process_wrapper_with_unsupported_inner_command_is_passthrough() {
        assert_eq!(
            rewrite_command_no_prefixes("timeout 300 mycustombinary --flag", &[]),
            None
        );
    }

    #[test]
    fn test_process_wrapper_never_doubles_rtk() {
        assert_eq!(
            rewrite_command_no_prefixes("timeout rtk cargo test", &[]),
            None
        );
    }

    #[test]
    fn test_process_wrapper_without_inner_command_is_passthrough() {
        assert_eq!(rewrite_command_no_prefixes("timeout 300", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("time", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("timeout -k 5s", &[]), None);
    }

    #[test]
    fn test_process_wrapper_refuses_shell_syntax() {
        for input in [
            "timeout 300 >out.log cargo test",
            "timeout 300 $(which cargo) test",
            "timeout 300 */bin/cargo test",
        ] {
            assert_eq!(rewrite_command_no_prefixes(input, &[]), None, "{}", input);
        }
    }

    /// A wrapper cannot peel a `(`, but it does not have to: the subshell is a
    /// boundary, so the commands inside it are rewritten where they stand and
    /// the wrapper keeps wrapping the subshell.
    #[test]
    fn test_a_wrapper_around_a_subshell_rewrites_inside_it() {
        assert_eq!(
            rewrite_command_no_prefixes("time (cargo build)", &[]),
            Some("time (rtk cargo build)".into())
        );
    }

    #[test]
    fn test_stdbuf_is_not_a_process_wrapper() {
        assert_eq!(
            rewrite_command_no_prefixes("stdbuf -oL cargo test", &[]),
            None
        );
    }

    #[test]
    fn test_process_wrapper_composes_with_prefixes_and_compounds() {
        assert_eq!(
            rewrite_command_no_prefixes("CI=1 timeout 300 cargo test", &[]),
            Some("CI=1 timeout 300 rtk cargo test".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("nice -n 10 timeout 300 cargo test", &[]),
            Some("nice -n 10 timeout 300 rtk cargo test".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("timeout 300 cargo test && time git status", &[]),
            Some("timeout 300 rtk cargo test && time rtk git status".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("command timeout 300 git status", &[]),
            Some("command timeout 300 rtk git status".into())
        );
    }

    #[test]
    fn test_process_wrapper_rewrite_is_idempotent() {
        assert_eq!(
            rewrite_command_no_prefixes("timeout 300 rtk cargo test", &[]),
            None
        );
    }

    #[test]
    fn test_process_wrapper_keeps_pipeline_context() {
        assert_eq!(
            rewrite_command_no_prefixes("timeout 300 git log | head -5", &[]),
            Some("timeout 300 rtk git log | head -5".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("cargo test | timeout 5 grep error", &[]),
            Some("cargo test | timeout 5 rtk grep error".into())
        );
    }

    #[test]
    fn test_process_wrapper_respects_exclusions() {
        assert_eq!(
            rewrite_command_no_prefixes("timeout 300 cargo test", &["cargo test".to_string()]),
            None
        );
    }

    #[test]
    fn test_transparent_prefix_multiple_configured() {
        let prefixes = vec!["shadowenv exec --".to_string(), "direnv exec .".to_string()];
        assert_eq!(
            super::rewrite_command("direnv exec . git status", &[], &prefixes),
            Some("direnv exec . rtk git status".into())
        );
    }

    #[test]
    fn test_transparent_prefixes_normalize_once() {
        let prefixes = vec![
            "  docker exec mycontainer  ".to_string(),
            "".to_string(),
            "docker".to_string(),
            "docker exec mycontainer".to_string(),
        ];
        assert_eq!(
            normalize_transparent_prefixes(&prefixes),
            vec!["docker exec mycontainer".to_string(), "docker".to_string()]
        );
    }

    #[test]
    fn test_transparent_prefix_overlapping_entries_use_longest_match() {
        let prefixes = vec!["docker".to_string(), "docker exec app".to_string()];
        assert_eq!(
            super::rewrite_command("docker exec app git status", &[], &prefixes),
            Some("docker exec app rtk git status".into())
        );
    }

    #[test]
    fn test_transparent_prefix_whole_word_matching() {
        // A prefix `"foo"` must NOT match `"foobar git status"`.
        let prefixes = vec!["foo".to_string()];
        assert_eq!(
            super::rewrite_command("foobar git status", &[], &prefixes),
            None
        );
    }

    #[test]
    fn test_transparent_prefix_empty_rest_returns_none() {
        let prefixes = vec!["shadowenv exec --".to_string()];
        assert_eq!(
            super::rewrite_command("shadowenv exec --", &[], &prefixes),
            None
        );
    }

    #[test]
    fn test_transparent_prefix_empty_entry_is_skipped() {
        // A blank entry in the config should not cause spurious matches or panics.
        let prefixes = vec!["".to_string(), "   ".to_string()];
        assert_eq!(
            super::rewrite_command("git status", &[], &prefixes),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_transparent_prefix_inside_compound() {
        // Each segment of `&&` / `;` should independently get prefix-stripped.
        let prefixes = vec!["shadowenv exec --".to_string()];
        assert_eq!(
            super::rewrite_command(
                "shadowenv exec -- git status && shadowenv exec -- cargo test",
                &[],
                &prefixes
            ),
            Some("shadowenv exec -- rtk git status && shadowenv exec -- rtk cargo test".into())
        );
    }

    #[test]
    fn test_transparent_prefix_respects_excluded() {
        // An excluded inner command should still produce no rewrite even behind
        // a transparent prefix.
        let prefixes = vec!["shadowenv exec --".to_string()];
        let excluded = vec!["git".to_string()];
        assert_eq!(
            super::rewrite_command("shadowenv exec -- git status", &excluded, &prefixes),
            None
        );
    }

    #[test]
    fn test_transparent_prefix_recursion_bounded() {
        // A prefix that could recurse forever (e.g. one that maps to itself)
        // must terminate once MAX_PREFIX_DEPTH is reached.
        let prefixes = vec!["wrap".to_string()];
        let mut cmd = String::new();
        for _ in 0..(MAX_PREFIX_DEPTH + 2) {
            cmd.push_str("wrap ");
        }
        cmd.push_str("git status");
        // Doesn't matter exactly what it returns — just that it doesn't stack-
        // overflow or loop forever. Exercise the code path.
        let _ = super::rewrite_command(&cmd, &[], &prefixes);
    }

    #[test]
    fn test_python3_m_pytest() {
        assert_eq!(
            rewrite_command_no_prefixes("python3 -m pytest tests/", &[]),
            Some("rtk pytest tests/".into())
        );
    }

    #[test]
    fn test_pip_show() {
        assert_eq!(
            rewrite_command_no_prefixes("pip show flask", &[]),
            Some("rtk pip show flask".into())
        );
    }

    #[test]
    fn test_gt_graphite() {
        assert_eq!(
            rewrite_command_no_prefixes("gt log", &[]),
            Some("rtk gt log".into())
        );
    }

    #[test]
    fn test_command_no_longer_ignored() {
        assert_ne!(
            classify_command("command git status"),
            Classification::Ignored
        );
    }

    // --- Pipe + operator rewrite ---

    #[test]
    fn test_rewrite_pipe_then_and() {
        assert_eq!(
            rewrite_command_no_prefixes("git log | head -5 && git stash", &[]),
            Some("rtk git log | head -5 && rtk git stash".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_then_semicolon() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo test | head; git status", &[]),
            Some("rtk cargo test | head; rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_then_or() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo test | grep FAIL || git stash", &[]),
            Some("cargo test | rtk grep FAIL || rtk git stash".into())
        );
    }

    #[test]
    fn test_rewrite_env_pipe_then_and() {
        assert_eq!(
            rewrite_command_no_prefixes(
                "RUST_BACKTRACE=1 cargo test 2>&1 | grep FAILED && git stash",
                &[]
            ),
            Some("RUST_BACKTRACE=1 cargo test 2>&1 | rtk grep FAILED && rtk git stash".into())
        );
    }

    #[test]
    fn test_rewrite_and_then_pipe() {
        assert_eq!(
            rewrite_command_no_prefixes("git status && cargo test | grep FAIL", &[]),
            Some("rtk git status && cargo test | rtk grep FAIL".into())
        );
    }

    #[test]
    fn test_rewrite_multi_pipe_then_and() {
        assert_eq!(
            rewrite_command_no_prefixes("git log | head | tail && git status", &[]),
            Some("rtk git log | head | tail && rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_pipeline_final_normalizes_prefixes() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo test | FOO=1 command grep FAILED", &[]),
            Some("cargo test | FOO=1 command rtk grep FAILED".into())
        );
        assert_eq!(
            super::rewrite_command(
                "cargo test | docker exec tools grep FAILED",
                &[],
                &["docker exec tools".into()]
            ),
            Some("cargo test | docker exec tools rtk grep FAILED".into())
        );
    }

    #[test]
    fn test_rewrite_opaque_grouped_pipeline_stays_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("echo x | { cat; git log; } | grep feat", &[]),
            None
        );
    }

    /// Only command text pipes or groups commands: a `case` pattern's `|` and
    /// brackets, and a `[[ ]]` regex's, leave the line to the rewrite, while
    /// a subshell next to a pipe still keeps it raw.
    #[test]
    fn test_rewrite_pattern_and_regex_brackets_group_nothing() {
        for (cmd, expected) in [
            (
                "[[ $y =~ ^(a|b)$ ]] && git status",
                Some("[[ $y =~ ^(a|b)$ ]] && rtk git status"),
            ),
            (
                "[[ $y =~ (a|b) ]] && git status | head",
                Some("[[ $y =~ (a|b) ]] && rtk git status | head"),
            ),
            (
                "case $y in a|b) git status;; esac",
                Some("case $y in a|b) rtk git status;; esac"),
            ),
            (
                "case $y in (a|b) git status | head;; esac",
                Some("case $y in (a|b) rtk git status | head;; esac"),
            ),
            (
                "case $y in @(a|b)) git status | head;; esac",
                Some("case $y in @(a|b)) rtk git status | head;; esac"),
            ),
            ("(ls) | head", None),
            ("ls | (head)", None),
            ("case $y in a) (ls) | head;; esac", None),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                expected,
                "{cmd:?}"
            );
        }
    }

    /// Word text is one word's, never a command: an extglob group, an array
    /// literal and a `${ }` keep their text as written, and nothing inside
    /// one ends the command it belongs to.
    #[test]
    fn test_rewrite_leaves_word_text_as_written() {
        for (cmd, expected) in [
            ("x=(git status)", None),
            ("declare -a a=(git status)", None),
            ("a+=(git status) && ls", Some("a+=(git status) && rtk ls")),
            ("arr=(a b) git status", Some("arr=(a b) rtk git status")),
            ("arr=(a b); git status", Some("arr=(a b); rtk git status")),
            ("ls !(ls)", Some("rtk ls !(ls)")),
            ("git log !(a) && ls", Some("rtk git log !(a) && rtk ls")),
            ("ls ?((ls))", Some("rtk ls ?((ls))")),
            (
                "ls x@(a;b)&&git status",
                Some("rtk ls x@(a;b)&&rtk git status"),
            ),
            // Word text's `|` and brackets count as a pipe and a group.
            ("ls @(a|b) && git status", None),
            (
                "ls ${x-;ls }; git status",
                Some("rtk ls ${x-;ls }; rtk git status"),
            ),
            (
                "ls ${y+git status} && git status",
                Some("rtk ls ${y+git status} && rtk git status"),
            ),
            ("git log ${x:-a b} | head", None),
            (
                "x=(${y-a b} c) && git status",
                Some("x=(${y-a b} c) && rtk git status"),
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                expected,
                "{cmd:?}"
            );
        }
    }

    /// `time` after `|` or `|&` is the program `time`, so a `[[` after it is
    /// an argument and `||` ends that command.
    #[test]
    fn test_rewrite_reads_time_after_a_pipe_as_a_program() {
        for (cmd, expected) in [
            (
                "ls | time [[ -f a || ls ]]",
                Some("ls | time [[ -f a || rtk ls ]]"),
            ),
            (
                "ls |& time [[ -f a || ls ]]",
                Some("ls |& time [[ -f a || rtk ls ]]"),
            ),
            (
                "ls | time -p [[ -f a || ls ]]",
                Some("ls | time -p [[ -f a || rtk ls ]]"),
            ),
            (
                "ls && time [[ -f a || ls ]] && ls",
                Some("rtk ls && time [[ -f a || ls ]] && rtk ls"),
            ),
            // The stage runs the program `time`, which no rule takes.
            ("ls | time git status", None),
            ("ls | time -p git status", None),
            ("git log | time head", None),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                expected,
                "{cmd:?}"
            );
        }
    }

    /// In a `case` pattern or a `[[ ]]` expression, word text is text of one
    /// of its words: a `)`, a `|` or a `]]` in a `${ }` ends nothing.
    #[test]
    fn test_rewrite_reads_word_text_in_patterns_and_expressions() {
        for (cmd, expected) in [
            (
                "case $x in ${y:-a)b}) ls;; esac",
                Some("case $x in ${y:-a)b}) rtk ls;; esac"),
            ),
            (
                "case $x in ${y:-a|b}) ls;; esac",
                Some("case $x in ${y:-a|b}) rtk ls;; esac"),
            ),
            (
                "case $x in ${y-a) ls;; b}) git status;; esac",
                Some("case $x in ${y-a) ls;; b}) rtk git status;; esac"),
            ),
            ("[[ ${x- ]] } == a || ls ]]", None),
            (
                "[[ ${x-)} == a || ls ]] && ls",
                Some("[[ ${x-)} == a || ls ]] && rtk ls"),
            ),
            (
                "[[ ${x-a ]] || ls } ]] && git status",
                Some("[[ ${x-a ]] || ls } ]] && rtk git status"),
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                expected,
                "{cmd:?}"
            );
        }
    }

    #[test]
    fn test_rewrite_stderr_pipe_stays_raw() {
        assert_eq!(
            rewrite_command_no_prefixes("cargo test |& grep FAILED", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("cargo test | grep FAILED |& wc -l", &[]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("cargo test |& grep FAILED && git status", &[]),
            Some("cargo test |& grep FAILED && rtk git status".into())
        );
    }

    // --- line-continuation handling (issue #1564) ---

    #[test]
    fn test_rewrite_leading_backslash_newline() {
        // The exact reproduction from #1564: a leading `\<NL>` made
        // the matcher see `\` as the command and bail out.
        assert_eq!(
            rewrite_command_no_prefixes("\\\ngit diff HEAD~1", &[]),
            Some("rtk git diff HEAD~1".into())
        );
    }

    #[test]
    fn test_rewrite_leading_backslash_crlf() {
        // CRLF line ending — same shape, Windows shells / Git Bash.
        assert_eq!(
            rewrite_command_no_prefixes("\\\r\ngit diff HEAD~1", &[]),
            Some("rtk git diff HEAD~1".into())
        );
    }

    #[test]
    fn test_rewrite_internal_backslash_newline() {
        // Embedded line continuation between subcommand and args:
        // `git diff \<NL>HEAD~1` is exactly equivalent to
        // `git diff HEAD~1` per bash semantics.
        assert_eq!(
            rewrite_command_no_prefixes("git diff \\\nHEAD~1", &[]),
            Some("rtk git diff HEAD~1".into())
        );
    }

    #[test]
    fn test_rewrite_backslash_newline_with_indent() {
        // Continuation followed by indentation — also collapsed.
        assert_eq!(
            rewrite_command_no_prefixes("git \\\n    diff HEAD~1", &[]),
            Some("rtk git diff HEAD~1".into())
        );
    }

    #[test]
    fn test_rewrite_no_line_continuation_unchanged() {
        // Sanity check: a command without any `\<NL>` should match
        // unchanged. This pins that the normalization step does not
        // regress the no-op fast path.
        assert_eq!(
            rewrite_command_no_prefixes("git diff HEAD~1", &[]),
            Some("rtk git diff HEAD~1".into())
        );
    }

    #[test]
    fn test_collapse_line_continuations_no_op() {
        // With nothing to join, the text comes back borrowed.
        assert!(matches!(
            collapse_line_continuations("git diff HEAD~1"),
            Cow::Borrowed("git diff HEAD~1")
        ));
    }

    // --- PHP tooling ---

    #[test]
    fn test_classify_phpunit() {
        assert!(matches!(
            classify_command("phpunit tests/"),
            Classification::Supported {
                rtk_equivalent: "rtk phpunit",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_vendor_bin_phpunit() {
        assert!(matches!(
            classify_command("vendor/bin/phpunit --filter EmailTest"),
            Classification::Supported {
                rtk_equivalent: "rtk phpunit",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_php_vendor_bin_phpunit() {
        assert!(matches!(
            classify_command("php vendor/bin/phpunit tests/"),
            Classification::Supported {
                rtk_equivalent: "rtk phpunit",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_phpunit() {
        assert_eq!(
            rewrite_command_no_prefixes("phpunit tests/", &[]),
            Some("rtk phpunit tests/".into())
        );
    }

    #[test]
    fn test_rewrite_vendor_bin_phpunit() {
        assert_eq!(
            rewrite_command_no_prefixes("vendor/bin/phpunit --filter EmailTest", &[]),
            Some("rtk phpunit --filter EmailTest".into())
        );
    }

    #[test]
    fn test_rewrite_dotslash_vendor_bin() {
        // `./vendor/bin/<tool>` is the common Laravel invocation form. classify
        // normalizes the leading `./`, but the rewrite strips literal prefixes,
        // so the `./vendor/bin/<tool>` prefix must be present or rewrite no-ops.
        assert_eq!(
            rewrite_command_no_prefixes("./vendor/bin/pint --test", &[]),
            Some("rtk pint --test".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("./vendor/bin/pest tests/", &[]),
            Some("rtk pest tests/".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("./vendor/bin/paratest", &[]),
            Some("rtk paratest".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("./vendor/bin/ecs check", &[]),
            Some("rtk ecs check".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("./vendor/bin/phpunit --filter EmailTest", &[]),
            Some("rtk phpunit --filter EmailTest".into())
        );
    }

    #[test]
    fn test_rewrite_php_tool_invocation_forms() {
        // phpunit carries the full matrix: php wrapper, ./, plain bin/, vendor/bin.
        // `decide` normalizes each to the same canonical rewrite.
        for cmd in [
            "phpunit tests/",
            "vendor/bin/phpunit tests/",
            "./vendor/bin/phpunit tests/",
            "bin/phpunit tests/",
            "./bin/phpunit tests/",
            "php vendor/bin/phpunit tests/",
            "php phpunit tests/",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some("rtk phpunit tests/".into()),
                "form: {cmd}"
            );
        }

        // pest/pint/ecs/paratest use the simpler variant: ./ and vendor/bin only.
        for cmd in ["pint", "vendor/bin/pint", "./vendor/bin/pint", "./pint"] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some("rtk pint".into()),
                "form: {cmd}"
            );
        }
        // Forms the simpler variant intentionally does not accept (no php
        // wrapper, no plain bin/) — must not rewrite rather than misfire.
        assert_eq!(
            rewrite_command_no_prefixes("php vendor/bin/pint", &[]),
            None
        );
        assert_eq!(rewrite_command_no_prefixes("bin/pint", &[]), None);
    }

    #[test]
    fn test_classify_phpstan() {
        assert!(matches!(
            classify_command("vendor/bin/phpstan analyse src/"),
            Classification::Supported {
                rtk_equivalent: "rtk phpstan",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_phpstan_direct() {
        assert!(matches!(
            classify_command("phpstan analyse --level=9"),
            Classification::Supported {
                rtk_equivalent: "rtk phpstan",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_phpstan_vendor_bin() {
        assert_eq!(
            rewrite_command_no_prefixes("vendor/bin/phpstan analyse src/", &[]),
            Some("rtk phpstan analyse src/".into())
        );
    }

    #[test]
    fn test_rewrite_phpstan_php_prefix() {
        assert_eq!(
            rewrite_command_no_prefixes("php vendor/bin/phpstan analyse", &[]),
            Some("rtk phpstan analyse".into())
        );
    }

    #[test]
    fn test_rewrite_phpstan_version_not_rewritten() {
        assert_eq!(rewrite_command_no_prefixes("phpstan --version", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("phpstan list", &[]), None);
        assert_eq!(
            rewrite_command_no_prefixes("phpstan clear-result-cache", &[]),
            None
        );
    }

    #[test]
    fn test_classify_pest() {
        assert!(matches!(
            classify_command("vendor/bin/pest tests/"),
            Classification::Supported {
                rtk_equivalent: "rtk pest",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_pint() {
        assert!(matches!(
            classify_command("vendor/bin/pint --test"),
            Classification::Supported {
                rtk_equivalent: "rtk pint",
                ..
            }
        ));
    }

    #[test]
    fn test_php_artisan_rewrites() {
        assert!(matches!(
            classify_command("php artisan migrate"),
            Classification::Supported {
                rtk_equivalent: "rtk php",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_phpt_run_tests() {
        assert!(matches!(
            classify_command("php run-tests.php Zend/tests/"),
            Classification::Supported {
                rtk_equivalent: "rtk phpt",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_phpt_run_tests() {
        assert_eq!(
            rewrite_command_no_prefixes("php run-tests.php Zend/tests/67468.phpt", &[]),
            Some("rtk phpt Zend/tests/67468.phpt".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("php run-tests.php", &[]),
            Some("rtk phpt".into())
        );
    }

    #[test]
    fn test_normalize_php_tool_command_custom_bin_dir() {
        use std::path::PathBuf;
        let dirs = vec![PathBuf::from("tools/bin"), PathBuf::from("vendor/bin")];
        assert_eq!(
            normalize_php_tool_command_with_dirs("tools/bin/phpunit tests/", &dirs),
            "phpunit tests/"
        );
        assert_eq!(
            normalize_php_tool_command_with_dirs("./tools/bin/pest", &dirs),
            "pest"
        );
    }

    /// `jj` is covered only by a TOML filter, never by the native RULES table,
    /// so the bare case pins the TOML branch of the rewrite path and keeps the
    /// wrapper assertions below from passing vacuously when TOML is disabled.
    /// #2375: wrappers are peeled before matching; `is_fully_anchored` in
    /// `core::toml_filter` is what keeps a filter off the wrapper itself.
    #[test]
    fn test_toml_filter_rewrites_bare_and_wrapped_invocations() {
        assert_eq!(
            rewrite_command_no_prefixes("jj log", &[]),
            Some("rtk jj log".into()),
        );
        assert_eq!(
            rewrite_command_no_prefixes("timeout 5 /usr/bin/jj log", &[]),
            Some("timeout 5 rtk /usr/bin/jj log".into()),
        );
        assert_eq!(
            rewrite_command_no_prefixes("nohup /opt/tools/jj log", &[]),
            Some("nohup rtk /opt/tools/jj log".into()),
        );
    }

    #[test]
    fn test_path_qualified_liquibase_is_not_rewritten() {
        // #3757 originally requested path-qualified rewriting, but registry
        // normalization currently classifies the basename without rewriting
        // the original argv[0]. Pin that existing behavior explicitly.
        assert_eq!(
            rewrite_command_no_prefixes("/usr/bin/liquibase update", &[]),
            None,
        );
    }

    // --- The walk over transparent layers ---

    #[test]
    fn test_bare_time_is_the_reserved_word_and_gnu_time_goes_through_a_path() {
        // Where a pipeline starts, bare `time` takes only `-p` and `--`, so
        // `-q` is the command it runs. Through a path it is GNU time.
        assert_eq!(rewrite_command_no_prefixes("time -q git status", &[]), None);
        assert_eq!(
            rewrite_command_no_prefixes("/usr/bin/time -q git status", &[]),
            Some("/usr/bin/time -q rtk git status".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("/usr/bin/time -pv cargo build", &[]),
            Some("/usr/bin/time -pv rtk cargo build".into())
        );
        for cmd in [
            "time -pp git status",
            "time -p -p git status",
            "time time -v git status",
            "time -p time -v git status",
            "time -f %e git status",
            // A redirect glued to an option leaves its word no option, read
            // the same way by the walk and by the line's grammar.
            "time -p>out git status",
            "time -->x git status",
        ] {
            assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None, "{cmd:?}");
        }
        for (cmd, expected) in [
            ("time -- git status", "time -- rtk git status"),
            ("time -p -- git status", "time -p -- rtk git status"),
            ("time time git status", "time time rtk git status"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "{cmd:?}"
            );
        }
    }

    /// Behind a layer that runs a program, and in a later stage of a pipeline,
    /// bare `time` is GNU time, found by `$PATH`, with its own grammar.
    #[test]
    fn test_time_is_gnu_time_where_no_pipeline_starts() {
        for (input, expected) in [
            ("FOO=1 time -v cargo build", "FOO=1 time -v rtk cargo build"),
            (
                "LC_ALL=C time -o log git status",
                "LC_ALL=C time -o log rtk git status",
            ),
            ("env time -f %e git status", "env time -f %e rtk git status"),
            (
                "exec time -f %e git status",
                "exec time -f %e rtk git status",
            ),
            ("nice time -v cargo build", "nice time -v rtk cargo build"),
            (
                "timeout 5 time -v git status",
                "timeout 5 time -v rtk git status",
            ),
            (
                "git log | time -v grep foo",
                "git log | time -v rtk grep foo",
            ),
            (
                "git log | time -f %e grep x",
                "git log | time -f %e rtk grep x",
            ),
            (
                "git log |\n  time -v grep foo",
                "git log |\n  time -v rtk grep foo",
            ),
            ("time -p git status", "time -p rtk git status"),
            ("FOO=1 time -p git status", "FOO=1 time -p rtk git status"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(input, &[]),
                Some(expected.into()),
                "{input}"
            );
        }
        // The first stage of a pipeline is where one starts: `-v` is the
        // command there.
        assert_eq!(
            rewrite_command_no_prefixes("time -v grep x | cat", &[]),
            None
        );
    }

    /// The command behind a reserved word bash runs a pipeline after is
    /// rewritten, and the reserved word stays as written.
    #[test]
    fn test_the_command_behind_a_reserved_word_is_rewritten() {
        for (cmd, expected) in [
            (
                "if git diff --quiet; then git status; fi",
                "if rtk git diff --quiet; then rtk git status; fi",
            ),
            (
                "if true; then git status; else cargo build; fi",
                "if true; then rtk git status; else rtk cargo build; fi",
            ),
            (
                "until false; do git status; done",
                "until false; do rtk git status; done",
            ),
            (
                "for x in a b; do git status; done",
                "for x in a b; do rtk git status; done",
            ),
            (
                "while true; do time git status; done",
                "while true; do time rtk git status; done",
            ),
            ("then A=1 git status", "then A=1 rtk git status"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "{cmd:?}"
            );
        }
        for cmd in [
            "coproc git status",
            "for x in git status",
            "case git in",
            "A=1 then git status",
        ] {
            assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None, "{cmd:?}");
        }
    }

    /// A command inside a compound command that a pipe joins to another
    /// command writes the compound's output, or reads its input: every
    /// command in any of its lists is left as written, the first and the
    /// later ones, a `case` arm's too. A compound no pipe joins is rewritten
    /// like any other list, and a pipeline inside one of its lists is read
    /// like any other pipeline.
    #[test]
    fn test_a_compound_a_pipe_joins_is_left_as_written() {
        for cmd in [
            "for x in a; do git status; done | head",
            "while true; do time git status; done | cat",
            "git log | if true; then git status; fi",
            "for x in a; do git status; git log; done | wc -l",
            "if true; then git status; git log; fi | wc -l",
            "for x in a; do git status && git log; done | head",
            "case x in a) git log;; esac | wc -l",
            "case x in a) ls;; b) git status; git log;; esac | head",
            "if true; then :; else ls; git log; fi | wc -l",
            "while git status; do git log; ls; done | head",
            "for x in a; do if true; then ls; fi; git log; done | wc -l",
            "for x in a; do git status | head; git log; done | wc -l",
            "for x in a; do git log; done > out.txt | head",
            "ls | while read x; do git status; git log; done",
        ] {
            assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None, "{cmd:?}");
        }
        for (cmd, expected) in [
            (
                "if true; then git status; fi; ls | head",
                "if true; then rtk git status; fi; rtk ls | head",
            ),
            (
                "if true; then git status; git log; fi; ls | head",
                "if true; then rtk git status; rtk git log; fi; rtk ls | head",
            ),
            (
                "case x in a) git log;; esac; ls | head",
                "case x in a) rtk git log;; esac; rtk ls | head",
            ),
            (
                "for x in a; do git status; done 2>/dev/null&ls|head",
                "for x in a; do rtk git status; done 2>/dev/null&rtk ls|head",
            ),
            (
                "case $y in a) git status | head;; b) git log;; esac",
                "case $y in a) rtk git status | head;; b) rtk git log;; esac",
            ),
            (
                "for x in a; do if true; then git log; fi | head; git status; done",
                "for x in a; do if true; then git log; fi | head; rtk git status; done",
            ),
            (
                "if true; then git status; fi | grep x",
                "if true; then git status; fi | rtk grep x",
            ),
            ("time git status | head", "time rtk git status | head"),
            (
                "for x in a; do git status; git log; done",
                "for x in a; do rtk git status; rtk git log; done",
            ),
            (
                "case x in a) git log;; esac",
                "case x in a) rtk git log;; esac",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "{cmd:?}"
            );
        }
    }

    /// A compound command or a subshell right after `coproc` or `coproc
    /// NAME` is the coprocess's body, whose input and output are pipes: its
    /// commands are left as written, and the line's other commands are not.
    #[test]
    fn test_a_coprocess_body_is_left_as_written() {
        for cmd in [
            "coproc while true; do git status; done",
            "coproc for x in a; do git status; done",
            "coproc NAME while true; do git status; done",
            "coproc NAME for x in a; do git status; done",
            "coproc if true; then git status; fi",
            "coproc case x in a) git status;; esac",
            "coproc ( git status )",
            "coproc (git status)",
            "coproc ( git status; git log )",
            "coproc ( git status && git log )",
            "coproc NAME ( git log )",
            "coproc NAME (git log; git status)",
            "coproc ( (git status) )",
            "coproc ( while true; do git status; done )",
            "coproc ( git status",
            // A name of several tokens is still one word.
            "coproc NAME$x while true; do git status; done",
            "coproc NAME$x until false; do git status; done",
            "coproc NAME$x for x in a; do git status; done",
            "coproc NAME$x select x in a; do git status; done",
            "coproc NAME$x if true; then git status; fi",
            "coproc NAME${x} while true; do git status; done",
            "coproc NAME${x} until false; do git status; done",
            "coproc NAME${x} for x in a; do git status; done",
            "coproc NAME${x} select x in a; do git status; done",
            "coproc NAME${x} if true; then git status; fi",
            "coproc NAME$x ( git status )",
            "coproc NAME${x} ( git status )",
            "coproc NAME$(echo x) while true; do git status; done",
            "coproc ${x} ( git status )",
            "coproc NAME$x$y ( git status )",
            "coproc N{a} ( git status )",
            "coproc N* if true; then git status; fi",
            "coproc NAME@(x)y while true; do git status; done",
        ] {
            assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None, "{cmd:?}");
        }
        for (cmd, expected) in [
            (
                "git status | head; coproc while true; do git log; done",
                "rtk git status | head; coproc while true; do git log; done",
            ),
            (
                "coproc while true; do git log; done; git status",
                "coproc while true; do git log; done; rtk git status",
            ),
            (
                "coproc NAME while true; do git log; done; git status",
                "coproc NAME while true; do git log; done; rtk git status",
            ),
            (
                "coproc ( git log ); git status",
                "coproc ( git log ); rtk git status",
            ),
            (
                "coproc NAME ( git log ) && git status",
                "coproc NAME ( git log ) && rtk git status",
            ),
            (
                "coproc N{a} ( git log ); git status",
                "coproc N{a} ( git log ); rtk git status",
            ),
            ("ls; ( git status )", "rtk ls; ( rtk git status )"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "{cmd:?}"
            );
        }
    }

    /// A compound command one line of a block opens and a later line closes
    /// holds the lines between: when a pipe joins it to another command, the
    /// commands on all of them are left as written, and the block's other
    /// commands are not.
    #[test]
    fn test_a_compound_across_lines_that_a_pipe_joins_is_left_as_written() {
        for (cmd, expected) in [
            (
                "ls; while true; do\ngit status\nls; done | wc -l",
                "rtk ls; while true; do\ngit status\nls; done | wc -l",
            ),
            (
                "ls; for x in a; do\ngit status\ngit log; done | head",
                "rtk ls; for x in a; do\ngit status\ngit log; done | head",
            ),
            (
                "ls | while read x; do\ngit status\ngit log; done; ls",
                "ls | while read x; do\ngit status\ngit log; done; rtk ls",
            ),
            (
                "ls; while true; do\ngit status\nls; done",
                "rtk ls; while true; do\nrtk git status\nrtk ls; done",
            ),
            (
                "ls; while true; do\ngit status\nls; done; ls | head",
                "rtk ls; while true; do\nrtk git status\nrtk ls; done; rtk ls | head",
            ),
            (
                "ls; for x in a; do git status; done\nls | head",
                "rtk ls; for x in a; do rtk git status; done\nrtk ls | head",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "{cmd:?}"
            );
        }
    }

    /// `--` is read as `arg_tokenizer` reads the one argument, once the shell
    /// has resolved its quotes.
    #[test]
    fn test_dash_dash_is_the_tokenizers_separator() {
        for word in ["--", "'--'", "\"--\""] {
            assert!(is_dash_dash(word), "{word:?}");
        }
        for word in ["-", "---", "--x", "--=", "x"] {
            assert!(!is_dash_dash(word), "{word:?}");
        }
    }

    /// A user prefix never takes a reserved word the walk steps over: the
    /// word after `time`'s options is the command, whatever a prefix says.
    #[test]
    fn test_a_reserved_word_is_no_prefixs_to_claim() {
        let prefixes = vec!["time -v".to_string(), "then x".to_string()];
        for cmd in ["time -v git status", "then x git status"] {
            assert_eq!(super::rewrite_command(cmd, &[], &prefixes), None, "{cmd:?}");
        }
    }

    #[test]
    fn test_bare_uv_run_with_no_inner_command_is_not_rewritten() {
        assert_eq!(rewrite_command_no_prefixes("uv run", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("uv run  ", &[]), None);
    }

    /// A quoted value that holds ` rtk ` is one word, not the word `rtk`.
    #[test]
    fn test_double_wrap_guard_checks_shell_words_not_blank_split_text() {
        assert_eq!(
            rewrite_command_no_prefixes(r#"/usr/bin/time -f "%e rtk %U" git status"#, &[]),
            Some(r#"/usr/bin/time -f "%e rtk %U" rtk git status"#.into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("timeout 5 rtk git status", &[]),
            None
        );
    }

    /// GNU nice has no `+N` spelling, so `+5` is the program it runs.
    #[test]
    fn test_nice_plus_prefixed_number_is_not_rewritten() {
        assert_eq!(rewrite_command_no_prefixes("nice +5 cargo test", &[]), None);
        assert_eq!(rewrite_command_no_prefixes("nice +5 git status", &[]), None);
    }

    /// A run of `NAME=value` and `env` words is one layer however long, so it
    /// does not use up the depth budget.
    #[test]
    fn test_one_layer_per_assignment_run() {
        assert_eq!(
            rewrite_command_no_prefixes("env env env env env env env env env env git status", &[]),
            Some("env env env env env env env env env env rtk git status".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes(
                "A=1 env B=2 env C=3 timeout 5 nice -n 5 command git status",
                &[]
            ),
            Some("A=1 env B=2 env C=3 timeout 5 nice -n 5 command rtk git status".into())
        );
    }

    #[test]
    fn test_process_wrapper_2375_and_3574_regression_rows() {
        for (input, expected) in [
            ("nice -n 10 ls -la", "nice -n 10 rtk ls -la"),
            ("time git log --oneline -5", "time rtk git log --oneline -5"),
            (
                "timeout 30 gh run view 123 --log-failed",
                "timeout 30 rtk gh run view 123 --log-failed",
            ),
            (
                "timeout 60 php artisan tinker",
                "timeout 60 rtk php artisan tinker",
            ),
            ("nohup git status", "nohup rtk git status"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(input, &[]),
                Some(expected.into()),
                "{input}"
            );
        }
    }

    #[test]
    fn test_a_wrapper_head_with_an_expansion_or_a_glob_is_no_wrapper() {
        for input in [
            "*/timeout 300 git status",
            "${D}/timeout 300 git status",
            "$D/timeout 300 git status",
            "/usr/*/time git status",
        ] {
            assert_eq!(rewrite_command_no_prefixes(input, &[]), None, "{input}");
        }
        // An unbraced `$NAME` glued to the operand is one word, which fills it.
        assert_eq!(
            rewrite_command_no_prefixes("timeout 5$UNIT git status", &[]),
            Some("timeout 5$UNIT rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_env_prefixed_commands() {
        for (input, expected) in [
            ("NODE_ENV=test git status", "NODE_ENV=test rtk git status"),
            (
                "PRISMA_USER_CONSENT=\"yes\" pnpm install",
                "PRISMA_USER_CONSENT=\"yes\" rtk pnpm install",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(input, &[]),
                Some(expected.into()),
                "{input}"
            );
        }
    }

    /// `NAME=value` is an assignment only where a simple command starts:
    /// behind `timeout`, `nice`, `exec` or `uv run` it is a program's name.
    /// `env` is a program found wherever it sits, so its own operands are its
    /// own anywhere.
    #[test]
    fn test_shell_assignment_position_gating() {
        for input in [
            "timeout 300 A=1 git status",
            "nice FOO=1 git status",
            "exec A=1 git status",
            "uv run A=1 pytest",
            "noglob A=1 git status",
        ] {
            assert_eq!(rewrite_command_no_prefixes(input, &[]), None, "{input}");
        }
        assert_eq!(
            rewrite_command_no_prefixes("timeout 5 env A=1 git status", &[]),
            Some("timeout 5 env A=1 rtk git status".into())
        );
        // A later pipeline stage starts its own simple command.
        assert_eq!(
            rewrite_command_no_prefixes("cargo test | FOO=1 command grep FAILED", &[]),
            Some("cargo test | FOO=1 command rtk grep FAILED".into())
        );
    }

    /// Once `timeout`'s one operand is read, its own options are over: `-s`
    /// and `--` there are the program it runs.
    #[test]
    fn test_wrapper_options_end_at_the_first_operand() {
        for cmd in [
            "timeout 300 -s KILL git status",
            "timeout 300 -- cargo test",
        ] {
            assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None, "{cmd:?}");
        }
        assert_eq!(
            rewrite_command_no_prefixes("timeout -s KILL 5 cargo test", &[]),
            Some("timeout -s KILL 5 rtk cargo test".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("timeout -vk 5 300 cargo test", &[]),
            Some("timeout -vk 5 300 rtk cargo test".into())
        );
    }

    /// A user prefix is tried at the start of every layer whose own command
    /// has no rewrite, not only behind `uv run`.
    #[test]
    fn test_a_user_prefix_is_tried_at_every_layer() {
        assert_eq!(
            super::rewrite_command("env -i git status", &[], &["env -i".to_string()]),
            Some("env -i rtk git status".into())
        );
        assert_eq!(
            super::rewrite_command(
                "nice -n 19 ionice git status",
                &[],
                &["nice -n 19 ionice".to_string()]
            ),
            Some("nice -n 19 ionice rtk git status".into())
        );
        let frozen = vec!["uv run --frozen".to_string()];
        assert_eq!(
            super::rewrite_command("uv run --frozen pytest", &["pytest".to_string()], &frozen),
            None
        );
        assert_eq!(
            super::rewrite_command(
                "uv run --frozen git status",
                &["pytest".to_string()],
                &frozen
            ),
            Some("uv run --frozen rtk git status".into())
        );
    }

    /// A prefix that would end inside the words of a built-in layer it covers
    /// is skipped, so `rtk` never lands inside a wrapper's own grammar.
    #[test]
    fn test_a_prefix_ending_inside_a_layer_is_skipped() {
        for (cmd, prefix) in [
            ("nice -5 xyz git status", "nice"),
            ("nice timeout 5 xyz git status", "nice timeout"),
        ] {
            assert_eq!(
                super::rewrite_command(cmd, &[], &[prefix.to_string()]),
                None,
                "{cmd:?}"
            );
        }
    }

    #[test]
    fn test_a_prefix_matches_whole_words_written_as_configured() {
        let prefixes = vec!["myjail run".to_string()];
        for (cmd, expected) in [
            ("myjail run git status", Some("myjail run rtk git status")),
            ("myjail runs git status", None),
            ("myjail run\u{a0}git status", None),
            ("myjail  run git status", None),
        ] {
            assert_eq!(
                super::rewrite_command(cmd, &[], &prefixes),
                expected.map(String::from),
                "{cmd:?}"
            );
        }
    }

    /// What follows a user prefix may be an assignment, and is no place where
    /// a pipeline starts.
    #[test]
    fn test_a_user_prefix_keeps_the_simple_command_start() {
        let prefixes = vec!["sudo".to_string()];
        for (cmd, expected) in [
            ("sudo A=1 git status", "sudo A=1 rtk git status"),
            ("sudo A=1 B=2 git status", "sudo A=1 B=2 rtk git status"),
            ("sudo git status", "sudo rtk git status"),
            ("A=1 sudo B=2 git status", "A=1 sudo B=2 rtk git status"),
            ("git log | sudo A=1 grep x", "git log | sudo A=1 rtk grep x"),
            (
                "timeout 5 sudo A=1 git status",
                "timeout 5 sudo A=1 rtk git status",
            ),
            ("sudo time -v git status", "sudo time -v rtk git status"),
        ] {
            assert_eq!(
                super::rewrite_command(cmd, &[], &prefixes).as_deref(),
                Some(expected),
                "{cmd:?}"
            );
        }
    }

    // --- `exclude_commands` behind the layers under `uv run` ---

    /// Exclusion is decided before the context gates, so an excluded command
    /// is excluded wherever it sits.
    #[test]
    fn test_decide_answers_excluded_in_every_context() {
        let excluded = compile_exclude_patterns(&["pytest".to_string()]);
        let text = "pytest -q";
        let tokens = tokenize(text);
        let words = words(text, &tokens);
        let line = Line {
            text,
            words: &words,
            tokens: &tokens,
        };
        for context in [
            RewriteContext::Normal,
            RewriteContext::PipelineFinal,
            RewriteContext::PipelineProducer,
        ] {
            assert_eq!(
                decide(&line, 0, &excluded, context),
                Some(Decision::Excluded)
            );
        }
        assert!(matches!(
            decide(&line, 0, &[], RewriteContext::Normal),
            Some(Decision::Rewrite(_))
        ));
    }

    /// `uv run`'s fall-through to its own rule reads what it wraps with the
    /// rewrite's own walk, so no layer hides an excluded command from it.
    #[test]
    fn test_uv_run_fallthrough_reads_through_every_layer_for_exclusion() {
        let excluded = vec!["pytest".to_string()];
        let nested = format!("{}pytest", "uv run ".repeat(11));
        for input in [
            "uv run pytest",
            "uv run timeout 5 pytest",
            "uv run noglob pytest",
            "uv run command pytest",
            "uv run /usr/bin/pytest",
            "uv run uv run pytest",
            "uv run time pytest",
            "uv run nice pytest",
            "python -m pytest",
            "uv run python -m pytest",
            "uv run nice python -m pytest",
            "uv run timeout 5 python -m pytest",
            "uv run env python -m pytest",
            "uv run --with x pytest",
            "uv run --with x python -m pytest",
            "uv run --frozen pytest",
            "uv run -- pytest",
            "uv run -- python -m pytest",
            nested.as_str(),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(input, &excluded),
                None,
                "{input}"
            );
        }
        for (input, prefix) in [
            ("uv run x pytest", "x"),
            ("uv run nice -n 19 ionice pytest", "nice -n 19 ionice"),
        ] {
            assert_eq!(
                super::rewrite_command(input, &excluded, &[prefix.to_string()]),
                None,
                "{input}"
            );
        }
        for (input, expected) in [
            ("uv run -- cargo test", "rtk uv run -- cargo test"),
            (
                "uv run timeout 5 cargo test",
                "uv run timeout 5 rtk cargo test",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(input, &excluded).as_deref(),
                Some(expected),
                "{input}"
            );
        }
        // Under any exclusion, an option the walk cannot read past leaves the
        // command behind it unread, and the wrapper is not matched whole.
        for input in ["uv run --with x pytest", "uv run --frozen ruff check"] {
            assert_eq!(
                rewrite_command_no_prefixes(input, &["git".to_string()]),
                None,
                "{input}"
            );
        }
        // Without exclusions the wrapper is matched whole, or its command is.
        assert_eq!(
            rewrite_command_no_prefixes("uv run python -m pytest", &[]),
            Some("uv run rtk pytest".into())
        );
        for cmd in [
            "uv run -- pytest",
            "uv run --with x pytest",
            "uv run --with x python -m pytest",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]),
                Some(format!("rtk {cmd}")),
                "{cmd:?}"
            );
        }
        assert!(
            super::rewrite_command(
                "uv run nice -n 19 ionice pytest",
                &[],
                &["nice -n 19 ionice".to_string()]
            )
            .is_some()
        );
    }

    /// The command `uv run` wraps is excluded as bash runs its words, their
    /// quotes and escapes removed, as much as when it is written plain.
    #[test]
    fn test_uv_run_fallthrough_reads_an_excluded_command_as_bash_runs_it() {
        let excluded = vec!["pytest".to_string()];
        for input in [
            "uv run 'pytest'",
            "uv run \"pytest\" -x",
            "uv run p\"ytest\" -x",
            "uv run \\pytest",
            "uv run '/usr/bin/pytest'",
            "uv run timeout 5 'pytest'",
            "uv run env A=1 \\pytest",
            "uv run -- p'ytest'",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(input, &excluded),
                None,
                "{input}"
            );
            assert_eq!(
                rewrite_command_no_prefixes(input, &[]),
                Some(format!("rtk {input}")),
                "{input}"
            );
        }
        assert_eq!(
            rewrite_command_no_prefixes("uv run 'pytestx'", &excluded).as_deref(),
            Some("rtk uv run 'pytestx'")
        );
        // A program named like an assignment is read as `decide` reads it,
        // which matches no exclusion against it.
        assert_eq!(
            rewrite_command_no_prefixes("uv run uv run FOO=1 make", &["^[^a-z]".to_string()])
                .as_deref(),
            Some("rtk uv run uv run FOO=1 make")
        );
    }

    /// An exclusion reads the command on the words bash runs, up to where it
    /// ends: a redirection behind it is no part of it, and a quoted path is
    /// one word, whose basename is the program's name.
    #[test]
    fn test_exclusion_reads_the_words_up_to_the_redirections() {
        for (pattern, input, unexcluded) in [
            (
                "^pytest$",
                "uv run 'pytest' 2>&1",
                "rtk uv run 'pytest' 2>&1",
            ),
            ("^pytest$", "uv run pytest 2>&1", "uv run rtk pytest 2>&1"),
            (
                "^pytest$",
                "uv run timeout 5 'pytest' 2>&1",
                "rtk uv run timeout 5 'pytest' 2>&1",
            ),
            (
                "pytest",
                "uv run timeout 5 'pytest' 2>&1",
                "rtk uv run timeout 5 'pytest' 2>&1",
            ),
            (
                "pytest",
                "uv run '/opt/my tools/pytest' -x",
                "rtk uv run '/opt/my tools/pytest' -x",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(input, &[pattern.to_string()]),
                None,
                "{input} under {pattern}"
            );
            assert_eq!(
                rewrite_command_no_prefixes(input, &[]).as_deref(),
                Some(unexcluded),
                "{input}"
            );
        }
        assert_eq!(
            rewrite_command_no_prefixes("uv run '/opt/my tools/pytest' -x", &["^pytest$".into()])
                .as_deref(),
            Some("rtk uv run '/opt/my tools/pytest' -x")
        );
    }

    #[test]
    fn test_exclusion_reads_a_wrapper_head_by_the_name_it_runs() {
        for cmd in [
            "uv run env pytest",
            "uv run 'env' pytest",
            "uv run \\env pytest",
            "uv run 'command' pytest",
            "uv run \"noglob\" pytest",
            "uv run 'exec' pytest",
            "uv run 'uv' run pytest",
            "uv run \\uv\trun pytest",
            "timeout 5 'env' pytest -x",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &["pytest".into()]),
                None,
                "{cmd}"
            );
        }
    }

    #[test]
    fn test_a_path_spelled_wrapper_head_leaves_the_line_as_written() {
        for head in [
            "/usr/bin/env",
            "/usr/bin/command",
            "/usr/bin/exec",
            "/usr/bin/noglob",
            "'/usr/bin/env'",
            "/usr/bin/uv run",
        ] {
            for cmd in [
                format!("{head} git status"),
                format!("{head} pytest"),
                format!("timeout 5 {head} pytest -x"),
            ] {
                assert_eq!(rewrite_command_no_prefixes(&cmd, &[]), None, "{cmd}");
            }
            // `uv run` has its own rule, so it is refused only for what it
            // hides from an exclusion.
            for cmd in [
                format!("{head} pytest"),
                format!("timeout 5 {head} pytest -x"),
                format!("uv run {head} pytest"),
            ] {
                assert_eq!(
                    rewrite_command_no_prefixes(&cmd, &["pytest".into()]),
                    None,
                    "{cmd}"
                );
            }
        }
    }

    #[test]
    fn test_only_a_path_spelled_uv_run_is_refused_behind_uv_run() {
        let excluded = ["pytest".to_string()];
        // A `uv` spelled by a path, followed by anything but `run`, is another
        // program: the `uv run` that heads the command is matched whole.
        for (cmd, want) in [
            (
                "uv run /usr/bin/uv pip list",
                "rtk uv run /usr/bin/uv pip list",
            ),
            ("uv run ./uv pip list", "rtk uv run ./uv pip list"),
            ("uv run git status", "uv run rtk git status"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &excluded).as_deref(),
                Some(want),
                "{cmd}"
            );
        }
        for cmd in [
            "uv run /usr/bin/uv run pytest",
            "uv run /usr/bin/env xyz",
            "uv run /usr/bin/uv  run git status",
            "uv run /usr/bin/uv\trun git status",
        ] {
            assert_eq!(rewrite_command_no_prefixes(cmd, &excluded), None, "{cmd}");
        }
    }

    #[test]
    fn test_exclusion_reads_a_tab_after_the_tool() {
        assert_eq!(
            rewrite_command_no_prefixes("npx playwright\ttest", &["^playwright test".into()]),
            None
        );
    }

    #[test]
    fn test_exclusion_reads_the_basename_of_a_plain_first_word_only() {
        assert_eq!(
            rewrite_command_no_prefixes("/usr/bin/pytest -x", &["^pytest -x".into()]),
            None
        );
        // The cut inside the quotes would read `tools/pytest' -x`, which this
        // pattern matches; the command is excluded or not by its words alone.
        let cmd = "'/opt/my tools/pytest' -x";
        assert_eq!(
            rewrite_command_no_prefixes(cmd, &["^tools/pytest".into()]),
            rewrite_command_no_prefixes(cmd, &[]),
        );
    }

    /// A word that holds a blank is one word: `'pytest x'` runs a program
    /// named `pytest x`, which no pattern naming `pytest` covers.
    #[test]
    fn test_exclusion_never_splits_a_word() {
        for pattern in ["pytest", "^pytest$", "^pytest( |$)", "^pytest x"] {
            assert_eq!(
                rewrite_command_no_prefixes("uv run 'pytest x'", &[pattern.to_string()]).as_deref(),
                Some("rtk uv run 'pytest x'"),
                "{pattern}"
            );
        }
        assert_eq!(
            rewrite_command_no_prefixes("uv run 'pytest x'", &[r"^pytest\\ x$".into()]),
            None
        );
    }

    #[test]
    fn test_spell_words_escapes_only_a_blank_inside_a_word() {
        for (words, expected) in [
            (&["pytest", "-x"][..], "pytest -x"),
            (&["pytest x"][..], r"pytest\ x"),
            (&["a\tb"][..], "a\\\tb"),
            (&["\u{b}a b", "it's"][..], "\u{b}a\\ b it's"),
            (&[][..], ""),
        ] {
            assert_eq!(spell_words(words.iter().copied()), expected, "{words:?}");
        }
    }

    #[test]
    fn test_exclusion_reads_the_tool_off_the_words_bash_runs() {
        for cmd in [
            "npx playwright test",
            "npx \"playwright\" test",
            "npx 'playwright' test",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &["playwright".into()]),
                None,
                "{cmd}"
            );
        }
        assert_eq!(
            rewrite_command_no_prefixes("pytest ''", &["^pytest$".into()]),
            Some("rtk pytest ''".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("pytest", &["^pytest$".into()]),
            None
        );
    }

    #[test]
    fn test_exclusion_matches_the_command_as_typed_and_as_run() {
        for (cmd, pattern) in [
            ("git log --format=\"%H\"", "^git log --format=\"%H\""),
            ("git commit -m 'wip'", "^git commit -m 'wip'"),
            ("grep -r \"a b\" .", "^grep -r \"a b\""),
            ("git log --format=\"%H\"", "^git log --format=%H"),
            ("git commit -m 'wip'", "^git commit -m wip"),
            ("grep -r \"a b\" .", "^grep -r a\\\\ b"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[pattern.into()]),
                None,
                "{cmd} {pattern}"
            );
        }
        assert_eq!(
            rewrite_command_no_prefixes(
                "git log --format=\"%H\"",
                &["^git log --format=%h".into()]
            ),
            Some("rtk git log --format=\"%H\"".into())
        );
    }

    #[test]
    fn test_exclusion_holds_behind_a_wrapper_written_quoted() {
        for cmd in [
            "'timeout' 5 pytest",
            "\\timeout 5 pytest",
            "\"nice\" pytest",
            "uv  run pytest",
            "uv\trun timeout 5 pytest",
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &["pytest".into()]),
                None,
                "{cmd}"
            );
        }
    }

    #[test]
    fn test_uv_run_fallthrough_refuses_what_it_wraps_as_written() {
        assert_eq!(
            rewrite_command_no_prefixes("uv run env", &["env".to_string()]),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("uv run env", &[]),
            Some("rtk uv run env".into())
        );
        let excluded = vec!["uv run xyz".to_string()];
        for input in ["uv run uv run xyz", "uv run env uv run xyz"] {
            assert_eq!(
                rewrite_command_no_prefixes(input, &excluded),
                None,
                "{input}"
            );
        }
        assert_eq!(
            rewrite_command_no_prefixes("uv run uv run xyz", &[]),
            Some("uv run rtk uv run xyz".into())
        );
    }

    /// Layers deeper than the walk follows hide the command, which counts as
    /// excluded.
    #[test]
    fn test_uv_run_fallthrough_refuses_what_it_cannot_see_the_end_of() {
        let deep = format!("uv run {}pytest", "nice ".repeat(11));
        assert_eq!(
            rewrite_command_no_prefixes(&deep, &["pytest".to_string()]),
            None
        );
        let chain = format!("{}git status", "uv run ".repeat(12));
        assert_eq!(
            rewrite_command_no_prefixes(&chain, &["^git status".to_string()]),
            None
        );
        assert!(rewrite_command_no_prefixes(&deep, &[]).is_some());
    }

    /// A `NAME=value` word right behind `uv run` is a program's name, and the
    /// fall-through refuses it; `env` is a program, and falls through.
    #[test]
    fn test_uv_run_fallthrough_refuses_a_bare_assignment_or_rtk_disabled() {
        for cmd in [
            "uv run A=1 pytest",
            "uv run RTK_DISABLED=1 git status",
            "uv run RTK_DISABLED=1 python x.py",
            "uv run env RTK_DISABLED=1 git status",
            "uv run timeout 5 env RTK_DISABLED=1 pytest",
            "uv run nice env RTK_DISABLED=1 pytest",
            "uv run env A=1 RTK_DISABLED=1 x",
        ] {
            assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None, "{cmd:?}");
        }
        for (cmd, expected) in [
            ("uv run env python x.py", "rtk uv run env python x.py"),
            ("uv run env -i python x.py", "rtk uv run env -i python x.py"),
            ("uv run env", "rtk uv run env"),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "{cmd:?}"
            );
        }
        assert!(refused_on_disabled(
            "uv run timeout 5 env RTK_DISABLED=1 pytest"
        ));
        assert!(refused_on_disabled("uv run env RTK_DISABLED=1 git status"));
        assert!(!refused_on_disabled("uv run nice RTK_DISABLED=1 pytest"));
        assert!(!refused_on_disabled("uv run RTK_DISABLED=1 git status"));
        let excluded = compile_exclude_patterns(&["pytest".to_string()]);
        assert!(
            rewrite_line("uv run timeout 5 env RTK_DISABLED=1 pytest", &excluded, &[]).disabled
        );
    }

    // --- `RTK_DISABLED=` by assignment name ---

    #[test]
    fn test_rtk_disabled_is_matched_by_assignment_name() {
        for (cmd, expected) in [
            (
                "XRTK_DISABLED=1 git status",
                "XRTK_DISABLED=1 rtk git status",
            ),
            (
                "NOTE='set RTK_DISABLED=1 x' git status",
                "NOTE='set RTK_DISABLED=1 x' rtk git status",
            ),
            (
                "MSG='RTK_DISABLED=1' git status",
                "MSG='RTK_DISABLED=1' rtk git status",
            ),
            (
                "env MSG=RTK_DISABLED=1 git status",
                "env MSG=RTK_DISABLED=1 rtk git status",
            ),
            (
                "RTK_DISABLED=1; git status",
                "RTK_DISABLED=1; rtk git status",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "{cmd:?}"
            );
            assert!(!refused_on_disabled(cmd), "{cmd:?}");
        }
    }

    #[test]
    fn test_the_append_form_is_a_bypass_only_as_a_shell_assignment() {
        for cmd in [
            "RTK_DISABLED+=1 git status",
            "A=1 RTK_DISABLED+=1 git status",
            "RTK_DISABLED+=1 env A=1 git status",
            "env RTK_DISABLED=1 git status",
        ] {
            assert!(refused_on_disabled(cmd), "{cmd:?}");
        }
        for cmd in [
            "env RTK_DISABLED+=1 git status",
            "A=1 env RTK_DISABLED+=1 git status",
            "timeout 5 env RTK_DISABLED+=1 git status",
        ] {
            assert!(!refused_on_disabled(cmd), "{cmd:?}");
        }
        assert_eq!(
            rewrite_command_no_prefixes("env RTK_DISABLED+=1 git status", &[]),
            Some("env RTK_DISABLED+=1 rtk git status".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("RTK_DISABLED+=1 git status", &[]),
            None
        );
    }

    /// The warning follows the walk's stop: a retry behind a user prefix that
    /// stops at the marker refuses on it, and one that stops there and loses
    /// to a retry that decides draws none.
    #[test]
    fn test_a_retry_that_stops_at_the_marker_refuses_on_it() {
        let prefixes =
            normalize_transparent_prefixes(&["nohup".to_string(), "docker exec c".to_string()]);
        for cmd in [
            "nohup RTK_DISABLED=1 git status",
            "docker exec c RTK_DISABLED=1 git status",
        ] {
            let answer = rewrite_line(cmd, &[], &prefixes);
            assert_eq!(answer.text, None, "{cmd}");
            assert!(answer.disabled, "{cmd}");
        }
        let prefixes = normalize_transparent_prefixes(&[
            "timeout 5 nice -n 19 RTK_DISABLED=1".to_string(),
            "nice -n 19".to_string(),
        ]);
        let answer = rewrite_line(
            "timeout 5 nice -n 19 RTK_DISABLED=1 git status",
            &[],
            &prefixes,
        );
        assert_eq!(
            answer.text.as_deref(),
            Some("timeout 5 nice -n 19 RTK_DISABLED=1 rtk git status")
        );
        assert!(!answer.disabled);
    }

    // --- A subshell's closing `)` ---

    #[test]
    fn test_a_word_after_a_subshell_is_left_as_written() {
        for cmd in [
            "(true)git status",
            "(true) git status",
            "( (true)git status )",
            "( (true) git status )",
        ] {
            assert_eq!(rewrite_command_no_prefixes(cmd, &[]), None, "{cmd:?}");
        }
        for (cmd, expected) in [
            ("( git status )word", "( rtk git status )word"),
            ("( (git status) )", "( (rtk git status) )"),
            ("(true) >f; git status", "(true) >f; rtk git status"),
            ("(true) && git status", "(true) && rtk git status"),
            (
                "case x in a) git status;; esac",
                "case x in a) rtk git status;; esac",
            ),
            (
                "case x in a)git status;; esac",
                "case x in a)rtk git status;; esac",
            ),
        ] {
            assert_eq!(
                rewrite_command_no_prefixes(cmd, &[]).as_deref(),
                Some(expected),
                "{cmd:?}"
            );
        }
    }
}
