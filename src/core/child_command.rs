//! `ChildCommand` — a `std::process::Command` whose arguments are encoded for
//! the kind of child the program is.
//!
//! On Windows a child re-parses its own command line. An MSYS/Cygwin child
//! (Git for Windows' `grep`, `find`, `ls`, …) does it with Cygwin's
//! `build_argv` and `globify`, which strip quotes, expand globs and read
//! response files, so an argument meant literally is quoted for those rules
//! (#3727). A program cmd.exe parses — `cmd` itself and every `.bat`/`.cmd`
//! shim — decodes none of that quoting and is left on std's encoders instead,
//! and cmd's `/C` script is written verbatim through
//! [`ChildCommand::verbatim_arg`]. On Unix the argument vector reaches the child
//! as it is, so none of this applies there.
//!
//! The type holds its `Command` privately, with no `Deref` and no accessor, so
//! std's unencoded `arg` is out of reach: an extension trait could not replace
//! it, since an inherent method always wins over a trait method of the same name.
//!
//! Path operands are the one kind of argument a child may glob, and only as a
//! [`PathOperands`], which only a tool's own [`OperandGrammar`] can produce.

use std::ffi::OsStr;
use std::process::{Child, Command, ExitStatus, Output, Stdio};

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
#[cfg(any(windows, test))]
const CYGWIN_REINTERPRETED: &[char] = &['"', '\'', '?', '*', '[', '(', ')', '{', '}'];

/// Arguments are handled as UTF-16 code units, what a Windows command line is
/// made of, so an unpaired surrogate is encoded like any other text (#4116).
/// Every character the rules test for is ASCII.
#[cfg(any(windows, test))]
fn has_any(units: &[u16], set: &[char]) -> bool {
    units.iter().any(|&u| set.iter().any(|&c| u == c as u16))
}

#[cfg(any(windows, test))]
fn starts_with(units: &[u16], c: char) -> bool {
    units.first() == Some(&(c as u16))
}

#[cfg(any(windows, test))]
fn needs_quoting(units: &[u16], literal: Literal) -> bool {
    // `build_argv` ends an argument at `\n` or `\r` as well as at a space, and
    // replaces an unquoted word that starts with `@` by the contents of the
    // file it names, whatever the operand kind.
    let always = starts_with(units, '@') || has_any(units, &['\n', '\r']);
    let reinterpreted = starts_with(units, '~') || has_any(units, CYGWIN_REINTERPRETED);
    match literal {
        Literal::Yes => always || reinterpreted,
        // Losing a quote corrupts the argument; a glob that fails to expand is
        // recoverable, so a path operand is still wrapped for quote characters.
        // When `globify` runs on a word that is not a drive or UNC path, it reads
        // a backslash outside quotes as a glob escape (`src\*.rs` would arrive
        // as `src*.rs`), so such an operand is kept literal. A drive or UNC path
        // keeps its backslashes unquoted, and quoting it would halve a leading
        // `\\`, so it is left as it is.
        Literal::No => {
            always
                || has_any(units, &['"', '\''])
                || (has_any(units, &['\\']) && reinterpreted && !is_dos_path(units))
        }
    }
}

/// `globify`'s `is_dos_path`: a drive (`X:`) or a UNC path (`\\host\...`, the
/// host starting with a letter), whose backslashes it keeps literal outside
/// quotes.
#[cfg(any(windows, test))]
fn is_dos_path(units: &[u16]) -> bool {
    let at = |i: usize| units.get(i).copied();
    let is_alpha = |u: Option<u16>| u.is_some_and(|u| u < 0x80 && (u as u8).is_ascii_alphabetic());
    let backslash = Some(b'\\' as u16);
    let drive = is_alpha(at(0)) && at(1) == Some(b':' as u16);
    let unc = at(0) == backslash
        && at(1) == backslash
        && is_alpha(at(2))
        && units.iter().skip(3).any(|&u| Some(u) == backslash);
    drive || unc
}

