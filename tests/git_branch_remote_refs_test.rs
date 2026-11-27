use std::path::Path;
use std::process::{Command, Output};

mod common;

fn git(repo: &Path) -> Command {
    let mut cmd = common::native_command("git");
    common::isolate_git(&mut cmd);
    cmd.current_dir(repo);
    cmd
}

fn run(cmd: &mut Command) -> Output {
    let output = cmd.output().expect("run command");
    assert!(output.status.success(), "{cmd:?}: {output:?}");
    output
}

#[test]
fn explicit_remote_listings_preserve_distinct_refs_and_symbolic_heads() {
    let repo = common::temp_git_repo();
    run(git(repo.path()).args(["branch", "-M", "main"]));
    run(git(repo.path()).args(["branch", "topic"]));
    for remote in ["origin", "upstream"] {
        for name in ["main", "topic", "remote-only"] {
            run(git(repo.path()).args([
                "update-ref",
                &format!("refs/remotes/{remote}/{name}"),
                "HEAD",
            ]));
        }
        run(git(repo.path()).args([
            "symbolic-ref",
            &format!("refs/remotes/{remote}/HEAD"),
            &format!("refs/remotes/{remote}/main"),
        ]));
    }

    for args in [
        &["-a"][..],
        &["--all"],
        &["-r"],
        &["--remotes"],
        &["-ar"],
        &["--all", "--list", "*main"],
        &["-r", "--list", "*/remote-only"],
        &["-a", "--list", "*HEAD*"],
        &["-a", "--sort=-refname"],
        &["--remotes", "--list", "missing*"],
    ] {
        let native = run(git(repo.path()).args(["branch", "--no-color"]).args(args));
        let rtk = run(common::rtk_command()
            .current_dir(repo.path())
            .args(["git", "branch"])
            .args(args));
        assert_eq!(
            String::from_utf8_lossy(&rtk.stdout),
            String::from_utf8_lossy(&native.stdout),
            "{args:?}"
        );
    }
}
