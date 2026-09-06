//! End-to-end coverage for the Pi/Oh My Pi extension lifecycle.

use std::path::Path;
use std::process::{Command, Output};

#[cfg(unix)]
use std::os::unix::fs::symlink;

use tempfile::TempDir;

fn run_rtk(cwd: &Path, agent_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .env("HOME", cwd.join("home"))
        .env("PI_CODING_AGENT_DIR", agent_dir)
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn rtk")
}

#[cfg(unix)]
fn run_rtk_without_agent_dir(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .env("HOME", cwd.join("home"))
        .env_remove("PI_CODING_AGENT_DIR")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn rtk")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn omp_dry_run_missing_includes_footer() {
    let project = tempfile::tempdir().unwrap();
    let agent_dir = project.path().join("omp-agent");

    let output = run_rtk(
        project.path(),
        &agent_dir,
        &[
            "init",
            "--agent",
            "omp",
            "--global",
            "--uninstall",
            "--dry-run",
        ],
    );

    assert!(
        output.status.success(),
        "OMP dry-run failed: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("[dry-run] Nothing written."),
        "missing dry-run footer: {}",
        stdout(&output)
    );
}

#[test]
fn omp_dry_run_stock_includes_footer_and_real_uninstall_mentions_restart() {
    let project = tempfile::tempdir().unwrap();
    let agent_dir = project.path().join("omp-agent");

    let pi_install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global"],
    );
    assert!(
        pi_install.status.success(),
        "Pi install failed: {}",
        stderr(&pi_install)
    );

    let install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global"],
    );
    assert!(
        install.status.success(),
        "OMP install failed: {}",
        stderr(&install)
    );
    assert!(stderr(&install).contains("share the global extension path"));

    let dry_run = run_rtk(
        project.path(),
        &agent_dir,
        &[
            "init",
            "--agent",
            "omp",
            "--global",
            "--uninstall",
            "--dry-run",
        ],
    );
    assert!(
        dry_run.status.success(),
        "OMP uninstall dry-run failed: {}",
        stderr(&dry_run)
    );
    assert!(
        stdout(&dry_run).contains("[dry-run] would prompt before removing shared Pi/OMP extension")
    );
    assert!(stdout(&dry_run).contains("[dry-run] Nothing written."));

    let skipped = run_rtk(
        project.path(),
        &agent_dir,
        &[
            "init",
            "--agent",
            "omp",
            "--global",
            "--uninstall",
            "--no-patch",
        ],
    );
    assert!(
        !skipped.status.success(),
        "OMP uninstall skip unexpectedly succeeded: {}",
        stderr(&skipped)
    );
    assert!(!stdout(&skipped).contains("Skipped removal of shared Pi/OMP extension"));
    assert!(stderr(&skipped).contains("was not removed"));
    assert!(agent_dir.join("extensions/rtk.ts").exists());

    let uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &[
            "init",
            "--agent",
            "omp",
            "--global",
            "--uninstall",
            "--auto-patch",
        ],
    );
    assert!(
        uninstall.status.success(),
        "OMP uninstall failed: {}",
        stderr(&uninstall)
    );
    assert!(stdout(&uninstall).contains("Restart OMP to apply changes."));
    assert!(stderr(&uninstall).contains("share the global extension path"));
    assert!(!agent_dir.join("extensions/rtk.ts").exists());
}

