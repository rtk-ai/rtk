//! OpenCode plugin: install/uninstall helpers.
use super::*;
use crate::hooks::constants::{CONFIG_DIR, OPENCODE_PLUGIN_FILE, OPENCODE_SUBDIR, PLUGIN_SUBDIR};

// Embedded OpenCode plugin (auto-rewrite)
const OPENCODE_PLUGIN: &str = include_str!("../../../hooks/opencode/rtk.ts");

/// Oldest OpenCode `rtk init` installs `OPENCODE_PLUGIN` for.
///
/// The plugin default-exports an object carrying a V1 `server()` and a V2
/// `setup()`, the shape OpenCode's own V2 plugin docs prescribe, and those docs
/// support it from 1.18.29. 2.x reads `id` and `setup()` and ignores
/// `server()`.
const MIN_OPENCODE: (u32, u32, u32) = (1, 18, 29);

/// Pull `major.minor.patch` out of `opencode --version` ("1.3.3", "opencode 1.3.3",
/// "v2.0.22"). `None` when there is no such run of digits.
fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let word = s.split_whitespace().find(|w| {
        w.trim_start_matches(|c: char| !c.is_ascii_digit())
            .starts_with(|c: char| c.is_ascii_digit())
    })?;
    let mut it = word
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// How long `opencode --version` may run before it counts as no version.
///
/// A wrapper that fetches or migrates on first launch must not block `rtk init`.
/// Past this, the probe is abandoned and init proceeds as if OpenCode had not
/// answered — the same branch as a missing binary. Long enough for a cold
/// `opencode --version`, short enough that a hung shim does not stall init.
const OPENCODE_VERSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run `cmd` and collect its output.
///
/// `None` when it cannot be spawned or does not exit within `timeout`. On Unix
/// the child is its own process group, and a timeout kills that group, so a
/// wrapper and the fetch or migration it started both stop.
fn output_with_timeout(
    cmd: &mut std::process::Command,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    use std::thread;
    use std::time::Instant;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    // `.output()` closes stdin; a version probe must not consume the caller's.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().ok()?;
    let Some(mut stdout) = child.stdout.take() else {
        stop_child(&mut child);
        return None;
    };
    let Some(mut stderr) = child.stderr.take() else {
        stop_child(&mut child);
        return None;
    };
    // Drain both pipes while we wait. A child that writes more than the pipe
    // buffer would otherwise block forever, and `try_wait` would never see it exit.
    let stdout_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                stop_child(&mut child);
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                return None;
            }
            Ok(None) => thread::sleep(std::time::Duration::from_millis(20)),
            Err(_) => {
                stop_child(&mut child);
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                return None;
            }
        }
    };

    Some(std::process::Output {
        status,
        stdout: stdout_handle.join().ok()?,
        stderr: stderr_handle.join().ok()?,
    })
}

