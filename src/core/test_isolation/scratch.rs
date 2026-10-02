//! A throwaway directory for whatever a test process would otherwise write
//! under `~/.local/share/rtk/`.
//!
//! `tests/common/mod.rs` compiles this file into its own target by path, so
//! everything here must stay free of `crate::` references.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
#[cfg(unix)]
use std::sync::OnceLock;

/// Point a spawned rtk at this process's scratch directory.
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
/// environment, so there only the named files are redirected.
pub fn redirect_rtk_data(cmd: &mut Command) {
    redirect_rtk_data_to(cmd, &scratch_dir().join("home"));
}

/// [`redirect_rtk_data`], with `root` standing in for the scratch directory:
/// a unit test that has its own root hands the child the same one, so what it
/// writes in-process and what the child reads are the same files.
pub fn redirect_rtk_data_to(cmd: &mut Command, root: &Path) {
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

    // A child is built without `cfg(test)`, so it obeys every variable it
    // inherits. rtk's own (`RTK_TEE=0`, `RTK_NO_TOML=1`, …) and the others it
    // reads would make it behave as the developer's shell has it, or write
    // where their directories are; they are cleared first, so the paths pinned
    // next stand, and a test that wants one sets it on the command afterwards.
    for name in inherited_with_prefix("RTK_") {
        cmd.env_remove(name);
    }
    for inherited in INHERITED_VARS {
        cmd.env_remove(inherited);
    }
    // The git a child runs sees the same repository and configuration as the
    // raw git a test compares it with. The locale is left to the test: one
    // comparing the child's output with a native tool pins both or neither.
    isolate_git_config(cmd);

    cmd.env("RTK_DB_PATH", root.join("rtk").join("history.db"))
        .env("RTK_TEE_DIR", root.join("rtk").join("tee"))
        .env("RTK_RECALL_DB", root.join("rtk").join("recall.db"))
        .env("XDG_DATA_HOME", root)
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .env("HOME", root)
        // Cleared with the other `RTK_` variables, but set again: on Windows
        // the child still reads the developer's real config directory, and a
        // consent recorded there would let it send a ping.
        .env("RTK_TELEMETRY_DISABLED", "1");
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

/// The variables rtk reads that are not its own: the agents' directories,
/// which lie outside the scratch tree; Gemini's workspace trust, which decides
/// whether a project's settings are read; the CI indicators the filter-trust
/// override checks; and Composer's bin directory, which decides which tool a
/// PHP command runs.
const INHERITED_VARS: [&str; 13] = [
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "HERMES_HOME",
    "COPILOT_HOME",
    "PI_CODING_AGENT_DIR",
    "FACTORY_HOME_OVERRIDE",
    "GEMINI_CLI_TRUST_WORKSPACE",
    "CI",
    "GITHUB_ACTIONS",
    "GITLAB_CI",
    "JENKINS_URL",
    "BUILDKITE",
    "COMPOSER_BIN_DIR",
];

/// Run git as nothing on the developer's machine configures it: no global or
/// system configuration, no `~/.config/git/ignore` or `attributes`, an empty
/// template for `git init`, no repository above the temporary directory,
/// messages in English, and none of the `GIT_`
/// variables their shell exports — the ones that point git at another
/// repository, add configuration, swap the diff tool or trace to stderr. A
/// signing requirement, a hook in an init template, an excluded file pattern
/// or an exported `GIT_DIR` would otherwise fail a test's setup, or change what
/// it compares.
pub fn isolate_git(cmd: &mut Command) {
    isolate_git_config(cmd);
    cmd.env("LC_ALL", "C").env_remove("LANGUAGE");
}

/// [`isolate_git`] without the locale, for the git rtk's own code spawns in a
/// test: which language that git speaks is rtk's decision, and part of what
/// the test checks.
pub fn isolate_git_config(cmd: &mut Command) {
    for name in inherited_with_prefix("GIT_") {
        cmd.env_remove(name);
    }
    static TEMPLATE: LazyLock<PathBuf> = LazyLock::new(|| {
        // An empty template copies nothing; one that does not exist makes
        // `git init` warn.
        let template = scratch_dir().join("git-template");
        let _ = std::fs::create_dir_all(&template);
        template
    });
    let root = scratch_dir();
    let home = root.join("home");
    cmd.env("GIT_CONFIG_GLOBAL", root.join("no-gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TEMPLATE_DIR", &*TEMPLATE)
        // The same home a child gets from `redirect_rtk_data`, so isolating a
        // child's git does not move its configuration.
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("HOME", home)
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
    std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| {
            name.to_str()
                .is_some_and(|name| name.to_ascii_uppercase().starts_with(prefix))
        })
        .collect()
}
