//! `ChildCommand` — a `std::process::Command` whose arguments are encoded for
//! the kind of child the program is.
//!
//! On Windows a child re-parses its own command line. An MSYS/Cygwin child
//! (Git for Windows' `grep`, `find`, `ls`, …) does it with Cygwin's
//! `build_argv` and `globify`, which strip quotes, expand globs and read
//! response files, so an argument meant literally is quoted for those rules
//! (#3727). A program cmd.exe parses — `cmd` itself and every `.bat`/`.cmd`
//! shim — decodes none of that quoting and is left on std's encoders instead,
//! and on Windows cmd's `/S /C` script is written verbatim to its command line.
//! On Unix the argument vector reaches the child as it is, so none of this
//! applies there.
//!
//! The type holds its `Command` privately, with no `Deref` and no accessor, so
//! std's unencoded `arg` is out of reach: an extension trait could not replace
//! it, since an inherent method always wins over a trait method of the same name.

use std::ffi::OsStr;
use std::process::{Child, Command, ExitStatus, Output, Stdio};

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

/// `globify`'s `is_dos_path` for a UNC path: `\\`, a letter, and a later `\`.
#[cfg(any(windows, test))]
fn is_unc_path(units: &[u16]) -> bool {
    const BACKSLASH: u16 = b'\\' as u16;
    matches!(units, [BACKSLASH, BACKSLASH, host, rest @ ..]
        if u8::try_from(*host).is_ok_and(|h| h.is_ascii_alphabetic()) && rest.contains(&BACKSLASH))
}

