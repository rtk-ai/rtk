//! Keeps the test suite off the developer's own rtk data.
//!
//! A spawned rtk resolves its tracking database and tee spool exactly as a
//! normal invocation does, with nothing in the child to tell that its parent
//! is a test, so the redirection belongs at the call site. The scan at the
//! bottom of this file enforces it.

mod scratch;

use crate::core::user_dirs;
use crate::core::user_env;
use regex::Regex;
use scratch::redirect_rtk_data_to;
pub use scratch::{isolate_git, isolate_git_config, scratch_dir, temp_git_repo, tempdir};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

/// This thread's stand-in for the user's directories: the directory a test
/// gave [`with_root`], or else `home` in the process's scratch directory.
///
/// Not the scratch directory itself: that is the top of every upward search a
/// test makes from a directory of its own, and a `.claude` another test
/// installed in a shared home would turn up there as a project's.
pub fn root() -> PathBuf {
    ROOT.with(|root| root.borrow().clone())
        .unwrap_or_else(|| scratch_dir().join("home"))
}

/// Run `f` with `dir` standing in for the user's directories on this thread:
/// `user_dirs::home`, `config` and `data` all resolve under it. A test that
/// installs or removes files there then shares them with no test running
/// beside it. The previous root comes back when `f` returns or panics.
pub fn with_root<R>(dir: &Path, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ROOT.with(|root| *root.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(ROOT.with(|root| root.borrow_mut().replace(dir.to_path_buf())));
    f()
}

thread_local! {
    static ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// A test's stay in a directory of its own, from [`enter`]: while it lives,
/// `user_dirs::working_dir` — and through it `current_dir` and
/// `in_working_dir` — answer that directory on this thread. Dropping it puts
/// the previous answer back, also when the test panics.
///
/// The process's own working directory does not move. rtk resolves the working
/// directory through `user_dirs`, so the record is all a test needs, and tests
/// that enter a directory run in parallel rather than queueing on one shared
/// resource. A relative path handed straight to the filesystem is the
/// exception: it still resolves against the checkout the tests run from, so a
/// test writes its fixtures through the directory it entered.
pub struct Entered(Option<PathBuf>);

/// Enter `dir` for as long as the returned guard lives. `dir` must lie in the
/// scratch directory, the only place `user_dirs::working_dir` answers from.
pub fn enter(dir: &Path) -> Entered {
    assert!(
        in_scratch(dir),
        "enter a test_isolation::tempdir(), not {}: user_dirs::working_dir answers \
         only inside the scratch directory",
        dir.display()
    );
    Entered(ENTERED.with(|entered| entered.borrow_mut().replace(dir.to_path_buf())))
}

impl Drop for Entered {
    fn drop(&mut self) {
        ENTERED.with(|entered| *entered.borrow_mut() = self.0.take());
    }
}

/// The directory this thread [`enter`]ed, while the guard lives.
pub fn entered() -> Option<PathBuf> {
    ENTERED.with(|entered| entered.borrow().clone())
}

thread_local! {
    static ENTERED: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` as a test of an agent integration: `var` names `dir`, the agent's
/// own directory, and the user's directories resolve under `<tmp>/home` on
/// this thread. Keeping the home apart from `dir` is what catches code that
/// ignores the variable and falls back to the home.
pub fn with_agent_dir<R>(tmp: &Path, var: &str, dir: &Path, f: impl FnOnce() -> R) -> R {
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).expect("create the test's home");
    with_root(&home, || user_env::with_path(var, Some(dir), f))
}

/// Whether `path` lies inside the scratch directory, as spelt or with links
/// resolved: the working directory comes back resolved (`/private/var/…` on
/// macOS), while the scratch path is spelt as the temporary directory was.
pub fn in_scratch(path: &Path) -> bool {
    static RESOLVED: LazyLock<PathBuf> = LazyLock::new(|| {
        scratch_dir()
            .canonicalize()
            .unwrap_or_else(|_| scratch_dir().to_path_buf())
    });
    path.starts_with(scratch_dir()) || path.starts_with(&*RESOLVED)
}

/// This thread's stand-in for the project a command runs in, when the test has
/// not entered a directory of its own: `project` under [`root`], empty until
/// something writes there. Apart from the home, so a local install that lands
/// in the home instead is caught.
pub fn project() -> PathBuf {
    let project = root().join("project");
    std::fs::create_dir_all(&project).expect("create the test's project directory");
    project
}

/// The tracking database for this test thread's root.
pub fn db_path() -> PathBuf {
    data_dir().join(crate::core::constants::HISTORY_DB)
}

/// This thread's stand-in for `~/.local/share/rtk`.
fn data_dir() -> PathBuf {
    user_dirs::data().expect("a test build always resolves a data dir")
}

/// The binary the spawn tests exercise, in the same profile and target
/// directory as the harness asking for it. Private, so no test outside this
/// module can hand the binary to `Command::new` itself.
///
/// `tests/` gets this from `CARGO_BIN_EXE_rtk`, which cargo sets only for
/// integration targets.
fn rtk_bin() -> PathBuf {
    // The harness runs from `<target>/<profile>/deps/`, and cargo puts the bin
    // one level up from it. Taking the path from the running test covers a
    // custom target directory, `--target <triple>` and `--profile <name>`
    // alike; none of those reach the test binary as an environment variable.
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug"))
        .join(format!("rtk{}", std::env::consts::EXE_SUFFIX))
}

/// Whether the binary has been built, and built since the last edit to what
/// goes into it.
///
/// For a test that is not `#[ignore]`d and so has to skip quietly rather than
/// fail. `cargo test --all` rebuilds and uplifts the bin because the
/// integration targets need it, but `cargo test --bin rtk` builds only the
/// harness — the binary left over from an earlier build would otherwise be
/// spawned as though it were current.
pub fn rtk_binary_is_built() -> bool {
    let Ok(built) = std::fs::metadata(rtk_bin()).and_then(|m| m.modified()) else {
        return false;
    };
    match newest_source_change() {
        Some(newest) => built >= newest,
        None => true,
    }
}

/// When the binary's inputs were last touched, or `None` if the sources are
/// not there to look at: the Rust sources, the filters `build.rs` embeds from
/// `src/`, every file an `include_str!` or `include_bytes!` names outside the
/// test fixtures, and the build files. Anything else — a README, a hook's own
/// test script — cargo does not rebuild for, and counting it would leave the
/// binary looking stale for good.
fn newest_source_change() -> Option<std::time::SystemTime> {
    static EMBEDDED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)"#).expect("valid regex")
    });
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sources: Vec<PathBuf> = files_under(&root.join("src"))
        .into_iter()
        .filter(|f| f.extension().is_some_and(|e| e == "rs" || e == "toml"))
        .collect();
    let embedded: Vec<PathBuf> = sources
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "rs"))
        .filter_map(|f| Some((f.parent()?.to_path_buf(), std::fs::read_to_string(f).ok()?)))
        .flat_map(|(dir, code)| {
            EMBEDDED
                .captures_iter(&code)
                .map(|c| dir.join(&c[1]))
                .collect::<Vec<_>>()
        })
        .filter(|f| !f.components().any(|c| c.as_os_str() == "fixtures"))
        .collect();
    sources
        .iter()
        .chain(&embedded)
        .cloned()
        .chain(["build.rs", "Cargo.toml", "Cargo.lock"].map(|f| root.join(f)))
        .filter_map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
        .max()
}

