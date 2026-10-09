//! rtk's own command line: whether a command runs rtk, and whether it asks rtk
//! to run another command unfiltered.

use super::lexer::{Word, resolve_word_text};

/// A command that runs rtk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RtkInvocation {
    /// The second word is `proxy`: rtk runs the command after it unfiltered.
    pub(crate) proxy: bool,
}

/// `Some` when the command made of `words` runs rtk: its first word is `rtk`
/// once its quotes and escapes are removed, as bash reads it before looking the
/// command up, so `'rtk'`, `"rtk"` and `\rtk` run rtk too. The second word is
/// read the same way.
pub(crate) fn rtk_invocation(words: &[Word<'_>]) -> Option<RtkInvocation> {
    let mut texts = words.iter().map(|word| resolve_word_text(word.text));
    (texts.next()? == "rtk").then(|| RtkInvocation {
        proxy: texts.next().is_some_and(|second| second == "proxy"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cmdline::lexer::{tokenize, words};

    fn invocation(cmd: &str) -> Option<RtkInvocation> {
        rtk_invocation(&words(cmd, &tokenize(cmd)))
    }

    const RTK: Option<RtkInvocation> = Some(RtkInvocation { proxy: false });
    const PROXY: Option<RtkInvocation> = Some(RtkInvocation { proxy: true });

    /// The first word decides, as bash ends it and removes its quoting.
    #[test]
    fn test_rtk_invocation_reads_the_first_word_as_bash_runs_it() {
        for (cmd, expected) in [
            ("rtk git status", RTK),
            ("rtk", RTK),
            ("rtk\tgit status", RTK),
            ("  rtk  ls", RTK),
            ("rtk\nls", RTK),
            ("rtk proxyfoo x", RTK),
            ("'rtk' git status", RTK),
            ("\"rtk\" git status", RTK),
            ("\\rtk git status", RTK),
            ("r'tk' git status", RTK),
            ("rtk proxy git log", PROXY),
            ("rtk\tproxy git status", PROXY),
            ("rtk 'proxy' git log", PROXY),
            ("\\rtk \\proxy git log", PROXY),
            ("rtkx ls", None),
            ("xrtk ls", None),
            ("rtk\r ls", None),
            ("'rtk git' status", None),
            ("rtk\\ git status", None),
            ("rtk;ls", None),
            ("git status", None),
            ("", None),
        ] {
            assert_eq!(invocation(cmd), expected, "{cmd:?}");
        }
    }
}
