//! `ChildCommand` — a `std::process::Command` whose arguments are always encoded
//! so MSYS/Cygwin children on Windows receive them intact (#3727).
//!
//! # Why a newtype and not an extension trait
//!
//! An extension trait cannot do this job. Inherent methods win over trait
//! methods, so a trait declaring `fn arg(&mut self, ..)` for `Command` compiles
//! fine and is then *never called* — every call site silently keeps std's
//! encoding. That is why the encoding has to live on a type that does not have
//! std's `arg` at all.
//!
//! # Why there is no `Deref`
//!
//! `impl Deref<Target = Command>` would put std's inherent `arg` back within
//! reach via `(&mut *cmd).arg(..)`, with no unsafe and no visible unwrap, which
//! defeats the whole point. Reaching the inner `Command` is therefore spelled
//! out: [`ChildCommand::as_std_mut`] or [`ChildCommand::into_std`].

use std::ffi::OsStr;
use std::process::{Child, Command, ExitStatus, Output, Stdio};

use crate::core::utils::quote_arg_for_child;

#[allow(dead_code)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Literal {
    /// The child must see these bytes exactly; suppress its globbing.
    Yes,
    /// The child may glob-expand this operand.
    No,
}

/// Characters Cygwin's `build_argv` and `globify` reinterpret when the parent is
/// not a Cygwin process (`winsup/cygwin/dcrt0.cc`): both quote characters, and
/// `globify`'s own trigger set. A leading `~` is expanded via `GLOB_TILDE`.
///
/// std only ever quotes on a space or a tab, so every one of these reaches an
/// MSYS child reinterpreted unless we wrap the argument ourselves.
const CYGWIN_REINTERPRETED: &[char] = &['"', '\'', '?', '*', '[', '(', ')', '{', '}'];

#[cfg_attr(not(windows), allow(dead_code))]
fn needs_quoting(s: &str, literal: Literal) -> bool {
    match literal {
        // Losing a quote corrupts the argument; a glob that fails to expand is
        // recoverable. So a path operand is still wrapped for quote characters.
        Literal::No => s.contains('"') || s.contains('\''),
        Literal::Yes => s.starts_with('~') || s.contains(CYGWIN_REINTERPRETED),
    }
}

#[allow(dead_code)]
pub struct ChildCommand {
    inner: Command,
    /// cmd.exe parses by its own rules and `raw_arg` would bypass the escaping
    /// std applies for batch files, so those stay on std's path unconditionally.
    #[cfg_attr(not(windows), allow(dead_code))]
    is_batch: bool,
}

#[allow(dead_code)]
impl ChildCommand {
    pub fn new<S: AsRef<OsStr>>(program: S) -> Self {
        let program = program.as_ref();
        let is_batch = std::path::Path::new(program)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd"));
        Self {
            // nosemgrep: dynamic-command-execution -- program is a tool path rtk resolved itself, never a shell string
            inner: Command::new(program),
            is_batch,
        }
    }