/// Every file under `dir`.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                files_under(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

/// Build a `Command` for the rtk binary with the data it writes redirected to
/// this thread's [`root`]: the scratch directory, or the test's own.
///
/// `tests/common/mod.rs` holds the twin of this for the integration tests.
pub fn rtk_command() -> Command {
    let bin = rtk_bin();
    assert!(
        bin.exists(),
        "rtk binary not found at {} — run `cargo build` first",
        bin.display()
    );
    // nosemgrep: dynamic-command-execution — rtk_bin's own build artefact, not a caller's program name
    let mut cmd = Command::new(bin);
    redirect_rtk_data_to(&mut cmd, &root());
    cmd
}

/// The ways a test names the rtk binary: as cargo hands it to an integration
/// test, as the installed one on `PATH`, or as a path built into the target
/// directory. The scan rejects them anywhere but [`CONSTRUCTORS`]. It catches
/// the accident, not every spelling: a path assembled with `format!` or
/// `concat!` gets through, while matching `target/debug` anywhere would refuse
/// the cargo filter's own fixtures.
const NAMES_RTK: [&str; 4] = [
    "CARGO_BIN_EXE_rtk",
    "Command::new(\"rtk\")",
    "join(\"debug\")",
    "\"target/debug",
];

/// The two files allowed to name the binary: this module's `rtk_command` and
/// its `tests/` twin.
const CONSTRUCTORS: [&str; 2] = ["src/core/test_isolation/mod.rs", "tests/common/mod.rs"];

