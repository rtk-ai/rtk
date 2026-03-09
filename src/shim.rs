use anyhow::{Context, Result};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::metadata;

const RTK_OPERATIONAL_COMMAND_BYPASS_ENV: &str = "RTK_BYPASS_OPERATIONAL_COMMAND_SHIMS";
const RTK_RECURSION_DEPTH_ENV: &str = "RTK_RECURSION_DEPTH";
const RTK_RECURSION_DEPTH_LIMIT: u32 = 32;

fn is_executable(path: &Path) -> bool {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn is_single_normal_component(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    )
}

fn is_same_executable(candidate: &Path, current_exe: &Path) -> bool {
    let candidate_canon = fs::canonicalize(candidate).ok();
    let current_canon = fs::canonicalize(current_exe).ok();

    if let (Some(a), Some(b)) = (&candidate_canon, &current_canon) {
        if a == b {
            return true;
        }
        #[cfg(unix)]
        {
            if let (Ok(ma), Ok(mb)) = (fs::metadata(a), fs::metadata(b)) {
                if ma.dev() == mb.dev() && ma.ino() == mb.ino() {
                    return true;
                }
            }
        }
    }

    #[cfg(unix)]
    {
        if let (Ok(ma), Ok(mb)) = (fs::metadata(candidate), fs::metadata(current_exe)) {
            if ma.dev() == mb.dev() && ma.ino() == mb.ino() {
                return true;
            }
        }
    }

    false
}

#[cfg(windows)]
fn command_candidates(program: &str) -> Vec<String> {
    let has_ext = Path::new(program).extension().is_some();
    if has_ext {
        return vec![program.to_string()];
    }
    let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    pathext
        .split(';')
        .filter(|s| !s.trim().is_empty())
        .map(|ext| format!("{program}{ext}"))
        .collect()
}

#[cfg(not(windows))]
fn command_candidates(program: &str) -> Vec<String> {
    vec![program.to_string()]
}

fn resolve_non_self_from_paths(
    program: &str,
    paths: &[PathBuf],
    current_exe: &Path,
) -> Option<PathBuf> {
    let candidates = command_candidates(program);
    for dir in paths {
        for candidate_name in &candidates {
            let candidate = dir.join(candidate_name);
            if !is_executable(&candidate) {
                continue;
            }
            if is_same_executable(&candidate, current_exe) {
                continue;
            }
            return Some(candidate);
        }
    }
    None
}

fn resolve_non_self_command(program: &str) -> Result<PathBuf> {
    let current_exe = env::current_exe().context("Failed to resolve current executable path")?;

    if program.contains('/') || program.contains('\\') {
        let candidate = PathBuf::from(program);
        if is_same_executable(&candidate, &current_exe) {
            anyhow::bail!(
                "Resolved command '{}' points to current rtk executable; refusing recursive invocation",
                program
            );
        }
        return Ok(candidate);
    }

    let path_env = env::var_os("PATH").unwrap_or_default();
    let paths: Vec<PathBuf> = env::split_paths(&path_env).collect();
    if let Some(path) = resolve_non_self_from_paths(program, &paths, &current_exe) {
        return Ok(path);
    }

    anyhow::bail!(
        "Unable to find native command '{}' in PATH without matching current rtk executable",
        program
    )
}

pub(crate) fn native_command(program: &str) -> Result<Command> {
    let resolved = resolve_non_self_command(program)?;
    Ok(Command::new(resolved))
}

fn default_shim_bin_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("rtk-shims")
        .join("bin")
}

fn ensure_shim_install_supported_platform() -> Result<()> {
    #[cfg(windows)]
    {
        anyhow::bail!(
            "rtk shim install is currently unsupported on Windows; symlink-based shim installation requires elevated OS policy/privileges. Use a Unix-like environment for now."
        );
    }

    #[cfg(not(windows))]
    {
        Ok(())
    }
}

