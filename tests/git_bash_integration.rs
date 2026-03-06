#![cfg(windows)]

use std::borrow::Cow;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn rtk_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_rtk"))
}

fn git_bash_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    for env_name in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = env::var_os(env_name) {
            let root = PathBuf::from(root);
            candidates.push(root.join("Git").join("bin").join("bash.exe"));
            candidates.push(root.join("Git").join("usr").join("bin").join("bash.exe"));
        }
    }

    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        let root = PathBuf::from(local_app_data).join("Programs").join("Git");
        candidates.push(root.join("bin").join("bash.exe"));
        candidates.push(root.join("usr").join("bin").join("bash.exe"));
    }

    candidates
}

fn git_bash_path() -> Option<PathBuf> {
    git_bash_candidates()
        .into_iter()
        .find(|path| path.is_file())
}

fn git_bash_available() -> bool {
    git_bash_path().is_some()
}

fn to_git_bash_path(path: &Path) -> String {
    let raw: Cow<'_, str> = path.to_string_lossy();
    let normalized = raw.replace('\\', "/");

    let bytes = normalized.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && bytes[2] == b'/' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        format!("/{}/{}", drive, &normalized[3..])
    } else {
        normalized
    }
}

fn run_git_bash(script: &str) -> Output {
    let git_bash = git_bash_path().expect("Git Bash is required for this integration test");
    let rtk_dir = to_git_bash_path(
        rtk_binary()
            .parent()
            .expect("rtk test binary should have a parent directory"),
    );

    Command::new(git_bash)
        .arg("-lc")
        .arg(format!("export PATH=\"$PATH:{}\"; {}", rtk_dir, script))
        .current_dir(repo_root())
        .output()
        .expect("failed to execute Git Bash")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn git_bash_is_available() {
    let Some(git_bash) = git_bash_path() else {
        eprintln!("Git Bash is not installed; skipping Git Bash integration checks");
        return;
    };

    let output = Command::new(git_bash)
        .arg("--version")
        .output()
        .expect("failed to execute Git Bash --version");

    assert!(
        output.status.success(),
        "expected Git Bash to work, stderr was: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("GNU bash"),
        "expected GNU bash output, stdout was: {}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("pc-msys"),
        "expected Git for Windows Bash output, stdout was: {}",
        stdout(&output)
    );
}

#[test]
fn rtk_binary_runs_by_absolute_path_inside_git_bash() {
    if !git_bash_available() {
        eprintln!("Git Bash is not installed; skipping Git Bash integration checks");
        return;
    }

    let output = run_git_bash(&format!("\"{}\" --version", to_git_bash_path(rtk_binary())));

    assert!(
        output.status.success(),
        "expected success, stderr was: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("rtk"),
        "expected version output to mention rtk, stdout was: {}",
        stdout(&output)
    );
}

#[test]
fn rtk_tools_run_inside_git_bash_via_path() {
    if !git_bash_available() {
        eprintln!("Git Bash is not installed; skipping Git Bash integration checks");
        return;
    }

    let output = run_git_bash(
        "command -v rtk && rtk ls . && rtk read Cargo.toml && rtk rewrite \"git status\"",
    );

    assert!(
        output.status.success(),
        "expected success, stderr was: {}",
        stderr(&output)
    );

    let text = stdout(&output);
    assert!(
        text.contains("/rtk"),
        "expected Git Bash to resolve rtk on PATH, stdout was: {}",
        text
    );
    assert!(
        text.contains("Cargo.toml"),
        "expected output to mention Cargo.toml, stdout was: {}",
        text
    );
    assert!(
        text.contains("rtk git status"),
        "expected rewrite output, stdout was: {}",
        text
    );
}

#[test]
fn bash_check_installation_script_runs_with_rtk_on_path() {
    if !git_bash_available() {
        eprintln!("Git Bash is not installed; skipping Git Bash integration checks");
        return;
    }

    let output = run_git_bash("bash scripts/check-installation.sh");

    assert!(
        output.status.success(),
        "expected success, stderr was: {}",
        stderr(&output)
    );

    let text = stdout(&output);
    assert!(
        text.contains("RTK Installation Verification"),
        "expected script banner, stdout was: {}",
        text
    );
    assert!(
        text.contains("Full-featured RTK installation detected"),
        "expected successful installation verification, stdout was: {}",
        text
    );
}
