//! End-to-end coverage for the Pi/Oh My Pi extension lifecycle.

use std::path::Path;
use std::process::Output;

#[cfg(unix)]
use std::os::unix::fs::symlink;

use tempfile::TempDir;

mod common;

fn run_rtk(cwd: &Path, agent_dir: &Path, args: &[&str]) -> Output {
    common::rtk_command()
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
    common::rtk_command()
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
fn preexisting_shared_extension_gets_a_partial_record() {
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
    // RTK did not install the file that was already there and cannot know who
    // did, so the marker keeps the other agent's ownership an open question.
    assert_eq!(
        std::fs::read_to_string(&ownership).unwrap(),
        "omp\nunknown-prior\n",
        "a pre-existing extension must be recorded as partially owned, not OMP-only"
    );

    // OMP is a recorded owner, so removing the file as Pi is a known conflict
    // and must be protected even though the record is partial.
    let pi_uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        !pi_uninstall.status.success(),
        "recorded OMP ownership must protect the shared file"
    );
    assert!(stderr(&pi_uninstall).contains("share the global extension path"));
    assert!(extension.exists());

    // The reverse direction is the uncertain one: Pi is not recorded, but the
    // marker says the record predates RTK, so this warns instead of blocking.
    let omp_uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global", "--uninstall"],
    );
    assert!(
        omp_uninstall.status.success(),
        "uncertain ownership should warn and proceed: {}",
        stderr(&omp_uninstall)
    );
    assert!(stderr(&omp_uninstall).contains("could not confirm both agents' ownership"));
    assert!(!extension.exists());
    assert!(!ownership.exists());
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
fn relocated_install_preserves_other_agent_ownership_after_manual_delete() {
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
    // Deleting the file does not unconfigure OMP: it still resolves to this
    // path, and reinstalling for Pi restores the integration for both agents.
    // The recorded ownership therefore survives the delete.
    assert_eq!(std::fs::read_to_string(&ownership).unwrap(), "omp\npi\n");

    let uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        !uninstall.status.success(),
        "recorded OMP ownership must still protect the shared file"
    );
    assert!(stderr(&uninstall).contains("share the global extension path"));
    assert!(extension.exists());

    let approved = run_rtk(
        project.path(),
        &agent_dir,
        &[
            "init",
            "--agent",
            "pi",
            "--global",
            "--uninstall",
            "--auto-patch",
        ],
    );
    assert!(
        approved.status.success(),
        "approved shared uninstall failed: {}",
        stderr(&approved)
    );
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
    assert!(
        stdout(&output).contains("[dry-run] would prompt before removing modified Pi extension")
    );
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
    assert!(
        stdout(&output).contains("[dry-run] would prompt before removing modified OMP extension")
    );
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
    assert!(
        !stderr(&install).contains("could not confirm both agents' ownership"),
        "a first install on an empty path must not warn about unknown ownership"
    );
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
    assert!(
        !stderr(&omp_install).contains("could not confirm both agents' ownership"),
        "a first install on an empty path must not warn about unknown ownership"
    );

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

