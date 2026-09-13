//! End-to-end coverage of the hook decision entry points.
//!
//! `rtk rewrite`'s exit-code protocol is a public contract consumed entirely
//! outside this crate -- `hooks/hermes/rtk-rewrite/__init__.py`,
//! `hooks/opencode/rtk.ts`, `hooks/pi/rtk.ts` and `openclaw/index.ts` all
//! branch on it -- and `rewrite_cmd`'s in-module `exit_code_protocol` asserts
//! against a hand-copied `expected_exit_code()` table without ever calling
//! `run()`. These tests spawn the real binary in a sandboxed
//! HOME/CLAUDE_CONFIG_DIR and pin the actual `(exit code, stdout)` pairs,
//! including the #1155 invariant that a `Default` verdict exits 3 and never 0.

use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// An isolated machine: no developer settings, no user rtk config, no real HOME.
struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    claude_home: PathBuf,
    project: PathBuf,
}

impl Sandbox {
    /// Build a sandbox whose project-level `.claude/settings.json` carries
    /// exactly the given rules, and nothing else anywhere.
    fn with_rules(deny: &[&str], ask: &[&str], allow: &[&str]) -> Self {
        let root = TempDir::new().expect("tempdir");
        let home = root.path().join("home");
        let claude_home = root.path().join("claude-home");
        let project = root.path().join("project");
        std::fs::create_dir_all(home.join(".config")).expect("mkdir home config");
        std::fs::create_dir_all(&claude_home).expect("mkdir claude home");
        std::fs::create_dir_all(project.join(".claude")).expect("mkdir project claude");
        // A tee artefact plus an existing recall store: the recall path only
        // writes to a store that already exists.
        let tee = root.path().join("tee");
        std::fs::create_dir_all(&tee).expect("mkdir tee");
        std::fs::write(tee.join("1755590000_cargo-test.log"), "boom\n").expect("write tee log");
        std::fs::write(root.path().join("recall.db"), b"").expect("seed recall store");

        let quote = |rules: &[&str]| {
            rules
                .iter()
                .map(|r| format!("\"Bash({r})\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let settings = format!(
            r#"{{"permissions": {{"deny": [{}], "ask": [{}], "allow": [{}]}}}}"#,
            quote(deny),
            quote(ask),
            quote(allow)
        );
        std::fs::write(project.join(".claude/settings.json"), settings).expect("write settings");

        Self {
            _root: root,
            home,
            claude_home,
            project,
        }
    }

    /// A sandbox with no permission rules at all — every command lands on the
    /// `Default` verdict.
    fn bare() -> Self {
        Self::with_rules(&[], &[], &[])
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(args)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("CLAUDE_CONFIG_DIR", &self.claude_home)
            .env("RTK_DB_PATH", self.project.join("rtk.db"))
            .env("RTK_TEE_DIR", self.tee_dir())
            .env("RTK_RECALL_DB", self.recall_db())
            .env("LC_ALL", "C")
            .output()
            .expect("spawn rtk");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        // A crash produces empty stdout too, which would let every "expect no
        // output" assertion below pass vacuously.
        assert!(
            !stderr.contains("panicked"),
            "rtk panicked on {args:?}: {stderr}"
        );
        (
            out.status.code().expect("exit code"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr,
        )
    }

    fn tee_dir(&self) -> PathBuf {
        self._root.path().join("tee")
    }

    fn recall_db(&self) -> PathBuf {
        self._root.path().join("recall.db")
    }

    fn tee_log(&self) -> String {
        self.tee_dir()
            .join("1755590000_cargo-test.log")
            .to_string_lossy()
            .into_owned()
    }

    /// Whether anything was written to the recall store. It is seeded empty, and
    /// the schema is only created when a recall is actually recorded.
    fn recorded_a_recall(&self) -> bool {
        std::fs::metadata(self.recall_db())
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    }

    fn rewrite(&self, cmd: &str) -> (i32, String) {
        let (code, stdout, _) = self.run(&["rewrite", cmd]);
        (code, stdout)
    }
}

impl Sandbox {
    /// Feed a Claude PreToolUse payload to the in-process hook and return stdout.
    fn hook_claude(&self, cmd: &str) -> String {
        use std::io::Write;
        use std::process::Stdio;
        let payload = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": cmd },
        })
        .to_string();
        let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["hook", "claude"])
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("CLAUDE_CONFIG_DIR", &self.claude_home)
            .env("RTK_DB_PATH", self.project.join("rtk.db"))
            .env("RTK_TEE_DIR", self.tee_dir())
            .env("RTK_RECALL_DB", self.recall_db())
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rtk hook claude");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(payload.as_bytes())
            .expect("write payload");
        let out = child.wait_with_output().expect("wait rtk");
        // The hook protocol requires exit 0 whatever it decides; without this a
        // crash is indistinguishable from a deliberate defer, and every
        // `assert_eq!(..., None)` below would pass vacuously.
        assert_eq!(
            out.status.code(),
            Some(0),
            "rtk hook claude exited non-zero for {cmd:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// The command the in-process hook would substitute, or `None` when it defers.
    fn hook_claude_rewrite(&self, cmd: &str) -> Option<String> {
        let stdout = self.hook_claude(cmd);
        if stdout.trim().is_empty() {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(&stdout).expect("hook emitted valid JSON");
        v.pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .map(str::to_owned)
    }
}

/// `rtk rewrite`'s four documented exit codes, against real permission rules.
///
/// The table in `rewrite_cmd`'s doc comment is the contract every delegate
/// branches on; this is the only place it is checked end to end.
mod rewrite_exit_codes {
    use super::Sandbox;

    #[test]
    fn allow_rule_exits_zero_with_the_rewrite() {
        let sb = Sandbox::with_rules(&[], &[], &["git status"]);
        assert_eq!(sb.rewrite("git status"), (0, "rtk git status".into()));
    }

    #[test]
    fn ask_rule_exits_three_with_the_rewrite() {
        let sb = Sandbox::with_rules(&[], &["git status"], &[]);
        assert_eq!(sb.rewrite("git status"), (3, "rtk git status".into()));
    }

    #[test]
    fn deny_rule_exits_two_and_says_nothing() {
        let sb = Sandbox::with_rules(&["git status"], &[], &[]);
        assert_eq!(sb.rewrite("git status"), (2, String::new()));
    }

    /// A deny rule matching *any* segment denies the whole chain (#1213).
    #[test]
    fn deny_rule_on_one_segment_denies_the_compound() {
        let sb = Sandbox::with_rules(&["rm -rf *"], &[], &[]);
        assert_eq!(
            sb.rewrite("git status && rm -rf /tmp/x"),
            (2, String::new())
        );
    }

    #[test]
    fn unknown_command_exits_one_and_says_nothing() {
        let sb = Sandbox::bare();
        assert_eq!(sb.rewrite("htop"), (1, String::new()));
    }

    /// SECURITY (#1155): with no rule matching, the verdict is `Default`, and
    /// `Default` must exit 3 (ask) — never 0. Exit 0 tells the hook it may
    /// auto-allow, so mapping `Default` there would auto-approve every
    /// rewritable command on a machine with no permission rules at all.
    #[test]
    fn default_verdict_exits_three_never_zero() {
        let sb = Sandbox::bare();
        let (code, stdout) = sb.rewrite("git status");
        assert_eq!(code, 3, "Default verdict must exit 3 (ask), not 0 (allow)");
        assert_eq!(stdout, "rtk git status");
    }

    #[test]
    fn compound_command_rewrites_every_segment() {
        let sb = Sandbox::bare();
        assert_eq!(
            sb.rewrite("git status && cargo test"),
            (3, "rtk git status && rtk cargo test".into())
        );
    }

    /// A file-descriptor dup is not a file target, so the rewrite still happens.
    #[test]
    fn fd_dup_redirect_still_rewrites() {
        let sb = Sandbox::bare();
        assert_eq!(
            sb.rewrite("git status 2>&1"),
            (3, "rtk git status 2>&1".into())
        );
    }

    /// Constructs the permission gate cannot decompose are never rewritten,
    /// so a hidden command can't ride along inside an approved rewrite.
    #[test]
    fn unattestable_constructs_pass_through() {
        let sb = Sandbox::bare();
        for cmd in [
            "git status $(rm -rf /tmp/x)",
            "git status `rm -rf /tmp/x`",
            "git log > /tmp/out.txt",
        ] {
            assert_eq!(sb.rewrite(cmd), (1, String::new()), "cmd: {cmd}");
        }
    }

    #[test]
    fn heredoc_passes_through() {
        let sb = Sandbox::bare();
        assert_eq!(sb.rewrite("cat <<EOF"), (1, String::new()));
        assert_eq!(sb.rewrite("git status <<EOF"), (1, String::new()));
    }
}

/// Reading back a tee artefact records a recall, and a denied command does not.
///
/// Both entry points perform that bookkeeping themselves rather than through the
/// shared decision, because the `rtk hook check` diagnostic must not write
/// counters that `rtk gain` reports. Two hand-written copies of one rule is
/// exactly what drifts, and nothing else asserts it.
mod recall_tracking {
    use super::Sandbox;

    #[test]
    fn rewrite_records_a_tee_read_unless_denied() {
        let sb = Sandbox::bare();
        assert!(!sb.recorded_a_recall(), "store starts empty");
        sb.rewrite(&format!("tail -n +52 {}", sb.tee_log()));
        assert!(
            sb.recorded_a_recall(),
            "reading a tee artefact records a recall"
        );

        let denied = Sandbox::with_rules(&["tail *"], &[], &[]);
        let (code, _) = denied.rewrite(&format!("tail -n +52 {}", denied.tee_log()));
        assert_eq!(code, 2, "the deny rule must match, or this proves nothing");
        assert!(
            !denied.recorded_a_recall(),
            "a denied command must not record a recall"
        );
    }

    #[test]
    fn hook_records_a_tee_read_unless_denied() {
        let sb = Sandbox::bare();
        sb.hook_claude(&format!("tail -n +52 {}", sb.tee_log()));
        assert!(sb.recorded_a_recall(), "the hook path records a recall too");

        let denied = Sandbox::with_rules(&["tail *"], &[], &[]);
        denied.hook_claude(&format!("tail -n +52 {}", denied.tee_log()));
        assert!(
            !denied.recorded_a_recall(),
            "a denied command must not record a recall"
        );
    }

    /// A command that reads nothing from the tee directory records nothing.
    #[test]
    fn unrelated_command_records_nothing() {
        let sb = Sandbox::bare();
        sb.rewrite("git status");
        assert!(!sb.recorded_a_recall());
    }
}

/// The two decision paths, side by side on one corpus.
///
/// `rtk rewrite` (subprocess path) and `rtk hook claude` (in-process path)
/// answer the same question for the same command. Asserting both against one
/// corpus keeps a change to either from silently moving them apart.
///
/// Follows the shape of `registry.rs`'s `segmenter_consistency` module.
mod decision_consistency {
    use super::Sandbox;

    /// Everything the two paths already agree on: the rewrite is identical
    /// where one happens, and both stay silent where it doesn't.
    #[test]
    fn both_paths_agree_on_the_corpus() {
        let sb = Sandbox::bare();
        let cases: [(&str, Option<&str>); 9] = [
            ("git status", Some("rtk git status")),
            (
                "git status && cargo test",
                Some("rtk git status && rtk cargo test"),
            ),
            ("git status 2>&1", Some("rtk git status 2>&1")),
            ("git log | head", Some("rtk git log | head")),
            ("htop", None),
            ("git status $(rm -rf /tmp/x)", None),
            ("git status `rm -rf /tmp/x`", None),
            ("git log > /tmp/out.txt", None),
            ("cat <<EOF", None),
        ];

        for (cmd, expected) in cases {
            let (code, stdout) = sb.rewrite(cmd);
            let via_rewrite = match code {
                0 | 3 => Some(stdout),
                _ => None,
            };
            let via_hook = sb.hook_claude_rewrite(cmd);
            assert_eq!(
                via_rewrite.as_deref(),
                expected,
                "rtk rewrite disagreed with the pinned corpus for: {cmd}"
            );
            assert_eq!(
                via_hook.as_deref(),
                expected,
                "rtk hook claude disagreed with the pinned corpus for: {cmd}"
            );
        }
    }

    /// The one place the two paths differ.
    ///
    /// A command that is already RTK-prefixed rewrites to itself. Every hook
    /// discards that; `rtk rewrite` reports it, exiting 3 with the command
    /// unchanged on stdout. The plugins that shell out to it gate on
    /// `rewritten != command` for exactly this reason.
    #[test]
    fn identity_rewrite_is_where_the_paths_diverge() {
        let sb = Sandbox::bare();

        // Subprocess path: reported as an ask-rewrite, output identical to input.
        assert_eq!(
            sb.rewrite("rtk git status"),
            (3, "rtk git status".into()),
            "rtk rewrite reports the no-op rewrite"
        );

        // In-process path: nothing to say.
        assert_eq!(
            sb.hook_claude_rewrite("rtk git status"),
            None,
            "rtk hook claude defers on the no-op rewrite"
        );
    }
}

/// `rtk hook check` answers the same question the hooks answer.
///
/// A diagnostic that reported a rewrite the hooks refuse to apply would be
/// worse than none, so it routes through the shared decision and is pinned
/// here against the hooks themselves.
mod hook_check {
    use super::Sandbox;

    #[test]
    fn reports_the_rewrite_for_a_plain_command() {
        let sb = Sandbox::bare();
        let (code, stdout, _) = sb.run(&["hook", "check", "git status"]);
        assert_eq!((code, stdout.trim()), (0, "rtk git status"));
    }

    #[test]
    fn exits_one_for_an_unknown_command() {
        let sb = Sandbox::bare();
        let (code, stdout, _) = sb.run(&["hook", "check", "htop"]);
        assert_eq!((code, stdout.trim()), (1, ""));
    }

    /// Both hook paths refuse these, so the diagnostic must refuse them too.
    #[test]
    fn agrees_with_the_hooks_on_what_is_never_rewritten() {
        let sb = Sandbox::bare();

        for cmd in [
            "git status $(rm -rf /tmp/x)",
            "git log > /tmp/out.txt",
            "cat <<EOF",
        ] {
            let (code, stdout, _) = sb.run(&["hook", "check", cmd]);
            assert_eq!((code, stdout.trim()), (1, ""), "hook check on: {cmd}");
            assert_eq!(sb.hook_claude_rewrite(cmd), None, "hook claude on: {cmd}");
            assert_eq!(sb.rewrite(cmd).0, 1, "rtk rewrite on: {cmd}");
        }
    }

    /// An already-RTK-prefixed command rewrites to itself, and no agent applies
    /// that: the in-process hosts discard it in `hook_cmd`, and the ones whose
    /// plugin shells out to `rtk rewrite` discard it themselves. Only the bare
    /// `rtk rewrite` CLI reports it -- see `decision_consistency`.
    #[test]
    fn no_agent_applies_a_rewrite_that_changed_nothing() {
        let sb = Sandbox::bare();

        for agent in ["claude", "pi", "kimi"] {
            let (code, stdout, _) = sb.run(&["hook", "check", "--agent", agent, "rtk git status"]);
            assert_eq!((code, stdout.trim()), (1, ""), "agent: {agent}");
        }
        assert_eq!(sb.hook_claude_rewrite("rtk git status"), None);

        // The CLI itself still reports it, which is what the delegates guard against.
        assert_eq!(sb.rewrite("rtk git status"), (3, "rtk git status".into()));
    }

    /// Every install target answers; only a genuine typo is rejected.
    #[test]
    fn answers_for_every_supported_agent() {
        let sb = Sandbox::bare();
        for agent in [
            "claude",
            "copilot",
            "cursor",
            "gemini",
            "droid",
            "vibe",
            "opencode",
            "openclaw",
            "pi",
            "omp",
            "hermes",
            "codex",
            "windsurf",
            "cline",
            "kilocode",
            "antigravity",
            "kimi",
        ] {
            let (code, stdout, _) = sb.run(&["hook", "check", "--agent", agent, "git status"]);
            assert_eq!(
                (code, stdout.trim()),
                (0, "rtk git status"),
                "agent: {agent}"
            );
        }
    }

    /// `--agent` selects whose rules are consulted. Hosts read different
    /// settings files, so answering with Claude's verdict for another agent
    /// would misdescribe the very hook being diagnosed: here a Claude deny rule
    /// must not deny for Gemini, which has no rules of its own in this sandbox.
    #[test]
    fn agent_flag_selects_the_hosts_own_rules() {
        let sb = Sandbox::with_rules(&["git status"], &[], &[]);

        let (code, stdout, _) = sb.run(&["hook", "check", "--agent", "claude", "git status"]);
        assert_eq!((code, stdout.trim()), (1, ""), "claude denies");

        let (code, stdout, _) = sb.run(&["hook", "check", "--agent", "gemini", "git status"]);
        assert_eq!(
            (code, stdout.trim()),
            (0, "rtk git status"),
            "gemini has no deny rule here, so the rewrite stands"
        );
    }

    #[test]
    fn unknown_agent_is_rejected_rather_than_answered_for_claude() {
        let sb = Sandbox::bare();
        let (code, stdout, stderr) = sb.run(&["hook", "check", "--agent", "nope", "git status"]);
        assert_eq!((code, stdout.trim()), (2, ""));
        assert!(stderr.contains("Unknown agent: nope"), "stderr: {stderr}");
    }

    /// A denied command is not rewritten, and says so distinctly.
    #[test]
    fn reports_a_deny_rule_separately_from_no_rewrite() {
        let sb = Sandbox::with_rules(&["git status"], &[], &[]);
        let (code, stdout, stderr) = sb.run(&["hook", "check", "git status"]);
        assert_eq!((code, stdout.trim()), (1, ""));
        assert!(
            stderr.contains("Denied by a permission rule"),
            "stderr: {stderr}"
        );
    }
}
