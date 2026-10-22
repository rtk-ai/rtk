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

/// The installed OpenCode version banner, or `None` when there is none we can read.
#[cfg(not(test))]
fn opencode_version() -> Option<String> {
    let out = std::process::Command::new("opencode")
        .arg("--version")
        .output()
        .ok()?;
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
    // A dry run skips the floor check, so it has no reason to spawn OpenCode.
    let banner = if ctx.dry_run {
        None
    } else {
        opencode_version()
    };
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
    if !dry_run && let Some(found) = too_old_opencode(banner) {
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

    // Pi integration tests
}