    /// Append one argument that must reach the child **literally**.
    ///
    /// This is the safe default: a pattern, a flag value, a regex. The argument
    /// is quoted whenever it holds a character Cygwin's `build_argv`/`globify`
    /// would reinterpret, which also stops the child from glob-expanding it.
    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.push(arg.as_ref(), Literal::Yes);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.push(arg.as_ref(), Literal::Yes);
        }
        self
    }

    /// Append a path operand the child is still allowed to glob-expand.
    ///
    /// Use only for operands where child-side globbing is wanted — `ls`, `wc`,
    /// `tree`, `find` roots invoked from a shell that did not glob. Quoting is
    /// still applied for `"` and `'`, because losing a quote corrupts the
    /// argument outright, which is worse than a glob that does not expand.
    pub fn glob_arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.push(arg.as_ref(), Literal::No);
        self
    }

    pub fn glob_args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.push(arg.as_ref(), Literal::No);
        }
        self
    }

    /// The encoding this argument needs, or `None` to leave it to std.
    ///
    /// Only `raw_arg` is Windows-only, so the decision itself is made — and
    /// therefore asserted — on every platform. Keeping it out of the
    /// `#[cfg(windows)]` arm is deliberate: code that no target compiles is
    /// code that no test can reach.
    ///
    /// `None` for a non-UTF-8 argument (#4116) and for a `.bat`/`.cmd` shim,
    /// whose command line cmd.exe parses by its own rules.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn encoded(&self, arg: &OsStr, literal: Literal) -> Option<String> {
        match arg.to_str() {
            Some(s) if needs_quoting(s, literal) && !self.is_batch => Some(quote_arg_for_child(s)),
            _ => None,
        }
    }

    #[cfg(windows)]
    fn push(&mut self, arg: &OsStr, literal: Literal) {
        match self.encoded(arg, literal) {
            Some(encoded) => {
                std::os::windows::process::CommandExt::raw_arg(&mut self.inner, encoded);
            }
            None => {
                self.inner.arg(arg);
            }
        }
    }

    /// Unix: the argument vector reaches `execvp` verbatim, so there is nothing
    /// to encode.
    #[cfg(not(windows))]
    fn push(&mut self, arg: &OsStr, _literal: Literal) {
        self.inner.arg(arg);
    }

    // ---- plain forwards: nothing about these touches argument encoding ----

    pub fn env<K: AsRef<OsStr>, V: AsRef<OsStr>>(&mut self, k: K, v: V) -> &mut Self {
        self.inner.env(k, v);
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.envs(vars);
        self
    }

    pub fn env_remove<K: AsRef<OsStr>>(&mut self, k: K) -> &mut Self {
        self.inner.env_remove(k);
        self
    }

    pub fn current_dir<P: AsRef<std::path::Path>>(&mut self, dir: P) -> &mut Self {
        self.inner.current_dir(dir);
        self
    }

    pub fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stdin(cfg);
        self
    }

    pub fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stdout(cfg);
        self
    }

    pub fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stderr(cfg);
        self
    }

    pub fn output(&mut self) -> std::io::Result<Output> {
        self.inner.output()
    }

    pub fn status(&mut self) -> std::io::Result<ExitStatus> {
        self.inner.status()
    }

    pub fn spawn(&mut self) -> std::io::Result<Child> {
        self.inner.spawn()
    }

    pub fn get_program(&self) -> &OsStr {
        self.inner.get_program()
    }

    pub fn get_args(&self) -> std::process::CommandArgs<'_> {
        self.inner.get_args()
    }

    pub fn get_envs(&self) -> std::process::CommandEnvs<'_> {
        self.inner.get_envs()
    }

    /// Escape hatch: the inner `Command`, with std's own argument encoding.
    /// Only for a child that is known not to be MSYS/Cygwin-parsed.
    pub fn as_std_mut(&mut self) -> &mut Command {
        &mut self.inner
    }

    /// Escape hatch, by value. See [`ChildCommand::as_std_mut`].
    pub fn into_std(self) -> Command {
        self.inner
    }
}

impl std::fmt::Debug for ChildCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.inner, f)
    }
}

impl From<Command> for ChildCommand {
    fn from(inner: Command) -> Self {
        let is_batch = std::path::Path::new(inner.get_program())
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd"));
        Self { inner, is_batch }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_ordinary_arguments_unchanged() {
        let mut cmd = ChildCommand::new("grep");
        cmd.args(["-c", "pattern", "file.txt"]);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["-c", "pattern", "file.txt"]);
    }

