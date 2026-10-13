//! Glob expansion for the file operands a Windows caller left unexpanded.
//!
//! bash expands `*.rs` before rtk starts; PowerShell and cmd do not, so there
//! the operand reaches rtk as written. The MSYS child (`ls`, `wc`, `find`,
//! `grep`) used to expand it through Cygwin's `globify`, but `ChildCommand`
//! quotes it now so that a pattern reaches the child literally, which is right
//! for a regex or a `-name` value and wrong for a file operand. So each command
//! expands its own operands here, at the point where its parser knows which
//! arguments name files.
//!
//! The rules follow what `globify` did, as measured against Git for Windows'
//! `ls`: `*`, `?` and `[...]` match within one path component, case-sensitively;
//! a leading `.` is matched only by a literal `.`; `{a,b}` expands; a leading `~`
//! names the home directory; and an operand that matches nothing is kept as
//! written, so the child reports it. Two differences are deliberate. `\`
//! separates components, as in the paths a Windows caller writes (`globify` read
//! it as an escape). And an operand that names an existing path is never
//! expanded, so a name bash already produced, such as `a[1].txt`, is not globbed
//! a second time.
//!
//! On Unix the caller's shell has already expanded, so operands pass through.

#[cfg(any(windows, test))]
use std::path::Path;