#[test]
fn pi_dry_run_modified_extension_previews_confirmation_without_error() {
    let project = tempfile::tempdir().unwrap();
    let agent_dir = project.path().join("pi-agent");
    let extension_dir = project.path().join(".pi/extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    let extension = extension_dir.join("rtk.ts");
    let original = "// user-modified extension\nexport default () => {}\n";
    std::fs::write(&extension, original).unwrap();

    let output = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--dry-run"],
    );

    assert!(
        output.status.success(),
        "Pi dry-run failed: {}",
        stderr(&output)
    );
    assert!(stdout(&output).contains("[dry-run] would prompt before overwriting"));
    assert!(stdout(&output).contains("[dry-run] Nothing written."));
    assert_eq!(std::fs::read_to_string(&extension).unwrap(), original);

    let auto_dry_run = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--auto-patch", "--dry-run"],
    );
    assert!(
        auto_dry_run.status.success(),
        "Pi auto-patch dry-run failed: {}",
        stderr(&auto_dry_run)
    );
    assert!(stdout(&auto_dry_run).contains("[dry-run] would overwrite non-stock"));
    assert!(stdout(&auto_dry_run).contains("[dry-run] would update Pi extension"));
    assert_eq!(std::fs::read_to_string(&extension).unwrap(), original);

    let auto = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--auto-patch"],
    );
    assert!(
        auto.status.success(),
        "Pi auto-patch failed: {}",
        stderr(&auto)
    );
    assert_eq!(
        std::fs::read_to_string(extension).unwrap(),
        include_str!("../hooks/pi/rtk.ts")
    );
}

#[test]
fn pi_relocated_global_ignores_project_omp_config_when_sidecar_is_definitive() {
    let project = tempfile::tempdir().unwrap();
    let agent_dir = project.path().join("pi-agent");

    let install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global"],
    );
    assert!(
        install.status.success(),
        "Pi global install failed: {}",
        stderr(&install)
    );
    assert!(!stderr(&install).contains("share the global extension path"));
    std::fs::create_dir_all(project.path().join(".omp")).unwrap();

    let uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        uninstall.status.success(),
        "Pi global uninstall unexpectedly prompted or failed: {}",
        stderr(&uninstall)
    );
    assert!(!agent_dir.join("extensions/rtk.ts").exists());
}

#[test]
fn relocated_omp_ownership_is_tracked_symmetrically() {
    let project = tempfile::tempdir().unwrap();
    let agent_dir = project.path().join("omp-agent");
    let extension = agent_dir.join("extensions/rtk.ts");
    let ownership = agent_dir.join("extensions/.rtk-agents");

    let omp_install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global"],
    );
    assert!(
        omp_install.status.success(),
        "OMP global install failed: {}",
        stderr(&omp_install)
    );
    assert!(!stderr(&omp_install).contains("share the global extension path"));
    assert_eq!(std::fs::read_to_string(&ownership).unwrap(), "omp\n");

    let pi_uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        !pi_uninstall.status.success(),
        "Pi uninstall unexpectedly removed an OMP-owned shared extension: {}",
        stderr(&pi_uninstall)
    );
    assert!(stderr(&pi_uninstall).contains("share the global extension path"));
    assert!(extension.exists());

    let omp_uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global", "--uninstall"],
    );
    assert!(
        omp_uninstall.status.success(),
        "OMP-only uninstall failed: {}",
        stderr(&omp_uninstall)
    );
    assert!(!stderr(&omp_uninstall).contains("share the global extension path"));
    assert!(!extension.exists());
    assert!(!ownership.exists());
}

#[test]
fn relocated_global_legacy_fallback_warns_but_proceeds() {
    let project = tempfile::tempdir().unwrap();
    let agent_dir = project.path().join("pi-agent");
    let extension_dir = agent_dir.join("extensions");
    let extension = extension_dir.join("rtk.ts");
    std::fs::create_dir_all(&extension_dir).unwrap();
    std::fs::write(&extension, include_str!("../hooks/pi/rtk.ts")).unwrap();
    std::fs::create_dir_all(project.path().join(".omp")).unwrap();

    let uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        uninstall.status.success(),
        "legacy global uninstall unexpectedly used project-local OMP config: {}",
        stderr(&uninstall)
    );
    assert!(!stderr(&uninstall).contains("share the global extension path"));
    assert!(!extension.exists());

    std::fs::write(&extension, include_str!("../hooks/pi/rtk.ts")).unwrap();
    std::fs::create_dir_all(project.path().join("home/.omp")).unwrap();
    let fallback = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        fallback.status.success(),
        "home-level OMP fallback should not block a legacy uninstall: {}",
        stderr(&fallback)
    );
    assert!(stderr(&fallback).contains("could not confirm both agents' ownership"));
    assert!(stderr(&fallback).contains("proceeding with Pi uninstall"));
    assert!(!extension.exists());
}

