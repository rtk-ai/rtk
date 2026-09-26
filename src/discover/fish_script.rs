//! Classifies unambiguously-fish command strings and wraps them for explicit fish execution.
//!
//! Hosts that evaluate Bash-tool command strings with a POSIX/zsh layer choke on
//! fish-only syntax (`if … end`, `; and`) before RTK ever runs. [`try_wrap`] is
//! asked first in [`crate::hooks::decision::decide`]: a script that is provably
//! fish — a fish-only marker at command position and no POSIX disambiguator — is
//! rewritten to `rtk run --shell fish -c '<script>'`, a form every host layer
//! parses as one command with one quoted argument. Anything ambiguous is left
//! alone and takes the decision path it always did.
//!
//! The classification is this module's own: it does not ask the shared lexer to
//! treat fish keywords as unattestable, because that lexer reads the host's
//! command string as bash (see `lexer::ShellDialect`).

use super::lexer::{self, ParsedToken, TokenKind};
use super::shell_wrapper::is_shell_wrapper_candidate;

/// Keywords only fish accepts at command position. One of these, at a command
/// boundary and with no POSIX marker anywhere, is what makes a script *provably*
/// fish rather than merely fish-compatible.
const FISH_ONLY_KEYWORDS: &[&str] = &["end", "begin", "switch", "and", "or", "not"];

/// Keywords only POSIX shells accept at command position — their presence vetoes
/// fish classification.
const POSIX_ONLY_KEYWORDS: &[&str] = &["then", "fi", "do", "done", "esac", "elif"];

/// Wrap `cmd` for explicit fish execution when it is unambiguously a fish script.
///
/// Returns `None` (caller keeps its defer behavior) when the script is not
/// provably fish, when the command already delegates (`rtk …` or an explicit
/// shell `-c` wrapper), when the script contains a fish-divergent backslash
/// sequence (`\\` or `\'`), on Windows, when `hooks.wrap_fish_scripts` is
/// disabled, or when no `fish` binary is resolvable.
///
/// The cheap, pure classification runs first; the config read and the `fish`
/// PATH probe run only once the command is classified fish. So common non-fish
/// commands reaching the unattestable gate (`echo $(date)`, `git log > out`,
/// POSIX blocks) do zero I/O here.
pub fn try_wrap(cmd: &str) -> Option<String> {
    try_wrap_with_probes(
        cmd,
        || {
            crate::core::config::Config::load()
                .map(|c| c.hooks.wrap_fish_scripts)
                .unwrap_or(true)
        },
        || crate::core::utils::resolve_binary("fish").is_ok(),
    )
}

/// [`try_wrap`] with the fish-binary probe injected and the config assumed
/// enabled, for deterministic tests. Pure: never reads the RTK config.
#[cfg(test)]
pub(crate) fn try_wrap_gated(cmd: &str, fish_available: bool) -> Option<String> {
    try_wrap_with_probes(cmd, || true, || fish_available)
}

