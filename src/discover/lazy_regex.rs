//! A `^`-anchored regex that is compiled only when a haystack could match it.
//!
//! The hook runs in a fresh process for every command an agent runs, so each
//! regex it compiles is paid for on every call. Compiling all rule patterns and
//! a user's `exclude_commands` up front cost more than the rest of a hook call;
//! parsing them is a fraction of that, and the parse already says what every
//! match must start with.

use regex::Regex;
use regex_syntax::hir::{Hir, Look, literal::Extractor};
use std::sync::OnceLock;

pub(crate) struct LazyRegex {
    pattern: String,
    /// Literals every match starts with; `None` when the pattern does not
    /// pin them down, and then any haystack may match.
    starts: Option<Vec<Vec<u8>>>,
    compiled: OnceLock<Result<Regex, regex::Error>>,
}

impl LazyRegex {
    /// Parses `pattern` now, so a malformed one is reported at once.
    pub(crate) fn new(pattern: &str) -> Result<Self, Box<regex_syntax::Error>> {
        let hir = regex_syntax::parse(pattern).map_err(Box::new)?;
        Ok(Self {
            pattern: pattern.to_string(),
            starts: match_starts(&hir),
            compiled: OnceLock::new(),
        })
    }

    /// False only when no match is possible; true means "compile and see".
    pub(crate) fn may_match(&self, haystack: &str) -> bool {
        self.starts.as_ref().is_none_or(|starts| {
            starts
                .iter()
                .any(|start| haystack.as_bytes().starts_with(start))
        })
    }

    pub(crate) fn compiled(&self) -> Result<&Regex, &regex::Error> {
        self.compiled_or_report(|_| {})
    }

    /// [`Self::compiled`], with `report` run once if the compile fails.
    pub(crate) fn compiled_or_report(
        &self,
        report: impl FnOnce(&regex::Error),
    ) -> Result<&Regex, &regex::Error> {
        self.compiled
            .get_or_init(|| Regex::new(&self.pattern).inspect_err(report))
            .as_ref()
    }
}

/// The prefix literals of a pattern anchored at the start of the haystack:
/// every match begins at offset 0 with one of them.
fn match_starts(hir: &Hir) -> Option<Vec<Vec<u8>>> {
    if !hir.properties().look_set_prefix().contains(Look::Start) {
        return None;
    }
    let literals = Extractor::new().extract(hir);
    Some(
        literals
            .literals()?
            .iter()
            .map(|literal| literal.as_bytes().to_vec())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agrees_with_regex(pattern: &str, haystacks: &[&str]) {
        let lazy = LazyRegex::new(pattern).unwrap();
        let regex = Regex::new(pattern).unwrap();
        for haystack in haystacks {
            if regex.is_match(haystack) {
                assert!(
                    lazy.may_match(haystack),
                    "{pattern:?} matches {haystack:?} but the gate refused it"
                );
            }
        }
    }

    #[test]
    fn gate_never_refuses_a_real_match() {
        let haystacks = [
            "git status",
            "yadm -C x diff",
            "npx tsc --noEmit",
            "pnpm exec tsc",
            "tsc",
            "./gradlew build",
            "gradle test",
            "ls",
            "",
            "Git status",
        ];
        for pattern in [
            r"^(?:git|yadm)\s+(?:-[Cc]\s+\S+\s+)*(status|log|diff)(?:\s|$)",
            r"^((p?np(m|x)|p?npm\s+(exec|run|run-script)|npm\s+(rum|urn|x)|pnpm\s+dlx)\s+)?tsc(\s|$)",
            r"^(?:\./gradlew|gradlew\.bat|gradlew|gradle)(?:\s+(test|build))?(\s|$)",
            r"^ls(\s|$)",
            r"^(?i)git",
            r"^(\S*/)?gradlew? (\S+ )*build",
        ] {
            agrees_with_regex(pattern, &haystacks);
        }
    }

    #[test]
    fn anchored_literal_prefix_refuses_other_commands() {
        let lazy = LazyRegex::new(r"^git\s+status").unwrap();
        assert!(!lazy.may_match("cargo build"));
        assert!(lazy.may_match("git status"));
        assert!(lazy.compiled.get().is_none(), "the gate must not compile");
    }

    #[test]
    fn unanchored_or_open_patterns_always_may_match() {
        for pattern in [r"git", r"^git|cargo", r"^\S*gradlew", r"^.*"] {
            let lazy = LazyRegex::new(pattern).unwrap();
            assert!(lazy.may_match("anything at all"), "{pattern:?}");
        }
    }
}