#[cfg(any(windows, test))]
fn needs_quoting(units: &[u16]) -> bool {
    // `build_argv` ends an argument at `\n` or `\r` as well as at a space, and
    // replaces an unquoted word that starts with `@` by the contents of the
    // file it names.
    starts_with(units, '@')
        || has_any(units, &['\n', '\r'])
        || starts_with(units, '~')
        || has_any(units, CYGWIN_REINTERPRETED)
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
            #[allow(clippy::disallowed_methods)] // the one place a child is built
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
        self.push(arg.as_ref());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.push(arg.as_ref());
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
    fn encoded(&self, units: &[u16]) -> Option<Vec<u16>> {
        if self.cmd_parsed {
            return None;
        }
        let needs = needs_quoting(units);
        if is_unc_path(units) && (needs || has_any(units, &[' ', '\t'])) {
            // Wrapped whole, `globify` halves the leading `\\`; left bare, it
            // expands the globs and braces in it. With `\\` and the host's first
            // letter outside the quotes it still reads a DOS path, so it keeps
            // the `\\`, and everything after it is quoted.
            let (prefix, rest) = units.split_at(3);
            return Some([prefix, &quote_arg_for_child(rest)].concat());
        }
        needs.then(|| quote_arg_for_child(units))
    }

    /// What [`ChildCommand::arg`] hands a Windows child for `arg`, or `None`
    /// when std's own encoding applies, so a caller's tests can assert the
    /// bytes on every platform.
    #[cfg(test)]
    pub(crate) fn literal_encoding(&self, arg: &str) -> Option<String> {
        let units: Vec<u16> = arg.encode_utf16().collect();
        self.encoded(&units)
            .map(|encoded| String::from_utf16_lossy(&encoded))
    }

    #[cfg(windows)]
    fn push(&mut self, arg: &OsStr) {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let units: Vec<u16> = arg.encode_wide().collect();
        match self.encoded(&units) {
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
    fn push(&mut self, arg: &OsStr) {
        self.inner.arg(arg);
    }

    /// Append `arg` to the child's command line exactly as written, with no
    /// encoding at all. Only for cmd.exe's `/C` script: cmd reads the rest of
    /// its command line itself, so any escaping would reach it as text.
    #[cfg(windows)]
    pub(crate) fn verbatim_arg(&mut self, arg: &str) -> &mut Self {
        debug_assert!(
            self.cmd_parsed,
            "verbatim_arg is for a program cmd.exe parses"
        );
        std::os::windows::process::CommandExt::raw_arg(&mut self.inner, arg);
        self
    }

    // ---- plain forwards: nothing about these touches argument encoding ----

    pub fn env<K: AsRef<OsStr>, V: AsRef<OsStr>>(&mut self, k: K, v: V) -> &mut Self {
        self.inner.env(k, v);
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

#[cfg(test)]
impl ChildCommand {
    /// Isolates the git rtk spawns itself in a test, as
    /// `test_isolation::isolate_git_config` does: it touches the environment
    /// alone, so no argument gets around the encoding.
    pub(crate) fn isolate_git_config(&mut self) -> &mut Self {
        crate::core::test_isolation::isolate_git_config(&mut self.inner);
        self
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

    fn needs(s: &str) -> bool {
        needs_quoting(&utf16(s))
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
        // Listed here rather than read from `CYGWIN_REINTERPRETED`, so dropping a
        // character from that set fails this test instead of shrinking it.
        for c in ['"', '\'', '?', '*', '[', '(', ')', '{', '}'] {
            let arg = format!("a{c}b");
            assert!(
                needs(&arg),
                "{arg:?} reaches Cygwin's build_argv intact only when quoted"
            );
        }
    }

    #[test]
    fn literal_operands_quote_a_leading_tilde_only() {
        assert!(needs("~/src"));
        assert!(!needs("a~b"));
    }

    #[test]
    fn literal_operands_leave_ordinary_text_alone() {
        for arg in ["grep", "-c", "pattern", "file.txt", "a-b_c.d", "café"] {
            assert!(!needs(arg), "{arg:?} needs no quoting");
        }
    }

    // `encoded` is the whole decision: what the child actually receives. These
    // run everywhere, so a regression does not need a Windows runner to show up.

    fn encoding_of(program: &str, arg: &str) -> Option<String> {
        // nosemgrep: dynamic-command-execution -- builds an argument, spawns nothing
        ChildCommand::new(program).literal_encoding(arg)
    }

    #[test]
    fn a_literal_operand_is_wrapped_so_the_child_cannot_reinterpret_it() {
        assert_eq!(
            encoding_of("grep", r#""type""#).as_deref(),
            Some(r#""\"type\"""#)
        );
        assert_eq!(encoding_of("grep", "it's").as_deref(), Some(r#""it's""#));
        assert_eq!(encoding_of("tree", "*.rs").as_deref(), Some(r#""*.rs""#));
    }

    #[test]
    fn a_batch_shim_is_left_on_stds_encoder_whatever_the_argument() {
        // cmd.exe parses its own command line, and std's `make_bat_command_line`
        // is what keeps CVE-2024-24576 closed, so nothing here may use raw_arg.
        for arg in [r#""type""#, "it's", "*.rs", "~/src", "a\"b"] {
            assert_eq!(
                encoding_of("gradlew.bat", arg),
                None,
                "{arg:?} on a .bat shim must stay on std's encoder"
            );
            assert_eq!(encoding_of("mvnw.cmd", arg), None);
        }
    }

    #[test]
    fn an_unpaired_surrogate_is_quoted_like_any_other_text() {
        // #4116: a Windows argument need not be valid UTF-16, and the rules
        // work on its code units, so it is still protected from the child.
        let arg = [u16::from(b'('), 0xD800, u16::from(b')')];
        let encoded = ChildCommand::new("grep").encoded(&arg);
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
        assert_eq!(encoding_of("grep", "@x").as_deref(), Some(r#""@x""#));
        assert_eq!(encoding_of("grep", "a@x"), None);
    }

    #[test]
    fn a_regex_escaping_a_bracket_reaches_the_child_intact() {
        // Unquoted, `globify` reads `\[` as a `GLOB_QUOTE` escape and drops the
        // backslash: `^\[TAG\]` arrives as the class `^[TAG]` and matches other
        // lines (#4102), and `^\[T` as an unmatched `[`. Wrapped, the backslashes
        // stay single, since none precedes a quote or ends the argument.
        assert_eq!(
            encoding_of("grep", r"^\[TAG\]").as_deref(),
            Some(r#""^\[TAG\]""#)
        );
        assert_eq!(encoding_of("grep", r"^\[T").as_deref(), Some(r#""^\[T""#));
    }

    #[test]
    fn a_backslash_with_no_metacharacter_is_left_alone() {
        // `globify` only reads a backslash as an escape inside `glob()`, which it
        // calls only for a word holding a metacharacter, so these arrive as typed.
        for arg in [r"a\.b", r"\bword\b", r"src\main.rs", r"\$HOME"] {
            assert_eq!(encoding_of("grep", arg), None, "{arg:?}");
        }
    }

    #[test]
    fn a_unc_path_keeps_its_leading_backslashes_outside_the_quotes() {
        // Each checked against Cygwin's `build_argv`/`globify` and the MSVCRT
        // rules: both kinds of child receive the argument as written.
        for (arg, sent) in [
            (r"\\srv\share\x(1)", r#"\\s"rv\share\x(1)""#),
            (r"\\srv\share\*.rs", r#"\\s"rv\share\*.rs""#),
            (r"\\srv\share\{a,b}", r#"\\s"rv\share\{a,b}""#),
            (r"\\srv\share\it's", r#"\\s"rv\share\it's""#),
            (r#"\\srv\share\q"x"#, r#"\\s"rv\share\q\"x""#),
            (r"\\srv\my share\x", r#"\\s"rv\my share\x""#),
            (r"\\srv\share\x(1)\", r#"\\s"rv\share\x(1)\\""#),
        ] {
            assert_eq!(encoding_of("grep", arg).as_deref(), Some(sent), "{arg:?}");
        }
    }

    #[test]
    fn a_plain_unc_path_is_left_to_std() {
        assert_eq!(encoding_of("grep", r"\\srv\share\x"), None);
        assert_eq!(encoding_of("grep", r"\\srv\share\"), None);
    }

    #[test]
    fn a_unc_path_with_no_letter_host_is_wrapped_whole() {
        // `globify` reads none of these as a DOS path, so the split would not
        // hold; wrapping keeps a native child exact.
        for (arg, sent) in [
            (r"\\1srv\share\x(1)", r#""\\1srv\share\x(1)""#),
            (r"\\?\C:\x(1)", r#""\\?\C:\x(1)""#),
            (r"\\srv(1)", r#""\\srv(1)""#),
        ] {
            assert_eq!(encoding_of("grep", arg).as_deref(), Some(sent), "{arg:?}");
        }
    }

    #[test]
    fn a_line_break_is_quoted_so_it_does_not_split_the_argument() {
        assert_eq!(encoding_of("grep", "a\nb").as_deref(), Some("\"a\nb\""));
        assert_eq!(encoding_of("grep", "a\rb").as_deref(), Some("\"a\rb\""));
    }
}