/// Shared gate chain for [`try_wrap`] and `try_wrap_gated`. The pure checks
/// (platform, delegation skip, fish classification, backslash veto) run before
/// the two probes, which are consulted only once the script is confirmed fish;
/// both are `FnOnce`, so a caller pays for the config read and the PATH scan
/// only on the fish path.
fn try_wrap_with_probes(
    cmd: &str,
    enabled: impl FnOnce() -> bool,
    fish: impl FnOnce() -> bool,
) -> Option<String> {
    if cfg!(windows) {
        // cmd/PowerShell host layers do not honor POSIX single quotes, so the
        // wrapped form could be mis-tokenized before reaching rtk.
        return None;
    }

    let script = cmd.trim();
    if script.split_whitespace().next() == Some("rtk") || is_shell_wrapper_candidate(script) {
        return None;
    }
    if !is_unambiguous_fish(script) {
        return None;
    }
    // The wrap is the one rewrite RTK emits for a script it did not decompose,
    // so it refuses what the permission gate refuses: command/process
    // substitution and file-target redirects, plus fish's own `(cmd)`
    // substitution, which the shared gate reads as a subshell. What is left is
    // control flow — `; and`, `if … end` — which the host would have failed to
    // parse at all. See `src/hooks/README.md`.
    //
    // Read from the same comment-stripped text the classification uses. An
    // apostrophe in a comment opens the shared lexer's quote state and swallows
    // everything after it into one argument, so a gate reading the raw script
    // sees no redirect and no substitution at all while the classifier, reading
    // the stripped code, sees clean fish.
    let code = strip_comments(script);
    if lexer::contains_unattestable_construct(&code) || contains_fish_substitution(&code) {
        return None;
    }
    // Fish single-quoted strings diverge from POSIX single-quote semantics for
    // exactly two sequences: `\\` collapses to one backslash and `\'` becomes a
    // literal quote (a backslash before any other character is literal in both).
    // Refuse scripts containing either sequence so every wrapped script
    // round-trips byte-identically under sh/bash/zsh *and* fish host layers. A
    // lone backslash (e.g. `printf '%s\n'`) still wraps.
    if script.contains("\\\\") || script.contains("\\'") {
        return None;
    }
    if !enabled() || !fish() {
        return None;
    }

    Some(wrap(script))
}

/// Assemble the wrapped form for a script already cleared by the gates.
///
/// Separate from [`try_wrap`] so the caller can rewrite the script's own
/// commands first and wrap the result: the gates answer for the script the host
/// submitted, the assembly runs on the one RTK hands back.
pub(crate) fn wrap(script: &str) -> String {
    format!(
        "rtk run --shell fish -c '{}'",
        escape_single_quoted(script.trim())
    )
}

/// True for fish's `(cmd)` command substitution, which the shared lexer reads
/// as a POSIX subshell and therefore does not refuse.
fn contains_fish_substitution(cmd: &str) -> bool {
    lexer::tokenize_with_newlines(cmd)
        .iter()
        .any(|token| token.kind == TokenKind::Shellism && matches!(token.value.as_str(), "(" | ")"))
}

/// True only when the command contains a fish-only marker at command position and
/// nothing a POSIX shell would need to parse it (see module docs for the lists).
pub(crate) fn is_unambiguous_fish(cmd: &str) -> bool {
    if cmd.contains('\0') {
        return false;
    }
    // A trailing comment is prose, not code: `echo $((1+2))  # x; and y` is a
    // POSIX command whose comment happens to contain a separator and an English
    // `and`. Every shell here agrees where a comment starts and ends, so the
    // span is dropped before anything is classified.
    let code = strip_comments(cmd);
    if has_unclosed_quote_or_escape(&code) {
        return false;
    }
    classify_tokens(&lexer::tokenize_with_newlines(&code))
}

/// Drop every unquoted comment — a word-initial `#` through the end of its line.
fn strip_comments(cmd: &str) -> String {
    let mut code = String::with_capacity(cmd.len());
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut at_word_start = true;
    let mut in_comment = false;

    for character in cmd.chars() {
        if in_comment {
            if character == '\n' {
                in_comment = false;
                at_word_start = true;
                code.push(character);
            }
            continue;
        }
        if escaped {
            escaped = false;
            at_word_start = false;
            code.push(character);
            continue;
        }
        match character {
            '\\' if quote != Some('\'') => {
                escaped = true;
                at_word_start = false;
            }
            '#' if quote.is_none() && at_word_start => {
                in_comment = true;
                continue;
            }
            '\'' | '"' => {
                quote = match quote {
                    Some(open) if open == character => None,
                    None => Some(character),
                    open => open,
                };
                at_word_start = false;
            }
            // A word also starts after an unquoted operator: both bash and fish
            // read `cmd;# note` as a command and a comment.
            ' ' | '\t' | '\n' | ';' | '|' | '&' if quote.is_none() => at_word_start = true,
            _ => at_word_start = false,
        }
        code.push(character);
    }

    code
}