/// A test that spawns the rtk binary must redirect the child's data, or the
/// run lands in the developer's real history.
///
/// `rtk_bin` is private, so no file under `src/` reaches the binary's path
/// through this module. `tests/` receives it from cargo instead, which
/// visibility has nothing to say about, and this is what covers that.
#[test]
fn tests_that_spawn_rtk_isolate_their_data() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    let mut constructors_seen = Vec::new();

    for dir in ["src", "tests"] {
        for path in rust_files(&root.join(dir)) {
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string()
                .replace('\\', "/");
            if CONSTRUCTORS.contains(&relative.as_str()) {
                constructors_seen.push(relative);
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read source");
            if let Some(name) = NAMES_RTK.iter().find(|name| src.contains(*name)) {
                offenders.push(format!("{relative}: {name}"));
            }
        }
    }

    // Both constructors turning up proves the tree was actually walked. The
    // sources are found through a path baked in at compile time, so a run on a
    // machine that only has the built artefacts would otherwise scan nothing
    // and report success.
    constructors_seen.sort();
    assert_eq!(
        constructors_seen,
        CONSTRUCTORS,
        "the scan did not reach {:?}",
        CONSTRUCTORS
            .iter()
            .filter(|c| !constructors_seen.iter().any(|s| s == *c))
            .collect::<Vec<_>>()
    );

    offenders.sort();
    assert!(
        offenders.is_empty(),
        "these files name the rtk binary themselves, and a child spawned from \
         that path writes to the developer's real ~/.local/share/rtk/ — \
         fabricating rows in their savings history, evicting the raw output \
         kept there for recovery, and risking database corruption when several \
         children write at once. Build the command with `common::rtk_command()` \
         under tests/, or `test_isolation::rtk_command()` under src/:\n  {}",
        offenders.join("\n  ")
    );
}

/// The files allowed to resolve a user location or read the environment
/// themselves: `user_dirs` and `user_env`, which are the redirects, and this
/// scanner, which carries the patterns as literals.
const RESOLVERS: [&str; 3] = [
    "src/core/user_dirs.rs",
    "src/core/user_env.rs",
    "src/core/test_isolation/mod.rs",
];

/// A use of the `dirs` or `home` crate, or of `std::env::home_dir`. Word
/// boundaries keep `user_dirs::` and names merely ending in `dirs` out.
static DIRS_USE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:dirs::|home::home_dir|env::home_dir)|\buse\s+(?:dirs|home)\b")
        .expect("valid regex")
});

/// An environment read — `var`, `var_os`, `vars` or `vars_os` — up to its
/// opening parenthesis, capturing a `user_env::` in front of it when the read
/// goes through the accessor. [`call_text`] takes it on to the closing one.
static ENV_READ: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\buser_env::)?\bvars?(?:_os)?\(").expect("valid regex"));

/// `env::var` and its siblings named without being called — imported, aliased,
/// or passed as a function — which [`ENV_READ`] cannot see the argument of.
static ENV_READ_BY_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\benv::(?:\{[^}]*\bvars?(?:_os)?\b|vars?(?:_os)?\s+as\b|vars?(?:_os)?\s*[^\s(\w])")
        .expect("valid regex")
});