#[test]
fn preexisting_shared_extension_without_sidecar_stays_uncertain() {
    let project = TempDir::new().unwrap();
    let agent_dir = project.path().join("pi-agent");
    let extension = agent_dir.join("extensions/rtk.ts");
    let ownership = agent_dir.join("extensions/.rtk-agents");
    std::fs::create_dir_all(extension.parent().unwrap()).unwrap();
    std::fs::write(&extension, include_str!("../hooks/pi/rtk.ts")).unwrap();

    let install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global"],
    );
    assert!(
        install.status.success(),
        "OMP install failed: {}",
        stderr(&install)
    );
    assert!(stderr(&install).contains("pre-existing extension has no ownership record"));
    assert!(
        !ownership.exists(),
        "a pre-existing extension without ownership must not become OMP-only"
    );

    let uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        uninstall.status.success(),
        "uncertain legacy uninstall should proceed: {}",
        stderr(&uninstall)
    );
    assert!(stderr(&uninstall).contains("could not confirm both agents' ownership"));
    assert!(!extension.exists());
}

#[test]
fn unreadable_ownership_sidecar_warns_without_erasing_state() {
    let project = TempDir::new().unwrap();
    let agent_dir = project.path().join("omp-agent");
    let extension = agent_dir.join("extensions/rtk.ts");
    let ownership = agent_dir.join("extensions/.rtk-agents");

    let pi_install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global"],
    );
    assert!(
        pi_install.status.success(),
        "Pi install failed: {}",
        stderr(&pi_install)
    );
    std::fs::write(&ownership, [0xff, 0xfe, 0xfd]).unwrap();

    let omp_install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global"],
    );
    assert!(
        omp_install.status.success(),
        "OMP install should not be blocked by an unreadable ownership sidecar: {}",
        stderr(&omp_install)
    );
    assert!(stderr(&omp_install).contains("ownership state"));
    assert!(stderr(&omp_install).contains("treating ownership as unknown"));
    assert!(stderr(&omp_install).contains("could not be updated because ownership is unknown"));
    assert_eq!(std::fs::read(&ownership).unwrap(), [0xff, 0xfe, 0xfd]);
    assert!(extension.exists());

    let pi_uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        pi_uninstall.status.success(),
        "unknown ownership should warn and proceed with explicit uninstall: {}",
        stderr(&pi_uninstall)
    );
    assert!(stderr(&pi_uninstall).contains("could not confirm both agents' ownership"));
    assert!(stderr(&pi_uninstall).contains("proceeding with Pi uninstall"));
    assert!(!extension.exists());
    assert!(!ownership.exists());
}

#[test]
fn relocated_install_reconciles_stale_ownership_after_manual_delete() {
    let project = TempDir::new().unwrap();
    let agent_dir = project.path().join("omp-agent");
    let extension = agent_dir.join("extensions/rtk.ts");
    let ownership = agent_dir.join("extensions/.rtk-agents");

    for agent in ["pi", "omp"] {
        let install = run_rtk(
            project.path(),
            &agent_dir,
            &["init", "--agent", agent, "--global"],
        );
        assert!(
            install.status.success(),
            "{agent} install failed: {}",
            stderr(&install)
        );
    }
    assert_eq!(std::fs::read_to_string(&ownership).unwrap(), "omp\npi\n");

    std::fs::remove_file(&extension).unwrap();
    let reinstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global"],
    );
    assert!(
        reinstall.status.success(),
        "Pi reinstall failed: {}",
        stderr(&reinstall)
    );
    assert_eq!(std::fs::read_to_string(&ownership).unwrap(), "pi\n");

    let uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        uninstall.status.success(),
        "stale OMP ownership still blocked Pi uninstall: {}",
        stderr(&uninstall)
    );
    assert!(!stderr(&uninstall).contains("share the global extension path"));
    assert!(!extension.exists());
    assert!(!ownership.exists());
}

