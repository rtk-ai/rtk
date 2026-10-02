//! Where the user's own files live, and the single place that asks `dirs`.
//!
//! Every path outside the project that rtk reads, writes or deletes resolves
//! here: its own data and config directories, and the home the agents' own
//! directories hang off; so does the project itself, where rtk looks for
//! project-scoped settings. In a test build each answer lies in the scratch
//! directory instead, so `cargo test` neither touches the developer's files nor takes its
//! results from them.
//!
//! `data` and `config` name rtk's own directory inside the platform's; `home` is
//! the exception and names the home itself, because rtk does not own it — the
//! agents' directories (`~/.claude`, `~/.cursor`) are what hang off it.
//!
//! Calling `dirs` anywhere else bypasses the redirect, so
//! `test_isolation::only_user_dirs_resolves_user_locations` refuses it, and
//! any direct environment read with it: a path through [`env_path`], anything
//! else through `user_env`.

use super::constants;
#[cfg(test)]
use crate::core::test_isolation;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Everything rtk keeps on disk: `~/.local/share/rtk` and its platform
/// equivalents. The tracking database, tee spool, telemetry salt, trust store
/// and hook markers all hang off this.
///
/// `None` when the platform gives no answer, which `dirs` reports for a
/// container UID with no passwd entry and no `HOME`. Callers that accept a
/// path from the environment or the config file check that first — see
/// `tracking::get_db_path`.
///
/// In a test build it is under `test_isolation::root`, so a `cargo test` run
/// leaves the developer's own untouched.
pub fn data() -> Option<PathBuf> {
    #[cfg(not(test))]
    {
        dirs::data_local_dir().map(|d| d.join(constants::RTK_DATA_DIR))
    }
    #[cfg(test)]
    {
        Some(test_isolation::root().join(constants::RTK_DATA_DIR))
    }
}

/// rtk's own directory inside the user's configuration directory, holding
/// `filters.toml` and `config.toml`.
///
/// In a test build it is under `test_isolation::root`, so a `cargo test` run
/// leaves the developer's own untouched. It sits under the directory
/// `test_isolation::scratch::redirect_rtk_data` pins `XDG_CONFIG_HOME` to, so on
/// Linux an in-process writer and a spawned `rtk` resolve the same file. A child
/// resolves elsewhere on the other two platforms, where `dirs::config_dir()`
/// reads no `XDG_CONFIG_HOME`: `$HOME/Library/Application Support/rtk` on macOS,
/// and on Windows the real RoamingAppData, which the environment cannot redirect.
///
/// Redirecting in Rust rather than through the environment is what makes the
/// in-process half hold on all three.
pub fn config() -> Option<PathBuf> {
    #[cfg(not(test))]
    {
        dirs::config_dir().map(|d| d.join(constants::RTK_DATA_DIR))
    }
    #[cfg(test)]
    {
        Some(
            test_isolation::root()
                .join(".config")
                .join(constants::RTK_DATA_DIR),
        )
    }
}

/// The user's home directory, the root every agent's own config directory hangs
/// off (`~/.claude`, `~/.cursor`, `~/.config/opencode`, …).
///
/// In a test build it is `test_isolation::root`, so a test that installs or
/// uninstalls an agent integration cannot read, rewrite or delete the files the
/// developer has there. That includes the destructive paths: the legacy-hook
/// migration removes files under it, and `uninstall` rewrites `hooks.json`.
pub fn home() -> Option<PathBuf> {
    #[cfg(not(test))]
    {
        dirs::home_dir()
    }
    #[cfg(test)]
    {
        Some(test_isolation::root())
    }
}

/// [`data`], falling back to rtk's own directory under `root` where the
/// platform gives no answer. For callers that carry on without one rather
/// than failing.
pub fn data_under(root: &str) -> PathBuf {
    data().unwrap_or_else(|| PathBuf::from(root).join(constants::RTK_DATA_DIR))
}

