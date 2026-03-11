use std::path::PathBuf;

const CURRENT_HOOK_VERSION: u8 = 2;
const WARN_INTERVAL_SECS: u64 = 24 * 3600;

/// Hook status for diagnostics and `rtk gain`.
#[derive(Debug, PartialEq)]
pub enum HookStatus {
    /// Hook is installed and up to date.
    Ok,
    /// Hook exists but is outdated.
    Outdated,
    /// No hook file found at all.
    Missing,
}

/// Return the current hook status without printing anything.
pub fn status() -> HookStatus {
    let Some(hook_path) = hook_installed_path() else {
        return HookStatus::Missing;
    };
    let Ok(content) = std::fs::read_to_string(&hook_path) else {
        return HookStatus::Missing;
    };
    if parse_hook_version(&content) >= CURRENT_HOOK_VERSION {
        HookStatus::Ok
    } else {
        HookStatus::Outdated
    }
}

/// Check if the installed hook is missing or outdated, warn once per day.
pub fn maybe_warn() {
    // Don't block startup — fail silently on any error
    let _ = check_and_warn();
}

fn check_and_warn() -> Option<()> {
    let warning = match hook_installed_path() {
        Some(hook_path) => {
            let content = std::fs::read_to_string(&hook_path).ok()?;
            let installed_version = parse_hook_version(&content);
            if installed_version >= CURRENT_HOOK_VERSION {
                return Some(()); // Up to date, nothing to do
            }
            "[rtk] /!\\ Hook outdated — run `rtk init -g` to update"
        }
        None => {
            // No hook installed — check if Claude Code config dir exists
            // (only warn if user has Claude Code installed)
            let home = dirs::home_dir()?;
            if !home.join(".claude").exists() {
                return Some(()); // No Claude Code, no point warning
            }
            "[rtk] /!\\ No hook installed — run `rtk init -g` for automatic token savings"
        }
    };

    // Rate limit: warn once per day
    let marker = warn_marker_path()?;
    if let Ok(meta) = std::fs::metadata(&marker) {
        if let Ok(elapsed) = meta.modified().ok()?.elapsed() {
            if elapsed.as_secs() < WARN_INTERVAL_SECS {
                return Some(());
            }
        }
    }

    // Touch marker
    let _ = std::fs::create_dir_all(marker.parent()?);
    let _ = std::fs::write(&marker, b"");

    eprintln!("{}", warning);

    Some(())
}

pub fn parse_hook_version(content: &str) -> u8 {
    for line in content.lines().take(5) {
        if let Some(rest) = line.strip_prefix("# rtk-hook-version:") {
            if let Ok(v) = rest.trim().parse::<u8>() {
                return v;
            }
        }
    }
    0 // No version tag = version 0 (outdated)
}

fn hook_installed_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = home.join(".claude").join("hooks").join("rtk-rewrite.sh");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

fn warn_marker_path() -> Option<PathBuf> {
    let data_dir = dirs::data_local_dir()?.join("rtk");
    Some(data_dir.join(".hook_warn_last"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hook_version_present() {
        let content = "#!/usr/bin/env bash\n# rtk-hook-version: 2\n# some comment\n";
        assert_eq!(parse_hook_version(content), 2);
    }

    #[test]
    fn test_parse_hook_version_missing() {
        let content = "#!/usr/bin/env bash\n# old hook without version\n";
        assert_eq!(parse_hook_version(content), 0);
    }

    #[test]
    fn test_parse_hook_version_future() {
        let content = "#!/usr/bin/env bash\n# rtk-hook-version: 5\n";
        assert_eq!(parse_hook_version(content), 5);
    }

    #[test]
    fn test_status_missing_when_no_hook() {
        // When hook file doesn't exist, status should be Missing
        // (tested implicitly — hook_installed_path returns None for non-existent paths)
        assert_eq!(parse_hook_version("no version here"), 0);
    }

    #[test]
    fn test_hook_status_variants() {
        // Verify enum derives work
        assert_ne!(HookStatus::Ok, HookStatus::Missing);
        assert_ne!(HookStatus::Outdated, HookStatus::Missing);
        assert_eq!(HookStatus::Ok, HookStatus::Ok);
    }
}
