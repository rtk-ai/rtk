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
/// shell `-c` wrapper), when the comment scan refuses because the two shells
/// disagree where a comment starts, when the permission gate could not
/// decompose the script (substitution, a file-target redirect) or it carries
/// fish's own `(cmd)` substitution or `&|` pipe, when the script contains a
/// fish-divergent backslash sequence (`\\` or `\'`), on Windows, when
/// `hooks.wrap_fish_scripts` is disabled, or when no `fish` binary is
/// resolvable.
///
/// The cheap, pure classification runs first; the config read and the `fish`
/// PATH probe run only once the command is classified fish. So common non-fish
/// commands reaching the unattestable gate (`echo $(date)`, `git log > out`,
/// POSIX blocks) do zero I/O here.
pub fn try_wrap(cmd: &str) -> Option<String> {
    try_wrap_with_probes(
        cmd,
        || crate::core::config::cached_config().hooks.wrap_fish_scripts,
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
    let code = strip_comments(script)?;
    if lexer::contains_unattestable_construct(&code) || contains_fish_substitution(&code) {
        return None;
    }
    // `&|` pipes stdout and stderr in fish, and `&` opens the same kind of
    // pair in zsh. The shared lexer reads each as a background `&` followed by
    // a pipe or a negation, so the inner rewrite re-emits the two spaced —
    // `rtk git status & | cat` — which fish rejects outright. The wrap
    // promises the argument carries the same commands, so a script RTK cannot
    // rewrite without changing what it runs is left alone, exactly as the
    // `fish -c` wrapper path leaves it.
    if lexer::opens_disown_pair(&code) {
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

/// True for a command [`wrap`] produced.
///
/// The wrap's promise is that its strongest outcome is `Ask`: the script
/// travels unattested, and the verdict it was judged against was read as bash.
/// Anything that may relax an ask — `hooks::decision::ApprovalOwner` — asks
/// this first.
pub(crate) fn is_wrapped(cmd: &str) -> bool {
    cmd.starts_with("rtk run --shell fish -c '")
}

/// The script's words, dequoted and grouped by the command they belong to,
/// read from the same comment-stripped code the classification and the gates
/// read.
///
/// Callers that answer a security question about the script — "could a deny
/// rule match anything in here" — must read these words rather than the raw
/// text: an apostrophe in a comment opens the shared lexer's quote state and
/// swallows everything after it into one argument.
///
/// Grouping matters as much as dequoting. `shell_split` alone merges a word
/// with the operator glued to it, so `not git push; echo blocked` yields
/// `push;` and a rule naming `git push` never matches. Splitting on command
/// separators first makes a run of words a run of one command's argv.
///
/// `None` means *the words cannot be read*, which a security caller must treat
/// as "assume the worst":
/// - the comment scan refuses (the two shells disagree where a comment starts);
/// - the code carries a `\r`, which bash keeps inside the word and fish does
///   not, so `pwd\r` here is `pwd` there;
/// - the code carries a backslash outside single quotes. Bash and fish resolve
///   those differently in both directions — bash elides `\<newline>` to join
///   words while this lexer keeps the newline, and fish expands `\x70` to `p`
///   while bash leaves `x70` — so the word this reads is not the word fish
///   runs. (`\\` and `\'`, the single-quote divergences, are refused outright
///   by [`try_wrap`] before this is ever consulted.)
/// - the code carries a character fish resolves at run time outside single
///   quotes — `$`, `{`, `*`, `[` or `~`. `set -l r echo; $r DENIED` runs
///   `echo DENIED` and `/bin/ec*o` runs `/bin/echo`; no reading of the text
///   says so.
pub(crate) fn code_word_runs(cmd: &str) -> Option<Vec<Vec<String>>> {
    let code = strip_comments(cmd)?;
    if code.contains('\r') || has_unreadable_word(&code) {
        return None;
    }
    Some(
        command_texts(&code)
            .iter()
            .map(|segment| {
                // An empty argument (`echo '' DENIED`) is not a word any
                // matcher can see: the permission gate's own normalization
                // drops it, so the words either side are adjacent there and
                // have to be adjacent here.
                lexer::shell_split(segment)
                    .into_iter()
                    .filter(|word| !word.is_empty())
                    .collect()
            })
            .collect(),
    )
}

/// True for a character outside single quotes whose word this lexer reads
/// differently from the way fish runs it.
///
/// Single-quoted text is literal in both shells once [`try_wrap`] has refused
/// `\\` and `\'`. Everything else is fair game, double quotes included: both
/// shells resolve escapes and expansions there and they do not resolve the
/// same ones, so only `'…'` suppresses the refusal. The test is the character,
/// not whether it would expand — `echo "{a}"` is literal in fish, but deciding
/// that requires being fish.
///
/// - `\` — bash elides `\<newline>`, fish expands `\x70` to `p`;
/// - `$` — an expansion, whose value is the command fish runs, not the text;
/// - `{` — a brace list, which fish expands into several words;
/// - `*` and `[` — globs, which fish resolves against the filesystem, so
///   `/bin/ec*o` is `/bin/echo` by the time it runs;
/// - `~` *at the start of a word* — home-directory expansion, likewise
///   resolved before the command runs. Fish expands it nowhere else, so
///   `git diff HEAD~3 HEAD` reads as itself.
fn has_unreadable_word(code: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut word_start = true;
    for c in code.chars() {
        match c {
            '\'' | '"' if quote.is_none() => quote = Some(c),
            _ if quote == Some(c) => quote = None,
            '\\' | '$' | '{' | '*' | '[' if quote != Some('\'') => return true,
            '~' if word_start && quote.is_none() => return true,
            _ => {}
        }
        // Quoted text is one word however it is spaced, and an operator ends
        // the word before it as surely as a space does.
        word_start =
            quote.is_none() && matches!(c, ' ' | '\t' | '\n' | ';' | '|' | '&' | '(' | ')');
    }
    false
}

/// The code split where a command ends, with redirects removed rather than
/// truncated at.
///
/// `lexer::split_for_permissions` is the gate's own segmenter and cuts each
/// segment at its first redirect, which is right for a gate reading the
/// command *before* the redirect. It is wrong for reading argv: fish keeps
/// `DENIED` in `echo A 2>&1 DENIED` as an argument of `echo`, so a segment
/// ending at `2>&1` hides it. Here the redirect and its operand are dropped
/// and the words either side belong to the same command, as the shell runs it.
///
/// Words glued together in the source stay glued (`'a'b` is one word); a
/// dropped redirect always separates, since a redirect ends a word in both
/// shells.
fn command_texts(code: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut last_end: Option<usize> = None;

    let tokens = lexer::tokenize_with_newlines(code);
    let mut i = 0;
    while let Some(token) = tokens.get(i) {
        i += 1;
        let end = token.offset + token.value.len();
        match token.kind {
            TokenKind::Operator | TokenKind::Pipe(_) => {
                segments.push(std::mem::take(&mut current));
                last_end = None;
            }
            TokenKind::Shellism if matches!(token.value.as_str(), "&" | "(" | ")") => {
                segments.push(std::mem::take(&mut current));
                last_end = None;
            }
            TokenKind::Redirect => {
                i = skip_redirect_operand(&tokens, i, &token.value);
                last_end = None;
            }
            _ => {
                if !current.is_empty() && last_end != Some(token.offset) {
                    current.push(' ');
                }
                current.push_str(&token.value);
                last_end = Some(end);
            }
        }
    }
    segments.push(current);

    segments.retain(|segment| !segment.trim().is_empty());
    segments
}

/// Index of the first token after a redirect's operand.
///
/// An fd duplication carries its target inside the operator (`2>&1`), so what
/// follows is an argument of the command, not a redirect target. Every other
/// redirect takes the next word, glued (`2>/dev/null`) or spaced
/// (`2> /dev/null`) — and only that word, since `> f arg` redirects to `f` and
/// passes `arg` to the command.
fn skip_redirect_operand(tokens: &[ParsedToken], mut i: usize, redirect: &str) -> usize {
    if !lexer::redirect_takes_operand(redirect) {
        return i;
    }
    let ends_operand = |part: &ParsedToken| {
        matches!(
            part.kind,
            TokenKind::Operator | TokenKind::Pipe(_) | TokenKind::Shellism
        )
    };
    let Some(first) = tokens.get(i).filter(|part| !ends_operand(part)) else {
        return i;
    };
    // A word can be several tokens (`>$HOME/x`), which the lexer reports with
    // no gap between them.
    let mut operand_end = first.offset + first.value.len();
    i += 1;
    while let Some(part) = tokens.get(i) {
        if part.offset != operand_end || ends_operand(part) {
            break;
        }
        operand_end = part.offset + part.value.len();
        i += 1;
    }
    i
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
    // `and`. The span is dropped before anything is classified — and where the
    // two shells disagree about where it starts, nothing is classified at all.
    let Some(code) = strip_comments(cmd) else {
        return false;
    };
    if has_unclosed_quote_or_escape(&code) {
        return false;
    }
    classify_tokens(&lexer::tokenize_with_newlines(&code))
}

/// Drop every unquoted comment — a word-initial `#` through the end of its line
/// — or report that the two shells do not agree where the comment starts.
///
/// `&` is the one boundary character they read differently. Since fish 3.0 it
/// is job control only at the end of a job, so a `#` glued to it continues the
/// word rather than opening a comment:
///
/// ```text
/// bash -c 'echo A&#y'   ->  A        (comment)
/// fish -c 'echo A&#y'   ->  A&#y     (one literal word)
/// ```
///
/// Taking either reading is wrong: bash's hides whatever follows from the gates
/// while fish executes it, and fish's re-admits the comment text as code. The
/// scanner refuses instead, and the script keeps the host's own behaviour.
fn strip_comments(cmd: &str) -> Option<String> {
    let mut code = String::with_capacity(cmd.len());
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut at_word_start = true;
    let mut in_comment = false;
    // Unquoted, unescaped `&` immediately before the character being read: one
    // is job control, two are the `and` operator, and an escaped or quoted `&`
    // is an ordinary character that resets the run.
    let mut ampersands = 0usize;

    for character in cmd.chars() {
        if in_comment {
            if character == '\n' {
                in_comment = false;
                at_word_start = true;
                // The run belonged to the line the comment closed.
                ampersands = 0;
                code.push(character);
            }
            continue;
        }
        if escaped {
            escaped = false;
            at_word_start = false;
            ampersands = 0;
            code.push(character);
            continue;
        }
        match character {
            '\\' if quote != Some('\'') => {
                escaped = true;
                at_word_start = false;
                ampersands = 0;
            }
            '#' if quote.is_none() && at_word_start => {
                // `&&#` is an operator followed by a comment in both shells;
                // a lone `&#` is where they diverge.
                if ampersands == 1 {
                    return None;
                }
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
                ampersands = 0;
            }
            '&' if quote.is_none() => {
                at_word_start = true;
                ampersands += 1;
            }
            // A word also starts after an unquoted operator: both bash and fish
            // read `cmd;# note` as a command and a comment.
            ' ' | '\t' | '\n' | ';' | '|' if quote.is_none() => {
                at_word_start = true;
                ampersands = 0;
            }
            _ => {
                at_word_start = false;
                ampersands = 0;
            }
        }
        code.push(character);
    }

    Some(code)
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

    /// `&` is the one boundary character bash and fish read differently: a `#`
    /// glued to it opens a comment in bash and continues the word in fish. Bash's
    /// reading would hide a redirect from the gates that fish then executes;
    /// fish's would re-admit comment text as code. Neither is safe, so the
    /// script is left to the host.
    #[test]
    fn test_a_comment_glued_to_an_ampersand_is_never_classified() {
        for cmd in [
            "test -d src; and echo HIT&#y > /tmp/LEAK",
            "test -d src; and echo HIT&#y (whoami)",
            "if test -d src\n  echo A&#y > /tmp/LEAK\nend",
            "sleep 1&# note; and more",
        ] {
            assert!(
                !is_unambiguous_fish(cmd),
                "dialects disagree, so nothing is classified: {cmd:?}"
            );
            assert!(try_wrap_gated(cmd, true).is_none(), "{cmd:?}");
        }
    }

    /// `&&` and `||` are operators in both shells, so a `#` after them is a
    /// comment in both — those keep the ordinary strip.
    #[test]
    fn test_operator_pairs_still_end_a_word() {
        // The marker sits *outside* the comment, so stripping is what leaves a
        // classifiable script: these rows fail if the refusal widens to `&&`,
        // `||` or a spaced `&`, which both shells read the same way.
        assert!(is_unambiguous_fish("test -d src; and git status &&# note"));
        assert!(is_unambiguous_fish("test -d src; and git status ||# note"));
        assert!(is_unambiguous_fish("test -d src; and git status & # note"));

        // An escaped `&` is an ordinary character, so the `&` after it is the
        // lone one the shells disagree about — that still refuses.
        assert!(!is_unambiguous_fish(
            "test -d src; and echo \\&&#y > /tmp/LEAK"
        ));
    }

    /// The ampersand run belongs to the line it was read on: a `&&` that ended
    /// in a comment must not make the next line's lone `&#` read as a pair.
    #[test]
    fn test_the_ampersand_run_does_not_cross_a_comment() {
        assert!(!is_unambiguous_fish(
            "true; and echo ok &&# comment\n&#y > /tmp/LEAK"
        ));
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

    /// The words a security caller reads are one command's argv, with the
    /// redirect removed rather than truncated at — fish keeps `DENIED` as an
    /// argument of `echo`, and a reader that stops at `2>&1` never sees it.
    #[test]
    fn test_code_word_runs_keeps_argv_across_a_redirect() {
        let argv = |words: &[&str]| {
            Some(vec![
                words
                    .iter()
                    .map(|word| word.to_string())
                    .collect::<Vec<_>>(),
            ])
        };
        // An fd duplication carries its target; a redirect's own operand goes,
        // glued or spaced, and the words either side stay with the command.
        assert_eq!(
            code_word_runs("not echo A 2>&1 DENIED"),
            argv(&["not", "echo", "A", "DENIED"])
        );
        assert_eq!(
            code_word_runs("not echo 2>/dev/null DENIED"),
            argv(&["not", "echo", "DENIED"])
        );
        assert_eq!(
            code_word_runs("not echo 2> /dev/null DENIED"),
            argv(&["not", "echo", "DENIED"])
        );
    }

    /// An empty argument is a word to the lexer and nothing at all to the
    /// matcher, which normalizes it away — so the words either side have to be
    /// adjacent here too, or a rule naming them both misses.
    #[test]
    fn test_code_word_runs_drops_empty_arguments() {
        assert_eq!(
            code_word_runs("not echo '' A DENIED"),
            Some(vec![vec![
                "not".to_string(),
                "echo".to_string(),
                "A".to_string(),
                "DENIED".to_string()
            ]])
        );
    }

    /// One list per command, so a run of words is an argv a shell assembles:
    /// `push;` is not a word, and `echo` belongs to the next command.
    #[test]
    fn test_code_word_runs_groups_by_command() {
        assert_eq!(
            code_word_runs("not git push; echo blocked"),
            Some(vec![
                vec!["not".to_string(), "git".to_string(), "push".to_string()],
                vec!["echo".to_string(), "blocked".to_string()]
            ])
        );
    }

    /// `None` is "these words cannot be read", the answer a security caller
    /// has to treat as the worst case: an escape the two shells resolve
    /// differently, a `\r`, or anything fish resolves at run time — a
    /// variable, a brace list, a glob, a `~`.
    #[test]
    fn test_code_word_runs_refuses_what_it_cannot_read() {
        for cmd in [
            "if not pwd\r\n  echo BODY\r\nend",
            "if not \\x70wd\n  echo BODY\nend",
            "if not p\\\nwd\n  echo BODY\nend",
            "set -l runner echo; $runner DENIED; and true",
            "set -l a p; set -l b wd; $a$b; and true",
            "{echo,ls} DENIED; and true",
            "echo \"it's $HOME\"; and true",
            "not /bin/ec*o DENIED",
            "not /bin/ec[h]o DENIED",
            "not echo ~root",
        ] {
            assert_eq!(code_word_runs(cmd), None, "{cmd:?}");
        }
        // Single-quoted text is literal in both shells, so it reads fine.
        assert_eq!(
            code_word_runs("echo '$HOME {a,b}'; and true"),
            Some(vec![
                vec!["echo".to_string(), "$HOME {a,b}".to_string()],
                vec!["and".to_string(), "true".to_string()]
            ])
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

    /// `&|` is fish's stdout-and-stderr pipe. The shared lexer reads it as a
    /// background `&` plus a pipe and the rewrite re-emits the two spaced,
    /// which fish rejects — so a wrap would hand back a script fish cannot
    /// parse. The `fish -c` wrapper path refuses the same shape.
    #[cfg(not(windows))]
    #[test]
    fn test_fish_stderr_pipe_is_not_wrapped() {
        for cmd in [
            "git status &| cat; and true",
            "git log -5 &| head -3; and true",
            "begin\n  git status &| cat\nend",
        ] {
            assert!(try_wrap_gated(cmd, true).is_none(), "{cmd:?}");
        }
        // A background `&` followed by a *separate* pipeline still wraps: the
        // two are only the fish pipe when they are adjacent.
        assert!(try_wrap_gated("sleep 1 & git status | cat; and true", true).is_some());
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