#[cfg(unix)]
#[test]
fn unaliased_paths_do_not_report_on_ownership_state() {
    let project = TempDir::new().unwrap();
    let extension_dir = project.path().join(".pi/extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    // A stray record at a path only Pi resolves to: no ownership decision is
    // made here, so its contents must not be announced.
    std::fs::write(extension_dir.join(".rtk-agents"), "pi\ngarbage-entry\n").unwrap();

    let install = run_rtk_without_agent_dir(project.path(), &["init", "--agent", "pi"]);
    assert!(install.status.success(), "{}", stderr(&install));
    assert!(
        !stderr(&install).contains("ownership state"),
        "unaliased install must not report on ownership state: {}",
        stderr(&install)
    );
}

#[cfg(unix)]
#[test]
fn crlf_extension_is_not_reported_as_up_to_date() {
    let project = TempDir::new().unwrap();
    let extension_dir = project.path().join(".pi/extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    // Still stock content, but byte-different, so the next install rewrites it.
    std::fs::write(
        extension_dir.join("rtk.ts"),
        include_str!("../hooks/pi/rtk.ts")
            .replace("\r\n", "\n")
            .replace('\n', "\r\n"),
    )
    .unwrap();

    let show = run_rtk_without_agent_dir(project.path(), &["init", "--show", "--agent", "pi"]);
    assert!(show.status.success(), "{}", stderr(&show));
    assert!(
        !stdout(&show).contains("(up to date)"),
        "a file the next install rewrites must not be reported as up to date: {}",
        stdout(&show)
    );
    assert!(stdout(&show).contains("stock version"));
}

#[cfg(unix)]
#[test]
fn shared_uninstall_dry_run_matches_the_real_run() {
    let project = TempDir::new().unwrap();
    let agent_dir = project.path().join("shared-agent");
    let ownership = agent_dir.join("extensions/.rtk-agents");

    for agent in ["pi", "omp"] {
        let install = run_rtk(
            project.path(),
            &agent_dir,
            &["init", "--agent", agent, "--global"],
        );
        assert!(install.status.success(), "{agent} install failed");
    }
    assert_eq!(std::fs::read_to_string(&ownership).unwrap(), "omp\npi\n");

    let preview = run_rtk(
        project.path(),
        &agent_dir,
        &[
            "init",
            "--agent",
            "pi",
            "--global",
            "--uninstall",
            "--dry-run",
            "--auto-patch",
        ],
    );
    assert!(preview.status.success(), "{}", stderr(&preview));

    let real = run_rtk(
        project.path(),
        &agent_dir,
        &[
            "init",
            "--agent",
            "pi",
            "--global",
            "--uninstall",
            "--auto-patch",
        ],
    );
    assert!(real.status.success(), "{}", stderr(&real));

    // The real run removes the file and its record with it; the preview must say the same.
    assert!(!ownership.exists());
    assert!(
        stdout(&preview).contains("would remove RTK extension ownership state"),
        "dry-run described a different outcome than the real run: {}",
        stdout(&preview)
    );
}

fn shared_agent_dir_with_record(record: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    // One agent directory serves both agents, so their global extension paths alias.
    let project = TempDir::new().unwrap();
    let agent_dir = project.path().join("agent");
    let extension = agent_dir.join("extensions/rtk.ts");
    std::fs::create_dir_all(extension.parent().unwrap()).unwrap();
    std::fs::write(&extension, include_str!("../hooks/pi/rtk.ts")).unwrap();
    let ownership = extension.with_file_name(".rtk-agents");
    std::fs::write(&ownership, record).unwrap();
    (project, agent_dir, ownership)
}

#[test]
fn a_declined_shared_install_does_not_announce_the_shared_integration() {
    let (project, agent_dir, _ownership) = shared_agent_dir_with_record("pi\n");
    let extension = agent_dir.join("extensions/rtk.ts");
    let unrelated = "// user-modified extension\nexport default () => {}\n";
    std::fs::write(&extension, unrelated).unwrap();

    let install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global", "--no-patch"],
    );
    assert!(
        !install.status.success(),
        "--no-patch declines the overwrite"
    );
    assert_eq!(std::fs::read_to_string(&extension).unwrap(), unrelated);
    assert!(
        !stderr(&install).contains("enables the shared integration"),
        "an install that does not happen must not be announced: {}",
        stderr(&install)
    );
}

#[test]
fn a_record_holding_only_the_marker_stays_repairable() {
    let (project, agent_dir, ownership) = shared_agent_dir_with_record("unknown-prior\n");

    let install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global"],
    );
    assert!(install.status.success(), "{}", stderr(&install));
    assert_eq!(
        std::fs::read_to_string(&ownership).unwrap(),
        "pi\nunknown-prior\n",
        "an install must be able to add itself to a record that names no agent yet"
    );
}

#[test]
fn recording_both_agents_completes_a_partial_record() {
    let (project, agent_dir, ownership) = shared_agent_dir_with_record("omp\nunknown-prior\n");

    let install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global"],
    );
    assert!(install.status.success(), "{}", stderr(&install));
    assert_eq!(
        std::fs::read_to_string(&ownership).unwrap(),
        "omp\npi\n",
        "a record naming both agents is complete, so the marker goes"
    );
}