/// A read of the process's working directory, which `user_dirs::current_dir`
/// answers for code that runs under a test: called, imported, or passed as a
/// function. Relative paths resolve against it too, and no scan sees those;
/// `user_dirs::in_working_dir` is their counterpart.
static CURRENT_DIR_READ: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\benv::(?:current_dir\b|\{[^}]*\bcurrent_dir\b)").expect("valid regex")
});

/// `code` without its `#[cfg(test)]` items: the test modules, and the
/// test-only helpers beside them, which may look at the real working directory
/// — `Entered` has to, to put it back. An item runs to its first `;` or to the
/// brace closing its first `{`, whichever comes first; braces inside string and
/// character literals are skipped. The attribute on anything smaller than an
/// item — a statement, a field, a match arm — strips too much, which only ever
/// hides code from the scan.
fn without_test_items(code: &str) -> String {
    let mut kept = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(at) = rest.find("#[cfg(test)]") {
        kept.push_str(&rest[..at]);
        let after = &rest[at + "#[cfg(test)]".len()..];
        rest = &after[item_end(after)..];
    }
    kept.push_str(rest);
    kept
}

/// Where the item starting in `code` ends, as [`without_test_items`] reads it.
fn item_end(code: &str) -> usize {
    let mut depth = 0usize;
    let mut chars = code.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => {
                while let Some((_, c)) = chars.next() {
                    match c {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '\'' if code[at..].starts_with("'{'") || code[at..].starts_with("'}'") => {
                chars.next();
                chars.next();
            }
            ';' if depth == 0 => return at + 1,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
    }
    code.len()
}

/// A read through the accessors — `env_path`, `user_env::var` or `var_os` —
/// up to its opening parenthesis.
static ACCESSOR_READ: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:env_path|user_env::var(?:_os)?)\(").expect("valid regex"));

/// A call from `start` through the parenthesis matching the one that ends
/// `head`, with its argument, however deeply that nests. The whole rest of the
/// file when the parentheses never balance, so a malformed read still shows.
fn call_text<'a>(code: &'a str, head: regex::Match<'_>) -> (&'a str, &'a str) {
    let open = head.end() - 1;
    let mut depth = 0usize;
    for (offset, c) in code[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let close = open + offset;
                    return (&code[head.start()..=close], code[open + 1..close].trim());
                }
            }
            _ => {}
        }
    }
    (&code[head.start()..], &code[open + 1..])
}

/// The environment reads that stay direct, as written, each with its reason.
/// Each covers one read: a second identical call in the same file is refused.
const DIRECT_READS: [(&str, &str); 4] = [
    // Builds the `PATH` a Windows test hands a child, extending the one it runs
    // under so the child still finds its tools.
    ("src/core/utils.rs", r#"var_os("PATH")"#),
    // Only `rtk hook audit` reaches it, and no test runs that in-process.
    // `user_dirs::home` would move it to the Windows profile folder, which is
    // #1681's question, not this scan's.
    ("src/hooks/hook_audit_cmd.rs", r#"var("HOME")"#),
    // `rtk env` shows the environment it runs in; that is its output.
    ("src/cmds/system/env_cmd.rs", "vars()"),
    // Finds the `RTK_` variables a child would inherit, to clear them.
    ("src/core/test_isolation/scratch.rs", "vars_os()"),
];

/// The environment variable an accessor call names: a literal names itself, a
/// constant is looked up. `None` for anything else, such as a name built at
/// run time.
fn env_var_name(arg: &str) -> Option<String> {
    use crate::hooks::constants::{COPILOT_HOME_ENV, DROID_HOME_ENV, PI_CODING_AGENT_DIR_ENV};
    if let Some(literal) = arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')) {
        return Some(literal.to_string());
    }
    let value = match arg.rsplit("::").next().unwrap_or(arg) {
        "COPILOT_HOME_ENV" => COPILOT_HOME_ENV,
        "DROID_HOME_ENV" => DROID_HOME_ENV,
        "PI_CODING_AGENT_DIR_ENV" => PI_CODING_AGENT_DIR_ENV,
        "REWRITE_HOST_ENV" => crate::hooks::decision::REWRITE_HOST_ENV,
        "TELEMETRY_DISABLED_ENV" => crate::core::telemetry_cmd::TELEMETRY_DISABLED_ENV,
        _ => return None,
    };
    Some(value.to_string())
}

/// Source files under `src/`, as repository-relative paths with their code:
/// every line but `//` comments, so prose naming a pattern is not a use of it.
fn source_code() -> Vec<(String, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    rust_files(&root.join("src"))
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string()
                .replace('\\', "/");
            let code = std::fs::read_to_string(&path)
                .expect("read source")
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            (relative, code)
        })
        .collect()
}

