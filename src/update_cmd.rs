use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RELEASE_API_URL: &str = "https://api.github.com/repos/rtk-ai/rtk/releases/latest";
const INSTALL_SCRIPT_URL: &str = "https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh";
const CARGO_INSTALL_REPO: &str = "https://github.com/rtk-ai/rtk";

enum UpdatePlan {
    Run {
        message: String,
        program: String,
        args: Vec<String>,
        envs: Vec<(String, String)>,
    },
    Manual {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateChannel {
    Homebrew,
    CargoDefaultBin,
    ScriptDefaultBin(PathBuf),
    Nix,
    Unsupported,
}

pub fn run(_verbose: u8) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    let exe_path = current_executable_path()?;
    let channel = detect_update_channel(&exe_path);

    match build_update_plan(&channel, current_version)? {
        UpdatePlan::Run {
            message,
            program,
            args,
            envs,
        } => {
            println!("{}", message);
            execute_update(&program, &args, &envs)?;
            println!("Update complete. Run `rtk --version` to verify the installed binary.");
        }
        UpdatePlan::Manual { message } => {
            println!("{}", message);
        }
    }

    Ok(())
}

fn build_update_plan(channel: &UpdateChannel, current_version: &str) -> Result<UpdatePlan> {
    match channel {
        UpdateChannel::Homebrew => Ok(UpdatePlan::Run {
            message: "Updating RTK with Homebrew (`brew upgrade rtk`).".to_string(),
            program: "brew".to_string(),
            args: vec!["upgrade".to_string(), "rtk".to_string()],
            envs: Vec::new(),
        }),
        UpdateChannel::ScriptDefaultBin(install_dir) => {
            let latest_version = fetch_latest_version(current_version)?;
            Ok(build_script_update_plan(
                install_dir.clone(),
                current_version,
                &latest_version,
            ))
        }
        UpdateChannel::CargoDefaultBin => {
            let latest_version = fetch_latest_version(current_version)?;
            Ok(build_cargo_update_message(current_version, &latest_version))
        }
        UpdateChannel::Nix => Ok(UpdatePlan::Manual {
            message: "RTK appears to be managed by Nix. Update the package in your flake/profile, then rebuild your environment.".to_string(),
        }),
        UpdateChannel::Unsupported => Ok(UpdatePlan::Manual {
            message: format!(
                "Automatic update is available for Homebrew installs and the default Quick Install path (`~/.local/bin/rtk`). For this binary layout, reinstall manually with one of:\n\n  brew upgrade rtk\n  cargo install --git {} --tag <release-tag> --force\n  curl -fsSL {} | sh",
                CARGO_INSTALL_REPO, INSTALL_SCRIPT_URL
            ),
        }),
    }
}

fn fetch_latest_version(current_version: &str) -> Result<String> {
    let body = ureq::get(RELEASE_API_URL)
        .set("User-Agent", &format!("rtk/{}", current_version))
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .context("Failed to query the latest RTK release from GitHub")?
        .into_string()
        .context("Failed to read GitHub release metadata")?;

    let json: serde_json::Value =
        serde_json::from_str(&body).context("Failed to parse GitHub release metadata")?;
    let tag = json
        .get("tag_name")
        .and_then(|value| value.as_str())
        .context("GitHub release metadata did not include tag_name")?;

    Ok(normalize_version(tag).to_string())
}

fn build_script_update_plan(
    install_dir: PathBuf,
    current_version: &str,
    latest_version: &str,
) -> UpdatePlan {
    if !is_newer_version(latest_version, current_version) {
        return UpdatePlan::Manual {
            message: format!("rtk is already up to date (v{}).", current_version),
        };
    }

    UpdatePlan::Run {
        message: format!(
            "Updating RTK v{} -> v{} in {}.",
            current_version,
            latest_version,
            install_dir.display()
        ),
        program: "sh".to_string(),
        args: vec!["-c".to_string(), format!("curl -fsSL {} | sh", INSTALL_SCRIPT_URL)],
        envs: vec![
            ("RTK_VERSION".to_string(), format!("v{}", latest_version)),
            (
                "RTK_INSTALL_DIR".to_string(),
                install_dir.to_string_lossy().to_string(),
            ),
        ],
    }
}

fn build_cargo_update_message(current_version: &str, latest_version: &str) -> UpdatePlan {
    if !is_newer_version(latest_version, current_version) {
        return UpdatePlan::Manual {
            message: format!("rtk is already up to date (v{}).", current_version),
        };
    }

    UpdatePlan::Manual {
        message: format!(
            "RTK looks like a default Cargo install. Update it with:\n\n  cargo install --git {} --tag v{} --force",
            CARGO_INSTALL_REPO, latest_version
        ),
    }
}

fn current_executable_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("Failed to locate the current RTK binary")?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

fn detect_update_channel(exe_path: &Path) -> UpdateChannel {
    let path = exe_path.to_string_lossy();

    if path.contains("/Cellar/rtk/") {
        return UpdateChannel::Homebrew;
    }

    if path.contains("/nix/store/") {
        return UpdateChannel::Nix;
    }

    if let Some(bin_dir) = default_quick_install_dir() {
        if exe_path == bin_dir.join(binary_name()) {
            return UpdateChannel::ScriptDefaultBin(bin_dir);
        }
    }

    if is_default_cargo_install(exe_path) {
        return UpdateChannel::CargoDefaultBin;
    }

    UpdateChannel::Unsupported
}

fn is_default_cargo_install(exe_path: &Path) -> bool {
    default_cargo_bin_dir()
        .map(|bin_dir| exe_path == bin_dir.join(binary_name()))
        .unwrap_or(false)
}

fn default_cargo_bin_dir() -> Option<PathBuf> {
    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        return Some(PathBuf::from(cargo_home).join("bin"));
    }