/// True while a quote or an escape is still open at the end of `cmd`.
///
/// A script RTK cannot finish lexing is never classified: the marker it would
/// key on might be inside the unterminated string. Local to this module — the
/// shared gate has no such check, and wrapping is the only caller.
fn has_unclosed_quote_or_escape(cmd: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for character in cmd.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if matches!(character, '\'' | '"') {
            match quote {
                Some(open) if open == character => quote = None,
                None => quote = Some(character),
                _ => {}
            }
        }
    }

    quote.is_some() || escaped
}

fn classify_tokens(tokens: &[ParsedToken]) -> bool {
    let mut command_position = true;
    let mut has_fish_marker = false;

    for token in tokens {
        match token.kind {
            TokenKind::Operator | TokenKind::Pipe(_) => command_position = true,
            TokenKind::Shellism if token.value == "&" => command_position = true,
            // Fish has no backtick substitution: a script using one is POSIX,
            // so wrapping it would print the backticks instead of running the
            // command. The lexer emits an unquoted backtick as its own
            // shellism, quoted ones stay inside their Arg.
            TokenKind::Shellism if token.value == "`" => return false,
            // Heredocs (`<<`, `<<-`, `<<<` all tokenize with a `<<` prefix) do not
            // exist in fish — the script must be POSIX or malformed.
            TokenKind::Redirect if token.value.starts_with("<<") => return false,
            TokenKind::Arg => {
                // `[[ … ]]` is bash/zsh-only wherever it appears.
                if token.value == "[[" {
                    return false;
                }
                if command_position {
                    if POSIX_ONLY_KEYWORDS.contains(&token.value.as_str()) {
                        return false;
                    }
                    if FISH_ONLY_KEYWORDS.contains(&token.value.as_str()) {
                        has_fish_marker = true;
                    }
                    command_position = false;
                }
            }
            _ => {}
        }
    }

    has_fish_marker
}