/// Stop `child` and every process in its group, then reap it.
fn stop_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        if pid > 0 {
            #[allow(unsafe_code)]
            // nosemgrep: unsafe-block
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The installed OpenCode version banner, or `None` when there is none we can read.
#[cfg(not(test))]
fn opencode_version() -> Option<String> {
    let mut cmd = std::process::Command::new("opencode");
    cmd.arg("--version");
    let out = output_with_timeout(&mut cmd, OPENCODE_VERSION_TIMEOUT)?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Unit tests never ask the developer's own OpenCode for its version; the
/// version-dependent branch is exercised through
/// `ensure_opencode_plugin_installed_for`.
#[cfg(test)]
fn opencode_version() -> Option<String> {
    None
}

/// The version when it is older than `MIN_OPENCODE`, as `x.y.z`.
///
/// `None` for an OpenCode we could not ask, could not parse, or one that is new
/// enough, so an unreadable version never turns into a complaint. The banner
/// itself is not reported: `opencode --version` prints more than the number on
/// some builds, and the extra text breaks the message across lines.
fn too_old_opencode(banner: Option<&str>) -> Option<String> {
    let banner = banner?;
    let (major, minor, patch) = parse_version(banner)?;
    ((major, minor, patch) < MIN_OPENCODE).then(|| format!("{major}.{minor}.{patch}"))
}

// Embedded Pi extension (auto-rewrite)
// Stable code marker used to recognize a modified RTK extension without
// relying on explanatory comments that users may remove. The marker matches
// both the current `pi.exec` call and older stock revisions that imported
// `exec` locally before invoking it.
// SHA-256 hashes of stock Pi extension revisions that may exist on user
// machines, including the current embedded file. Keep historical entries
// when changing the extension so an untouched older install can still be
// removed safely. Hashes are computed after normalizing CRLF to LF and
// trimming trailing whitespace, matching the comparisons below.
// The history-based test below verifies that this list remains append-only
// for every revision in the current checkout's ancestor history.
pub(super) fn resolve_opencode_dir() -> Result<PathBuf> {
    resolve_home_subdir(CONFIG_DIR).map(|p| p.join(OPENCODE_SUBDIR))
}

// Pi coding agent support

/// Return OpenCode plugin path: ~/.config/opencode/plugins/rtk.ts
pub(super) fn opencode_plugin_path(opencode_dir: &Path) -> PathBuf {
    opencode_dir.join(PLUGIN_SUBDIR).join(OPENCODE_PLUGIN_FILE)
}

/// Prepare OpenCode plugin directory and return install path
pub(super) fn prepare_opencode_plugin_path() -> Result<PathBuf> {
    let opencode_dir = resolve_opencode_dir()?;
    let path = opencode_plugin_path(&opencode_dir);
    // Directory creation is deferred to install time (caller guards on dry_run).
    Ok(path)
}

/// Write OpenCode plugin file if missing or outdated.
///
/// Below [`MIN_OPENCODE`] nothing is written and an existing file is left
/// alone, so whatever already works keeps working: an older OpenCode may not
/// load this plugin, or may fail to start with it.
///
/// Returns whether a plugin is in place afterwards, so callers report a plugin
/// path only when there is one.
pub(super) fn ensure_opencode_plugin_installed(path: &Path, ctx: InitContext) -> Result<bool> {
    // Reading the version does not touch the filesystem, so a dry run applies
    // the same floor as a real run. A probe that fails or hangs reads as no
    // version, and init installs (or previews) the plugin.
    let banner = opencode_version();
    ensure_opencode_plugin_installed_for(path, ctx, banner.as_deref())
}

/// [`ensure_opencode_plugin_installed`] against an already-read version banner.
///
/// Taking the banner rather than spawning `opencode --version` here is what
/// makes the no-write branch testable, and what stops the tests from running
/// whatever OpenCode the developer happens to have installed.
fn ensure_opencode_plugin_installed_for(
    path: &Path,
    ctx: InitContext,
    banner: Option<&str>,
) -> Result<bool> {
    let InitContext { dry_run, .. } = ctx;
    if let Some(found) = too_old_opencode(banner) {
        let need = format!("{}.{}.{}", MIN_OPENCODE.0, MIN_OPENCODE.1, MIN_OPENCODE.2);
        let existing = if path.exists() {
            "left the plugin that is already there in place"
        } else {
            "installed nothing"
        };
        println!(
            "  [rtk] OpenCode {found} is older than {need}, the oldest OpenCode this plugin \
             supports, so {existing}.\n        \
             Upgrade OpenCode, then re-run `rtk init -g --opencode`."
        );
        return Ok(false);
    }
    // Ensure parent dir exists (skip in dry-run)
    if !dry_run && let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create OpenCode plugin directory: {}",
                parent.display()
            )
        })?;
    }
    write_if_changed(path, OPENCODE_PLUGIN, "OpenCode plugin", ctx)?;
    Ok(true)
}

/// Remove OpenCode plugin file
pub(super) fn remove_opencode_plugin(ctx: InitContext) -> Result<Vec<PathBuf>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let opencode_dir = resolve_opencode_dir()?;
    let path = opencode_plugin_path(&opencode_dir);
    let mut removed = Vec::new();

    if path.exists() {
        if dry_run {
            println!("[dry-run] would remove OpenCode plugin: {}", path.display());
        } else {
            // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove OpenCode plugin: {}", path.display()))?;
            if verbose > 0 {
                eprintln!("Removed OpenCode plugin: {}", path.display());
            }
        }
        removed.push(path);
    }

    Ok(removed)
}