/// Resolving a user location anywhere but `user_dirs`, or reading the
/// environment anywhere but `user_env` and `user_dirs::env_path`, bypasses
/// their `cfg(test)` redirects. A test reaching such a read then reads, writes
/// or deletes the developer's own files (`telemetry forget` and the legacy-hook
/// migration both delete), or takes its result from their settings and shell.
///
/// Every direct `var`/`var_os` read is refused, whatever it names, except the
/// few in [`DIRECT_READS`]; each of those must still be there, so an exemption
/// cannot outlive its read and wave a later one through. This catches the
/// accident, not a determined bypass: a name built at run time slips through.
#[test]
fn only_user_dirs_resolves_user_locations() {
    let mut offenders = Vec::new();
    let mut resolvers_seen = Vec::new();
    let mut exemptions_used = Vec::new();

    for (relative, code) in source_code() {
        if RESOLVERS.contains(&relative.as_str()) {
            resolvers_seen.push(relative);
            continue;
        }
        let production = without_test_items(&code);
        for found in DIRS_USE
            .find_iter(&code)
            .chain(ENV_READ_BY_NAME.find_iter(&code))
            .chain(CURRENT_DIR_READ.find_iter(&production))
        {
            offenders.push(format!("{relative}: {}", found.as_str()));
        }
        for read in ENV_READ.captures_iter(&code) {
            if read.get(1).is_some() {
                continue;
            }
            let (text, _) = call_text(&code, read.get(0).expect("whole match"));
            let call = format!("{relative}: {text}");
            let exempt = DIRECT_READS.contains(&(relative.as_str(), text));
            if exempt && !exemptions_used.contains(&call) {
                exemptions_used.push(call);
            } else {
                offenders.push(call);
            }
        }
    }

    resolvers_seen.sort();
    let mut expected = RESOLVERS.to_vec();
    expected.sort_unstable();
    assert_eq!(
        resolvers_seen,
        expected,
        "the scan did not reach {:?}",
        RESOLVERS
            .iter()
            .filter(|r| !resolvers_seen.iter().any(|s| s == *r))
            .collect::<Vec<_>>()
    );
    let stale: Vec<_> = DIRECT_READS
        .iter()
        .map(|(file, arg)| format!("{file}: {arg}"))
        .filter(|exemption| !exemptions_used.contains(exemption))
        .collect();
    offenders.sort();
    assert!(
        offenders.is_empty() && stale.is_empty(),
        "these files resolve a user location or read the environment \
         themselves, instead of through `user_dirs` (`home`, `config`, `data`, \
         `current_dir`, `env_path` for a path) or `user_env::var`, so a test \
         reaching them touches, or takes its result from, the developer's own \
         files and shell:\n  {}\n\
         these DIRECT_READS exemptions match no read any more; remove them:\n  {}",
        offenders.join("\n  "),
        stale.join("\n  ")
    );
}

