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
    assert_eq!(
        std::fs::read_to_string(extension_dir.join("rtk.ts.bak")).unwrap(),
        original,
        "an approved overwrite must preserve what it replaced"
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
    // Only the symlink was removed; Pi still owns the surviving file, so its
    // claim must outlive OMP's uninstall or the shared-file protection is lost.
    assert_eq!(
        std::fs::read_to_string(&pi_ownership).unwrap(),
        "pi\n",
        "the surviving owner's record must be kept, minus the uninstalled agent"
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
fn dangling_symlink_alias_is_detected_and_preserved() {
    let project = TempDir::new().unwrap();
    let pi_extension = project.path().join(".pi/extensions/rtk.ts");
    let omp_extension = project.path().join(".omp/extensions/rtk.ts");
    let ownership = pi_extension.with_file_name(".rtk-agents");

    // Only the link's own directory exists: the target directory is created by
    // the install, which must follow the link to decide where to write.
    std::fs::create_dir_all(omp_extension.parent().unwrap()).unwrap();
    symlink(&pi_extension, &omp_extension).unwrap();

    let install = run_rtk_without_agent_dir(project.path(), &["init", "--agent", "omp"]);
    assert!(
        install.status.success(),
        "OMP install through a dangling symlink failed: {}",
        stderr(&install)
    );

    assert!(
        std::fs::symlink_metadata(&omp_extension)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the user's symlink must survive the install"
    );
    assert!(
        pi_extension.exists(),
        "the extension must be written through the link to its target"
    );
    assert_eq!(
        std::fs::read_to_string(&ownership).unwrap(),
        "omp\n",
        "ownership must be recorded once, at the canonical location"
    );

    let pi_uninstall =
        run_rtk_without_agent_dir(project.path(), &["init", "--agent", "pi", "--uninstall"]);
    assert!(
        !pi_uninstall.status.success(),
        "Pi uninstall should protect the aliased extension"
    );
    assert!(pi_extension.exists());
}

#[cfg(unix)]
#[test]
fn dangling_directory_symlink_alias_installs_into_the_target() {
    let project = TempDir::new().unwrap();
    let pi_dir = project.path().join(".pi/extensions");
    let omp_dir = project.path().join(".omp/extensions");

    // The alias is on an ancestor directory, and its target does not exist yet.
    symlink(project.path().join(".pi"), project.path().join(".omp")).unwrap();

    let install = run_rtk_without_agent_dir(project.path(), &["init", "--agent", "omp"]);
    assert!(
        install.status.success(),
        "OMP install through a dangling directory symlink failed: {}",
        stderr(&install)
    );
    assert!(
        std::fs::symlink_metadata(project.path().join(".omp"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the user's directory symlink must survive the install"
    );
    assert!(pi_dir.join("rtk.ts").exists());
    assert!(omp_dir.join("rtk.ts").exists(), "both paths must resolve");
}

#[cfg(unix)]
#[test]
fn unaliased_paths_do_not_report_on_ownership_state() {
    let project = TempDir::new().unwrap();
    let extension_dir = project.path().join(".pi/extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    // A stray record at a path only Pi resolves to: no ownership decision is
    // made here, so its contents must not be announced.
    std::fs::write(extension_dir.join(".rtk-agents"), "garbage-entry\n").unwrap();

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
fn repeated_auto_patch_reuses_an_identical_backup() {
    let project = TempDir::new().unwrap();
    let extension_dir = project.path().join(".pi/extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    let extension = extension_dir.join("rtk.ts");
    let patched = format!("{}\n// local patch\n", include_str!("../hooks/pi/rtk.ts"));

    for _ in 0..3 {
        std::fs::write(&extension, &patched).unwrap();
        let install =
            run_rtk_without_agent_dir(project.path(), &["init", "--agent", "pi", "--auto-patch"]);
        assert!(install.status.success(), "{}", stderr(&install));
    }

    assert_eq!(
        std::fs::read_to_string(extension_dir.join("rtk.ts.bak")).unwrap(),
        patched
    );
    assert!(
        !extension_dir.join("rtk.ts.bak.1").exists(),
        "re-applying the same patch must not consume another backup slot"
    );
}

#[cfg(unix)]
#[test]
fn the_last_backup_slot_is_usable() {
    let project = TempDir::new().unwrap();
    let extension_dir = project.path().join(".pi/extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    let extension = extension_dir.join("rtk.ts");
    std::fs::write(
        &extension,
        format!("{}\n// local patch\n", include_str!("../hooks/pi/rtk.ts")),
    )
    .unwrap();

    // Occupy every slot but the last, each with distinct content so none of
    // them can be reused by the identical-content shortcut.
    std::fs::write(extension_dir.join("rtk.ts.bak"), "0").unwrap();
    for slot in 1..=8 {
        std::fs::write(
            extension_dir.join(format!("rtk.ts.bak.{slot}")),
            format!("{slot}"),
        )
        .unwrap();
    }

    let install =
        run_rtk_without_agent_dir(project.path(), &["init", "--agent", "pi", "--auto-patch"]);
    assert!(
        install.status.success(),
        "the final backup slot must be usable: {}",
        stderr(&install)
    );
    assert!(extension_dir.join("rtk.ts.bak.9").exists());
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

#[test]
fn unrecognised_sidecar_entry_still_protects_recorded_agents() {
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
        assert!(install.status.success(), "{agent} install failed");
    }
    // As a newer RTK that knows a third agent would have written it.
    std::fs::write(&ownership, "omp\npi\nzed\n").unwrap();

    let uninstall = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global", "--uninstall"],
    );
    assert!(
        !uninstall.status.success(),
        "an unparsable entry must not discard the recorded owners"
    );
    assert!(extension.exists());

    let install = run_rtk(
        project.path(),
        &agent_dir,
        &["init", "--agent", "pi", "--global"],
    );
    assert!(install.status.success(), "Pi reinstall failed");
    assert!(
        std::fs::read_to_string(&ownership).unwrap().contains("zed"),
        "entries from a newer RTK must survive a rewrite"
    );
}

#[cfg(unix)]
#[test]
fn cyclic_symlink_at_managed_path_does_not_abort() {
    let project = TempDir::new().unwrap();
    let extension_dir = project.path().join(".pi/extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    // Self-referential link: `canonicalize` reports ELOOP, so following the
    // chain by hand is the only thing standing between this and an abort.
    symlink("rtk.ts", extension_dir.join("rtk.ts")).unwrap();

    let install = run_rtk_without_agent_dir(project.path(), &["init", "--agent", "pi"]);
    assert!(
        install.status.code().is_some(),
        "install was killed by a signal instead of terminating: {}",
        stderr(&install)
    );
    // An unresolvable link is still something the user put there, so it gets
    // the same confirmation as any other non-stock content rather than being
    // replaced silently.
    assert!(
        !install.status.success(),
        "a cyclic link must not be replaced without confirmation: {}",
        stdout(&install)
    );
    assert!(
        std::fs::symlink_metadata(extension_dir.join("rtk.ts"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link must survive a declined overwrite"
    );

    let dry_run =
        run_rtk_without_agent_dir(project.path(), &["init", "--agent", "pi", "--dry-run"]);
    assert!(
        dry_run.status.code().is_some(),
        "dry-run was killed by a signal instead of terminating: {}",
        stderr(&dry_run)
    );

    let approved =
        run_rtk_without_agent_dir(project.path(), &["init", "--agent", "pi", "--auto-patch"]);
    assert!(
        approved.status.success(),
        "--auto-patch should replace a cyclic link: {}",
        stderr(&approved)
    );
    assert_eq!(
        std::fs::read_to_string(extension_dir.join("rtk.ts")).unwrap(),
        include_str!("../hooks/pi/rtk.ts")
    );
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

    // The real run removes the file itself, so the record goes with it. The
    // preview must have said so rather than describing a rewrite.
    assert!(!ownership.exists());
    assert!(
        stdout(&preview).contains("would remove RTK extension ownership state"),
        "dry-run described a different outcome than the real run: {}",
        stdout(&preview)
    );
}

#[cfg(unix)]
#[test]
fn modified_extension_uninstall_is_recoverable_with_auto_patch() {
    let project = TempDir::new().unwrap();
    let extension_dir = project.path().join(".pi/extensions");
    std::fs::create_dir_all(&extension_dir).unwrap();
    let extension = extension_dir.join("rtk.ts");
    // RTK content, but hand-edited so it matches no known stock revision.
    std::fs::write(
        &extension,
        format!("{}\n// local tweak\n", include_str!("../hooks/pi/rtk.ts")),
    )
    .unwrap();

    let refused =
        run_rtk_without_agent_dir(project.path(), &["init", "--agent", "pi", "--uninstall"]);
    assert!(
        !refused.status.success(),
        "modified RTK content must not be removed without approval"
    );
    assert!(stderr(&refused).contains("--auto-patch"));
    assert!(extension.exists());

    let approved = run_rtk_without_agent_dir(
        project.path(),
        &["init", "--agent", "pi", "--uninstall", "--auto-patch"],
    );
    assert!(
        approved.status.success(),
        "--auto-patch should approve removing modified RTK content: {}",
        stderr(&approved)
    );
    assert!(
        !extension.exists(),
        "approved uninstall must remove the modified extension"
    );
    assert_eq!(
        std::fs::read_to_string(extension_dir.join("rtk.ts.bak")).unwrap(),
        format!("{}\n// local tweak\n", include_str!("../hooks/pi/rtk.ts")),
        "the user's edits must survive an approved removal"
    );

    let second_edit = format!(
        "{}\n// a different tweak\n",
        include_str!("../hooks/pi/rtk.ts")
    );
    std::fs::write(&extension, &second_edit).unwrap();
    let again = run_rtk_without_agent_dir(
        project.path(),
        &["init", "--agent", "pi", "--uninstall", "--auto-patch"],
    );
    assert!(
        again.status.success(),
        "second uninstall failed: {}",
        stderr(&again)
    );
    assert_eq!(
        std::fs::read_to_string(extension_dir.join("rtk.ts.bak")).unwrap(),
        format!("{}\n// local tweak\n", include_str!("../hooks/pi/rtk.ts")),
        "the first backup must be preserved"
    );
    assert_eq!(
        std::fs::read_to_string(extension_dir.join("rtk.ts.bak.1")).unwrap(),
        second_edit
    );
}