/// Encode one argument for a child's raw command line the way libuv's
/// `quote_cmd_arg` does: wrap it in `"`, escape an inner `"` as `\"`, and double
/// every backslash run that ends up in front of a quote so it stays literal.
///
/// std's encoder emits the same bytes but wraps only for a space or a tab, which
/// is what leaves MSYS children with a mangled command line (#3727).
///
/// These bytes are exact for a native child, which parses by the MSVCRT rules.
/// An MSYS/Cygwin child differs in one case: inside the quotes `globify`
/// collapses every `\\` to `\`, so a run of two or more backslashes that is not
/// in front of a quote loses one backslash of each pair there. No single
/// encoding is exact for both parsers in that case (see #4326).
#[cfg(any(windows, test))]
fn quote_arg_for_child(units: &[u16]) -> Vec<u16> {
    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    let mut out = Vec::with_capacity(units.len() + 2);
    out.push(QUOTE);
    let mut backslashes = 0usize;
    for &unit in units {
        match unit {
            BACKSLASH => backslashes += 1,
            QUOTE => {
                out.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2 + 1));
                backslashes = 0;
                out.push(QUOTE);
            }
            _ => {
                out.extend(std::iter::repeat_n(BACKSLASH, backslashes));
                backslashes = 0;
                out.push(unit);
            }
        }
    }
    // The closing quote is a quote too, so a trailing run is doubled as well.
    out.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2));
    out.push(QUOTE);
    out
}

/// A shell family, told apart by the program's name.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum ShellKind {
    Cmd,
    PowerShell,
    Posix,
}

/// The shell family `shell` names, bare, with an extension or as a path.
///
/// Both separators are honoured on every platform, so a Windows path is
/// classified the same way wherever the decision is tested.
pub(crate) fn shell_kind(shell: &str) -> ShellKind {
    let basename = shell
        .rsplit_once(['/', '\\'])
        .map_or(shell, |(_, name)| name);
    let named = |names: &[&str]| names.iter().any(|n| basename.eq_ignore_ascii_case(n));

    if named(&["cmd", "cmd.exe"]) {
        ShellKind::Cmd
    } else if named(&["powershell", "powershell.exe", "pwsh", "pwsh.exe"]) {
        ShellKind::PowerShell
    } else {
        ShellKind::Posix
    }
}

/// True for a program whose command line cmd.exe parses: `cmd` itself, bare,
/// with its extension or as a path, and every `.bat`/`.cmd` shim. The literal
/// quoting is written for Cygwin's rules, which cmd does not decode, so these
/// are left to std: its batch-aware encoder for a shim, its general-purpose
/// one for `cmd` itself.
fn parses_like_cmd(program: &OsStr) -> bool {
    let is_batch = std::path::Path::new(program)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd"));
    is_batch
        || program
            .to_str()
            .is_some_and(|p| shell_kind(p) == ShellKind::Cmd)
}

/// Whether the child may glob-expand path operands, given the `MSYSTEM` rtk
/// was started with.
///
/// Git Bash and every MSYS2 shell set `MSYSTEM`, and the MSYS runtime forces it
/// into the environment of each Windows program it starts (`spenvs` in
/// `winsup/cygwin/environ.cc`). So its presence means rtk's caller is a POSIX
/// shell that has already expanded, or deliberately quoted, every operand, and
/// a second pass in an MSYS child would re-expand what the caller quoted
/// (`'app/[id]/page.tsx'`): operands stay literal. From PowerShell or cmd
/// nothing has globbed them, and the child does.
///
/// A PowerShell started from inside Git Bash inherits `MSYSTEM` and reads as
/// the first case, so its operands stay literal: nothing is reinterpreted, and
/// that shell can expand them itself.
pub(crate) fn child_globs(msystem: Option<&OsStr>) -> bool {
    msystem.is_none_or(|value| value.is_empty())
}

/// Literal unless the grammar bounded the operands and the caller's shell has
/// not already globbed them.
fn operand_encoding(bounded: bool) -> Literal {
    if bounded && child_globs(std::env::var_os("MSYSTEM").as_deref()) {
        Literal::No
    } else {
        Literal::Yes
    }
}

/// Which arguments of an argv a tool's grammar reads as path operands.
pub struct OperandSplit {
    /// Indices into the argv, ascending.
    pub indices: Vec<usize>,
    /// False when the grammar cannot tell where flag values end: a bare long
    /// flag (no attached `=value`) that names no option of the tool, or is a
    /// prefix of several, may have taken one of those arguments as its value.
    /// A unique prefix is read as the option it abbreviates, as `getopt_long`
    /// reads it, and an attached value never takes the next argument; both
    /// keep the split bounded.
    pub bounded: bool,
}