/// `user_env` keeps a test on its own side, but a spawned rtk is built without
/// `cfg(test)` and obeys whatever it inherits. Every variable rtk reads must
/// therefore be pinned — to a fixed value, or a path inside the scratch
/// directory — or removed by
/// `redirect_rtk_data`; one it merely inherits makes the child behave as the
/// developer's shell has it, or sends it to their directories.
#[test]
fn every_variable_rtk_reads_is_redirected_for_a_child() {
    let mut read = Vec::new();
    for (relative, code) in source_code() {
        if RESOLVERS.contains(&relative.as_str()) {
            continue;
        }
        for head in ACCESSOR_READ.find_iter(&code) {
            let (text, arg) = call_text(&code, head);
            let name = env_var_name(arg.trim_end_matches(',').trim()).unwrap_or_else(|| {
                panic!(
                    "{relative}: {text} names no variable `env_var_name` can resolve; \
                     name it with a literal, or add the constant there"
                )
            });
            read.push((relative.clone(), name));
        }
    }
    assert!(
        !read.is_empty(),
        "the scan found no read through the accessors"
    );

    // Never spawned: built only to read back what `redirect_rtk_data` sets.
    let mut cmd = Command::new("rtk");
    scratch::redirect_rtk_data(&mut cmd);
    let envs: Vec<_> = cmd.get_envs().collect();
    let mut leaks: Vec<_> = read
        .iter()
        .filter(|(_, name)| {
            match envs.iter().find(|(key, _)| *key == name.as_str()) {
                Some((_, None)) => false,
                // Pinned: a fixed switch, or a path that must lie in scratch.
                Some((_, Some(value))) => {
                    let value = Path::new(value);
                    value.is_absolute() && !value.starts_with(scratch_dir())
                }
                // Every inherited `RTK_` variable is cleared by prefix, and
                // appears here only when this shell exports it.
                None => !name.starts_with("RTK_"),
            }
        })
        .map(|(file, name)| format!("{file}: {name}"))
        .collect();
    leaks.sort();
    leaks.dedup();
    assert!(
        leaks.is_empty(),
        "a spawned rtk inherits these from the developer's shell; pin or remove \
         them in `redirect_rtk_data`:\n  {}",
        leaks.join("\n  ")
    );
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

/// `user_dirs` and the variables `redirect_rtk_data` hands a child are
/// separate literals. Reading the child's values off a real `Command`, rather
/// than restating them, is what makes a change to either side alone fail here
/// instead of sending an in-process writer and a spawned `rtk` to different
/// directories.
///
/// Linux only, the one platform where `dirs` takes both directories from those
/// variables. On macOS a child resolves them under
/// `$HOME/Library/Application Support`, inside the scratch directory but not
/// where the in-process build looks; on Windows, in the real known folders.
/// There, state written in-process is not visible to a child.
#[cfg(target_os = "linux")]
#[test]
fn user_dirs_agree_with_the_child_redirect() {
    assert_user_dirs_agree_with_the_child();
    let own = tempdir();
    with_root(own.path(), assert_user_dirs_agree_with_the_child);
}

#[cfg(target_os = "linux")]
fn assert_user_dirs_agree_with_the_child() {
    // Never spawned: built as `rtk_command` builds it, to read back what the
    // redirect sets.
    let mut cmd = Command::new("rtk");
    redirect_rtk_data_to(&mut cmd, &root());
    let child = |name: &str| {
        cmd.get_envs()
            .find(|(key, _)| *key == name)
            .and_then(|(_, value)| value)
            .map(PathBuf::from)
            .unwrap_or_else(|| panic!("redirect_rtk_data pins {name}"))
    };
    let rtk = crate::core::constants::RTK_DATA_DIR;
    assert_eq!(
        user_dirs::config().expect("a test build always resolves a config dir"),
        child("XDG_CONFIG_HOME").join(rtk)
    );
    assert_eq!(
        user_dirs::data().expect("a test build always resolves a data dir"),
        child("XDG_DATA_HOME").join(rtk)
    );
}

#[test]
fn entered_restores_after_a_panic() {
    let tmp = tempdir();
    let before = entered();
    let panicked = std::panic::catch_unwind(|| {
        let _entered = enter(tmp.path());
        assert_eq!(entered().as_deref(), Some(tmp.path()));
        panic!("the call under test fails");
    });
    assert!(panicked.is_err(), "the panic must propagate");
    assert_eq!(
        entered(),
        before,
        "a panic must not leave the thread in the test directory"
    );
}
