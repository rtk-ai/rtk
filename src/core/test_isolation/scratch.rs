//! A throwaway directory for whatever a test process would otherwise write
//! under `~/.local/share/rtk/`, and the environment a child rtk or git starts
//! from: what it keeps of the developer's, and what it gets pinned into that
//! directory instead.
//!
//! `tests/common/mod.rs` compiles this file into its own target by path, so
//! everything here must stay free of `crate::` references.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
#[cfg(unix)]
use std::sync::OnceLock;

/// Isolate a spawned rtk: its data in this process's scratch directory, and
/// nothing from the developer's environment but what `kept` names.
///
/// The child is built without `cfg(test)`, so `user_dirs::data`'s redirect
/// does not reach it and the environment is the only channel. `RTK_DB_PATH`,
/// `RTK_TEE_DIR` and `RTK_RECALL_DB` name the files a run writes directly. The rest —
/// telemetry salt, trust store, and the marker `hook_check::maybe_warn` writes
/// to rate-limit the developer's once-a-day warning — resolves through `dirs`,
/// which reads `XDG_DATA_HOME` on Linux and `HOME` on macOS. `XDG_CONFIG_HOME`
/// joins them so a child reads the same configuration on every machine.
///
/// Windows resolves its known folders through the shell API rather than the
/// environment, so there only the named files are redirected; the profile
/// variables are pinned for the tools a child runs, which read them instead.
pub fn isolate_rtk(cmd: &mut Command) {
    isolate_rtk_in(cmd, &child_home());
}

/// The environment [`isolate_rtk`] gives a child, leaving where the command
/// starts alone: for a native tool or git whose output a test compares with
/// the child's, so what only the developer's shell exports, a ripgrep
/// configuration file or `COLUMNS` say, reaches neither side rather than one.
pub fn isolate_environment(cmd: &mut Command) {
    isolate_environment_in(cmd, &child_home());
}

/// The home a child gets from [`isolate_rtk`], whose configuration the git a
/// test runs reads too.
fn child_home() -> PathBuf {
    scratch_dir().join("home")
}

/// [`isolate_rtk`], with `root` standing in for the scratch directory:
/// a unit test that has its own root hands the child the same one, so what it
/// writes in-process and what the child reads are the same files.
pub fn isolate_rtk_in(cmd: &mut Command, root: &Path) {
    // The child starts in the root's project directory, where an in-process
    // test with the same root looks too, rather than in the checkout the tests
    // run from, whose untracked settings (`.claude/settings.local.json`) its
    // project lookups would read. A test that wants a project of its own sets
    // `current_dir` afterwards.
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap_or_else(|e| {
        panic!(
            "create the child's project directory {}: {e}",
            project.display()
        )
    });
    cmd.current_dir(project);
    isolate_environment_in(cmd, root);
}

/// [`isolate_environment`], with `root` standing in for the scratch directory.
fn isolate_environment_in(cmd: &mut Command, root: &Path) {
    // A child is built without `cfg(test)`, so it obeys every variable it
    // inherits, and so does every tool it runs: rtk's own (`RTK_TEE=0`, …), the
    // agents' directories, and the options a shell script takes from its
    // environment would make it behave as the developer's shell has it, or write
    // where their directories are. It starts from an empty environment and keeps
    // only what `kept` names, so the paths pinned next stand, and a test that
    // wants a variable sets it on the command afterwards.
    cmd.env_clear();
    cmd.envs(kept_from_parent());
    // The git a child runs sees the same repository and configuration as the
    // raw git a test compares it with. The locale is left to the test: one
    // comparing the child's output with a native tool pins both or neither.
    pin_git(cmd);
    pin_home(cmd, root);

    cmd.env("RTK_DB_PATH", root.join("rtk").join("history.db"))
        .env("RTK_TEE_DIR", root.join("rtk").join("tee"))
        .env("RTK_RECALL_DB", root.join("rtk").join("recall.db"))
        .env("XDG_DATA_HOME", root)
        // Set although the environment starts empty: on Windows the child
        // still reads the developer's real config directory, and a consent
        // recorded there would let it send a ping.
        .env("RTK_TELEMETRY_DISABLED", "1");
    // The profile a Windows tool the child runs reads from the environment, as
    // cargo and git do, rather than through the shell API.
    #[cfg(windows)]
    {
        let roaming = root.join("AppData").join("Roaming");
        let local = root.join("AppData").join("Local");
        for dir in [&roaming, &local] {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| {
                panic!(
                    "create the child's profile directory {}: {e}",
                    dir.display()
                )
            });
        }
        cmd.env("USERPROFILE", root)
            .env("APPDATA", roaming)
            .env("LOCALAPPDATA", local);
    }
}