#[test]
fn pi_modified_uninstall_dry_run_is_non_failing_preview() {
    let project = tempfile::tempdir().unwrap();
    let agent_dir = project.path().join("pi-agent");
    let extension_dir = project.path().join(".pi/extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    let extension = extension_dir.join("rtk.ts");
    std::fs::write(
        &extension,
        format!(
            "{}\n// user modification\n",
            include_str!("../hooks/pi/rtk.ts")
        ),
    )
    .unwrap();

    let output = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--uninstall", "--dry-run"],
    );

    assert!(
        output.status.success(),
        "Pi uninstall dry-run failed: {}",
        stderr(&output)
    );
    assert!(stdout(&output).contains("[dry-run] would refuse to remove Pi extension"));
    assert!(stdout(&output).contains("[dry-run] Nothing written."));
    assert!(extension.exists());
}

#[test]
fn omp_modified_uninstall_dry_run_is_non_failing_preview() {
    let project = tempfile::tempdir().unwrap();
    let agent_dir = project.path().join("omp-agent");
    let extension_dir = project.path().join(".omp/extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    let extension = extension_dir.join("rtk.ts");
    std::fs::write(
        &extension,
        format!(
            "{}\n// user modification\n",
            include_str!("../hooks/pi/rtk.ts")
        ),
    )
    .unwrap();

    let output = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--uninstall", "--dry-run"],
    );

    assert!(
        output.status.success(),
        "OMP uninstall dry-run failed: {}",
        stderr(&output)
    );
    assert!(stdout(&output).contains("[dry-run] would refuse to remove OMP extension"));
    assert!(stdout(&output).contains("[dry-run] Nothing written."));
    assert!(extension.exists());
}

#[test]
fn omp_show_distinguishes_modified_and_unrelated_content() {
    let project = TempDir::new().unwrap();
    let agent_dir = project.path().join("omp-agent");
    let extension_dir = agent_dir.join("extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    std::fs::write(
        extension_dir.join("rtk.ts"),
        format!(
            "{}\n// user modification\n",
            include_str!("../hooks/pi/rtk.ts")
        ),
    )
    .unwrap();

    let output = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global", "--show"],
    );

    assert!(
        output.status.success(),
        "OMP show failed: {}",
        stderr(&output)
    );
    assert!(stdout(&output).contains(
        "modified RTK content - rtk init will ask before overwriting; use --auto-patch to replace"
    ));
    assert!(!stdout(&output).contains("will refuse to overwrite"));
    assert!(stdout(&output).contains(&format!(
        "# Configure {}",
        extension_dir.join("rtk.ts").display()
    )));

    std::fs::write(
        extension_dir.join("rtk.ts"),
        "// unrelated user file\nexport default () => {}\n",
    )
    .unwrap();
    let unrelated = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global", "--show"],
    );
    assert!(
        unrelated.status.success(),
        "OMP show failed for unrelated content: {}",
        stderr(&unrelated)
    );
    assert!(stdout(&unrelated).contains(
        "unrelated content - rtk init will ask before overwriting; use --auto-patch to replace"
    ));
}

#[test]
fn omp_show_reports_unreadable_extension_and_continues() {
    let project = TempDir::new().unwrap();
    let agent_dir = project.path().join("omp-agent");
    let extension_dir = agent_dir.join("extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    std::fs::write(extension_dir.join("rtk.ts"), [0xff, 0xfe, 0xfd]).unwrap();

    let output = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global", "--show"],
    );

    assert!(
        output.status.success(),
        "OMP show failed for unreadable extension: {}",
        stderr(&output)
    );
    assert!(stdout(&output).contains("Global extension:"));
    assert!(stdout(&output).contains("(unreadable)"));
    assert!(stdout(&output).contains("Project extension:"));
}