impl OperandSplit {
    /// The free positionals of `tokens`, bounded unless
    /// [`crate::core::arg_tokenizer::has_unknown_long`] finds a flag outside
    /// `long_options`, the tool's complete list.
    pub fn free_positionals(
        tokens: &[crate::core::arg_tokenizer::Token<'_>],
        long_options: &crate::core::arg_tokenizer::LongOptions<'_>,
    ) -> Self {
        Self {
            indices: tokens
                .iter()
                .filter(|t| t.is_free_positional())
                .map(|t| t.source_index)
                .collect(),
            bounded: !crate::core::arg_tokenizer::has_unknown_long(tokens, long_options),
        }
    }
}

/// A tool's own argument grammar, the only source of [`PathOperands`].
///
/// Letting the child glob is correct on path operands and nowhere else, and
/// telling them apart from flag values takes the tool's grammar: `ls -I
/// <pattern>`, `wc --files0-from <file>` and `tree -P <pattern>` all leave a
/// value among the arguments that do not start with a dash, and globbing a
/// value hands the child the reinterpretation #4102 is about.
///
/// The trait is sealed: only the grammars listed in this module implement it,
/// and adding one is the claim that its `split` bounds the operands.
pub trait OperandGrammar: sealed::Sealed {
    /// What else the tool reads while splitting (its flags, its patterns), so
    /// the caller does not parse the argv again.
    type Parsed;

    fn split<T: AsRef<str>>(&self, args: &[T]) -> (OperandSplit, Self::Parsed);
}

mod sealed {
    pub trait Sealed {}

    impl Sealed for crate::cmds::system::find_cmd::FindGrammar {}
    impl Sealed for crate::cmds::system::ls::LsGrammar {}
    impl Sealed for crate::cmds::system::search::SearchGrammar {}
    impl Sealed for crate::cmds::system::tree::TreeGrammar {}
    impl Sealed for crate::cmds::system::wc_cmd::WcGrammar {}
}

/// An argv together with its tool's [`OperandGrammar`] split of it, for
/// [`ChildCommand::split_args`] and [`SplitArgv::operands`].
///
/// Only [`SplitArgv::new`] builds one, so its operands are always the
/// grammar's own split of the argv it holds.
pub struct SplitArgv {
    args: Vec<String>,
    operands: Vec<usize>,
    bounded: bool,
}

impl SplitArgv {
    /// `args` split by `grammar`, and whatever else the grammar read on the way.
    pub fn new<G: OperandGrammar, T: AsRef<str>>(grammar: &G, args: &[T]) -> (Self, G::Parsed) {
        let (split, parsed) = grammar.split(args);
        let argv = Self {
            args: args.iter().map(|arg| arg.as_ref().to_string()).collect(),
            operands: split.indices,
            bounded: split.bounded,
        };
        (argv, parsed)
    }

    /// The argv as given.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// See [`OperandSplit::bounded`].
    pub fn is_bounded(&self) -> bool {
        self.bounded
    }

    /// The operands alone, for a caller that places them itself.
    pub fn operands(&self) -> PathOperands {
        PathOperands {
            operands: self
                .operands
                .iter()
                .filter_map(|&i| self.args.get(i).cloned())
                .collect(),
            bounded: self.bounded,
        }
    }
}

/// The path operands of an argv, as a tool's [`OperandGrammar`] split them: the
/// only thing [`ChildCommand::glob_args`] takes.
///
/// A value is always the grammar's own split of the argv it was given, never a
/// vector assembled elsewhere. The child globs the operands only when the split
/// was bounded and [`child_globs`] allows it; otherwise they reach it literally.
pub struct PathOperands {
    operands: Vec<String>,
    bounded: bool,
}

impl std::ops::Deref for PathOperands {
    type Target = [String];