/// `operands`, each glob expanded into the paths it matches, in order.
///
/// Call it only on arguments the command reads as file operands: a pattern, a
/// flag or a flag's value must reach the child as written.
pub fn expand_operands<I, S>(operands: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    #[cfg(windows)]
    {
        let cwd = crate::core::user_dirs::current_dir().ok();
        let home = crate::core::user_dirs::home();
        operands
            .into_iter()
            .flat_map(|operand| match &cwd {
                Some(cwd) => expand(operand.as_ref(), cwd, home.as_deref()),
                None => vec![operand.as_ref().to_string()],
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        operands
            .into_iter()
            .map(|operand| operand.as_ref().to_string())
            .collect()
    }
}

#[cfg(any(windows, test))]
fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

/// `spelled` resolved against `cwd` for the filesystem. Windows reads both
/// separators; elsewhere, where only the tests run this, `\` is turned into `/`
/// so the same assertions hold.
#[cfg(any(windows, test))]
fn on_disk(cwd: &Path, spelled: &str) -> std::path::PathBuf {
    if cfg!(windows) {
        cwd.join(spelled)
    } else {
        cwd.join(spelled.replace('\\', "/"))
    }
}

#[cfg(any(windows, test))]
fn has_wildcard(text: &str) -> bool {
    text.contains(['*', '?', '['])
}

/// Expands one operand against `cwd`, the directory relative operands resolve
/// against, with `home` standing in for a leading `~`.
#[cfg(any(windows, test))]
fn expand(operand: &str, cwd: &Path, home: Option<&Path>) -> Vec<String> {
    let is_pattern = has_wildcard(operand) || operand.contains('{') || operand.starts_with('~');
    if !is_pattern || on_disk(cwd, operand).symlink_metadata().is_ok() {
        return vec![operand.to_string()];
    }
    braces(operand)
        .into_iter()
        .flat_map(|word| {
            let word = tilde(&word, home);
            if !has_wildcard(&word) {
                return vec![word];
            }
            let mut matches = glob(&word, cwd);
            if matches.is_empty() {
                vec![word]
            } else {
                matches.sort();
                matches
            }
        })
        .collect()
}

/// Brace expansion: `{a,b}` becomes one word per alternative, nested and
/// repeated groups included. A group with no top-level comma (`{x}`) or no
/// closing brace stays literal.
#[cfg(any(windows, test))]
fn braces(word: &str) -> Vec<String> {
    let bytes = word.as_bytes();
    for open in (0..bytes.len()).filter(|&i| bytes[i] == b'{') {
        let mut depth = 0usize;
        let mut commas = Vec::new();
        for (i, &byte) in bytes.iter().enumerate().skip(open) {
            match byte {
                b'{' => depth += 1,
                b',' if depth == 1 => commas.push(i),
                b'}' => {
                    depth -= 1;
                    if depth > 0 {
                        continue;
                    }
                    if commas.is_empty() {
                        break;
                    }
                    let (head, tail) = (&word[..open], &word[i + 1..]);
                    let mut bounds = vec![open];
                    bounds.extend(commas);
                    bounds.push(i);
                    return bounds
                        .windows(2)
                        .flat_map(|w| braces(&format!("{head}{}{tail}", &word[w[0] + 1..w[1]])))
                        .collect();
                }
                _ => {}
            }
        }
    }
    vec![word.to_string()]
}

/// A leading `~`, alone or before a separator, replaced by the home directory.
/// `~user` is left alone.
#[cfg(any(windows, test))]
fn tilde(word: &str, home: Option<&Path>) -> String {
    match (word.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(is_separator) => {
            format!("{}{rest}", home.display())
        }
        _ => word.to_string(),
    }
}

/// The paths `word` matches, spelled the way the caller spelled the pattern:
/// each matched component replaces its pattern, and every separator is kept as
/// written. A trailing separator restricts the matches to directories.
#[cfg(any(windows, test))]
fn glob(word: &str, cwd: &Path) -> Vec<String> {
    let mut candidates = vec![String::new()];
    let mut rest = word;
    while !rest.is_empty() {
        let component_len = rest.find(is_separator).unwrap_or(rest.len());
        let (component, after) = rest.split_at(component_len);
        let separator_len = after.find(|c| !is_separator(c)).unwrap_or(after.len());
        let (separator, after) = after.split_at(separator_len);
        rest = after;

        if !has_wildcard(component) {
            for candidate in &mut candidates {
                candidate.push_str(component);
                candidate.push_str(separator);
            }
            continue;
        }
        candidates = candidates
            .iter()
            .flat_map(|candidate| {
                let mut names: Vec<String> = std::fs::read_dir(on_disk(cwd, candidate))
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .filter(|name| {
                        (!name.starts_with('.') || component.starts_with('.'))
                            && matches(component, name)
                    })
                    .collect();
                names.sort();
                names
                    .into_iter()
                    .map(|name| format!("{candidate}{name}{separator}"))
                    .collect::<Vec<_>>()
            })
            .collect();
    }

    candidates
        .into_iter()
        .filter(|candidate| {
            let path = on_disk(cwd, candidate);
            if candidate.ends_with(is_separator) {
                path.is_dir()
            } else {
                path.symlink_metadata().is_ok()
            }
        })
        .collect()
}

/// Whether `name` matches the one-component `pattern`: `*` any run, `?` any one
/// character, `[...]` a class (`!` or `^` negates, `a-z` ranges, a leading `]`
/// is literal). An unclosed `[` is a literal `[`.
#[cfg(any(windows, test))]
fn matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0usize, 0usize);
    // The last `*` seen, and the name position it currently swallows up to.
    let mut star: Option<(usize, usize)> = None;

    while n < name.len() {
        let step = match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
                continue;
            }
            Some('?') => Some(p + 1),
            Some('[') => match class(&pattern, p, name[n]) {
                Some((true, next)) => Some(next),
                Some((false, _)) => None,
                None => (name[n] == '[').then_some(p + 1),
            },
            Some(&c) => (c == name[n]).then_some(p + 1),
            None => None,
        };
        match (step, star) {
            (Some(next), _) => {
                p = next;
                n += 1;
            }
            (None, Some((star_p, star_n))) => {
                p = star_p + 1;
                n = star_n + 1;
                star = Some((star_p, star_n + 1));
            }
            (None, None) => return false,
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// Matches `c` against the class opening at `pattern[open]`. `None` when the
/// class never closes; otherwise whether `c` is in it, and the index just past
/// its `]`.
#[cfg(any(windows, test))]
fn class(pattern: &[char], open: usize, c: char) -> Option<(bool, usize)> {
    let mut i = open + 1;
    let negated = matches!(pattern.get(i), Some('!' | '^'));
    if negated {
        i += 1;
    }
    let first = i;
    let mut hit = false;
    while let Some(&lo) = pattern.get(i) {
        if lo == ']' && i > first {
            return Some((hit != negated, i + 1));
        }
        match (pattern.get(i + 1), pattern.get(i + 2)) {
            (Some('-'), Some(&hi)) if hi != ']' => {
                hit |= lo <= c && c <= hi;
                i += 3;
            }
            _ => {
                hit |= lo == c;
                i += 1;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_isolation;
    use std::fs;
    use std::path::PathBuf;

    /// A scratch tree: `Cargo.toml`, `b.rs`, `.hidden.rs`, `a[1].txt`,
    /// `a1.txt`, `sub/x.rs`, `sub/y.txt`.
    fn tree() -> (tempfile::TempDir, PathBuf) {
        let tmp = test_isolation::tempdir();
        let root = tmp.path().to_path_buf();
        for file in ["Cargo.toml", "b.rs", ".hidden.rs", "a[1].txt", "a1.txt"] {
            fs::write(root.join(file), "").expect("write fixture");
        }
        fs::create_dir(root.join("sub")).expect("mkdir sub");
        fs::write(root.join("sub").join("x.rs"), "").expect("write sub/x.rs");
        fs::write(root.join("sub").join("y.txt"), "").expect("write sub/y.txt");
        (tmp, root)
    }

    fn expand_in(root: &Path, operand: &str) -> Vec<String> {
        expand(operand, root, Some(Path::new("/home/u")))
    }

    #[test]
    fn a_star_matches_within_the_directory_and_skips_dotfiles() {
        let (_tmp, root) = tree();
        assert_eq!(expand_in(&root, "*.rs"), ["b.rs"]);
        assert_eq!(expand_in(&root, ".*.rs"), [".hidden.rs"]);
    }

    #[test]
    fn matching_is_case_sensitive_like_globify() {
        let (_tmp, root) = tree();
        assert_eq!(expand_in(&root, "*.TOML"), ["*.TOML"]);
    }

    #[test]
    fn an_operand_that_matches_nothing_is_kept_as_written() {
        let (_tmp, root) = tree();
        assert_eq!(expand_in(&root, "*.zz"), ["*.zz"]);
    }

    #[test]
    fn an_existing_path_is_not_globbed_again() {
        let (_tmp, root) = tree();
        assert_eq!(expand_in(&root, "a[1].txt"), ["a[1].txt"]);
        assert_eq!(expand_in(&root, "a[0-9].txt"), ["a1.txt"]);
    }

    #[test]
    fn both_separators_split_components_and_are_kept_as_written() {
        let (_tmp, root) = tree();
        assert_eq!(expand_in(&root, "sub/*.rs"), ["sub/x.rs"]);
        assert_eq!(expand_in(&root, "sub\\*.rs"), ["sub\\x.rs"]);
        assert_eq!(expand_in(&root, "s*/x.rs"), ["sub/x.rs"]);
    }

    #[test]
    fn a_trailing_separator_keeps_only_directories() {
        let (_tmp, root) = tree();
        assert_eq!(expand_in(&root, "*/"), ["sub/"]);
    }

    #[test]
    fn matches_come_back_sorted() {
        let (_tmp, root) = tree();
        assert_eq!(
            expand_in(&root, "*"),
            ["Cargo.toml", "a1.txt", "a[1].txt", "b.rs", "sub"]
        );
    }

    #[test]
    fn braces_expand_in_order_and_each_word_globs_on_its_own() {
        let (_tmp, root) = tree();
        assert_eq!(expand_in(&root, "{b,Cargo}.*"), ["b.rs", "Cargo.toml"]);
        assert_eq!(
            expand_in(&root, "sub/{x,nope}.rs"),
            ["sub/x.rs", "sub/nope.rs"]
        );
        assert_eq!(braces("a{b,{c,d}}e"), ["abe", "ace", "ade"]);
        assert_eq!(braces("{x}"), ["{x}"]);
        assert_eq!(braces("{a,b"), ["{a,b"]);
    }

    #[test]
    fn a_leading_tilde_names_home() {
        let (_tmp, root) = tree();
        assert_eq!(expand_in(&root, "~"), ["/home/u"]);
        assert_eq!(expand_in(&root, "~/notes"), ["/home/u/notes"]);
        assert_eq!(expand_in(&root, "~user/x"), ["~user/x"]);
    }

    #[test]
    fn a_plain_operand_passes_through() {
        let (_tmp, root) = tree();
        assert_eq!(expand_in(&root, "missing.txt"), ["missing.txt"]);
        assert_eq!(expand_in(&root, "-"), ["-"]);
    }

    #[test]
    fn the_matcher_follows_glob_rules() {
        assert!(matches("*.rs", "main.rs"));
        assert!(matches("a*b*c", "aXXbYc"));
        assert!(!matches("a*b*c", "aXXbY"));
        assert!(matches("?.rs", "a.rs"));
        assert!(!matches("?.rs", "ab.rs"));
        assert!(matches("[abc].rs", "b.rs"));
        assert!(matches("[!abc].rs", "d.rs"));
        assert!(!matches("[^abc].rs", "a.rs"));
        assert!(matches("[a-c]x", "bx"));
        assert!(matches("[]]x", "]x"));
        assert!(matches("[x", "[x"));
        assert!(matches("**", ""));
    }

    #[test]
    fn unix_leaves_operands_to_the_shell() {
        if cfg!(not(windows)) {
            assert_eq!(expand_operands(["*.rs", "{a,b}"]), ["*.rs", "{a,b}"]);
        }
    }
}