/// This process's scratch directory, named by `tempfile` so nothing can
/// pre-empt the path.
///
/// rtk chmods its database's parent to 0700 via `create_private_dir`, so the
/// database needs a directory of rtk's own rather than the shared temp root.
pub fn scratch_dir() -> &'static Path {
    static DIR: LazyLock<PathBuf> = LazyLock::new(|| {
        let dir = tempfile::Builder::new()
            .prefix("rtk-test-")
            .tempdir()
            .expect("create scratch directory for test data")
            .keep();
        remove_at_exit(&dir);
        dir
    });
    &DIR
}

/// Delete `dir` when the test binary exits.
///
/// The directory outlives every test in the process, so it is held in a
/// `static`, which Rust does not drop. `exit(3)` runs the handler registered
/// here whether the run passed or failed.
///
/// Two cases leave the directory in place: a run killed by a signal, and any
/// non-Unix platform, where there is no `atexit` to hand this to.
fn remove_at_exit(dir: &Path) {
    #[cfg(unix)]
    {
        static DOOMED: OnceLock<PathBuf> = OnceLock::new();

        extern "C" fn remove() {
            if let Some(dir) = DOOMED.get() {
                // nosemgrep: filesystem-deletion -- test-only cleanup of this process's own scratch directory, not production/user data.
                let _ = std::fs::remove_dir_all(dir);
            }
        }

        if DOOMED.set(dir.to_path_buf()).is_ok() {
            #[allow(unsafe_code)]
            // nosemgrep: unsafe-block — libc::atexit, as main.rs does for SIGPIPE; test-only
            unsafe {
                libc::atexit(remove);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// What a child keeps from the developer's environment, beyond what isolation
/// pins: where its tools are (`PATH`), what a tool installed outside the
/// system's directories needs to start (`LD_LIBRARY_PATH`, and nix-ld's loader
/// and libraries for a binary built for another distribution), and where
/// temporary files go (`TMPDIR`), whose path a socket there must fit within;
/// the locale and the time zone with the data and character set converters
/// they load, which a test comparing the child's output with a native tool
/// pins for both or neither; and the toolchain rustup sets for the tools it
/// proxies, so a child's `cargo` is the suite's once `HOME` no longer leads to
/// it, though with `CARGO_HOME` under the scratch home. A test binary run
/// outside cargo has them only if the shell exports them.
const KEPT: &[&str] = &[
    "PATH",
    "LD_LIBRARY_PATH",
    "NIX_LD",
    "NIX_LD_LIBRARY_PATH",
    "TMPDIR",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_ADDRESS",
    "LC_COLLATE",
    "LC_CTYPE",
    "LC_IDENTIFICATION",
    "LC_MEASUREMENT",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NAME",
    "LC_NUMERIC",
    "LC_PAPER",
    "LC_TELEPHONE",
    "LC_TIME",
    "LOCALE_ARCHIVE",
    "LOCPATH",
    "GUIX_LOCPATH",
    "NLSPATH",
    "GCONV_PATH",
    "TZ",
    "TZDIR",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
];

/// What a Windows child keeps besides: the system and program directories its
/// DLLs and tools resolve from, on every architecture, and the drive they sit
/// on; the temporary directory; the extensions `PATH` lookups try; the command
/// interpreter; and the system and processor a script or tool detects.
#[cfg(windows)]
const WINDOWS_KEPT: &[&str] = &[
    "OS",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "WINDIR",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMDATA",
    "COMMONPROGRAMFILES",
    "COMMONPROGRAMFILES(X86)",
    "PROGRAMW6432",
    "COMMONPROGRAMW6432",
    "PROGRAMFILES(ARM)",
    "COMMONPROGRAMFILES(ARM)",
    "TEMP",
    "TMP",
    "PATHEXT",
    "COMSPEC",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_ARCHITEW6432",
    "NUMBER_OF_PROCESSORS",
];

/// Whether a child keeps `name` from the developer's environment. Windows
/// matches variable names regardless of case, every other platform exactly.
pub(crate) fn kept(name: &str) -> bool {
    #[cfg(windows)]
    {
        KEPT.iter()
            .chain(WINDOWS_KEPT)
            .any(|kept| kept.eq_ignore_ascii_case(name))
    }
    #[cfg(not(windows))]
    {
        KEPT.contains(&name)
    }
}

/// The developer's values of the variables a child keeps.
fn kept_from_parent() -> impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)> {
    inherited().filter(|(name, _)| name.to_str().is_some_and(kept))
}

/// The environment this process inherited from the developer's shell. Every
/// read of it in this file goes through here, the one read site the scan of
/// direct environment reads allows the file.
fn inherited() -> std::env::VarsOs {
    std::env::vars_os()
}

/// Run git as nothing on the developer's machine configures it, in the
/// environment an rtk child's git gets: no global or system configuration, no
/// `~/.config/git/ignore` or `attributes`, an empty template for `git init`, no
/// repository above the temporary directory, and nothing their shell exports
/// but what `kept` names, so none of the `GIT_` variables that point git at
/// another repository, add configuration, swap the diff tool or trace to
/// stderr; and messages in English. A signing requirement, a hook in an init
/// template, an excluded file pattern or an exported `GIT_DIR` would otherwise
/// fail a test's setup, or change what it compares. It clears whatever the
/// command set before, so call it before setting the test's own variables.
pub fn isolate_git(cmd: &mut Command) {
    isolate_environment(cmd);
    cmd.env("LC_ALL", "C").env_remove("LANGUAGE");
}

/// The configuration half of [`isolate_git`], for the git rtk's own code spawns
/// in a test, which inherits the test process's environment: it removes the
/// `GIT_` variables and pins git's configuration, and leaves the locale alone,
/// since which language that git speaks is rtk's decision and part of what the
/// test checks.
pub fn isolate_git_config(cmd: &mut Command) {
    for name in inherited_with_prefix("GIT_") {
        cmd.env_remove(name);
    }
    pin_home(cmd, &child_home());
    pin_git(cmd);
}

/// Make `home` the home, and its `.config` the configuration directory, of
/// the command and of the git it runs, creating it for a command that runs
/// before any rtk child has.
fn pin_home(cmd: &mut Command, home: &Path) {
    std::fs::create_dir_all(home)
        .unwrap_or_else(|e| panic!("create the child's home {}: {e}", home.display()));
    cmd.env("XDG_CONFIG_HOME", home.join(".config"))
        .env("HOME", home);
}

/// Point git at no global or system configuration, an empty template for
/// `git init`, and no repository above the temporary directory.
fn pin_git(cmd: &mut Command) {
    static TEMPLATE: LazyLock<PathBuf> = LazyLock::new(|| {
        // An empty template copies nothing; one that does not exist makes
        // `git init` warn.
        let template = scratch_dir().join("git-template");
        let _ = std::fs::create_dir_all(&template);
        template
    });
    cmd.env("GIT_CONFIG_GLOBAL", scratch_dir().join("no-gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TEMPLATE_DIR", &*TEMPLATE)
        // No search for a repository above the temporary directory, where a
        // test's directories live: one found there — on Windows the temporary
        // directory sits under the profile, which may be a dotfiles repository —
        // would answer for the directory the test gave.
        .env("GIT_CEILING_DIRECTORIES", std::env::temp_dir());
}

/// A temporary directory inside this process's scratch directory, removed
/// with the rest of it should the test not get to drop it, and inside the
/// reach of `user_dirs::working_dir` for a test that moves into it.
pub fn tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("t-")
        .tempdir_in(scratch_dir())
        .expect("create a temporary directory in the scratch directory")
}

/// A throwaway git repository with one commit, set up with git isolated as
/// [`isolate_git`] runs it and its identity set locally, so it works with no
/// user git configuration at all. The `TempDir` deletes it on drop, so keep it
/// alive for the test.
pub fn temp_git_repo() -> tempfile::TempDir {
    let dir = tempdir();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "rtk-test@example.invalid"][..],
        &["config", "user.name", "rtk test"][..],
        &["commit", "-q", "--allow-empty", "-m", "init"][..],
    ] {
        let mut git = Command::new("git");
        git.args(args).current_dir(dir.path());
        isolate_git(&mut git);
        let ok = git
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
        assert!(ok, "git setup failed: {args:?}");
    }
    dir
}

/// The inherited variables whose names start with `prefix`, in any case:
/// Windows looks names up without regard to it.
fn inherited_with_prefix(prefix: &str) -> Vec<std::ffi::OsString> {
    inherited()
        .map(|(name, _)| name)
        .filter(|name| {
            name.to_str()
                .is_some_and(|name| name.to_ascii_uppercase().starts_with(prefix))
        })
        .collect()
}