#[test]
fn an_unreadable_record_is_reported_once() {
    // `zed` is no agent this version knows, so the record cannot be read.
    let (project, agent_dir, ownership) = shared_agent_dir_with_record("zed\n");

    let install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "omp", "--global"],
    );
    assert!(install.status.success(), "{}", stderr(&install));
    assert_eq!(
        std::fs::read_to_string(&ownership).unwrap(),
        "zed\n",
        "a record RTK cannot read beside an existing extension is left as it is"
    );
    assert_eq!(
        stderr(&install).matches("contains unknown entries").count(),
        1,
        "the record is reported on once, not again when it is re-read before the write: {}",
        stderr(&install)
    );
}

#[test]
fn an_unreadable_record_beside_no_extension_is_replaced() {
    let project = TempDir::new().unwrap();
    let agent_dir = project.path().join("agent");
    let extension = agent_dir.join("extensions/rtk.ts");
    std::fs::create_dir_all(extension.parent().unwrap()).unwrap();
    let ownership = extension.with_file_name(".rtk-agents");
    // Left behind by an extension that has since been deleted.
    std::fs::write(&ownership, "zed\n").unwrap();

    let install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global"],
    );
    assert!(install.status.success(), "{}", stderr(&install));
    assert_eq!(
        std::fs::read_to_string(&ownership).unwrap(),
        "pi\n",
        "a record describing no extension must not block recording the new install"
    );
}

#[cfg(unix)]
#[test]
fn modified_extension_is_removable_with_auto_patch() {
    for (agent, dir, name) in [("pi", ".pi", "Pi"), ("omp", ".omp", "OMP")] {
        let project = TempDir::new().unwrap();
        let extension_dir = project.path().join(dir).join("extensions");
        std::fs::create_dir_all(&extension_dir).unwrap();
        let extension = extension_dir.join("rtk.ts");
        // RTK content, but hand-edited so it matches no known stock revision.
        let edited = format!("{}\n// local tweak\n", include_str!("../hooks/pi/rtk.ts"));
        std::fs::write(&extension, &edited).unwrap();

        let refused =
            run_rtk_without_agent_dir(project.path(), &["init", "--agent", agent, "--uninstall"]);
        assert!(
            !refused.status.success(),
            "{agent}: modified RTK content must not be removed without approval"
        );
        assert!(stderr(&refused).contains("--auto-patch"), "{agent}");
        // The clause about the other agent sits between these two sentences.
        assert!(
            stderr(&refused)
                .contains("does not match the stock extension. Remove the file manually"),
            "{agent}: separate project directories share nothing: {}",
            stderr(&refused)
        );
        assert!(extension.exists(), "{agent}");

        let preview = run_rtk_without_agent_dir(
            project.path(),
            &[
                "init",
                "--agent",
                agent,
                "--uninstall",
                "--auto-patch",
                "--dry-run",
            ],
        );
        assert!(
            preview.status.success(),
            "{agent} dry-run: {}",
            stderr(&preview)
        );
        assert!(
            stdout(&preview).contains(&format!("[dry-run] would remove modified {name} extension")),
            "{agent}: {}",
            stdout(&preview)
        );
        assert_eq!(
            std::fs::read_to_string(&extension).unwrap(),
            edited,
            "{agent}: a dry run writes nothing"
        );

        let approved = run_rtk_without_agent_dir(
            project.path(),
            &["init", "--agent", agent, "--uninstall", "--auto-patch"],
        );
        assert!(
            approved.status.success(),
            "{agent}: --auto-patch should approve removing modified RTK content: {}",
            stderr(&approved)
        );
        assert!(
            !extension.exists(),
            "{agent}: approved uninstall must remove the modified extension"
        );
    }
}

