//! `rtk hook claude` with a pipeline `swap` rule: the gate that decides whether
//! to swap reads the same rules as the hook's own decision, and the two share
//! one read of Claude Code's settings.

mod common;

#[cfg(all(unix, feature = "engine"))]
mod unix {
    use crate::common::hook::{
        BROKEN_SETTINGS, CONFIG_PATHS, PIPELINE_PATHS, SWAP_GIT_STATUS, claude_payload, hook_stdin,
        isolated_home, parse_warnings,
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
        let home = isolated_home(&PIPELINE_PATHS.map(|path| (path, SWAP_GIT_STATUS)));
        let repo = home.path().join("repo");
        let repo = repo.to_str().expect("utf-8 home");

        let control = hook("git status", Some(repo), &home);
        assert!(
            swapped(&control),
            "control: the swap rule must fire outside a worktree: {}",
            String::from_utf8_lossy(&control.stdout)
        );

        let worktree = format!("{repo}/.claude/worktrees/feat-x");
        // A command whose text contains git anywhere is not swapped there,
        // and the hook rewrites the rest.
        for chain in [
            "git status",
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
        let cargo_home = isolated_home(&PIPELINE_PATHS.map(|path| (path, swap_cargo.as_str())));
        let output = hook("cargo build", Some(&worktree), &cargo_home);
        assert!(
            swapped(&output),
            "a command without git is swapped: {}",
            String::from_utf8_lossy(&output.stdout)
        );

        // What Claude Code reads is the replacement, so a task whose own name
        // contains git is not swapped there either, though the command is.
        let swap_digest = swap_cargo.replace("check-build", "git-digest");
        let digest_home = isolated_home(&PIPELINE_PATHS.map(|path| (path, swap_digest.as_str())));
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

    /// The gate judges a command under the hook's own rules and exclusions: an
    /// ask rule still asks about a command RTK filters, and keeps one it does
    /// not filter as typed, where the agent checks it; an allow rule carries
    /// over to the swap; and an excluded command stays as typed.
    #[test]
    fn the_swap_gate_judges_under_the_hooks_rules_and_exclusions() {
        const SETTINGS: &str = r#"{"permissions": {
            "ask": ["Bash(git status:*)", "Bash(./gated.sh:*)"],
            "allow": ["Bash(./open.sh:*)"]
        }}"#;
        const CONFIG: &str = "[hooks]\nexclude_commands = [\"./excluded.sh\"]\n";
        let swap = SWAP_GIT_STATUS.replace("\"git status\"", "\"git status|[.]/\"");
        let mut files = vec![(".claude/settings.json", SETTINGS)];
        files.extend(PIPELINE_PATHS.map(|path| (path, swap.as_str())));
        files.extend(CONFIG_PATHS.map(|path| (path, CONFIG)));
        let home = isolated_home(&files);
        let decision = |command: &str| {
            let output = hook(command, None, &home);
            let reply: serde_json::Value =
                serde_json::from_slice(&output.stdout).unwrap_or_default();
            let hook = &reply["hookSpecificOutput"];
            (
                swapped(&output),
                hook["permissionDecision"].as_str().map(str::to_string),
            )
        };
        let swapped_with = |answer: Option<&str>| (true, answer.map(str::to_string));

        assert_eq!(decision("./plain.sh"), swapped_with(None), "control");
        assert_eq!(decision("git status"), swapped_with(Some("ask")));
        assert_eq!(decision("./open.sh"), swapped_with(Some("allow")));
        assert!(!decision("./gated.sh").0, "an ask rule keeps it as typed");
        assert!(
            !decision("./excluded.sh").0,
            "an exclusion keeps it as typed"
        );
    }

    /// When the gate declines a swap, the hook decides on its own, against the
    /// rules the gate already read: a settings file RTK cannot parse warns as
    /// often as it does with no pipeline at all.
    #[test]
    fn a_declined_swap_reads_the_settings_no_more_often() {
        // `$(...)` is a construct the hook leaves to the host, so the gate
        // declines and the hook's own decision runs after it.
        const DEFERRED: &str = "git status $(date)";

        let plain_home = isolated_home(&[BROKEN_SETTINGS]);
        let plain = parse_warnings(&hook(DEFERRED, None, &plain_home));
        assert!(plain > 0, "control: the broken file must warn at all");

        let pipeline_home = isolated_home(&[
            BROKEN_SETTINGS,
            (PIPELINE_PATHS[0], SWAP_GIT_STATUS),
            (PIPELINE_PATHS[1], SWAP_GIT_STATUS),
        ]);
        assert!(
            swapped(&hook("git status", None, &pipeline_home)),
            "control: the swap rule must fire"
        );
        let output = hook(DEFERRED, None, &pipeline_home);
        assert!(!swapped(&output), "a deferred command is not swapped");
        assert_eq!(parse_warnings(&output), plain);

        // A swap that goes ahead reads them no more often than the hook does.
        let plain_filtered = parse_warnings(&hook("git status", None, &plain_home));
        assert_eq!(
            parse_warnings(&hook("git status", None, &pipeline_home)),
            plain_filtered
        );
    }
}
