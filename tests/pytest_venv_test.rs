//! Real Python/pytest regressions, with no network access during tests.
//! Prepare wheels with `python -m pip download -d /path/to/wheels pytest`, then run
//! `RTK_TEST_PYTEST_WHEELS=/path/to/wheels cargo test --test pytest_venv_test -- --ignored`.
//! Set RTK_TEST_PYTHON if Python with venv/ensurepip is not available as python3 (python on Windows).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn checked(command: &mut Command) -> Output {
    let output = command.output().expect("run fixture command");
    assert!(
        output.status.success(),
        "{command:?}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn executable(venv: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join(format!("{name}.exe"))
    } else {
        venv.join("bin").join(name)
    }
}

fn create_venv(root: &Path, install_pytest: bool) {
    let python = std::env::var_os("RTK_TEST_PYTHON")
        .unwrap_or_else(|| if cfg!(windows) { "python" } else { "python3" }.into());
    checked(Command::new(python).args(["-m", "venv"]).arg(root));
    if install_pytest {
        let wheels = std::env::var_os("RTK_TEST_PYTEST_WHEELS")
            .expect("set RTK_TEST_PYTEST_WHEELS to an offline pytest wheel directory");
        checked(
            Command::new(executable(root, "python"))
                .args(["-m", "pip", "install", "--no-index", "--find-links"])
                .arg(wheels)
                .arg("pytest"),
        );
    }
}

fn pytest_command(program: &Path, project: &Path, expected: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(project)
        .env("EXPECTED_PREFIX", expected)
        .env("PYTEST_DISABLE_PLUGIN_AUTOLOAD", "1")
        .env_remove("PYTHONPATH")
        .env_remove("PYTHONHOME");
    command
}

fn rtk_command(project: &Path, path_venv: &Path, expected: &Path) -> Command {
    let mut command = pytest_command(Path::new(env!("CARGO_BIN_EXE_rtk")), project, expected);
    command
        .arg("pytest")
        .env_remove("VIRTUAL_ENV")
        .env(
            "PATH",
            executable(path_venv, "python")
                .parent()
                .expect("bin directory"),
        )
        .env("RTK_DB_PATH", project.join("tracking.db"));
    command
}

fn write_prefix_test(project: &Path) {
    std::fs::create_dir_all(project).expect("create project");
    std::fs::write(
        project.join("test_prefix.py"),
        "import os, sys\nfrom pathlib import Path\ndef test_prefix():\n    assert Path(sys.prefix).resolve() == Path(os.environ['EXPECTED_PREFIX']).resolve()\n",
    )
    .expect("write interpreter assertion");
}

#[test]
#[ignore = "requires Python venv/ensurepip and RTK_TEST_PYTEST_WHEELS"]
fn empty_project_venv_falls_back_to_working_path_pytest() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let project = temp.path().join("project with spaces");
    let path_venv = temp.path().join("path-venv");
    write_prefix_test(&project);
    create_venv(&project.join(".venv"), false);
    create_venv(&path_venv, true);

    checked(pytest_command(&executable(&path_venv, "pytest"), &project, &path_venv).arg("-q"));
    checked(&mut rtk_command(&project, &path_venv, &path_venv));
}

#[test]
#[ignore = "requires Python venv/ensurepip and RTK_TEST_PYTEST_WHEELS"]
fn active_module_only_venv_precedes_project_entrypoint() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let project = temp.path().join("project");
    let active = temp.path().join("active venv");
    write_prefix_test(&project);
    create_venv(&active, true);
    create_venv(&project.join(".venv"), true);
    std::fs::remove_file(executable(&active, "pytest")).expect("remove active entrypoint");

    checked(
        pytest_command(&executable(&active, "python"), &project, &active)
            .args(["-m", "pytest", "-q"]),
    );
    checked(rtk_command(&project, &project.join(".venv"), &active).env("VIRTUAL_ENV", &active));
}

#[test]
#[ignore = "requires Python venv/ensurepip and RTK_TEST_PYTEST_WHEELS"]
fn failing_module_run_preserves_exit_code_without_retrying_path() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let project = temp.path().join("project");
    let active = temp.path().join("active-venv");
    let path_venv = temp.path().join("path-venv");
    write_prefix_test(&project);
    create_venv(&active, true);
    create_venv(&path_venv, true);
    std::fs::remove_file(executable(&active, "pytest")).expect("remove active entrypoint");
    std::fs::write(
        project.join("test_failure.py"),
        "def test_failure():\n    assert False, 'intentional failure'\n",
    )
    .expect("write failing test");

    std::fs::write(
        project.join("conftest.py"),
        "import sys\nfrom pathlib import Path\ndef pytest_sessionstart(session):\n    with open('executed.txt', 'a') as log:\n        log.write(str(Path(sys.prefix).resolve()) + '\\n')\n",
    )
    .expect("record each test session's interpreter");

    for (expected_code, args) in [
        (1, vec![]),
        (5, vec!["-k", "no_matching_test"]),
        (2, vec![]),
    ] {
        if expected_code == 2 {
            std::fs::write(project.join("test_broken.py"), "this is invalid Python!\n")
                .expect("write collection error");
        }
        let native = pytest_command(&executable(&active, "python"), &project, &active)
            .args(["-m", "pytest", "-q"])
            .args(&args)
            .output()
            .expect("run native pytest");
        assert_eq!(native.status.code(), Some(expected_code));
        std::fs::remove_file(project.join("executed.txt")).expect("reset execution log");

        let output = rtk_command(&project, &path_venv, &active)
            .env("VIRTUAL_ENV", &active)
            .args(&args)
            .output()
            .expect("run rtk pytest");
        assert_eq!(output.status.code(), native.status.code());
        let executed =
            std::fs::read_to_string(project.join("executed.txt")).expect("read execution log");
        assert_eq!(executed.lines().count(), 1, "pytest must run exactly once");
        assert_eq!(
            Path::new(executed.trim())
                .canonicalize()
                .expect("resolve executed interpreter's prefix"),
            active.canonicalize().expect("resolve active venv")
        );
        std::fs::remove_file(project.join("executed.txt")).expect("reset execution log");
    }
}