pub(crate) fn resolve_shim_operational_commands(
    requested: &[String],
    allowed: &[String],
) -> Result<Vec<String>> {
    if requested.is_empty() {
        return Ok(allowed.to_vec());
    }

    let mut operational_commands = Vec::with_capacity(requested.len());
    for operational_command in requested {
        if !is_single_normal_component(operational_command) {
            anyhow::bail!(
                "invalid operational_command name: '{}'",
                operational_command
            );
        }
        if !allowed
            .iter()
            .any(|allowed_operational_command| allowed_operational_command == operational_command)
        {
            anyhow::bail!(
                "operational_command '{}' is not Shim-eligible in this rtk binary",
                operational_command
            );
        }
        operational_commands.push(operational_command.clone());
    }

    Ok(operational_commands)
}

fn create_operational_command_shim(target: &Path, link: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).with_context(|| {
            format!(
                "failed to create symlink '{}' -> '{}'",
                link.display(),
                target.display()
            )
        })?;
        Ok(())
    }

    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link).with_context(|| {
            format!(
                "failed to create symlink '{}' -> '{}'",
                link.display(),
                target.display()
            )
        })?;
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        anyhow::bail!("symlink creation is not supported on this platform");
    }
}

fn is_rtk_named_path(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some("rtk") | Some("rtk.exe")
    )
}

fn is_rtk_shim_symlink(link_path: &Path, target_rtk: &Path) -> bool {
    let Ok(link_target) = fs::read_link(link_path) else {
        return false;
    };

    let resolved_target = if link_target.is_absolute() {
        link_target
    } else {
        link_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(link_target)
    };

    if is_same_executable(&resolved_target, target_rtk) {
        return true;
    }

    if is_rtk_named_path(&resolved_target) {
        return true;
    }

    fs::canonicalize(&resolved_target)
        .map(|p| is_rtk_named_path(&p))
        .unwrap_or(false)
}

pub(crate) fn install_operational_command_shims(
    bin_dir: Option<PathBuf>,
    rtk_bin: Option<PathBuf>,
    force: bool,
    force_all: bool,
    operational_commands: &[String],
) -> Result<()> {
    ensure_shim_install_supported_platform()?;

    let bin_dir = bin_dir.unwrap_or_else(default_shim_bin_dir);
    fs::create_dir_all(&bin_dir).with_context(|| {
        format!(
            "failed to create operational_command-shim bin dir '{}'",
            bin_dir.display()
        )
    })?;

    let rtk_target = match rtk_bin {
        Some(path) => path,
        None => env::current_exe().context("failed to resolve current executable")?,
    };

    if !rtk_target.exists() {
        anyhow::bail!("rtk binary does not exist: {}", rtk_target.display());
    }
    if !rtk_target.is_file() {
        anyhow::bail!("rtk binary is not a file: {}", rtk_target.display());
    }
    if !is_executable(&rtk_target) {
        anyhow::bail!("rtk binary is not executable: {}", rtk_target.display());
    }

    let rtk_abs = fs::canonicalize(&rtk_target).unwrap_or(rtk_target);
    let mut created = 0usize;
    let mut replaced = 0usize;
    let mut skipped = 0usize;

    for operational_command in operational_commands {
        let link_path = bin_dir.join(operational_command);
        match fs::symlink_metadata(&link_path) {
            Ok(meta) => {
                if force_all {
                    if meta.file_type().is_dir() {
                        anyhow::bail!(
                            "refusing to replace existing directory: {}",
                            link_path.display()
                        );
                    }
                    fs::remove_file(&link_path).with_context(|| {
                        format!("failed to remove existing entry '{}'", link_path.display())
                    })?;
                    create_operational_command_shim(&rtk_abs, &link_path)?;
                    println!("replaced: {} -> {}", link_path.display(), rtk_abs.display());
                    replaced += 1;
                } else if force {
                    if meta.file_type().is_dir() {
                        anyhow::bail!(
                            "refusing to replace existing directory: {}",
                            link_path.display()
                        );
                    }
                    if !meta.file_type().is_symlink() || !is_rtk_shim_symlink(&link_path, &rtk_abs)
                    {
                        anyhow::bail!(
                            "refusing to replace non-rtk shim '{}'; rerun with --force-all to replace arbitrary files",
                            link_path.display()
                        );
                    }
                    fs::remove_file(&link_path).with_context(|| {
                        format!("failed to remove existing entry '{}'", link_path.display())
                    })?;
                    create_operational_command_shim(&rtk_abs, &link_path)?;
                    println!("replaced: {} -> {}", link_path.display(), rtk_abs.display());
                    replaced += 1;
                } else {
                    println!("skipped (exists): {}", link_path.display());
                    skipped += 1;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                create_operational_command_shim(&rtk_abs, &link_path)?;
                println!("created: {} -> {}", link_path.display(), rtk_abs.display());
                created += 1;
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("failed to inspect existing path '{}'", link_path.display())
                });
            }
        }
    }

    println!(
        "done: created={} replaced={} skipped={}",
        created, replaced, skipped
    );

    Ok(())
}