#[test]
fn omp_unreadable_install_is_recoverable_with_auto_patch() {
    let project = TempDir::new().unwrap();
    let agent_dir = project.path().join("omp-agent");
    let extension_dir = agent_dir.join("extensions");
    let extension = extension_dir.join("rtk.ts");
    std::fs::create_dir_all(&extension_dir).unwrap();
    let original = [0xff, 0xfe, 0xfd];
    std::fs::write(&extension, original).unwrap();

    let dry_run = run_rtk(
        project.path(),
        &agent_dir,
        &[
            "init",
            "--agent",
            "omp",
            "--global",
            "--auto-patch",
            "--dry-run",
        ],
    );
    assert!(
        dry_run.status.success(),
        "OMP unreadable install dry-run failed: {}",
        stderr(&dry_run)
    );
    assert!(stdout(&dry_run).contains("[dry-run] would overwrite non-stock"));
    assert!(stdout(&dry_run).contains("[dry-run] would update OMP extension"));
    assert_eq!(std::fs::read(&extension).unwrap(), original);

    let auto = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global", "--auto-patch"],
    );
    assert!(
        auto.status.success(),
        "OMP unreadable install failed: {}",
        stderr(&auto)
    );
    assert_eq!(
        std::fs::read_to_string(&extension).unwrap(),
        include_str!("../hooks/pi/rtk.ts")
    );
}

#[test]
fn omp_unreadable_uninstall_is_dry_run_preview_but_normal_failure() {
    let project = TempDir::new().unwrap();
    let agent_dir = project.path().join("omp-agent");
    let extension_dir = agent_dir.join("extensions");
    let extension = extension_dir.join("rtk.ts");
    std::fs::create_dir_all(&extension_dir).unwrap();
    std::fs::write(&extension, [0xff, 0xfe, 0xfd]).unwrap();

    let dry_run = run_rtk(
        project.path(),
        &agent_dir,
        &[
            "init",
            "--agent",
            "omp",
            "--global",
            "--uninstall",
            "--dry-run",
        ],
    );
    assert!(
        dry_run.status.success(),
        "OMP unreadable uninstall dry-run failed: {}",
        stderr(&dry_run)
    );
    assert!(stdout(&dry_run).contains("[dry-run] would leave unreadable OMP extension unchanged"));
    assert!(stdout(&dry_run).contains("[dry-run] Nothing written."));
    assert!(extension.exists());

    let uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global", "--uninstall"],
    );
    assert!(
        !uninstall.status.success(),
        "OMP unreadable uninstall unexpectedly succeeded"
    );
    assert!(stderr(&uninstall).contains("could not be read; leaving it alone"));
    assert!(extension.exists());
}

#[cfg(unix)]
#[test]
fn symlinked_global_agent_directories_are_detected_as_shared() {
    let project = TempDir::new().unwrap();
    let home = project.path().join("home");
    let pi_agent = home.join(".pi/agent");
    let omp_agent = home.join(".omp/agent");
    let extension = pi_agent.join("extensions/rtk.ts");
    let ownership = pi_agent.join("extensions/.rtk-agents");

    std::fs::create_dir_all(extension.parent().unwrap()).unwrap();
    std::fs::create_dir_all(omp_agent.parent().unwrap()).unwrap();
    symlink(&pi_agent, &omp_agent).unwrap();

    let install =
        run_rtk_without_agent_dir(project.path(), &["init", "--agent", "omp", "--global"]);
    assert!(
        install.status.success(),
        "OMP install through a symlinked agent directory failed: {}",
        stderr(&install)
    );
    assert!(stderr(&install).contains("resolve to the same global extension path"));
    assert_eq!(std::fs::read_to_string(&ownership).unwrap(), "omp\n");

    let uninstall = run_rtk_without_agent_dir(
        project.path(),
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        !uninstall.status.success(),
        "Pi uninstall should protect the symlinked shared extension"
    );
    assert!(stderr(&uninstall).contains("share the global extension path"));
    assert!(extension.exists());
}