/// Escape for single-quoted embedding: `'` → `'\''`. The idiom evaluates back to
/// the original script under sh/bash/zsh (quote-close, escaped quote, quote-open)
/// and under fish (`\'` is a literal quote; adjacent strings concatenate).
/// Newlines pass through untouched inside the quotes.
fn escape_single_quoted(script: &str) -> String {
    script.replace('\'', "'\\''")
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- is_unambiguous_fish: fish-only markers ------------------------------

    #[test]
    fn test_multiline_if_else_end_is_fish() {
        assert!(is_unambiguous_fish(
            "if test -d src\n  git status\nelse\n  echo missing\nend"
        ));
    }

    #[test]
    fn test_single_line_and_chain_is_fish() {
        assert!(is_unambiguous_fish("test -d src; and git status"));
        assert!(is_unambiguous_fish("test -d src; or echo missing"));
    }

    #[test]
    fn test_for_loop_with_fish_substitution_is_fish() {
        assert!(is_unambiguous_fish("for f in (ls)\n  echo $f\nend"));
    }

    #[test]
    fn test_begin_block_is_fish() {
        assert!(is_unambiguous_fish("begin; echo hi; end"));
    }

    #[test]
    fn test_switch_block_is_fish() {
        assert!(is_unambiguous_fish("switch $x\ncase a\necho a\nend"));
    }

    #[test]
    fn test_not_at_command_position_is_fish() {
        assert!(is_unambiguous_fish("not grep -q pattern file"));
    }

    #[test]
    fn test_function_block_detected_via_end() {
        assert!(is_unambiguous_fish(
            "function greet\n  echo hello $argv\nend"
        ));
    }

    // --- is_unambiguous_fish: POSIX vetoes -----------------------------------

    #[test]
    fn test_posix_if_then_fi_is_not_fish() {
        assert!(!is_unambiguous_fish("if [ -d src ]; then git status; fi"));
    }

    #[test]
    fn test_posix_for_do_done_is_not_fish() {
        assert!(!is_unambiguous_fish("for f in *.rs\ndo\n  echo $f\ndone"));
    }

    #[test]
    fn test_posix_case_esac_is_not_fish() {
        assert!(!is_unambiguous_fish(
            "case $x in\na) echo one ;;\nesac\nend"
        ));
    }

    #[test]
    fn test_heredoc_vetoes_fish() {
        assert!(!is_unambiguous_fish(
            "if test -d src\ncat <<EOF\nx\nEOF\nend"
        ));
    }

    #[test]
    fn test_double_bracket_vetoes_fish() {
        assert!(!is_unambiguous_fish("[[ -d src ]]\nend"));
    }

    // --- is_unambiguous_fish: ambiguous stays unclassified -------------------

    #[test]
    fn test_shared_keywords_without_marker_are_ambiguous() {
        assert!(!is_unambiguous_fish("if test -d src"));
        assert!(!is_unambiguous_fish("git status && cargo build"));
        assert!(!is_unambiguous_fish("git status"));
        assert!(!is_unambiguous_fish(""));
    }

    #[test]
    fn test_fish_keyword_as_argument_is_not_marker() {
        assert!(!is_unambiguous_fish("rg end src/"));
        assert!(!is_unambiguous_fish("printf '%s\\n' and"));
    }

    /// A comment is prose. Its separators and its English words are not code,
    /// and a POSIX command carrying one must not be handed to fish, which would
    /// fail to parse the live part and run nothing at all.
    #[test]
    fn test_comment_text_is_never_a_marker() {
        for cmd in [
            "echo $((1+2))  # x; and y",
            "echo \"${HOME}\"  # note; and more",
            "ls -la  # long listing; and hidden files",
            "ls # don't; and rm -rf /",
            // A word starts after an unquoted operator too, so the `#` glued to
            // one opens a comment just as a spaced `#` does.
            "echo ${HOME:-x};# note; and more",
            "ls -la|# note; and more",
            "sleep 1&# note; and more",
        ] {
            assert!(
                !is_unambiguous_fish(cmd),
                "comment must not classify: {cmd:?}"
            );
            assert!(try_wrap_gated(cmd, true).is_none(), "{cmd:?}");
        }
    }

    /// A `#` inside quotes is an argument, and a fish script may carry a
    /// comment of its own — neither changes the answer.
    #[test]
    fn test_comment_stripping_leaves_code_alone() {
        assert!(!is_unambiguous_fish("rg '#end' src/"));
        assert!(is_unambiguous_fish("test -d src; and git status # checked"));
    }

    #[test]
    fn test_quoted_fish_syntax_is_not_marker() {
        assert!(!is_unambiguous_fish("echo 'if x; and y; end'"));
        assert!(!is_unambiguous_fish("echo \"begin; end\""));
    }

    #[test]
    fn test_incomplete_quoting_is_never_classified() {
        assert!(!is_unambiguous_fish("if test -d src\necho 'unclosed\nend"));
        assert!(!is_unambiguous_fish("test -d src; and git status \\"));
    }

    #[test]
    fn test_backtick_substitution_is_not_fish() {
        // Fish has no backtick substitution: wrapping this would hand fish a
        // script that prints the backticks instead of running the command.
        assert!(!is_unambiguous_fish("echo `hostname`; and echo ok"));
        assert!(try_wrap_gated("echo `hostname`; and echo ok", true).is_none());
    }

    #[test]
    fn test_nul_byte_is_never_classified() {
        assert!(!is_unambiguous_fish("test -d src; and\0 git status; end"));
    }

    // --- escape_single_quoted -------------------------------------------------

    #[test]
    fn test_escape_plain_script_unchanged() {
        assert_eq!(escape_single_quoted("echo hi"), "echo hi");
    }

    #[test]
    fn test_escape_single_quotes() {
        assert_eq!(escape_single_quoted("echo 'a b'"), "echo '\\''a b'\\''");
    }

    #[test]
    fn test_escape_preserves_newlines_and_unicode() {
        assert_eq!(
            escape_single_quoted("echo 日本語\necho 'なか'"),
            "echo 日本語\necho '\\''なか'\\''"
        );
    }

    // --- try_wrap_gated --------------------------------------------------------

    #[cfg(not(windows))]
    #[test]
    fn test_wraps_multiline_fish_block() {
        assert_eq!(
            try_wrap_gated(
                "if test -d src\n  git status\nelse\n  echo missing\nend",
                true
            )
            .as_deref(),
            Some(
                "rtk run --shell fish -c 'if test -d src\n  git status\nelse\n  echo missing\nend'"
            )
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_wraps_script_containing_single_quotes() {
        assert_eq!(
            try_wrap_gated("echo 'a b'; and echo done", true).as_deref(),
            Some("rtk run --shell fish -c 'echo '\\''a b'\\''; and echo done'")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_single_backslash_script_is_wrapped() {
        // A lone backslash before an ordinary char is literal under both POSIX
        // and fish single-quotes, so an otherwise-fish script still wraps.
        assert_eq!(
            try_wrap_gated("printf '%s\\n' hi; and echo done", true).as_deref(),
            Some("rtk run --shell fish -c 'printf '\\''%s\\n'\\'' hi; and echo done'")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_double_backslash_script_is_not_wrapped() {
        // Classifies fish (`; and` marker) but contains `\\`, which fish
        // single-quotes collapse to one backslash — refuse so the wrap
        // round-trips byte-identically under a fish host layer.
        assert!(try_wrap_gated("echo '\\\\'; and echo ok", true).is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn test_backslash_quote_script_is_not_wrapped() {
        // Contains `\'`, which fish single-quotes turn into a literal quote.
        assert!(try_wrap_gated("echo \\'; and echo ok", true).is_none());
    }

    #[test]
    fn test_posix_and_ambiguous_scripts_are_not_wrapped() {
        assert!(try_wrap_gated("if [ -d src ]; then git status; fi", true).is_none());
        assert!(try_wrap_gated("git status && cargo build", true).is_none());
    }

    /// The wrap refuses what the permission gate refuses. A script whose
    /// segments cannot be decomposed — substitution, a file-target redirect, or
    /// fish's own `(cmd)` — keeps the defer behaviour instead of being emitted
    /// as an `rtk`-prefixed command.
    #[test]
    fn test_unattestable_scripts_are_not_wrapped() {
        for cmd in [
            "test -d src; and cat secrets.env > /tmp/leak",
            "test -d src; and echo $(whoami)",
            "test -d src; and echo `whoami`",
            "for f in (ls)\n  echo $f\nend",
            "begin; git status > out.txt; end",
        ] {
            assert!(
                try_wrap_gated(cmd, true).is_none(),
                "unattestable script must defer: {cmd:?}"
            );
        }
    }

    /// An apostrophe in a comment opens the shared lexer's quote state and
    /// swallows the rest of the script into one argument, so a gate reading the
    /// raw text sees neither the redirect nor the substitution that follows.
    /// The gates read the same stripped code the classification does.
    #[test]
    fn test_a_comment_quote_cannot_blind_the_gates() {
        for cmd in [
            "test -d src; and git status # don't\ncat secrets.env > /tmp/leak",
            "test -d src; and git status # don't\necho SUBST=(whoami)",
            "test -d src; and git status # 5\" wide\ncat secrets.env > /tmp/leak",
        ] {
            assert!(
                try_wrap_gated(cmd, true).is_none(),
                "a comment must not hide what follows it: {cmd:?}"
            );
        }
    }

    #[test]
    fn test_missing_fish_binary_defers() {
        assert!(try_wrap_gated("test -d src; and git status", false).is_none());
    }

    #[test]
    fn test_rtk_prefixed_command_is_not_wrapped() {
        // `and` at command position would classify without the rtk gate.
        assert!(try_wrap_gated("rtk git status; and echo ok", true).is_none());
    }

    #[test]
    fn test_explicit_shell_wrapper_is_not_wrapped() {
        assert!(try_wrap_gated("fish -c 'if x; end'; and echo ok", true).is_none());
        assert!(try_wrap_gated("fish -c 'if x; end'", true).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_never_wraps() {
        assert!(try_wrap_gated("test -d src; and git status", true).is_none());
    }
}
