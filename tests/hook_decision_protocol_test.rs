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
        // KuSh (#3953): The relevant scenario is effectively:
        // - create/use the sqlite recall store first,
        // - then switch/use tee mode,
        // - then reproduce the rejected-Ask case.
        // A fresh tee-only installation does not expose the phantom count because
        // the tee recording path uses open_existing.
        let tee = root.path().join("tee");
        std::fs::create_dir_all(&tee).expect("mkdir tee");
        std::fs::write(tee.join("1755590000_cargo-test.log"), "boom\n").expect("write tee log");
        let recall_db_path = root.path().join("recall.db");
        let conn = rusqlite::Connection::open(&recall_db_path).expect("seed sqlite recall store");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS recall (
                hash TEXT PRIMARY KEY,
                command TEXT NOT NULL,
                cwd TEXT,
                exit_code INTEGER,
                created_at INTEGER NOT NULL,
                total_lines INTEGER NOT NULL,
                shown_upto INTEGER NOT NULL,
                byte_size INTEGER NOT NULL,
                truncated INTEGER NOT NULL,
                codec TEXT NOT NULL,
                blob BLOB NOT NULL,
                recalled INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS tee_reads (
                path TEXT PRIMARY KEY
            );
            CREATE TABLE IF NOT EXISTS recall_stats (
                slug TEXT NOT NULL,
                mode TEXT NOT NULL,
                elisions INTEGER NOT NULL DEFAULT 0,
                recalls INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (slug, mode)
            );",
        )
        .expect("init recall schema");

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

    /// A fresh tee-only installation: explicit tee mode configured via application mechanism,
    /// and recall.db does not exist yet.
    fn bare_fresh_tee_only() -> Self {
        let sb = Self::bare();
        let (code, _, _) = sb.run(&["config", "recall", "tee"]);
        assert_eq!(code, 0, "rtk config recall tee must succeed");
        let _ = std::fs::remove_file(sb.recall_db());
        sb
    }

    fn has_any_recall_db_files(&self) -> bool {
        let root = self._root.path();
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("recall.db") {
                    return true;
                }
            }
        }
        false
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

    fn run_with_stdin_bytes(&self, args: &[&str], input: &[u8]) -> (i32, Vec<u8>, Vec<u8>) {
        use std::io::Write;
        use std::process::Stdio;

        let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(args)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("CLAUDE_CONFIG_DIR", &self.claude_home)
            .env("RTK_DB_PATH", self.project.join("rtk.db"))
            .env("RTK_TEE_DIR", self.tee_dir())
            .env("RTK_RECALL_DB", self.recall_db())
            .env("LC_ALL", "C")
            .env("RTK_SUPPRESS_HOOK_WARNING", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rtk");

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input).expect("write to stdin");
        }
        let out = child.wait_with_output().expect("wait rtk");
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains("panicked"),
            "rtk panicked on {args:?}"
        );
        (out.status.code().unwrap_or(1), out.stdout, out.stderr)
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

    /// Whether any tee read has been recorded into the recall store.
    fn recorded_a_recall(&self) -> bool {
        let Ok(conn) = rusqlite::Connection::open(self.recall_db()) else {
            return false;
        };
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tee_reads", [], |r| r.get(0))
            .unwrap_or(0);
        count > 0
    }

    fn recall_count(&self, slug: &str) -> i64 {
        let Ok(conn) = rusqlite::Connection::open(self.recall_db()) else {
            return 0;
        };
        conn.query_row(
            "SELECT recalls FROM recall_stats WHERE slug = ?1 AND mode = 'tee'",
            [slug],
            |r| r.get(0),
        )
        .unwrap_or(0)
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
    use std::process::Command;

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

    #[test]
    fn unapproved_ask_does_not_record_a_tee_read() {
        let sb = Sandbox::bare();
        assert!(!sb.recorded_a_recall(), "store starts empty");
        let (code, rewritten) = sb.rewrite(&format!("cat {}", sb.tee_log()));
        assert_eq!(code, 3, "cat without allow rule must be Ask (exit 3)");
        assert!(
            rewritten.starts_with("rtk read "),
            "must rewrite to rtk read"
        );
        assert!(
            !sb.recorded_a_recall(),
            "an unapproved Ask must NOT record a recall at the hook gate"
        );
        assert_eq!(sb.recall_count("cargo-test"), 0);
    }

    #[test]
    fn unapproved_ask_does_not_burn_dedup_slot_for_subsequent_defer() {
        let sb = Sandbox::bare();
        // 1. Unapproved Ask
        let (code, _) = sb.rewrite(&format!("cat {}", sb.tee_log()));
        assert_eq!(code, 3);
        assert!(
            !sb.recorded_a_recall(),
            "unapproved Ask must not record a recall"
        );

        // 2. Subsequent genuine Defer read (tail -n +52)
        let (code2, _) = sb.rewrite(&format!("tail -n +52 {}", sb.tee_log()));
        assert_eq!(code2, 1, "tail offset must be Defer (exit 1)");
        assert!(
            sb.recorded_a_recall(),
            "subsequent genuine Defer read must be recorded"
        );
        assert_eq!(sb.recall_count("cargo-test"), 1);
    }

    #[test]
    fn in_process_hook_ask_does_not_record_a_tee_read() {
        let sb = Sandbox::bare();
        let _ = sb.hook_claude(&format!("cat {}", sb.tee_log()));
        assert!(
            !sb.recorded_a_recall(),
            "in-process hook Ask must not record a recall before execution"
        );
        assert_eq!(sb.recall_count("cargo-test"), 0);
    }

    #[test]
    fn allow_rewrite_read_counts_exactly_once_upon_execution() {
        let sb = Sandbox::with_rules(&[], &[], &["cat *"]);
        let (code, rewritten) = sb.rewrite(&format!("cat {}", sb.tee_log()));
        assert_eq!(code, 0, "must be AllowRewrite (exit 0)");
        assert!(rewritten.starts_with("rtk read "));
        assert!(
            !sb.recorded_a_recall(),
            "AllowRewrite at hook gate must not record before execution"
        );
        assert_eq!(sb.recall_count("cargo-test"), 0);

        // Host executes rewritten command: rtk read <tee_log>
        let (read_code, _, _) = sb.run(&["read", &sb.tee_log()]);
        assert_eq!(read_code, 0);
        assert!(
            sb.recorded_a_recall(),
            "executing rtk read on tee log must record a recall"
        );
        assert_eq!(sb.recall_count("cargo-test"), 1);

        // Running a second time is deduped
        let (read_code2, _, _) = sb.run(&["read", &sb.tee_log()]);
        assert_eq!(read_code2, 0);
        assert_eq!(sb.recall_count("cargo-test"), 1);
    }

    #[test]
    fn approved_ask_records_exactly_once_when_executed() {
        let sb = Sandbox::bare();
        let (code, _rewritten) = sb.rewrite(&format!("cat {}", sb.tee_log()));
        assert_eq!(code, 3);
        assert!(!sb.recorded_a_recall());

        let (read_code, _, _) = sb.run(&["read", &sb.tee_log()]);
        assert_eq!(read_code, 0);
        assert!(sb.recorded_a_recall());
        assert_eq!(sb.recall_count("cargo-test"), 1);

        // Second execution deduped
        let (read_code2, _, _) = sb.run(&["read", &sb.tee_log()]);
        assert_eq!(read_code2, 0);
        assert_eq!(sb.recall_count("cargo-test"), 1);
    }

    #[test]
    fn executed_grep_on_tee_file_records_recall_and_dedups() {
        let sb = Sandbox::bare();
        assert!(!sb.recorded_a_recall());
        let (grep_code, _, _) = sb.run(&["grep", "boom", &sb.tee_log()]);
        assert_eq!(grep_code, 0);
        assert!(
            sb.recorded_a_recall(),
            "executing rtk grep on tee log must record a recall"
        );
        assert_eq!(sb.recall_count("cargo-test"), 1);

        // Second grep deduped
        let (grep_code2, _, _) = sb.run(&["grep", "boom", &sb.tee_log()]);
        assert_eq!(grep_code2, 0);
        assert_eq!(sb.recall_count("cargo-test"), 1);
    }

    #[test]
    fn failed_read_of_missing_tee_file_records_nothing() {
        let sb = Sandbox::bare();
        let missing = sb.tee_dir().join("1755590000_missing.log");
        let (code, _, _) = sb.run(&["read", &missing.to_string_lossy()]);
        assert_ne!(code, 0, "reading missing file must fail");
        assert!(!sb.recorded_a_recall(), "failed read must count nothing");
        assert_eq!(sb.recall_count("missing"), 0);
    }

    #[test]
    fn failed_grep_of_missing_tee_file_records_nothing() {
        let sb = Sandbox::bare();
        let missing = sb.tee_dir().join("1755590000_missing.log");
        let (code, _, _) = sb.run(&["grep", "foo", &missing.to_string_lossy()]);
        assert_ne!(code, 0, "grep on missing file must fail");
        assert!(!sb.recorded_a_recall(), "failed grep must count nothing");
        assert_eq!(sb.recall_count("missing"), 0);
    }

    #[test]
    fn auxiliary_flag_patterns_file_records_tee_recall() {
        let sb = Sandbox::bare();
        let target = sb.project.join("target.txt");
        std::fs::write(&target, "boom in project\n").unwrap();
        assert!(!sb.recorded_a_recall());

        let (code, _, _) = sb.run(&["grep", "-f", &sb.tee_log(), target.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert!(
            sb.recorded_a_recall(),
            "grep -f <tee_log> must record a recall"
        );
        assert_eq!(sb.recall_count("cargo-test"), 1);

        // Dedup holds on repeat
        let (code2, _, _) = sb.run(&["grep", "-f", &sb.tee_log(), target.to_str().unwrap()]);
        assert_eq!(code2, 0);
        assert_eq!(sb.recall_count("cargo-test"), 1);
    }

    #[test]
    fn auxiliary_flag_ignore_file_records_tee_recall_when_rg_available() {
        if !std::process::Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }
        let sb = Sandbox::bare();
        let target = sb.project.join("target.txt");
        std::fs::write(&target, "hello world\n").unwrap();
        assert!(!sb.recorded_a_recall());

        let (code, _, _) = sb.run(&[
            "rg",
            "--ignore-file",
            &sb.tee_log(),
            "hello",
            target.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert!(sb.recorded_a_recall());
        assert_eq!(sb.recall_count("cargo-test"), 1);
    }

    #[test]
    fn auxiliary_flag_attached_ignore_file_records_tee_recall_when_rg_available() {
        if !std::process::Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }
        let sb = Sandbox::bare();
        let target = sb.project.join("target.txt");
        std::fs::write(&target, "hello world\n").unwrap();

        // Use a fresh candidate to prove the attached form itself records the recall
        let fresh_tee = sb.tee_dir().join("1755590001_attached-ignore.log");
        std::fs::write(&fresh_tee, "ignored_pattern\n").unwrap();
        assert_eq!(sb.recall_count("attached-ignore"), 0);

        let (code, _, _) = sb.run(&[
            "rg",
            &format!("--ignore-file={}", fresh_tee.to_string_lossy()),
            "hello",
            target.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert_eq!(sb.recall_count("attached-ignore"), 1);
    }

    #[test]
    fn replace_flag_value_is_not_treated_as_tee_candidate() {
        if !std::process::Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }
        let sb = Sandbox::bare();
        let target = sb.project.join("target.txt");
        std::fs::write(&target, "hello world\n").unwrap();
        assert!(!sb.recorded_a_recall());

        let (code, _, _) = sb.run(&[
            "rg",
            &format!("--replace={}", sb.tee_log()),
            "hello",
            target.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert!(
            !sb.recorded_a_recall(),
            "--replace value must NOT be treated as a tee candidate"
        );
        assert_eq!(sb.recall_count("cargo-test"), 0);
    }

    #[test]
    fn search_pattern_containing_tee_path_is_not_treated_as_tee_candidate() {
        let sb = Sandbox::bare();
        let target = sb.project.join("target.txt");
        std::fs::write(&target, format!("key={}\n", sb.tee_log())).unwrap();
        assert!(!sb.recorded_a_recall());

        let pattern = format!("key={}", sb.tee_log());
        let (code, _, _) = sb.run(&["grep", &pattern, target.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert!(
            !sb.recorded_a_recall(),
            "a search pattern containing =<tee_path> must NOT be treated as a tee candidate"
        );
        assert_eq!(sb.recall_count("cargo-test"), 0);
    }

    #[test]
    fn unrelated_flag_value_looking_like_tee_path_is_not_treated_as_candidate() {
        let sb = Sandbox::bare();
        let target = sb.project.join("target.txt");
        std::fs::write(&target, "hello world\n").unwrap();
        assert!(!sb.recorded_a_recall());

        let (code, _, _) = sb.run(&["grep", "-e", &sb.tee_log(), target.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(
            !sb.recorded_a_recall(),
            "-e pattern flag value must NOT be treated as a file candidate"
        );
        assert_eq!(sb.recall_count("cargo-test"), 0);
    }

    #[cfg(unix)]
    struct PermGuard<'a>(&'a std::path::Path);

    #[cfg(unix)]
    impl<'a> Drop for PermGuard<'a> {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o644));
        }
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_existing_tee_file_with_suppressed_diagnostics_does_not_count_until_readable() {
        use std::os::unix::fs::PermissionsExt;

        let sb = Sandbox::bare();
        let normal = sb.project.join("normal.txt");
        std::fs::write(&normal, "match line boom\n").unwrap();

        let unreadable = sb.tee_dir().join("1755590002_unreadable.log");
        std::fs::write(&unreadable, "boom\n").unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
        let _guard = PermGuard(&unreadable);

        // Precondition check: opening must actually fail for an unreadable file
        if std::fs::File::open(&unreadable).is_ok() {
            eprintln!("Skipping unreadable test: environment permits reading chmod 000 file");
            return;
        }

        // 1. grep quiet + suppressed diagnostics: grep -qs
        let (code, _, _) = sb.run(&[
            "grep",
            "-qs",
            "boom",
            &unreadable.to_string_lossy(),
            normal.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "grep -qs exits 0 on match in normal.txt");
        assert_eq!(
            sb.recall_count("unreadable"),
            0,
            "unreadable tee file must NOT count even when exit code is 0 and diagnostics suppressed"
        );

        // 2. Restore readability
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            std::fs::File::open(&unreadable).is_ok(),
            "precondition: file must now be readable"
        );

        // 3. Rerun: must count once
        let (code2, _, _) = sb.run(&[
            "grep",
            "-qs",
            "boom",
            &unreadable.to_string_lossy(),
            normal.to_str().unwrap(),
        ]);
        assert_eq!(code2, 0);
        assert_eq!(
            sb.recall_count("unreadable"),
            1,
            "now-readable tee file must count once"
        );

        // 4. Subsequent execution remains deduped
        let (code3, _, _) = sb.run(&[
            "grep",
            "-qs",
            "boom",
            &unreadable.to_string_lossy(),
            normal.to_str().unwrap(),
        ]);
        assert_eq!(code3, 0);
        assert_eq!(
            sb.recall_count("unreadable"),
            1,
            "subsequent execution must remain deduped"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_existing_tee_file_rg_no_messages_does_not_count_when_rg_available() {
        if !std::process::Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }
        use std::os::unix::fs::PermissionsExt;

        let sb = Sandbox::bare();
        let normal = sb.project.join("normal.txt");
        std::fs::write(&normal, "match line boom\n").unwrap();

        let unreadable = sb.tee_dir().join("1755590003_unreadable_rg.log");
        std::fs::write(&unreadable, "boom\n").unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
        let _guard = PermGuard(&unreadable);

        // Precondition check: opening must actually fail for an unreadable file
        if std::fs::File::open(&unreadable).is_ok() {
            eprintln!("Skipping unreadable test: environment permits reading chmod 000 file");
            return;
        }

        let (code, _, _) = sb.run(&[
            "rg",
            "-q",
            "--no-messages",
            "boom",
            &unreadable.to_string_lossy(),
            normal.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert_eq!(
            sb.recall_count("unreadable_rg"),
            0,
            "unreadable tee file must NOT count under rg -q --no-messages"
        );

        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            std::fs::File::open(&unreadable).is_ok(),
            "precondition: file must now be readable"
        );

        let (code2, _, _) = sb.run(&[
            "rg",
            "-q",
            "--no-messages",
            "boom",
            &unreadable.to_string_lossy(),
            normal.to_str().unwrap(),
        ]);
        assert_eq!(code2, 0);
        assert_eq!(sb.recall_count("unreadable_rg"), 1);
    }

    #[cfg(unix)]
    #[test]
    fn streaming_path_with_unreadable_tee_operand_surfaces_stderr_and_does_not_count() {
        use std::os::unix::fs::PermissionsExt;

        let sb = Sandbox::bare();
        let unreadable = sb.tee_dir().join("1755590004_streaming_bad.log");
        std::fs::write(&unreadable, "boom\n").unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
        let _guard = PermGuard(&unreadable);

        // Precondition check: opening must actually fail for an unreadable file
        if std::fs::File::open(&unreadable).is_ok() {
            eprintln!("Skipping unreadable test: environment permits reading chmod 000 file");
            return;
        }

        let (_code, _, stderr) =
            sb.run(&["grep", "-q", "boom", &unreadable.to_string_lossy(), "-"]);
        assert!(
            stderr.contains("Permission denied")
                || stderr.contains("permission denied")
                || stderr.contains("denied"),
            "stderr must surface permission error in streaming path: {stderr}"
        );
        assert_eq!(
            sb.recall_count("streaming_bad"),
            0,
            "unreadable tee file in streaming path must not count"
        );
    }

    #[test]
    fn streaming_passthrough_nul_output_byte_faithful() {
        if !std::process::Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }

        use std::io::Write;
        use std::process::Stdio;

        let sb = Sandbox::bare();
        let input = b"boom line\nother line\n";

        // Native rg
        let mut native_cmd = Command::new("rg");
        native_cmd
            .args(["-l", "--null", "boom", "-"])
            .current_dir(&sb.project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = native_cmd.spawn().expect("spawn native rg");
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input).expect("write native stdin");
        }
        let native_out = child.wait_with_output().expect("wait native rg");

        // RTK
        let (rtk_code, rtk_stdout, rtk_stderr) =
            sb.run_with_stdin_bytes(&["rg", "-l", "--null", "boom", "-"], input);

        assert_eq!(rtk_code, native_out.status.code().unwrap_or(1));
        assert_eq!(
            rtk_stdout, native_out.stdout,
            "stdout must match native rg byte-for-byte without injected newline"
        );
        assert_eq!(
            rtk_stdout, b"<stdin>\x00",
            "stdout must be exact bytes <stdin>\\0"
        );
        assert_eq!(rtk_stderr, native_out.stderr);
    }

    #[test]
    fn streaming_passthrough_crlf_output_byte_faithful() {
        use std::io::Write;
        use std::process::Stdio;

        let sb = Sandbox::bare();
        let input = b"first\r\nboom\r\nsecond\r\n";

        // Native grep
        let mut native_cmd = Command::new("grep");
        native_cmd
            .args(["-a", "-o", "boom\r", "-"])
            .current_dir(&sb.project)
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = native_cmd.spawn().expect("spawn native grep");
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input).expect("write native stdin");
        }
        let native_out = child.wait_with_output().expect("wait native grep");

        // RTK
        let (rtk_code, rtk_stdout, _rtk_stderr) =
            sb.run_with_stdin_bytes(&["grep", "-a", "-o", "boom\r", "-"], input);

        assert_eq!(rtk_code, native_out.status.code().unwrap_or(1));
        assert_eq!(
            rtk_stdout, native_out.stdout,
            "CRLF output must match native grep byte-for-byte"
        );
        assert_eq!(
            rtk_stdout, b"boom\r\n",
            "stdout must preserve \\r\\n exactly rather than stripping \\r"
        );
    }

    #[test]
    fn streaming_passthrough_non_utf8_output_byte_faithful() {
        use std::io::Write;
        use std::process::Stdio;

        let sb = Sandbox::bare();
        let input = vec![b'A', 0xFF, 0xFE, b'B', b'\n'];

        // Native grep with LC_ALL=C
        let mut native_cmd = Command::new("grep");
        native_cmd
            .args(["-a", "-o", "A.*B", "-"])
            .current_dir(&sb.project)
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = native_cmd.spawn().expect("spawn native grep");
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&input).expect("write native stdin");
        }
        let native_out = child.wait_with_output().expect("wait native grep");

        // RTK
        let (rtk_code, rtk_stdout, _rtk_stderr) =
            sb.run_with_stdin_bytes(&["grep", "-a", "-o", "A.*B", "-"], &input);

        assert_eq!(rtk_code, native_out.status.code().unwrap_or(1));
        assert_eq!(
            rtk_stdout, native_out.stdout,
            "non-UTF8 output must match native grep byte-for-byte"
        );
        assert_eq!(
            rtk_stdout,
            vec![b'A', 0xFF, 0xFE, b'B', b'\n'],
            "arbitrary bytes 0xFF 0xFE must survive without replacement"
        );
    }

    #[test]
    fn streaming_passthrough_stderr_forwarded_once_and_available_to_tee_accounting() {
        let sb = Sandbox::bare();
        let valid_tee = sb.tee_log();
        assert_eq!(sb.recall_count("cargo-test"), 0);

        let missing = sb.project.join("definitely_missing_file.txt");
        let input = b"match line boom\n";

        let (code, stdout, stderr) = sb.run_with_stdin_bytes(
            &[
                "grep",
                "-q",
                "boom",
                missing.to_str().unwrap(),
                &valid_tee,
                "-",
            ],
            input,
        );

        assert_eq!(code, 0);
        assert_eq!(stdout, b"", "quiet grep emits nothing on stdout");

        let stderr_str = String::from_utf8_lossy(&stderr);
        assert!(
            stderr_str.contains("definitely_missing_file.txt")
                || stderr_str.contains("No such file"),
            "stderr must contain diagnostic for missing file: {stderr_str}"
        );
        let count_err = stderr_str.matches("definitely_missing_file.txt").count();
        assert_eq!(
            count_err, 1,
            "diagnostic for missing file must appear exactly once in stderr: {stderr_str}"
        );

        assert_eq!(
            sb.recall_count("cargo-test"),
            1,
            "valid tee file must be recorded in accounting despite streaming stderr forwarding"
        );
    }

    #[test]
    fn streaming_passthrough_missing_tee_file_diagnostic_prevents_accounting() {
        let sb = Sandbox::bare();
        let missing_tee = sb.tee_dir().join("1755590005_missing_stream.log");
        let input = b"boom line\n";

        let (code, stdout, stderr) = sb.run_with_stdin_bytes(
            &["grep", "-q", "boom", &missing_tee.to_string_lossy(), "-"],
            input,
        );

        assert_eq!(code, 0);
        assert_eq!(stdout, b"");
        let stderr_str = String::from_utf8_lossy(&stderr);
        assert!(
            stderr_str.contains("1755590005_missing_stream.log")
                || stderr_str.contains("No such file"),
            "stderr must report missing tee file: {stderr_str}"
        );
        assert_eq!(
            sb.recall_count("missing_stream"),
            0,
            "missing tee file must NOT be recorded in accounting"
        );
    }

    #[test]
    fn basename_collision_with_unrelated_failing_file_still_records_valid_tee_recall() {
        let sb = Sandbox::bare();
        let valid_tee = sb.tee_log();
        assert_eq!(sb.recall_count("cargo-test"), 0);

        let unrelated_dir = sb.project.join("other_dir");
        std::fs::create_dir_all(&unrelated_dir).unwrap();
        let unrelated_missing = unrelated_dir.join("1755590000_cargo-test.log");
        assert!(!unrelated_missing.exists());

        let (code, _, stderr) = sb.run(&[
            "grep",
            "-q",
            "boom",
            &valid_tee,
            unrelated_missing.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "exits 0 because valid_tee matched");
        assert!(
            stderr.contains(unrelated_missing.to_str().unwrap()),
            "stderr must report error for unrelated missing file: {stderr}"
        );
        assert!(
            !stderr.contains(&valid_tee),
            "stderr must not report error for valid tee file"
        );
        assert_eq!(
            sb.recall_count("cargo-test"),
            1,
            "valid tee file recall must STILL be recorded despite unrelated basename collision in stderr"
        );
    }

    #[test]
    fn quiet_grep_with_missing_tee_file_and_matching_file_does_not_record_or_burn_dedup() {
        let sb = Sandbox::bare();
        let missing = sb.tee_dir().join("1755590000_missing.log");
        let normal = sb.project.join("normal.txt");
        std::fs::write(&normal, "match line boom\n").unwrap();

        assert!(!sb.recorded_a_recall());

        // 1. Missing tee file + matching file with -q
        // Grep exits 0 because normal.txt matched, but missing file was not read.
        let (code, _, _) = sb.run(&[
            "grep",
            "-q",
            "boom",
            &missing.to_string_lossy(),
            normal.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "quiet grep exits 0 on match");
        assert!(
            !sb.recorded_a_recall(),
            "missing tee file must not be recorded even though grep returned 0"
        );
        assert_eq!(sb.recall_count("missing"), 0);

        // 2. Now create the tee file with real content
        std::fs::write(&missing, "line 1 boom\nline 2\n").unwrap();

        // 3. Genuine successful read must then count once
        let (code2, _, _) = sb.run(&[
            "grep",
            "-q",
            "boom",
            &missing.to_string_lossy(),
            normal.to_str().unwrap(),
        ]);
        assert_eq!(code2, 0);
        assert!(
            sb.recorded_a_recall(),
            "existing tee file read successfully must now record a recall"
        );
        assert_eq!(sb.recall_count("missing"), 1);

        // 4. Dedup must still hold afterward
        let (code3, _, _) = sb.run(&[
            "grep",
            "-q",
            "boom",
            &missing.to_string_lossy(),
            normal.to_str().unwrap(),
        ]);
        assert_eq!(code3, 0);
        assert_eq!(sb.recall_count("missing"), 1);
    }

    #[test]
    fn quiet_rg_with_missing_tee_file_and_matching_file_does_not_record_when_rg_available() {
        if !std::process::Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }
        let sb = Sandbox::bare();
        let missing = sb.tee_dir().join("1755590000_missingrg.log");
        let normal = sb.project.join("normal.txt");
        std::fs::write(&normal, "match line boom\n").unwrap();

        let (code, _, _) = sb.run(&[
            "rg",
            "-q",
            "boom",
            &missing.to_string_lossy(),
            normal.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert!(!sb.recorded_a_recall());
        assert_eq!(sb.recall_count("missingrg"), 0);

        std::fs::write(&missing, "line 1 boom\n").unwrap();
        let (code2, _, _) = sb.run(&[
            "rg",
            "-q",
            "boom",
            &missing.to_string_lossy(),
            normal.to_str().unwrap(),
        ]);
        assert_eq!(code2, 0);
        assert!(sb.recorded_a_recall());
        assert_eq!(sb.recall_count("missingrg"), 1);

        let (code3, _, _) = sb.run(&[
            "rg",
            "-q",
            "boom",
            &missing.to_string_lossy(),
            normal.to_str().unwrap(),
        ]);
        assert_eq!(code3, 0);
        assert_eq!(sb.recall_count("missingrg"), 1);
    }

    #[test]
    fn fresh_installation_without_recall_db_does_not_create_it_or_record_tee_recall() {
        let sb = Sandbox::bare_fresh_tee_only();
        assert!(!sb.has_any_recall_db_files());

        // 1. Successful rtk read
        let (read_code, _, _) = sb.run(&["read", &sb.tee_log()]);
        assert_eq!(read_code, 0);
        assert!(
            !sb.has_any_recall_db_files(),
            "successful rtk read must not create recall.db or sidecars on fresh installation"
        );

        // 2. Successful rtk grep
        let (grep_code, _, _) = sb.run(&["grep", "boom", &sb.tee_log()]);
        assert_eq!(grep_code, 0);
        assert!(
            !sb.has_any_recall_db_files(),
            "successful rtk grep must not create recall.db or sidecars on fresh installation"
        );

        // 3. Defer hook path
        let (defer_code, _) = sb.rewrite(&format!("tail -n +52 {}", sb.tee_log()));
        assert_eq!(defer_code, 1);
        assert!(
            !sb.has_any_recall_db_files(),
            "defer hook must not create recall.db or sidecars on fresh installation"
        );

        // 4. Ask rewrite
        let (ask_code, _) = sb.rewrite(&format!("cat {}", sb.tee_log()));
        assert_eq!(ask_code, 3);
        assert!(
            !sb.has_any_recall_db_files(),
            "ask rewrite must not create recall.db or sidecars on fresh installation"
        );
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