    #[test]
    fn batch_programs_are_detected_at_construction() {
        assert!(ChildCommand::new("gradlew.bat").is_batch);
        assert!(ChildCommand::new("foo.CMD").is_batch);
        assert!(!ChildCommand::new("grep").is_batch);
        assert!(!ChildCommand::new("/usr/bin/grep").is_batch);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn re_encodes_only_arguments_holding_a_quote() {
        let mut cmd = ChildCommand::new("grep");
        cmd.args(["-c", r#""type""#, "q.jsonl"]);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["-c", r#""\"type\"""#, "q.jsonl"]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn batch_shims_stay_on_stds_encoding() {
        // cmd.exe parses by its own rules, so `raw_arg` must not be used there.
        let mut cmd = ChildCommand::new("gradlew.bat");
        cmd.arg(r#""type""#);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, [r#""type""#]);
    }

    // `needs_quoting` decides the whole encoding on Windows, where neither the
    // Linux nor the macOS job compiles the branch that calls it. It is a pure
    // function, so it is asserted directly and everywhere.

    #[test]
    fn literal_operands_quote_every_character_cygwin_reinterprets() {
        for c in CYGWIN_REINTERPRETED {
            let arg = format!("a{c}b");
            assert!(
                needs_quoting(&arg, Literal::Yes),
                "{arg:?} reaches Cygwin's build_argv intact only when quoted"
            );
        }
    }

    #[test]
    fn literal_operands_quote_a_leading_tilde_only() {
        assert!(needs_quoting("~/src", Literal::Yes));
        assert!(!needs_quoting("a~b", Literal::Yes));
    }

    #[test]
    fn literal_operands_leave_ordinary_text_alone() {
        for arg in ["grep", "-c", "pattern", "file.txt", "a-b_c.d", "café"] {
            assert!(
                !needs_quoting(arg, Literal::Yes),
                "{arg:?} needs no quoting"
            );
        }
    }

    // `encoded` is the whole decision: what the child actually receives. These
    // run everywhere, so a regression does not need a Windows runner to show up.

    fn encoding_of(program: &str, arg: &str, literal: Literal) -> Option<String> {
        // nosemgrep: dynamic-command-execution -- builds an argument, spawns nothing
        ChildCommand::new(program).encoded(OsStr::new(arg), literal)
    }

    #[test]
    fn a_literal_operand_is_wrapped_so_the_child_cannot_reinterpret_it() {
        assert_eq!(
            encoding_of("grep", r#""type""#, Literal::Yes).as_deref(),
            Some(r#""\"type\"""#)
        );
        assert_eq!(
            encoding_of("grep", "it's", Literal::Yes).as_deref(),
            Some(r#""it's""#)
        );
        assert_eq!(
            encoding_of("tree", "*.rs", Literal::Yes).as_deref(),
            Some(r#""*.rs""#)
        );
    }

    #[test]
    fn a_glob_operand_keeps_its_metacharacters_but_not_its_quotes() {
        assert_eq!(encoding_of("ls", "*.py", Literal::No), None);
        assert_eq!(encoding_of("ls", "~/src", Literal::No), None);
        assert_eq!(
            encoding_of("ls", "it's", Literal::No).as_deref(),
            Some(r#""it's""#)
        );
    }

    #[test]
    fn a_batch_shim_is_left_on_stds_encoder_whatever_the_argument() {
        // cmd.exe parses its own command line, and std's `make_bat_command_line`
        // is what keeps CVE-2024-24576 closed, so nothing here may use raw_arg.
        for arg in [r#""type""#, "it's", "*.rs", "~/src", "a\"b"] {
            for literal in [Literal::Yes, Literal::No] {
                assert_eq!(
                    encoding_of("gradlew.bat", arg, literal),
                    None,
                    "{arg:?} on a .bat shim must stay on std's encoder"
                );
                assert_eq!(encoding_of("mvnw.cmd", arg, literal), None);
            }
        }
    }

    /// Every `glob_arg`/`glob_args` call site in the tree, as a guard.
    ///
    /// A wrong choice here is the one defect this module's own tests cannot
    /// see: both spellings compile, both are correct Rust, and on Unix `push`
    /// ignores the distinction entirely, so no behavioural test on Linux or
    /// macOS can tell them apart.
    ///
    /// `glob_args` is only ever correct on a slice rtk has itself separated
    /// into path operands; on a vector still holding a flag value or a search
    /// pattern it hands the child exactly the reinterpretation #4102 is about.
    /// Adding a site means proving the slice is split, so this list is meant to
    /// be edited deliberately, not updated to make a test pass.
    #[test]
    fn glob_operands_are_only_used_on_slices_rtk_has_split() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found: Vec<String> = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                // This module defines and exercises them; it is not a call site.
                if path.file_name().is_some_and(|f| f == "child_command.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read source");
                // The bare name, not `.glob_arg(`: UFCS spells the same call
                // `ChildCommand::glob_arg(&mut cmd, x)`.
                if text.contains("glob_arg") {
                    let rel = path.strip_prefix(&root).expect("under src");
                    found.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        found.sort();

        // Both slices are bounded by the tool's own grammar: find's by its
        // leading options and first expression token, the search engines' by
        // the arg_tokenizer value specs that pull patterns out first.
        let expected = ["cmds/system/find_cmd.rs", "cmds/system/search.rs"];
        assert_eq!(
            found, expected,
            "glob_arg belongs on parsed path operands only — see this test's doc comment"
        );
    }

    #[test]
    fn a_non_utf8_argument_is_left_to_std() {
        // #4116: the encoder is defined over `&str`, so these fall through.
        let arg = non_utf8_os_string();
        assert_eq!(ChildCommand::new("grep").encoded(&arg, Literal::Yes), None);
    }

    #[cfg(unix)]
    fn non_utf8_os_string() -> std::ffi::OsString {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0x66, 0x6f, 0x80, 0x6f])
    }

    #[cfg(windows)]
    fn non_utf8_os_string() -> std::ffi::OsString {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0x66, 0x6f, 0xD800, 0x6f])
    }

    #[test]
    fn glob_operands_quote_quotes_but_keep_the_glob_expandable() {
        // Quote characters would be eaten by build_argv, so they are encoded,
        assert!(needs_quoting("it's", Literal::No));
        assert!(needs_quoting(r#"say "hi""#, Literal::No));
        // but the glob metacharacters are left for the child to expand.
        for arg in ["*.py", "src/**", "f?o", "[abc].rs", "{a,b}.rs", "~/src"] {
            assert!(
                !needs_quoting(arg, Literal::No),
                "{arg:?} is the child's to expand"
            );
        }
    }
}