    fn deref(&self) -> &[String] {
        &self.operands
    }
}

pub struct ChildCommand {
    inner: Command,
    /// Whether cmd.exe parses this program's command line; see [`parses_like_cmd`].
    /// Only the encoder reads it, and only Windows encodes.
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    cmd_parsed: bool,
}

impl ChildCommand {
    pub fn new<S: AsRef<OsStr>>(program: S) -> Self {
        let cmd_parsed = parses_like_cmd(program.as_ref());
        Self {
            // nosemgrep: dynamic-command-execution -- program is a tool path rtk resolved itself, never a shell string
            inner: Command::new(program),
            cmd_parsed,
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

    /// Append path operands the child may glob-expand (see [`PathOperands`]).
    ///
    /// Quoting is still applied for `"` and `'`, because losing a quote
    /// corrupts the argument outright, which is worse than a glob that does
    /// not expand.
    pub fn glob_args(&mut self, operands: &PathOperands) -> &mut Self {
        let literal = operand_encoding(operands.bounded);
        for arg in &operands.operands {
            self.push(OsStr::new(arg), literal);
        }
        self
    }

    /// Append a whole argv in order, its path operands as
    /// [`ChildCommand::glob_args`] would and every other argument literally.
    pub fn split_args(&mut self, argv: &SplitArgv) -> &mut Self {
        let operand = operand_encoding(argv.bounded);
        for (i, arg) in argv.args.iter().enumerate() {
            let literal = if argv.operands.binary_search(&i).is_ok() {
                operand
            } else {
                Literal::Yes
            };
            self.push(OsStr::new(arg), literal);
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
    /// `None` for a program whose command line cmd.exe parses by its own rules
    /// (see [`parses_like_cmd`]).
    #[cfg(any(windows, test))]
    fn encoded(&self, units: &[u16], literal: Literal) -> Option<Vec<u16>> {
        (!self.cmd_parsed && needs_quoting(units, literal)).then(|| quote_arg_for_child(units))
    }

    /// What [`ChildCommand::arg`] hands a Windows child for `arg`, or `None`
    /// when std's own encoding applies, so a caller's tests can assert the
    /// bytes on every platform.
    #[cfg(test)]
    pub(crate) fn literal_encoding(&self, arg: &str) -> Option<String> {
        let units: Vec<u16> = arg.encode_utf16().collect();
        self.encoded(&units, Literal::Yes)
            .map(|encoded| String::from_utf16_lossy(&encoded))
    }

    #[cfg(windows)]
    fn push(&mut self, arg: &OsStr, literal: Literal) {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let units: Vec<u16> = arg.encode_wide().collect();
        match self.encoded(&units, literal) {
            Some(encoded) => {
                std::os::windows::process::CommandExt::raw_arg(
                    &mut self.inner,
                    std::ffi::OsString::from_wide(&encoded),
                );
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

    /// Append `arg` to the child's command line exactly as written, with no
    /// encoding at all. Only for cmd.exe's `/C` script: cmd reads the rest of
    /// its command line itself, so any escaping would reach it as text.
    #[cfg(windows)]
    pub(crate) fn verbatim_arg(&mut self, arg: &str) -> &mut Self {
        std::os::windows::process::CommandExt::raw_arg(&mut self.inner, arg);
        self
    }

    /// Off Windows there is no command line to write verbatim, only an
    /// argument vector.
    #[cfg(not(windows))]
    pub(crate) fn verbatim_arg(&mut self, arg: &str) -> &mut Self {
        self.inner.arg(arg);
        self
    }

    // ---- plain forwards: nothing about these touches argument encoding ----

    pub fn env<K: AsRef<OsStr>, V: AsRef<OsStr>>(&mut self, k: K, v: V) -> &mut Self {
        self.inner.env(k, v);
        self
    }

    #[cfg(test)]
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

    #[cfg(test)]
    pub fn get_args(&self) -> std::process::CommandArgs<'_> {
        self.inner.get_args()
    }

    #[cfg(test)]
    pub fn get_envs(&self) -> std::process::CommandEnvs<'_> {
        self.inner.get_envs()
    }
}

impl std::fmt::Debug for ChildCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.inner, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// `quote_arg_for_child` over text, for readable expectations.
    fn quote(s: &str) -> String {
        String::from_utf16_lossy(&quote_arg_for_child(&utf16(s)))
    }

    fn needs(s: &str, literal: Literal) -> bool {
        needs_quoting(&utf16(s), literal)
    }

    // ===== Child-process argument quoting (issue #3727) =====

    #[test]
    fn test_quote_arg_wraps_a_quote_with_no_space_around_it() {
        assert_eq!(quote(r#"a"b"#), r#""a\"b""#);
    }

    #[test]
    fn test_quote_arg_wraps_the_reported_json_key_pattern() {
        // `rtk grep -c '"type"' file`, the shape reported in #3727.
        assert_eq!(quote(r#""type""#), r#""\"type\"""#);
    }

    #[test]
    fn test_quote_arg_wraps_repeated_quotes() {
        assert_eq!(quote(r#"a""b"#), r#""a\"\"b""#);
    }

    #[test]
    fn test_quote_arg_doubles_backslash_runs_before_a_quote() {
        assert_eq!(quote(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(quote(r#"a\\"b"#), r#""a\\\\\"b""#);
    }

    #[test]
    fn test_quote_arg_doubles_a_trailing_backslash_run() {
        assert_eq!(quote(r#"a"b\"#), r#""a\"b\\""#);
    }

    #[test]
    fn test_quote_arg_keeps_backslashes_that_precede_ordinary_text() {
        assert_eq!(quote(r#"C:\a\b "x""#), r#""C:\a\b \"x\"""#);
    }

    /// Pins the one case where these bytes are exact for a native child but not
    /// for an MSYS one: inside the quotes Cygwin's `globify` collapses `\\`
    /// to `\`, so MSYS grep receives `\(` and `C:\Temp (x86)` for these two
    /// (verified against msys2-runtime's own `build_argv`/`globify`). A native
    /// child, which parses by the MSVCRT rules, receives them intact (#4326).
    #[test]
    fn test_quote_arg_keeps_a_backslash_run_before_ordinary_text() {
        assert_eq!(quote(r"\\("), r#""\\(""#);
        assert_eq!(quote(r"C:\\Temp (x86)"), r#""C:\\Temp (x86)""#);
    }

    #[test]
    fn test_quote_arg_keeps_a_space_inside_the_wrapping() {
        assert_eq!(quote(r#"a b "c""#), r#""a b \"c\"""#);
    }

    #[test]
    fn shell_kind_ignores_case_extension_and_directory() {
        for cmd in [
            "cmd",
            "CMD",
            "cmd.exe",
            "Cmd.EXE",
            r"C:\Windows\System32\cmd.exe",
        ] {
            assert_eq!(shell_kind(cmd), ShellKind::Cmd, "{cmd:?}");
        }
        assert_eq!(shell_kind("/usr/bin/cmd"), ShellKind::Cmd);
        for pwsh in [
            "pwsh",
            "PowerShell.exe",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
        ] {
            assert_eq!(shell_kind(pwsh), ShellKind::PowerShell, "{pwsh:?}");
        }
        for posix in [
            "sh",
            "/bin/bash",
            r"C:\msys64\usr\bin\bash.exe",
            "cmdx",
            "cmd.sh",
        ] {
            assert_eq!(shell_kind(posix), ShellKind::Posix, "{posix:?}");
        }
    }

    #[test]
    fn forwards_ordinary_arguments_unchanged() {
        let mut cmd = ChildCommand::new("grep");
        cmd.args(["-c", "pattern", "file.txt"]);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["-c", "pattern", "file.txt"]);
    }

    #[test]
    fn cmd_parsed_programs_are_recognised_by_name() {
        for program in [
            "gradlew.bat",
            "foo.CMD",
            "cmd",
            "CMD.EXE",
            r"C:\Windows\System32\cmd.exe",
        ] {
            assert!(parses_like_cmd(OsStr::new(program)), "{program:?}");
        }
        for program in [
            "grep",
            "/usr/bin/grep",
            "cmdx",
            r"C:\msys64\usr\bin\bash.exe",
        ] {
            assert!(!parses_like_cmd(OsStr::new(program)), "{program:?}");
        }
    }

    #[test]
    fn cmd_itself_is_left_on_stds_encoder_whatever_the_argument() {
        // `rtk proxy cmd /c echo (hi)` must reach cmd.exe as `(hi)`, not `"(hi)"`.
        for program in ["cmd", "Cmd.exe", r"C:\Windows\System32\cmd.exe"] {
            for arg in ["(hi)", r#""type""#, "it's", "*.rs", "~/src"] {
                assert_eq!(
                    ChildCommand::new(program).literal_encoding(arg),
                    None,
                    "{arg:?} to {program:?} must stay on std's encoder"
                );
            }
        }
        assert_eq!(
            ChildCommand::new("grep")
                .literal_encoding("(hi)")
                .as_deref(),
            Some(r#""(hi)""#)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn re_encodes_only_arguments_cygwin_would_reinterpret() {
        let mut cmd = ChildCommand::new("grep");
        cmd.args(["-c", r#""type""#, "*.rs", "q.jsonl"]);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["-c", r#""\"type\"""#, r#""*.rs""#, "q.jsonl"]);
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
                needs(&arg, Literal::Yes),
                "{arg:?} reaches Cygwin's build_argv intact only when quoted"
            );
        }
    }

    #[test]
    fn literal_operands_quote_a_leading_tilde_only() {
        assert!(needs("~/src", Literal::Yes));
        assert!(!needs("a~b", Literal::Yes));
    }

    #[test]
    fn literal_operands_leave_ordinary_text_alone() {
        for arg in ["grep", "-c", "pattern", "file.txt", "a-b_c.d", "café"] {
            assert!(!needs(arg, Literal::Yes), "{arg:?} needs no quoting");
        }
    }

    // `encoded` is the whole decision: what the child actually receives. These
    // run everywhere, so a regression does not need a Windows runner to show up.

    fn encoding_of(program: &str, arg: &str, literal: Literal) -> Option<String> {
        // nosemgrep: dynamic-command-execution -- builds an argument, spawns nothing
        ChildCommand::new(program)
            .encoded(&utf16(arg), literal)
            .map(|encoded| String::from_utf16_lossy(&encoded))
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

    #[test]
    fn an_unpaired_surrogate_is_quoted_like_any_other_text() {
        // #4116: a Windows argument need not be valid UTF-16, and the rules
        // work on its code units, so it is still protected from the child.
        let arg = [u16::from(b'('), 0xD800, u16::from(b')')];
        let encoded = ChildCommand::new("grep").encoded(&arg, Literal::Yes);
        assert_eq!(
            encoded,
            Some(vec![
                u16::from(b'"'),
                u16::from(b'('),
                0xD800,
                u16::from(b')'),
                u16::from(b'"'),
            ])
        );
    }

    #[test]
    fn a_leading_at_sign_is_quoted_so_it_is_not_a_response_file() {
        // `build_argv` replaces an unquoted `@file` by the file's contents.
        for literal in [Literal::Yes, Literal::No] {
            assert_eq!(
                encoding_of("grep", "@x", literal).as_deref(),
                Some(r#""@x""#)
            );
            assert_eq!(encoding_of("grep", "a@x", literal), None);
        }
    }

    #[test]
    fn a_line_break_is_quoted_so_it_does_not_split_the_argument() {
        for literal in [Literal::Yes, Literal::No] {
            assert_eq!(
                encoding_of("grep", "a\nb", literal).as_deref(),
                Some("\"a\nb\"")
            );
            assert_eq!(
                encoding_of("grep", "a\rb", literal).as_deref(),
                Some("\"a\rb\"")
            );
        }
    }

    /// Rows from the msys2-runtime oracle: what an MSYS child receives for each
    /// encoding of a glob operand.
    #[test]
    fn a_drive_or_unc_glob_operand_keeps_its_backslashes_unquoted() {
        // Quoted, `\\srv\sh\*.rs` reaches MSYS as `\srv\sh\*.rs`; unquoted it is intact.
        for arg in [
            r"\\srv\sh\*.rs",
            r"\\srv\sh\(x)",
            r"C:\a\*.rs",
            r"c:\a\(x)",
            r"C:rel\*.rs",
        ] {
            assert_eq!(encoding_of("grep", arg, Literal::No), None, "{arg:?}");
        }
        // Not a UNC path to `globify`: the host must start with a letter and be
        // followed by another backslash.
        for arg in [r"\\1srv\x\*.rs", r"\\srv*"] {
            assert!(encoding_of("grep", arg, Literal::No).is_some(), "{arg:?}");
        }
        // A quote character is still quoted, drive or not.
        assert!(encoding_of("grep", r"C:\a\it's", Literal::No).is_some());
    }

    #[test]
    fn a_glob_operand_with_a_backslash_is_kept_literal() {
        // Outside quotes `globify` reads it as an escape: `src\*.rs` would
        // reach the child as `src*.rs`.
        assert_eq!(
            encoding_of("grep", r"src\*.rs", Literal::No).as_deref(),
            Some(r#""src\*.rs""#)
        );
        assert_eq!(encoding_of("grep", r"src\a.rs", Literal::No), None);
    }

    #[test]
    fn operands_glob_only_when_the_caller_is_not_an_msys_shell() {
        assert!(child_globs(None));
        assert!(child_globs(Some(OsStr::new(""))));
        for msystem in ["MINGW64", "MSYS", "UCRT64", "CLANG64"] {
            assert!(!child_globs(Some(OsStr::new(msystem))), "{msystem}");
        }
    }

    #[test]
    fn glob_operands_quote_quotes_but_keep_the_glob_expandable() {
        // Quote characters would be eaten by build_argv, so they are encoded,
        assert!(needs("it's", Literal::No));
        assert!(needs(r#"say "hi""#, Literal::No));
        // but the glob metacharacters are left for the child to expand.
        for arg in ["*.py", "src/**", "f?o", "[abc].rs", "{a,b}.rs", "~/src"] {
            assert!(!needs(arg, Literal::No), "{arg:?} is the child's to expand");
        }
    }
}