#[test]
fn a_modified_removal_says_when_the_other_agent_loads_the_file() {
    for (agent, other) in [("pi", "OMP"), ("omp", "Pi")] {
        let partial = format!("{agent}\nunknown-prior\n");
        for (record, consequence) in [
            (
                Some("omp\npi\n"),
                format!("{other} loads the same file, so removing it removes it for {other} too"),
            ),
            (
                Some(partial.as_str()),
                format!("{other} may load the same file"),
            ),
            (None, format!("{other} may load the same file")),
        ] {
            let (project, agent_dir, ownership) =
                shared_agent_dir_with_record(record.unwrap_or(""));
            if record.is_none() {
                std::fs::remove_file(&ownership).unwrap();
            }
            let extension = agent_dir.join("extensions/rtk.ts");
            let edited = format!("{}\n// local tweak\n", include_str!("../hooks/pi/rtk.ts"));
            std::fs::write(&extension, &edited).unwrap();

            // No patch flag: the question is asked, and declined because stdin is not a terminal.
            let refused = run_rtk(
                project.path(),
                &agent_dir,
                &["init", "--agent", agent, "--global", "--uninstall"],
            );
            assert!(!refused.status.success(), "{agent} {record:?}");
            assert_eq!(
                stderr(&refused).matches(consequence.as_str()).count(),
                2,
                "{agent} {record:?}: the question and the advice to rerun with --auto-patch must both say who else loses the file: {}",
                stderr(&refused)
            );
            assert_eq!(std::fs::read_to_string(&extension).unwrap(), edited);

            let preview = run_rtk(
                project.path(),
                &agent_dir,
                &[
                    "init",
                    "--agent",
                    agent,
                    "--global",
                    "--uninstall",
                    "--dry-run",
                ],
            );
            assert!(
                preview.status.success(),
                "{agent} {record:?}: {}",
                stderr(&preview)
            );
            assert!(
                stdout(&preview).contains(consequence.as_str()),
                "{agent} {record:?}: the dry-run preview of the question must say it too: {}",
                stdout(&preview)
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn an_install_that_creates_the_shared_path_records_its_owner() {
    // `.omp` links to a `.pi` that does not exist yet: the paths only resolve to
    // one file once the Pi install creates `.pi/extensions`.
    let project = TempDir::new().unwrap();
    symlink(".pi", project.path().join(".omp")).unwrap();

    let install = run_rtk_without_agent_dir(project.path(), &["init", "--agent", "pi"]);
    assert!(install.status.success(), "{}", stderr(&install));
    assert_eq!(
        std::fs::read_to_string(project.path().join(".pi/extensions/.rtk-agents")).unwrap(),
        "pi\n"
    );

    let uninstall = run_rtk_without_agent_dir(
        project.path(),
        &["init", "--agent", "omp", "--uninstall", "--no-patch"],
    );
    assert!(
        !uninstall.status.success(),
        "removing OMP's view of the file must not take Pi's extension: {}",
        stderr(&uninstall)
    );
    assert!(stderr(&uninstall).contains("was not removed"));
    assert!(project.path().join(".pi/extensions/rtk.ts").exists());
}

#[cfg(unix)]
#[test]
fn a_record_replaced_when_the_paths_meet_is_not_reported_as_unknown() {
    // Pi's extension is a link to OMP's, which does not exist yet: the paths only
    // meet once the OMP install writes the file. The leftover record beside it
    // describes no extension and is replaced, so warning that ownership is
    // unknown would describe a decision RTK never takes.
    let project = TempDir::new().unwrap();
    let omp_extensions = project.path().join(".omp/extensions");
    let pi_extensions = project.path().join(".pi/extensions");
    std::fs::create_dir_all(&omp_extensions).unwrap();
    std::fs::create_dir_all(&pi_extensions).unwrap();
    std::fs::write(omp_extensions.join(".rtk-agents"), "garbage\n").unwrap();
    symlink("../../.omp/extensions/rtk.ts", pi_extensions.join("rtk.ts")).unwrap();

    let install = run_rtk_without_agent_dir(project.path(), &["init", "--agent", "omp"]);
    assert!(install.status.success(), "{}", stderr(&install));
    assert!(
        !stderr(&install).contains("treating ownership as unknown"),
        "{}",
        stderr(&install)
    );
    assert_eq!(
        std::fs::read_to_string(omp_extensions.join(".rtk-agents")).unwrap(),
        "omp\n"
    );
}
