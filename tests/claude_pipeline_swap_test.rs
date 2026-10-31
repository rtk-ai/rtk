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
    /// (#3864). The gate swaps exactly what the hook would rewrite, so a git
    /// command the hook leaves alone there is not swapped either, while a chain
    /// whose other half the hook still rewrites is.
    #[test]
    fn the_swap_gate_swaps_only_what_the_hook_rewrites_in_a_managed_worktree() {
        // A settings file of its own, so the hook stops looking for one at
        // this home rather than reading whatever sits above it.
        let home = isolated_home(&[
            (PIPELINE_PATHS[0], SWAP_GIT_STATUS),
            (PIPELINE_PATHS[1], SWAP_GIT_STATUS),
            (".claude/settings.json", "{}"),
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
        let chain = hook("git status && cargo build", Some(&worktree), &home);
        assert!(
            swapped(&chain),
            "a chain the hook still rewrites is swapped: {}",
            String::from_utf8_lossy(&chain.stdout)
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
