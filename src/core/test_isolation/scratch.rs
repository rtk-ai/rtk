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
/// The child is built without `cfg(test)`, so `constants::data_dir`'s redirect
/// does not reach it and the environment is the only channel. `RTK_DB_PATH`
/// and `RTK_TEE_DIR` name the two files a filter run writes. The rest —
/// telemetry salt, trust store, and the marker `hook_check::maybe_warn` writes
/// to rate-limit the developer's once-a-day warning — resolves through `dirs`,
/// which reads `XDG_DATA_HOME` on Linux and `HOME` on macOS. `XDG_CONFIG_HOME`
/// joins them so a child reads the same configuration on every machine.
///
/// Windows resolves its known folders through the shell API rather than the
/// environment, so there only the two named files are redirected.
pub fn redirect_rtk_data(cmd: &mut Command) {
    let root = scratch_dir();
    cmd.env("RTK_DB_PATH", root.join("rtk").join("history.db"))
        .env("RTK_TEE_DIR", root.join("rtk").join("tee"))
        .env("XDG_DATA_HOME", root)
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .env("HOME", root);
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
            unsafe {
                libc::atexit(remove);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}