pub(super) fn run_opencode_only_mode(ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let opencode_plugin_path = prepare_opencode_plugin_path()?;
    let installed = ensure_opencode_plugin_installed(&opencode_plugin_path, ctx)?;
    if !dry_run && installed {
        println!("\nOpenCode plugin installed (global).\n");
        println!("  OpenCode: {}", opencode_plugin_path.display());
        println!("  Restart OpenCode. Test with: git status\n");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // One shape, two loaders: the 2.x loader reads `id` and `setup()` off the
    // default export, and 1.x calls `server()` on it. A callable default fails
    // the 2.x schema check, so neither half may become a function again.
    #[test]
    fn plugin_is_an_object_default_with_both_entrypoints() {
        assert!(OPENCODE_PLUGIN.contains("const RtkOpenCodePlugin = {"));
        assert!(OPENCODE_PLUGIN.contains("id: \"rtk\""));
        assert!(OPENCODE_PLUGIN.contains("setup(ctx"));
        assert!(OPENCODE_PLUGIN.contains("server()"));
        assert!(OPENCODE_PLUGIN.contains("\"execute.before\""));
        assert!(OPENCODE_PLUGIN.contains("\"tool.execute.before\""));
        assert!(!OPENCODE_PLUGIN.contains("export default RtkOpenCodePlugin("));
    }

    // The floor decides whether init writes the plugin at all.
    #[test]
    fn version_floor_is_pinned() {
        for banner in ["1.18.28", "opencode 1.3.3", "1.1.4", "0.14.2"] {
            assert!(too_old_opencode(Some(banner)).is_some(), "{banner}");
        }
        // The reported version is trimmed, so the message cannot wrap mid-sentence.
        assert_eq!(
            too_old_opencode(Some("opencode 1.1.4\nsome build note\n")).as_deref(),
            Some("1.1.4")
        );
        for banner in ["1.18.29", "1.18.34", "1.19.0", "2.0.26"] {
            assert_eq!(too_old_opencode(Some(banner)), None, "{banner}");
        }
        // unreadable or unparsable is not evidence of an old OpenCode
        assert_eq!(too_old_opencode(None), None);
        assert_eq!(too_old_opencode(Some("")), None);
        assert_eq!(too_old_opencode(Some("opencode (unknown)")), None);
        assert_eq!(too_old_opencode(Some("v")), None);
    }

    /// Dry-run used to skip the floor and preview a write the real run refuses.
    #[test]
    fn an_old_opencode_dry_run_refuses_and_writes_nothing() {
        let dry = InitContext {
            dry_run: true,
            ..InitContext::default()
        };
        let temp = TempDir::new().unwrap();
        let plugin_path = opencode_plugin_path(&temp.path().join("opencode"));

        let installed =
            ensure_opencode_plugin_installed_for(&plugin_path, dry, Some("1.1.4")).unwrap();
        assert!(
            !installed,
            "dry-run must refuse an old OpenCode, same as a real run"
        );
        assert!(
            !plugin_path.exists(),
            "dry-run must not create the plugin it refuses"
        );

        fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        fs::write(&plugin_path, "// already there").unwrap();
        let installed =
            ensure_opencode_plugin_installed_for(&plugin_path, dry, Some("1.1.4")).unwrap();
        assert!(!installed);
        assert_eq!(
            fs::read_to_string(&plugin_path).unwrap(),
            "// already there",
            "dry-run must leave an existing plugin untouched"
        );
    }

    #[test]
    fn an_old_opencode_leaves_the_plugin_file_untouched() {
        // Writing it is what breaks startup, so the no-write branch is the one
        // that matters: an existing plugin has to survive, and a missing one
        // has to stay missing.
        for banner in ["1.1.4", "1.3.3", "opencode 1.1.4\nsome build note\n"] {
            let temp = TempDir::new().unwrap();
            let plugin_path = opencode_plugin_path(&temp.path().join("opencode"));

            fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
            fs::write(&plugin_path, "// whatever was already there").unwrap();
            let installed = ensure_opencode_plugin_installed_for(
                &plugin_path,
                InitContext::default(),
                Some(banner),
            )
            .unwrap();
            assert!(
                !installed,
                "{banner} must report that nothing was installed"
            );
            assert_eq!(
                fs::read_to_string(&plugin_path).unwrap(),
                "// whatever was already there",
                "{banner} must not overwrite what was there"
            );

            fs::remove_file(&plugin_path).unwrap();
            let installed = ensure_opencode_plugin_installed_for(
                &plugin_path,
                InitContext::default(),
                Some(banner),
            )
            .unwrap();
            assert!(!installed);
            assert!(!plugin_path.exists(), "{banner} must not create the file");
        }
    }

    #[test]
    fn a_new_or_unreadable_opencode_still_gets_the_plugin() {
        for banner in [
            None,
            Some("1.18.29"),
            Some("2.0.26"),
            Some("opencode (unknown)"),
        ] {
            let temp = TempDir::new().unwrap();
            let plugin_path = opencode_plugin_path(&temp.path().join("opencode"));
            let installed =
                ensure_opencode_plugin_installed_for(&plugin_path, InitContext::default(), banner)
                    .unwrap();
            assert!(installed, "{banner:?} must get the plugin");
            assert_eq!(fs::read_to_string(&plugin_path).unwrap(), OPENCODE_PLUGIN);
        }
    }

    #[test]
    fn test_opencode_plugin_install_and_update() {
        let temp = TempDir::new().unwrap();
        let opencode_dir = temp.path().join("opencode");
        let plugin_path = opencode_plugin_path(&opencode_dir);

        fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        assert!(!plugin_path.exists());

        // No banner: this test is about the file, not the developer's OpenCode.
        let installed =
            ensure_opencode_plugin_installed_for(&plugin_path, InitContext::default(), None)
                .unwrap();
        assert!(installed);
        let content = fs::read_to_string(&plugin_path).unwrap();
        assert_eq!(content, OPENCODE_PLUGIN);

        fs::write(&plugin_path, "// old").unwrap();
        let installed_again =
            ensure_opencode_plugin_installed_for(&plugin_path, InitContext::default(), None)
                .unwrap();
        assert!(installed_again);
        let content_updated = fs::read_to_string(&plugin_path).unwrap();
        assert_eq!(content_updated, OPENCODE_PLUGIN);
    }

    #[test]
    fn test_opencode_plugin_remove() {
        let temp = TempDir::new().unwrap();
        let opencode_dir = temp.path().join("opencode");
        let plugin_path = opencode_plugin_path(&opencode_dir);
        fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        fs::write(&plugin_path, OPENCODE_PLUGIN).unwrap();

        assert!(plugin_path.exists());
        // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
        fs::remove_file(&plugin_path).unwrap();
        assert!(!plugin_path.exists());
    }

    #[test]
    fn opencode_version_timeout_stays_within_init_budget() {
        // A cold `opencode --version` still has to fit, and the integration
        // test's own deadline (15s) stays above this ceiling.
        assert!(OPENCODE_VERSION_TIMEOUT >= std::time::Duration::from_secs(1));
        assert!(OPENCODE_VERSION_TIMEOUT <= std::time::Duration::from_secs(5));
    }

    #[test]
    fn version_probe_is_none_when_the_binary_is_missing() {
        let mut cmd = std::process::Command::new("rtk-no-such-opencode-binary");
        assert!(output_with_timeout(&mut cmd, std::time::Duration::from_secs(1)).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_returns_output_when_the_command_exits() {
        let mut cmd = std::process::Command::new("echo");
        cmd.arg("1.18.29");
        let out = output_with_timeout(&mut cmd, std::time::Duration::from_secs(2))
            .expect("echo should finish");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1.18.29");
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_reports_a_failing_command() {
        let mut cmd = std::process::Command::new("false");
        let out = output_with_timeout(&mut cmd, std::time::Duration::from_secs(2))
            .expect("false should finish");
        assert!(!out.status.success());
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_gives_up_when_the_command_hangs() {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        let started = std::time::Instant::now();
        let out = output_with_timeout(&mut cmd, std::time::Duration::from_millis(200));
        assert!(out.is_none());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "hung probe took {:?}",
            started.elapsed()
        );
    }

    /// A shell wrapper that backgrounds `sleep` is the hang the timeout exists
    /// for. Killing only the shell would leave that child running.
    #[cfg(unix)]
    #[test]
    fn version_probe_stops_a_hanging_wrapper_and_the_child_it_started() {
        let temp = TempDir::new().unwrap();
        let pidfile = temp.path().join("child.pid");
        let script = format!("sleep 30 &\necho $! > '{}'\nwait\n", pidfile.display());
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(script);
        let started = std::time::Instant::now();
        assert!(output_with_timeout(&mut cmd, std::time::Duration::from_millis(500)).is_none());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "hung wrapper took {:?}",
            started.elapsed()
        );
        let pid: i32 = fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .expect("child pid");
        #[allow(unsafe_code)]
        // nosemgrep: unsafe-block
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        if alive {
            #[allow(unsafe_code)]
            // nosemgrep: unsafe-block
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        assert!(!alive, "grandchild sleep {pid} still running");
    }

    // Pi integration tests
}