/// The working directory, for project-scoped lookups (`.github/hooks/`,
/// `.rtk/filters/`).
///
/// In a test build, only the scratch directory a test has entered through
/// `test_isolation::enter`, on its own thread. The process's working directory
/// is the developer's checkout, and what they keep there untracked is theirs,
/// not the test's; and it is shared, so a test that moved elsewhere would
/// answer for every other one running beside it.
///
/// Relative paths (`composer.json`, `.rtk/filters.toml`) resolve against the
/// same directory, so they go through [`in_working_dir`].
pub fn working_dir() -> Option<PathBuf> {
    #[cfg(not(test))]
    {
        std::env::current_dir().ok()
    }
    #[cfg(test)]
    {
        test_isolation::entered().filter(|dir| test_isolation::in_scratch(dir))
    }
}

/// `dir` and the directories above it, for an upward search (`global.json`,
/// `.claude`). In a test build the search stops at the scratch directory:
/// above it lie the developer's own directories — on Windows the temporary
/// directory sits under their profile — whose files would decide the result.
pub fn ancestors(dir: &Path) -> impl Iterator<Item = &Path> {
    dir.ancestors().take_while(|dir| within_reach(dir))
}

/// The working directory, as `std::env::current_dir` gives it.
///
/// In a test build, the scratch directory the test entered, or else
/// `test_isolation::project`: rtk records the current project, and `init`
/// writes into it.
pub fn current_dir() -> std::io::Result<PathBuf> {
    #[cfg(not(test))]
    {
        std::env::current_dir()
    }
    #[cfg(test)]
    {
        Ok(working_dir().unwrap_or_else(test_isolation::project))
    }
}

/// `name`, relative to the working directory: the relative path itself, left
/// to the operating system.
///
/// In a test build, under [`current_dir`]'s answer. The process's own working
/// directory is the developer's checkout, and above it their home: an
/// untracked `.rtk/filters.toml` or `composer.json` there, or a `global.json`
/// in a parent directory, is theirs, and a local `rtk init` writes `CLAUDE.md`,
/// `AGENTS.md` and `.rtk/filters.toml` here.
pub fn in_working_dir(name: &str) -> PathBuf {
    #[cfg(not(test))]
    {
        PathBuf::from(name)
    }
    #[cfg(test)]
    {
        working_dir()
            .unwrap_or_else(test_isolation::project)
            .join(name)
    }
}

/// The project a command runs in: the nearest directory at or above the working
/// directory holding `marker` (`.claude`), or failing that the enclosing git
/// work tree.
///
/// In a test build the search stays inside the scratch directory, as
/// [`ancestors`] does, and skips git, which follows an exported `GIT_DIR` to the
/// developer's repository.
pub fn project_root(marker: &str) -> Option<PathBuf> {
    // Fast path: walk up the working directory, no subprocess needed.
    let start = working_dir()?;
    if let Some(dir) = ancestors(&start).find(|dir| dir.join(marker).exists()) {
        return Some(dir.to_path_buf());
    }

    #[cfg(not(test))]
    {
        // Fallback: git (spawns a subprocess, slower but handles monorepo layouts).
        let mut cmd = std::process::Command::new("git");
        cmd.args(["rev-parse", "--show-toplevel"]);
        let result = crate::core::stream::exec_capture(&mut cmd).ok()?;
        result
            .success()
            .then(|| PathBuf::from(result.stdout.trim()))
    }
    #[cfg(test)]
    {
        None
    }
}

/// Whether a test build may look at `dir`: only inside its scratch directory.
fn within_reach(dir: &Path) -> bool {
    #[cfg(not(test))]
    {
        let _ = dir;
        true
    }
    #[cfg(test)]
    {
        test_isolation::in_scratch(dir)
    }
}

/// A path-bearing environment variable: an agent's own directory
/// (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, …) or one of rtk's path overrides
/// (`RTK_DB_PATH`, `RTK_TEE_DIR`, …).
///
/// Read through `user_env`, so a test build sees only what the calling test
/// set: one exported in the developer's shell names their real files, and
/// obeying it would let a test write or delete them — or take its result from
/// them, as a `CLAUDE_CONFIG_DIR` carrying Bash deny rules does.
pub fn env_path(name: &str) -> Option<OsString> {
    super::user_env::var_os(name)
}
