//! `rtk hook claude` with a pipeline `swap` rule: the gate that decides whether
//! to swap reads the same rules, from the same directory, as the hook's own
//! decision, and the two share one read of Claude Code's settings.

mod common;

#[cfg(all(unix, feature = "engine"))]
mod unix {
    use crate::common::hook::{
        PIPELINE_PATHS, SWAP_GIT_STATUS, claude_payload, hook_stdin, isolated_home,
    };
    use std::process::Output;

    fn hook(command: &str, cwd: Option<&str>, home: &tempfile::TempDir) -> Output {
        hook_stdin("claude", &claude_payload(command, cwd), home)
    }

    fn swapped(output: &Output) -> bool {
        String::from_utf8_lossy(&output.stdout).contains("rtk check-build")
    }

    /// Inside a worktree Claude Code manages, the hook leaves `git` as typed
    /// (#3864). A swap would hand the whole command to `sh` inside `rtk`, which
    /// Claude Code refuses there whenever the text contains `git`, so the gate
    /// declines any such command, and swaps one without it.
    #[test]
    fn the_swap_gate_leaves_git_as_typed_in_a_managed_worktree() {
        let home = isolated_home(&[
            (PIPELINE_PATHS[0], SWAP_GIT_STATUS),
            (PIPELINE_PATHS[1], SWAP_GIT_STATUS),
        ]);
        let repo = home.path().join("repo");
        let repo = repo.to_str().expect("utf-8 home");

        let control = hook("git status", Some(repo), &home);
        assert!(
            swapped(&control),
            "control: the swap rule must fire outside a worktree: {}",
            String::from_utf8_lossy(&control.stdout)
        );

        let worktree = format!("{repo}/.claude/worktrees/feat-x");
        let output = hook("git status", Some(&worktree), &home);
        assert!(
            !swapped(&output),
            "git in a managed worktree must not be swapped: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        // A command whose text contains git anywhere is not swapped there,
        // and the hook rewrites the rest.
        for chain in [
            "git status && cargo build",
            "cargo build && git status",
            "FOO=1 git status && cargo build",
            "bash -c 'git status' && cargo build",
            "cargo build && git status | xargs -n1 git add",
            "cargo build && find . -name x -exec git status {} +",
            "cargo build && echo legit status",
        ] {
            let output = hook(chain, Some(&worktree), &home);
            assert!(
                !swapped(&output),
                "{chain}: git in a managed worktree must not be swapped: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        let output = hook("git status && cargo build", Some(&worktree), &home);
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("git status && rtk cargo build"),
            "the hook leaves git as typed and rewrites the rest: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        // The control: a swap rule matching a command without git still
        // swaps it in a worktree.
        let swap_cargo = SWAP_GIT_STATUS.replace("\"git status\"", "\"cargo build\"");
        let cargo_home = isolated_home(&[
            (PIPELINE_PATHS[0], swap_cargo.as_str()),
            (PIPELINE_PATHS[1], swap_cargo.as_str()),
        ]);
        let output = hook("cargo build", Some(&worktree), &cargo_home);
        assert!(
            swapped(&output),
            "a command without git is swapped: {}",
            String::from_utf8_lossy(&output.stdout)
        );

        // What Claude Code reads is the replacement, so a task whose own name
        // contains git is not swapped there either, though the command is.
        let swap_digest = swap_cargo.replace("check-build", "git-digest");
        let digest_home = isolated_home(&[
            (PIPELINE_PATHS[0], swap_digest.as_str()),
            (PIPELINE_PATHS[1], swap_digest.as_str()),
        ]);
        let digest = |cwd| {
            let output = hook("cargo build", Some(cwd), &digest_home);
            String::from_utf8_lossy(&output.stdout).contains("rtk git-digest")
        };
        assert!(digest(repo), "control: the task swaps outside a worktree");
        assert!(
            !digest(&worktree),
            "a replacement naming git is not swapped"
        );
    }

    /// When the gate declines a swap, the hook decides on its own, against the
    /// rules the gate already read: a settings file RTK cannot parse warns as
    /// often as it does with no pipeline at all.
    #[test]
    fn a_declined_swap_reads_the_settings_no_more_often() {
        const BROKEN: (&str, &str) = (".claude/settings.json", "{ not json");
        let warnings = |output: &Output| {
            String::from_utf8_lossy(&output.stderr)
                .matches("failed to parse permissions")
                .count()
        };
        // `$(...)` is a construct the hook leaves to the host, so the gate
        // declines and the hook's own decision runs after it.
        const DEFERRED: &str = "git status $(date)";

        let plain_home = isolated_home(&[BROKEN]);
        let plain = warnings(&hook(DEFERRED, None, &plain_home));
        assert!(plain > 0, "control: the broken file must warn at all");

        let pipeline_home = isolated_home(&[
            BROKEN,
            (PIPELINE_PATHS[0], SWAP_GIT_STATUS),
            (PIPELINE_PATHS[1], SWAP_GIT_STATUS),
        ]);
        assert!(
            swapped(&hook("git status", None, &pipeline_home)),
            "control: the swap rule must fire"
        );
        let output = hook(DEFERRED, None, &pipeline_home);
        assert!(!swapped(&output), "a deferred command is not swapped");
        assert_eq!(warnings(&output), plain);
    }
}
