use crate::core::user_dirs;
use crate::core::user_env;
use crate::core::utils::{resolve_binary, resolved_command, tool_exists};
use std::path::{Path, PathBuf};
use std::process::Command;

const LOCAL_VENV_DIRS: &[&str] = &[".venv", "venv"];

fn has_active_python_env_from(virtual_env: Option<&str>, conda_prefix: Option<&str>) -> bool {
    virtual_env.is_some_and(|v| !v.trim().is_empty())
        || conda_prefix.is_some_and(|v| !v.trim().is_empty())
}

fn has_active_python_env() -> bool {
    has_active_python_env_from(
        user_env::var("VIRTUAL_ENV").as_deref(),
        user_env::var("CONDA_PREFIX").as_deref(),
    )
}

fn local_venv_bin_candidates_in(base_dir: &Path, binary: &str) -> Vec<PathBuf> {
    let subdirs: &[&str] = if cfg!(windows) {
        &["Scripts", "bin"]
    } else {
        &["bin"]
    };
    let mut paths = Vec::new();
    for venv in LOCAL_VENV_DIRS {
        for sub in subdirs {
            paths.push(base_dir.join(venv).join(sub).join(binary));
        }
    }
    paths
}

fn find_local_venv_binary_in(base_dir: &Path, binary: &str) -> Option<PathBuf> {
    local_venv_bin_candidates_in(base_dir, binary)
        .into_iter()
        .find(|candidate| {
            let name = candidate.to_string_lossy();
            resolve_binary(&name).is_ok() || candidate.exists()
        })
}

/// Resolve a Python CLI tool (`pytest`, `ruff`, `mypy`, `sqlfluff`), preferring
/// the project's `.venv` or `venv` when no virtual environment is already active
/// on `PATH`, then `PATH`, then a `-m <tool>` interpreter fallback.
pub fn python_tool_command(tool: &str) -> Command {
    let cwd = user_dirs::in_working_dir(".");
    let active_env = has_active_python_env();

    if !active_env && let Some(local_tool) = find_local_venv_binary_in(&cwd, tool) {
        return resolved_command(&local_tool.to_string_lossy());
    }

    if tool_exists(tool) {
        return resolved_command(tool);
    }

    if !active_env {
        for py in ["python3", "python"] {
            if let Some(local_py) = find_local_venv_binary_in(&cwd, py) {
                let mut cmd = resolved_command(&local_py.to_string_lossy());
                cmd.arg("-m").arg(tool);
                return cmd;
            }
        }
    }

    let py = if tool_exists("python3") {
        "python3"
    } else {
        "python"
    };
    let mut cmd = resolved_command(py);
    cmd.arg("-m").arg(tool);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_active_python_env_detection() {
        assert!(!has_active_python_env_from(None, None));
        assert!(!has_active_python_env_from(Some(""), Some("  ")));
        assert!(has_active_python_env_from(Some("/tmp/.venv"), None));
        assert!(has_active_python_env_from(
            None,
            Some("/opt/conda/envs/dev")
        ));
    }

    #[test]
    fn test_find_local_venv_binary_prefers_dot_venv_over_venv() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dot_venv_bin = dir.path().join(".venv").join("bin");
        let venv_bin = dir.path().join("venv").join("bin");
        std::fs::create_dir_all(&dot_venv_bin).expect("mkdir .venv/bin");
        std::fs::create_dir_all(&venv_bin).expect("mkdir venv/bin");

        let venv_ruff = venv_bin.join("ruff");
        std::fs::write(&venv_ruff, "#!/bin/sh\n").expect("write venv ruff");
        assert_eq!(
            find_local_venv_binary_in(dir.path(), "ruff"),
            Some(venv_ruff)
        );

        let dot_venv_ruff = dot_venv_bin.join("ruff");
        std::fs::write(&dot_venv_ruff, "#!/bin/sh\n").expect("write .venv ruff");
        assert_eq!(
            find_local_venv_binary_in(dir.path(), "ruff"),
            Some(dot_venv_ruff)
        );
    }
}
