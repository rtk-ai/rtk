//! Keeps the test suite off the developer's own rtk data.
//!
//! A spawned rtk resolves its tracking database and tee spool exactly as a
//! normal invocation does, with nothing in the child to tell that its parent
//! is a test, so the redirection belongs at the call site. The scan at the
//! bottom of this file enforces it.

mod scratch;

pub use scratch::scratch_dir;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The tracking database for this test process.
pub fn db_path() -> PathBuf {
    data_dir().join(crate::core::constants::HISTORY_DB)
}

/// This process's stand-in for `~/.local/share/rtk`.
fn data_dir() -> PathBuf {
    scratch_dir().join(crate::core::constants::RTK_DATA_DIR)
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

/// Whether the binary has been built, and built since the last edit under
/// `src/`.
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

/// When `src/` was last touched, or `None` if the sources are not there to
/// look at.
fn newest_source_change() -> Option<std::time::SystemTime> {
    rust_files(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"))
        .iter()
        .filter_map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
        .max()
}

/// Build a `Command` for the rtk binary with the data it writes redirected to
/// this process's scratch directory.
///
/// `tests/common/mod.rs` holds the twin of this for the integration tests.
pub fn rtk_command() -> Command {
    let bin = rtk_bin();
    assert!(
        bin.exists(),
        "rtk binary not found at {} — run `cargo build` first",
        bin.display()
    );
    let mut cmd = Command::new(bin);
    scratch::redirect_rtk_data(&mut cmd);
    cmd
}

/// How cargo hands the binary's path to an integration test. The scan rejects
/// it anywhere but [`CONSTRUCTORS`].
const NAMES_RTK: &str = "CARGO_BIN_EXE_rtk";

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
            if src.contains(NAMES_RTK) {
                offenders.push(relative);
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
