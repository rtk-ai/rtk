use std::path::Path;
use std::process::{Command, Output};

mod common;

fn git(cwd: &Path) -> Command {
    let mut cmd = common::native_command("git");
    common::isolate_git(&mut cmd);
    cmd.current_dir(cwd);
    cmd
}

fn run(cmd: &mut Command) -> Output {
    let output = cmd.output().expect("run command");
    assert!(output.status.success(), "{cmd:?}: {output:?}");
    output
}

#[test]
fn compact_status_uses_native_relative_paths_and_respects_configuration() {
    let repo = common::temp_git_repo();
    let subdir = repo.path().join("packages/app");
    std::fs::create_dir_all(&subdir).expect("create subdirectory");
    for path in [
        "packages/app/modified.txt",
        "packages/app/old name.txt",
        "root.txt",
    ] {
        std::fs::write(repo.path().join(path), path).expect("write tracked file");
    }
    run(git(repo.path()).args(["add", "."]));
    run(git(repo.path()).args(["commit", "-qm", "fixture"]));
    std::fs::write(subdir.join("modified.txt"), "changed").expect("modify file");
    std::fs::write(repo.path().join("root.txt"), "changed").expect("modify root file");
    std::fs::write(subdir.join("untracked file.txt"), "new").expect("write untracked file");
    run(git(&subdir).args(["mv", "old name.txt", "new name.txt"]));
    run(git(repo.path()).args(["config", "color.status", "always"]));

    for relative in ["true", "false"] {
        run(git(repo.path()).args(["config", "status.relativePaths", relative]));
        for (cwd, globals) in [
            (subdir.as_path(), vec![]),
            (repo.path(), vec![]),
            (repo.path(), vec!["-C", "packages/app"]),
        ] {
            let native = run(git(cwd).args(&globals).args([
                "-c",
                "color.status=false",
                "status",
                "--short",
                "--branch",
            ]));
            let native = String::from_utf8(native.stdout).expect("native UTF-8");
            let expected: Vec<_> = native.lines().skip(1).collect();
            assert_eq!(expected.len(), 4, "{native}");
            for status_args in [&[][..], &["-sb"], &["--short", "--branch"]] {
                let wrapped = run(common::rtk_command()
                    .current_dir(cwd)
                    .arg("git")
                    .args(&globals)
                    .arg("status")
                    .args(status_args));
                let actual = String::from_utf8(wrapped.stdout).expect("rtk UTF-8");
                assert_eq!(
                    actual.lines().skip(1).collect::<Vec<_>>(),
                    expected,
                    "relative={relative}, cwd={cwd:?}, globals={globals:?}, args={status_args:?}"
                );
            }
        }
    }

    // An explicit porcelain request keeps Git's root-relative machine format.
    let native = run(git(&subdir).args(["status", "--porcelain"]));
    let wrapped =
        run(common::rtk_command()
            .current_dir(&subdir)
            .args(["git", "status", "--porcelain"]));
    assert_eq!(
        String::from_utf8_lossy(&wrapped.stdout).trim_end(),
        String::from_utf8_lossy(&native.stdout).trim_end()
    );
}