pub(crate) fn operational_command_name_from_argv0(argv0: &OsStr) -> Option<String> {
    let basename = Path::new(argv0).file_name()?.to_string_lossy();
    let trimmed = basename.strip_suffix(".exe").unwrap_or(&basename);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn build_parse_argv(raw_argv: &[OsString]) -> Vec<OsString> {
    if raw_argv.is_empty() {
        return vec![OsString::from("rtk")];
    }

    let Some(operational_command) = operational_command_name_from_argv0(&raw_argv[0]) else {
        return raw_argv.to_vec();
    };

    if !metadata::is_shim_eligible_top_level_command(&operational_command) {
        return raw_argv.to_vec();
    }

    let mut parse_argv = Vec::with_capacity(raw_argv.len() + 1);
    parse_argv.push(OsString::from("rtk"));
    parse_argv.push(OsString::from(operational_command));
    parse_argv.extend(raw_argv.iter().skip(1).cloned());
    parse_argv
}

fn current_recursion_depth() -> u32 {
    std::env::var(RTK_RECURSION_DEPTH_ENV)
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
}

fn install_child_bypass_env(depth: u32) {
    std::env::set_var(RTK_OPERATIONAL_COMMAND_BYPASS_ENV, "1");
    std::env::set_var(RTK_RECURSION_DEPTH_ENV, (depth + 1).to_string());
}

fn maybe_exec_native_operational_command(raw_argv: &[OsString]) -> Result<bool> {
    if std::env::var(RTK_OPERATIONAL_COMMAND_BYPASS_ENV).unwrap_or_default() != "1" {
        return Ok(false);
    }
    if raw_argv.is_empty() {
        return Ok(false);
    }

    let Some(operational_command) = operational_command_name_from_argv0(&raw_argv[0]) else {
        return Ok(false);
    };
    if !metadata::is_shim_eligible_top_level_command(&operational_command) {
        return Ok(false);
    }

    let mut cmd = native_command(&operational_command).with_context(|| {
        format!(
            "Failed to resolve native command for '{}'",
            operational_command
        )
    })?;
    cmd.args(raw_argv.iter().skip(1))
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());

    let status = cmd
        .status()
        .with_context(|| format!("Failed to execute native '{}'", operational_command))?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(true)
}

pub(crate) fn prepare_runtime_parse_argv() -> Result<Option<Vec<OsString>>> {
    let raw_argv: Vec<OsString> = std::env::args_os().collect();
    let recursion_depth = current_recursion_depth();
    if recursion_depth >= RTK_RECURSION_DEPTH_LIMIT {
        anyhow::bail!(
            "Detected recursive operational_command-shim invocation (depth={}). Refusing to continue.",
            recursion_depth
        );
    }

    if maybe_exec_native_operational_command(&raw_argv)? {
        return Ok(None);
    }

    // Child subprocesses should bypass operational_command rewrite and resolve native commands directly.
    install_child_bypass_env(recursion_depth);

    Ok(Some(build_parse_argv(&raw_argv)))
}

