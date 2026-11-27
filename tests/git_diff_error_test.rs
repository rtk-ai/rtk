use std::path::Path;
use std::process::Output;

mod common;

fn native(repo: &Path, args: &[&str]) -> Output {
    let mut cmd = common::native_command("git");
    common::isolate_git(&mut cmd);
    cmd.env("LC_ALL", "C")
        .current_dir(repo)
        .arg("diff")
        .args(args)
        .output()
        .expect("git diff")
}

fn rtk(repo: &Path, args: &[&str]) -> Output {
    common::rtk_command()
        .env("RTK_NO_TRACK", "1")
        .env("LC_ALL", "C")
        .current_dir(repo)
        .args(["git", "diff"])
        .args(args)
        .output()
        .expect("rtk git diff")
}

#[test]
fn a_rejected_diff_prints_gits_error() {
    let repo = common::temp_git_repo();
    for args in [
        &["--bogus"][..],
        &["-z", "--bogus"],
        &["--stat", "--bogus"],
        &["--numstat", "--bogus"],
        &["--stat", "no-such-ref"],
    ] {
        let native = native(repo.path(), args);
        let rtk = rtk(repo.path(), args);

        assert_ne!(native.status.code(), Some(0), "{args:?} fails in git");
        assert_eq!(rtk.status.code(), native.status.code(), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&rtk.stderr),
            String::from_utf8_lossy(&native.stderr),
            "{args:?}"
        );
    }
}