    home_dir().map(|home| home.join(".cargo").join("bin"))
}

fn default_quick_install_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        return None;
    }

    home_dir().map(|home| home.join(".local").join("bin"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "rtk.exe"
    } else {
        "rtk"
    }
}

fn execute_update(program: &str, args: &[String], envs: &[(String, String)]) -> Result<()> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    for (key, value) in envs {
        command.env(key, value);
    }

    let status = command
        .status()
        .with_context(|| format!("Failed to run updater command: {}", program))?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "Updater command exited with status {}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        )
    }
}

fn is_newer_version(latest: &str, current: &str) -> bool {
    compare_versions(latest, current).is_gt()
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let left = parse_version_parts(left);
    let right = parse_version_parts(right);
    let max_len = left.len().max(right.len());

    for idx in 0..max_len {
        let left_part = left.get(idx).copied().unwrap_or(0);
        let right_part = right.get(idx).copied().unwrap_or(0);

        match left_part.cmp(&right_part) {
            std::cmp::Ordering::Equal => continue,
            ordering => return ordering,
        }
    }

    std::cmp::Ordering::Equal
}

fn parse_version_parts(version: &str) -> Vec<u64> {
    normalize_version(version)
        .split('.')
        .map(|segment| {
            segment
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.parse::<u64>().unwrap_or(0))
        .collect()
}

fn normalize_version(version: &str) -> &str {
    version.trim().trim_start_matches('v')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer_version_handles_semver() {
        assert!(is_newer_version("0.29.1", "0.29.0"));
        assert!(is_newer_version("v0.30.0", "0.29.9"));
        assert!(!is_newer_version("0.29.0", "0.29.0"));
        assert!(!is_newer_version("0.28.9", "0.29.0"));
    }

    #[test]
    fn test_detect_update_channel_homebrew() {
        let path = PathBuf::from("/opt/homebrew/Cellar/rtk/0.29.0/bin/rtk");
        assert_eq!(detect_update_channel(&path), UpdateChannel::Homebrew);
    }

    #[test]
    fn test_detect_update_channel_nix() {
        let path = PathBuf::from("/nix/store/abc123-rtk/bin/rtk");
        assert_eq!(detect_update_channel(&path), UpdateChannel::Nix);
    }

    #[test]
    fn test_build_update_plan_for_script_sets_exact_release() {
        let plan = build_script_update_plan(
            PathBuf::from("/home/user/.local/bin"),
            "0.29.0",
            "0.30.0",
        );

        match plan {
            UpdatePlan::Run { envs, .. } => {
                assert!(envs.iter().any(|(key, _)| key == "RTK_VERSION"));
                assert!(envs.iter().any(|(key, value)| {
                    key == "RTK_INSTALL_DIR" && value == "/home/user/.local/bin"
                }));
            }
            _ => panic!("expected runnable script plan"),
        }
    }

    #[test]
    fn test_build_update_plan_for_unsupported_is_manual() {
        let plan = build_update_plan(&UpdateChannel::Unsupported, "0.29.0").unwrap();
        match plan {
            UpdatePlan::Manual { message } => {
                assert!(message.contains("brew upgrade rtk"));
                assert!(message.contains("cargo install --git"));
            }
            _ => panic!("expected manual plan"),
        }
    }

    #[test]
    fn test_build_cargo_update_message_uses_exact_tag() {
        let plan = build_cargo_update_message("0.29.0", "0.30.0");
        match plan {
            UpdatePlan::Manual { message } => {
                assert!(message.contains("--tag v0.30.0 --force"));
            }
            _ => panic!("expected manual cargo update command"),
        }
    }
}