pub(crate) fn should_block_fallback_for_excluded_shim_command(parse_argv: &[OsString]) -> bool {
    let Some(operational_command) = parse_argv
        .first()
        .and_then(|s| operational_command_name_from_argv0(s))
    else {
        return false;
    };

    metadata::is_supported_top_level_command(&operational_command)
        && !metadata::is_shim_eligible_top_level_command(&operational_command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_build_parse_argv_rewrites_known_operational_command() {
        let raw = vec![
            OsString::from("/tmp/git"),
            OsString::from("status"),
            OsString::from("-s"),
        ];
        let rewritten = build_parse_argv(&raw);
        assert_eq!(
            rewritten,
            vec![
                OsString::from("rtk"),
                OsString::from("git"),
                OsString::from("status"),
                OsString::from("-s")
            ]
        );
    }

    #[test]
    fn test_build_parse_argv_rewrites_hyphenated_operational_command() {
        let raw = vec![
            OsString::from("/usr/local/bin/golangci-lint"),
            OsString::from("run"),
        ];
        let rewritten = build_parse_argv(&raw);
        assert_eq!(
            rewritten,
            vec![
                OsString::from("rtk"),
                OsString::from("golangci-lint"),
                OsString::from("run")
            ]
        );
    }

    #[test]
    fn test_build_parse_argv_does_not_rewrite_rtk_binary() {
        let raw = vec![
            OsString::from("/usr/local/bin/rtk"),
            OsString::from("git"),
            OsString::from("status"),
        ];
        assert_eq!(build_parse_argv(&raw), raw);
    }

    #[test]
    fn test_build_parse_argv_does_not_rewrite_unknown_operational_command() {
        let raw = vec![OsString::from("/tmp/random-tool"), OsString::from("status")];
        assert_eq!(build_parse_argv(&raw), raw);
    }

    #[test]
    fn test_build_parse_argv_does_not_rewrite_excluded_shim_command() {
        let raw = vec![OsString::from("/tmp/gain"), OsString::from("--graph")];
        assert_eq!(build_parse_argv(&raw), raw);
    }

    #[test]
    fn test_fallback_guard_blocks_excluded_shim_command_symlink() {
        let gain_like = vec![OsString::from("/tmp/gain"), OsString::from("--bad-flag")];
        assert!(should_block_fallback_for_excluded_shim_command(&gain_like));

        let git_like = vec![OsString::from("/tmp/git"), OsString::from("--bad-flag")];
        assert!(!should_block_fallback_for_excluded_shim_command(&git_like));
    }

    #[test]
    fn test_operational_command_name_from_argv0_strips_exe_suffix() {
        let name = operational_command_name_from_argv0(std::ffi::OsStr::new("git.exe"));
        assert_eq!(name.as_deref(), Some("git"));
    }

    #[test]
    fn test_shim_eligible_commands_excludes_meta_and_rtk_native() {
        let commands = metadata::shim_eligible_top_level_commands();
        assert!(commands.iter().any(|c| c == "git"));
        assert!(commands.iter().any(|c| c == "curl"));
        assert!(commands.iter().any(|c| c == "aws"));
        assert!(commands.iter().any(|c| c == "psql"));
        assert!(commands.iter().any(|c| c == "wc"));
        assert!(commands.iter().any(|c| c == "mypy"));
        assert!(!commands.iter().any(|c| c == "gain"));
        assert!(!commands.iter().any(|c| c == "read"));
        assert!(!commands.iter().any(|c| c == "shim"));
    }

    #[test]
    fn test_shim_eligible_commands_are_operational() {
        for cmd in metadata::shim_eligible_top_level_commands() {
            assert!(
                metadata::top_level_command_metadata(&cmd)
                    .map(|meta| meta.operational)
                    .unwrap_or(false),
                "Expected '{}' to be operational",
                cmd
            );
        }
    }

    #[test]
    fn test_metadata_commands_are_not_shim_eligible() {
        for meta in metadata::TOP_LEVEL_COMMAND_METADATA
            .iter()
            .filter(|meta| meta.metadata)
        {
            assert!(
                !meta.shim,
                "metadata command '{}' must not be shim-eligible",
                meta.name
            );
            assert!(
                !metadata::is_shim_eligible_top_level_command(meta.name),
                "metadata command '{}' must not be shim-eligible at runtime",
                meta.name
            );
        }
    }

    #[test]
    fn test_resolve_shim_operational_commands_rejects_non_eligible() {
        let allowed = metadata::shim_eligible_top_level_commands();
        let err = resolve_shim_operational_commands(&["gain".to_string()], &allowed).unwrap_err();
        assert!(
            err.to_string().contains("not Shim-eligible"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_shim_exclusion_policy_covers_guarded_commands() {
        for cmd in [
            "gain",
            "discover",
            "learn",
            "init",
            "config",
            "shim",
            "proxy",
            "hook-audit",
            "cc-economics",
            "rewrite",
            "verify",
            "read",
            "smart",
        ] {
            assert!(metadata::is_supported_top_level_command(cmd));
            assert!(
                !metadata::is_shim_eligible_top_level_command(cmd),
                "Expected '{}' to be excluded from Shim operational_command mode",
                cmd
            );
        }
    }

    #[test]
    fn test_shim_is_not_operational() {
        let argv: Vec<OsString> = ["rtk", "shim", "install", "git"]
            .iter()
            .map(OsString::from)
            .collect();
        assert!(!metadata::is_operational_command_from_parse_argv(&argv));
    }

    #[test]
    fn test_resolve_shim_operational_commands_rejects_non_normal_component() {
        let allowed = vec![
            "git".to_string(),
            ".".to_string(),
            "..".to_string(),
            "foo/bar".to_string(),
        ];
        assert!(resolve_shim_operational_commands(&["git".to_string()], &allowed).is_ok());

        for invalid in [".", "..", "foo/bar"] {
            let err = resolve_shim_operational_commands(&[invalid.to_string()], &allowed)
                .expect_err("expected invalid operational_command name to fail");
            assert!(
                err.to_string()
                    .contains(&format!("invalid operational_command name: '{}'", invalid)),
                "unexpected error: {}",
                err
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn test_ensure_shim_install_supported_platform_non_windows() {
        ensure_shim_install_supported_platform().expect("non-Windows should support shim install");
    }

    #[cfg(windows)]
    #[test]
    fn test_ensure_shim_install_supported_platform_windows_fails_fast() {
        let err = ensure_shim_install_supported_platform()
            .expect_err("Windows shim install should fail fast");
        assert!(
            err.to_string().contains("unsupported on Windows"),
            "unexpected error: {}",
            err
        );
    }

    #[cfg(unix)]
    fn create_executable_file(path: &std::path::Path) {
        let mut f = fs::File::create(path).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "exit 0").unwrap();
        drop(f);

        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_non_self_from_paths_skips_self_symlink() {
        let current_exe = env::current_exe().unwrap();

        let first = tempdir().unwrap();
        let second = tempdir().unwrap();

        let fake_self = first.path().join("git");
        std::os::unix::fs::symlink(&current_exe, &fake_self).unwrap();

        let native_git = second.path().join("git");
        let mut f = fs::File::create(&native_git).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "echo native").unwrap();
        drop(f);

        let mut perms = fs::metadata(&native_git).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&native_git, perms).unwrap();

        let paths = vec![first.path().to_path_buf(), second.path().to_path_buf()];
        let resolved = resolve_non_self_from_paths("git", &paths, &current_exe).unwrap();
        assert_eq!(resolved, native_git);
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_non_self_command_errors_when_only_self_exists() {
        let current_exe = env::current_exe().unwrap();
        let dir = tempdir().unwrap();
        let fake_self = dir.path().join("ls");
        std::os::unix::fs::symlink(&current_exe, &fake_self).unwrap();

        let resolved = resolve_non_self_from_paths("ls", &[dir.path().to_path_buf()], &current_exe);
        assert!(resolved.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_install_operational_command_shims_create_skip_replace() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        let primary_dir = dir.path().join("primary");
        let secondary_dir = dir.path().join("secondary");
        fs::create_dir_all(&primary_dir).unwrap();
        fs::create_dir_all(&secondary_dir).unwrap();

        let primary_rtk = primary_dir.join("rtk");
        let secondary_rtk = secondary_dir.join("rtk");
        create_executable_file(&primary_rtk);
        create_executable_file(&secondary_rtk);
        let primary_abs = fs::canonicalize(&primary_rtk).unwrap();

        let commands = vec!["git".to_string(), "curl".to_string()];
        install_operational_command_shims(
            Some(bin_dir.clone()),
            Some(primary_rtk.clone()),
            false,
            false,
            &commands,
        )
        .unwrap();

        let git_link = bin_dir.join("git");
        assert!(fs::symlink_metadata(&git_link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&git_link).unwrap(), primary_abs);

        install_operational_command_shims(
            Some(bin_dir.clone()),
            Some(primary_rtk.clone()),
            false,
            false,
            &commands,
        )
        .unwrap();
        assert_eq!(fs::read_link(&git_link).unwrap(), primary_abs);

        fs::remove_file(&git_link).unwrap();
        std::os::unix::fs::symlink(&secondary_rtk, &git_link).unwrap();

        install_operational_command_shims(
            Some(bin_dir.clone()),
            Some(primary_rtk),
            true,
            false,
            &["git".to_string()],
        )
        .unwrap();
        assert_eq!(fs::read_link(&git_link).unwrap(), primary_abs);
    }

    #[cfg(unix)]
    #[test]
    fn test_install_operational_command_shims_force_refuses_directory() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(bin_dir.join("git")).unwrap();

        let rtk_bin = dir.path().join("rtk");
        create_executable_file(&rtk_bin);

        let err = install_operational_command_shims(
            Some(bin_dir),
            Some(rtk_bin),
            true,
            false,
            &["git".to_string()],
        )
        .expect_err("expected replacing directory to fail");
        assert!(
            err.to_string()
                .contains("refusing to replace existing directory"),
            "unexpected error: {}",
            err
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_install_operational_command_shims_force_refuses_non_shim_without_force_all() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("git"), "plain file").unwrap();

        let rtk_bin = dir.path().join("rtk");
        create_executable_file(&rtk_bin);

        let err = install_operational_command_shims(
            Some(bin_dir.clone()),
            Some(rtk_bin.clone()),
            true,
            false,
            &["git".to_string()],
        )
        .expect_err("expected non-shim replacement to require --force-all");
        assert!(
            err.to_string().contains("--force-all"),
            "unexpected error: {}",
            err
        );

        install_operational_command_shims(
            Some(bin_dir.clone()),
            Some(rtk_bin),
            false,
            true,
            &["git".to_string()],
        )
        .unwrap();
        assert!(fs::symlink_metadata(bin_dir.join("git"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn test_install_operational_command_shims_rejects_non_executable_rtk_bin() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        let non_exec_rtk = dir.path().join("rtk");
        fs::write(&non_exec_rtk, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(&non_exec_rtk).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&non_exec_rtk, perms).unwrap();

        let err = install_operational_command_shims(
            Some(bin_dir),
            Some(non_exec_rtk),
            false,
            false,
            &["git".to_string()],
        )
        .expect_err("expected non-executable rtk bin to fail");
        assert!(
            err.to_string().contains("not executable"),
            "unexpected error: {}",
            err
        );
    }
}
