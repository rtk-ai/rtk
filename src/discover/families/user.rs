//! The user's own `[hooks].transparent_prefixes`: wrappers that change how a
//! command runs and not which one (`docker exec mycontainer`, `direnv exec .`,
//! `poetry run`). They are configuration text rather than a family of the
//! built-in tables, so all that is here is how an entry is read; the walker
//! matches each as literal words
//! ([`crate::core::cmdline::walker::peel_literal_prefix`]) and the rewrite
//! puts `rtk` behind it.

use crate::core::cmdline::bash_grammar::reserved_word;
use crate::core::cmdline::lexer::{lexes_into_whole_words, split_ifs, trim_ifs};
use crate::core::deferred;

/// The entries the walker is handed: without the `$IFS` bytes at either end,
/// with what cannot be matched against command words refused, longest first
/// (so `docker exec mycontainer` wins over `docker`) and without repeats.
///
/// An entry is matched as words of the command it wraps, so it has to lex
/// into whole words itself: `x 'a` would open a quote that runs into the
/// command after it, and no command could be split where the entry claims to
/// end. Nor may it start with a bash reserved word (`coproc`, `time`, `if`,
/// ...), which bash reads as grammar where a command starts and never as the
/// name of a program a prefix could wrap. Such an entry is refused with a
/// warning where the configuration is read, once per process that reads it:
/// every hook call, and every `rtk rewrite`.
pub(crate) fn normalize_transparent_prefixes(prefixes: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = prefixes
        .iter()
        .map(|prefix| trim_ifs(prefix))
        .filter(|prefix| !prefix.is_empty())
        .filter(|prefix| {
            let whole = lexes_into_whole_words(prefix);
            if !whole {
                deferred::warn(format_args!(
                    "[rtk] warning: ignoring transparent_prefixes entry '{prefix}': \
                     it has an unclosed quote or a trailing backslash"
                ));
            }
            whole
        })
        .filter(|prefix| {
            let reserved = split_ifs(prefix)
                .next()
                .filter(|word| reserved_word(word).is_some());
            if let Some(word) = reserved {
                deferred::warn(format_args!(
                    "[rtk] warning: ignoring transparent_prefixes entry '{prefix}': \
                     it starts with the bash reserved word '{word}'"
                ));
            }
            reserved.is_none()
        })
        .map(str::to_string)
        .collect();

    normalized.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    normalized.dedup();
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cmdline::walker::Position;
    use crate::discover::families::{AFTER_USER_PREFIX, LayerKind, peel_text};

    fn norm(entries: &[&str]) -> Vec<String> {
        normalize_transparent_prefixes(&entries.iter().map(|e| e.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn trims_orders_longest_first_and_dedups() {
        assert_eq!(
            norm(&["  poetry run ", "docker exec c", "docker", "docker", ""]),
            ["docker exec c", "poetry run", "docker"]
        );
    }

    /// Bash splits words on space, tab and newline only, and so does the
    /// trim: a non-breaking space at either end is part of the entry.
    #[test]
    fn trims_only_what_bash_splits_words_on() {
        assert_eq!(norm(&["\t\ndocker exec c \n"]), ["docker exec c"]);
        assert_eq!(norm(&["\u{a0}docker"]), ["\u{a0}docker"]);
    }

    #[test]
    fn keeps_a_prefix_with_balanced_quotes() {
        assert_eq!(norm(&["ssh -t \"my host\""]), ["ssh -t \"my host\""]);
        assert_eq!(norm(&["sh -c 'a b'"]), ["sh -c 'a b'"]);
    }

    #[test]
    fn refuses_an_entry_that_does_not_lex_into_whole_words() {
        for entry in ["x 'a", "x \"a", "x a\\", "sh -c \""] {
            assert!(norm(&[entry]).is_empty(), "{entry:?}");
        }
        // The rest of the list is kept.
        assert_eq!(norm(&["x 'a", "poetry run"]), ["poetry run"]);
    }

    /// Bash reads a reserved word where a command starts as grammar, so an
    /// entry that starts with one names no program and is refused, with a
    /// warning. Quoted, the word is an ordinary one.
    #[test]
    fn refuses_an_entry_that_starts_with_a_reserved_word() {
        for entry in [
            "coproc",
            "coproc NAME",
            "time",
            "time -p",
            "if",
            "while true",
        ] {
            let (kept, warnings) = deferred::capture(|| norm(&[entry, "poetry run"]));
            assert_eq!(kept, ["poetry run"], "{entry:?}");
            let word = entry.split(' ').next().unwrap_or_default();
            assert_eq!(
                warnings,
                [format!(
                    "[rtk] warning: ignoring transparent_prefixes entry '{entry}': \
                     it starts with the bash reserved word '{word}'"
                )],
                "{entry:?}"
            );
        }
        assert_eq!(
            norm(&["\"coproc\"", "timeout 5", "nice -n 19 time"]),
            ["nice -n 19 time", "timeout 5", "\"coproc\""]
        );
    }

    #[test]
    fn a_refused_entry_is_warned_about() {
        let (kept, warnings) = deferred::capture(|| norm(&["x 'a", "poetry run"]));
        assert_eq!(kept, ["poetry run"]);
        assert_eq!(
            warnings,
            [
                "[rtk] warning: ignoring transparent_prefixes entry 'x 'a': \
              it has an unclosed quote or a trailing backslash"
            ]
        );
    }

    #[test]
    fn an_even_run_of_trailing_backslashes_escapes_itself() {
        assert_eq!(norm(&["x a\\\\"]), ["x a\\\\"]);
    }

    fn peel(cmd: &str, prefixes: &[&str]) -> Option<(String, String)> {
        peel_text(cmd, &[], &norm(prefixes), Position::START).map(|p| {
            assert_eq!(p.kind, LayerKind::UserPrefix, "{cmd:?}");
            assert_eq!(p.next, AFTER_USER_PREFIX, "{cmd:?}");
            (p.prefix.to_string(), p.rest.to_string())
        })
    }

    #[test]
    fn accepts_a_prefix_ending_on_a_word_boundary() {
        assert_eq!(
            peel("docker exec c git status", &["docker exec c"]),
            Some(("docker exec c".into(), "git status".into()))
        );
        assert_eq!(
            peel("sh -c 'a b' git status", &["sh -c 'a b'"]),
            Some(("sh -c 'a b'".into(), "git status".into()))
        );
    }

    #[test]
    fn rejects_a_prefix_that_ends_inside_a_word() {
        assert_eq!(peel("docker execc git status", &["docker exec"]), None);
        assert_eq!(peel("docker exec'c' git status", &["docker exec"]), None);
        assert_eq!(
            peel("docker exec\u{a0}c git status", &["docker exec"]),
            None
        );
    }

    #[test]
    fn a_prefix_with_nothing_after_it_is_the_callers_to_refuse() {
        // The walker reports where the layer ends; whether an empty command
        // behind it is acceptable is the rewrite's call.
        assert_eq!(
            peel("poetry run", &["poetry run"]),
            Some(("poetry run".into(), "".into()))
        );
    }

    #[test]
    fn what_follows_a_prefix_may_be_an_assignment_but_is_no_pipeline_start() {
        let prefixes = norm(&["sudo"]);
        for pos in [Position::START, Position::STAGE, Position::INNER] {
            let next = peel_text("sudo A=1 git", &[], &prefixes, pos).map(|p| p.next);
            assert_eq!(next, Some(Position::STAGE), "{pos:?}");
        }
    }

    #[test]
    fn a_prefix_is_written_as_the_command_writes_it() {
        assert_eq!(peel("poetry  run git status", &["poetry run"]), None);
    }
}