#[cfg(unix)]
#[test]
fn symlinked_global_extension_files_share_one_ownership_state() {
    let project = TempDir::new().unwrap();
    let home = project.path().join("home");
    let pi_extension = home.join(".pi/agent/extensions/rtk.ts");
    let omp_extension = home.join(".omp/agent/extensions/rtk.ts");
    let pi_ownership = pi_extension.with_file_name(".rtk-agents");
    let omp_ownership = omp_extension.with_file_name(".rtk-agents");

    std::fs::create_dir_all(pi_extension.parent().unwrap()).unwrap();
    std::fs::create_dir_all(omp_extension.parent().unwrap()).unwrap();
    std::fs::write(&pi_extension, include_str!("../hooks/pi/rtk.ts")).unwrap();
    std::fs::write(&pi_ownership, "pi\n").unwrap();
    symlink(&pi_extension, &omp_extension).unwrap();

    let omp_install =
        run_rtk_without_agent_dir(project.path(), &["init", "--agent", "omp", "--global"]);
    assert!(
        omp_install.status.success(),
        "OMP install through a file symlink failed: {}",
        stderr(&omp_install)
    );
    assert!(stderr(&omp_install).contains("share the global extension path"));

    let pi_install =
        run_rtk_without_agent_dir(project.path(), &["init", "--agent", "pi", "--global"]);
    assert!(
        pi_install.status.success(),
        "Pi install through a file symlink failed: {}",
        stderr(&pi_install)
    );
    assert_eq!(std::fs::read_to_string(&pi_ownership).unwrap(), "omp\npi\n");
    assert!(
        !omp_ownership.exists(),
        "file-level aliases must not split ownership state"
    );

    let uninstall = run_rtk_without_agent_dir(
        project.path(),
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        !uninstall.status.success(),
        "Pi uninstall should protect the shared file-level symlink"
    );
    assert!(stderr(&uninstall).contains("share the global extension path"));
    assert!(pi_extension.exists());
    assert!(omp_extension.exists());

    let omp_uninstall = run_rtk_without_agent_dir(
        project.path(),
        &[
            "init",
            "--agent",
            "omp",
            "--global",
            "--uninstall",
            "--auto-patch",
        ],
    );
    assert!(
        omp_uninstall.status.success(),
        "OMP symlink uninstall failed: {}",
        stderr(&omp_uninstall)
    );
    assert!(stderr(&omp_uninstall).contains("changes a path used by the other agent"));
    assert!(!omp_extension.exists(), "the OMP symlink should be removed");
    assert!(pi_extension.exists(), "the real Pi file should survive");
    assert!(
        !pi_ownership.exists(),
        "the canonical ownership state should be removed after symlink uninstall"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_project_agent_directories_are_detected_as_shared() {
    let project = TempDir::new().unwrap();
    let pi_dir = project.path().join(".pi");
    let omp_dir = project.path().join(".omp");
    let pi_extension = pi_dir.join("extensions/rtk.ts");
    let ownership = pi_dir.join("extensions/.rtk-agents");

    std::fs::create_dir_all(pi_extension.parent().unwrap()).unwrap();
    symlink(&pi_dir, &omp_dir).unwrap();

    let omp_install = run_rtk_without_agent_dir(project.path(), &["init", "--agent", "omp"]);
    assert!(
        omp_install.status.success(),
        "project-local OMP install through a symlink failed: {}",
        stderr(&omp_install)
    );
    assert!(stderr(&omp_install).contains("resolve to the same project extension path"));

    let pi_install = run_rtk_without_agent_dir(project.path(), &["init", "--agent", "pi"]);
    assert!(
        pi_install.status.success(),
        "project-local Pi install through a symlink failed: {}",
        stderr(&pi_install)
    );
    assert!(stderr(&pi_install).contains("share the project extension path"));
    assert_eq!(std::fs::read_to_string(&ownership).unwrap(), "omp\npi\n");

    let pi_uninstall = run_rtk_without_agent_dir(
        project.path(),
        &["init", "--agent", "pi", "--uninstall", "--no-patch"],
    );
    assert!(
        !pi_uninstall.status.success(),
        "Pi uninstall should protect the project-local shared extension"
    );
    assert!(stderr(&pi_uninstall).contains("share the project extension path"));
    assert!(pi_extension.exists());
}
