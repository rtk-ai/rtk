mod common;

use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    let mut cmd = Command::new("git");
    common::isolate_git(&mut cmd);
    let output = cmd.args(args).current_dir(repo).output().expect("run git");
    assert!(output.status.success(), "{:?}", output);
    output.stdout
}

fn changed_lines(output: &[u8]) -> Vec<&[u8]> {
    output
        .split(|byte| *byte == b'\n')
        .filter(|line| {
            (line.starts_with(b"+") || line.starts_with(b"-"))
                && !line.starts_with(b"+++ ")
                && !line.starts_with(b"--- ")
        })
        .collect()
}

#[test]
fn git_diff_and_show_preserve_line_ending_changes() {
    for (before, after) in [
        (b"a\nb\nc\n".as_slice(), b"a\r\nb\r\nc\r\n".as_slice()),
        (b"a\r\nb\r\nc\r\n".as_slice(), b"a\nb\nc\n".as_slice()),
    ] {
        let repo = common::temp_git_repo();
        git(repo.path(), &["config", "core.autocrlf", "false"]);
        let file = repo.path().join("x.txt");
        std::fs::write(&file, before).expect("write baseline");
        git(repo.path(), &["add", "x.txt"]);
        git(repo.path(), &["commit", "-qm", "baseline"]);
        std::fs::write(&file, after).expect("change line endings");

        let raw = git(repo.path(), &["diff"]);
        assert_eq!(raw.iter().filter(|byte| **byte == b'\r').count(), 3);
        let output = common::rtk_command()
            .args(["git", "diff"])
            .current_dir(repo.path())
            .output()
            .expect("run rtk git diff");
        assert!(output.status.success(), "{:?}", output);
        assert_eq!(changed_lines(&output.stdout), changed_lines(&raw));
        assert!(output.stdout.windows(6).any(|part| part == b" +3 -3"));

        git(repo.path(), &["add", "x.txt"]);
        git(repo.path(), &["commit", "-qm", "change line endings"]);
        let raw = git(repo.path(), &["show", "--pretty=format:"]);
        let output = common::rtk_command()
            .args(["git", "show"])
            .current_dir(repo.path())
            .output()
            .expect("run rtk git show");
        assert!(output.status.success(), "{:?}", output);
        assert_eq!(changed_lines(&output.stdout), changed_lines(&raw));
    }
}
