//! End-to-end coverage for the fish-script wrap: `rtk rewrite` emits the
//! `rtk run --shell fish -c '<script>'` form, the quoting round-trips under
//! POSIX and fish host layers, and `rtk run` executes the wrapped script.

mod common;

#[cfg(unix)]
mod unix {
    use std::process::{Command, Output};

    const FISH_BLOCK: &str = "if test -d src\n  git status\nelse\n  echo missing\nend";

    fn rtk() -> Command {
        crate::common::rtk_command()
    }

    /// Run `rtk rewrite` with an isolated home so user config and Claude Code
    /// settings never leak into the verdict or the wrap gate.
    fn rewrite_isolated(command: &str, config_toml: Option<&str>) -> Output {
        let home = tempfile::tempdir().expect("create isolated home");
        if let Some(content) = config_toml {
            // dirs::config_dir() resolves differently per platform; cover both.
            for dir in [
                home.path().join("Library/Application Support/rtk"),
                home.path().join("rtk"),
            ] {
                std::fs::create_dir_all(&dir).expect("create config dir");
                std::fs::write(dir.join("config.toml"), content).expect("write config");
            }
        }
        rtk()
            .args(["rewrite", command])
            .current_dir(home.path())
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("RTK_TELEMETRY_DISABLED", "1")
            .output()
            .expect("run rtk rewrite")
    }

    /// `rtk rewrite` under a live deny rule, in an isolated home.
    fn rewrite_denied(command: &str, rule: &str) -> Output {
        let home = tempfile::tempdir().expect("create isolated home");
        let claude = home.path().join(".claude");
        std::fs::create_dir_all(&claude).expect("create .claude");
        std::fs::write(
            claude.join("settings.json"),
            format!(r#"{{"permissions":{{"deny":["Bash({rule})"]}}}}"#),
        )
        .expect("write settings");

        rtk()
            .args(["rewrite", command])
            .current_dir(home.path())
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("RTK_TELEMETRY_DISABLED", "1")
            .output()
            .expect("run rtk rewrite")
    }

    /// An isolated home carrying the rule files the test names, each written at
    /// `(relative path, contents)`.
    fn home_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("create isolated home");
        for (path, contents) in files {
            let target = home.path().join(path);
            std::fs::create_dir_all(target.parent().expect("rule file has a parent"))
                .expect("create rule dir");
            std::fs::write(&target, contents).expect("write rule file");
        }
        home
    }

    /// `rtk hook <agent>` fed a PreToolUse payload on stdin — the path a host
    /// actually takes, as opposed to the `rtk rewrite` CLI.
    fn hook_stdin(agent: &str, payload: &str, home: &tempfile::TempDir) -> Output {
        use std::io::Write;

        let mut child = rtk()
            .args(["hook", agent])
            .current_dir(home.path())
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("RTK_TELEMETRY_DISABLED", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn rtk hook");
        child
            .stdin
            .as_mut()
            .expect("hook stdin")
            .write_all(payload.as_bytes())
            .expect("write payload");
        child.wait_with_output().expect("run rtk hook")
    }

    /// `rtk hook check --agent <agent>` — the diagnostic that must answer for
    /// the agent named, including whose rules that agent's hook reads.
    fn hook_check(agent: &str, command: &str, home: &tempfile::TempDir) -> Output {
        rtk()
            .args(["hook", "check", "--agent", agent, "--", command])
            .current_dir(home.path())
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("RTK_TELEMETRY_DISABLED", "1")
            .output()
            .expect("run rtk hook check")
    }

    const DENIED_IN_CONDITION: &str = "if not rm -rf victim\n  echo x\nend";

    fn wrapped(output: &Output) -> bool {
        String::from_utf8_lossy(&output.stdout).contains("rtk run --shell fish")
    }

    /// The hook path reads the host's rules and hands the deny side to the
    /// decision, so a denied command in an `if` condition keeps the wrap off
    /// there too — not only through the `rtk rewrite` CLI.
    #[test]
    fn the_hook_path_passes_the_hosts_deny_rules_to_the_wrap() {
        let payload = format!(
            r#"{{"tool_name":"Bash","tool_input":{{"command":{}}}}}"#,
            serde_json::to_string(DENIED_IN_CONDITION).expect("encode command")
        );
        let denied = home_with(&[(
            ".claude/settings.json",
            r#"{"permissions":{"deny":["Bash(rm:*)"]}}"#,
        )]);
        let output = hook_stdin("claude", &payload, &denied);
        assert!(
            !wrapped(&output),
            "a denied command must never be wrapped: {}",
            String::from_utf8_lossy(&output.stdout)
        );

        // The control: without the rule the same payload wraps, so the test
        // cannot pass because the hook refused for some other reason.
        if which::which("fish").is_ok() {
            let open = home_with(&[]);
            assert!(
                wrapped(&hook_stdin("claude", &payload, &open)),
                "control: the hook wraps this script with no rules"
            );
        }
    }

    /// `rtk hook check` answers for the agent it was asked about, which
    /// includes whose rules that agent's hook reads: its own for an in-process
    /// host, Claude's for a delegate that shells out to `rtk rewrite`, and none
    /// at all for a rules-file install that has no hook.
    #[test]
    fn hook_check_consults_the_rules_the_named_agent_reads() {
        if which::which("fish").is_err() {
            // Without a fish the wrap never happens, so no row distinguishes
            // the rule sources; the unit tests cover the mapping itself.
            return;
        }
        let claude_denies = (
            ".claude/settings.json",
            r#"{"permissions":{"deny":["Bash(rm:*)"]}}"#,
        );
        let cursor_denies = (
            ".cursor/cli-config.json",
            r#"{"permissions":{"deny":["Shell(rm)"]}}"#,
        );

        // An in-process host reads its own rules, not Claude's.
        let home = home_with(&[cursor_denies]);
        assert!(!wrapped(&hook_check("cursor", DENIED_IN_CONDITION, &home)));
        assert!(wrapped(&hook_check("claude", DENIED_IN_CONDITION, &home)));

        // A delegate that shells out to `rtk rewrite` reads Claude's.
        let home = home_with(&[claude_denies]);
        assert!(!wrapped(&hook_check(
            "openclaw",
            DENIED_IN_CONDITION,
            &home
        )));
        // …and an in-process host does not borrow them.
        assert!(wrapped(&hook_check("cursor", DENIED_IN_CONDITION, &home)));

        // A rules-file install has no hook and no permission surface, so it
        // reads nothing — including Claude's rules.
        assert!(wrapped(&hook_check("kimi", DENIED_IN_CONDITION, &home)));
    }

    /// A deny rule keeps the wrap off, in every placement — not only the ones
    /// the bash-shaped segmenter can read.
    ///
    /// The property is *never wrapped*, not a particular exit code: fish starts
    /// commands where bash has no grammar at all (the condition of an `if` or a
    /// `while`, the right side of `and`/`or`/`not`), and bash refuses those
    /// scripts outright, so the rule only has to hold wherever RTK would hand
    /// the script to a fish. Exit 3 — a wrap — is the failure.
    ///
    /// Each row carries its own control: `wraps_unruled` says whether the same
    /// script wraps with no rules at all, so a row cannot pass by being
    /// unwrappable for some unrelated reason.
    #[test]
    fn deny_keeps_the_wrap_off_in_every_placement() {
        let wrapping = which::which("fish").is_ok();
        for (rule, command, wraps_unruled) in [
            // Not fish at all: the deny verdict itself stops it.
            ("rm:*", "rm -rf victim", false),
            ("rm:*", "if test -d src\n  rm -rf victim\nend", true),
            ("rm:*", "test -d src; and rm -rf victim", true),
            ("rm:*", "not rm -rf victim", true),
            ("rm:*", "if not rm -rf victim\n  echo x\nend", true),
            ("rm:*", "while not rm -rf victim\n  break\nend", true),
            ("rm:*", "if rm -rf victim\n  echo x\nend", true),
            ("rm:*", "while rm -rf victim\n  break\nend", true),
            (
                "rm:*",
                "if test -d src\n  true; and rm -rf victim\nend",
                true,
            ),
            // Quoting the program changes nothing: the words are compared
            // dequoted, the way the shell reads them.
            ("rm:*", "if not 'rm' -rf victim\n  echo x\nend", true),
            ("rm:*", "if not r'm' -rf victim\n  echo x\nend", true),
            ("rm:*", "if not \"rm\" -rf victim\n  echo x\nend", true),
            // The words come from the comment-stripped code: an apostrophe in a
            // comment opens the shared lexer's quote state and would swallow
            // the line that names the denied program into one argument.
            ("rm:*", "echo hi  # don't worry\nnot rm -rf victim", true),
            (
                "rm:*",
                "begin\n  echo hi  # don't\n  rm -rf victim\nend",
                true,
            ),
            // The operator glues itself to the word before it, so the words of
            // one command are read per command, not across the whole script.
            ("git push:*", "not git push; echo blocked", true),
            ("git push:*", "not git push|cat", true),
            // A rule may anchor its tail, so the run has to be able to end
            // before the end of the script.
            (
                "echo * DENIED",
                "if not echo ARG DENIED\n  echo BODY\nend",
                true,
            ),
            ("echo * DENIED", "not echo ARG DENIED extra; and true", true),
            ("rm * victim", "if not rm -rf victim\n  echo x\nend", true),
            // The emitted script carries commands the rewrite inserted, and a
            // rule may name those.
            ("rtk ls:*", "ls; and true", true),
            ("rtk git:*", "git diff HEAD~3 HEAD; and true", true),
            // Escapes this lexer and fish resolve differently — a `\r` that
            // splits words there and not here, a line continuation fish elides,
            // `\x70` which is `p` to fish. The words cannot be read, so the
            // wrap is refused.
            ("pwd:*", "if not pwd\r\n  echo BODY\r\nend", true),
            ("git push:*", "not git\rpush; and true", true),
            ("pwd:*", "if not p\\\nwd\n  echo BODY\nend", true),
            ("pwd:*", "if not \\x70wd\n  echo BODY\nend", true),
            // A redirect ends the word, not the command.
            ("echo * DENIED", "not echo A 2>&1 DENIED", true),
            ("git push:*", "not git 2>&1 push", true),
            // A rule the user wrote quoted still names the same command.
            (
                "echo 'DENIED'",
                "if not echo DENIED\n  echo BODY\nend",
                true,
            ),
            // An expansion's value is the command fish runs.
            (
                "echo DENIED",
                "set -l runner echo; $runner DENIED; and true",
                true,
            ),
            // An empty argument is a word here and nothing at all to the
            // matcher, and a redirect's operand may be spaced off it.
            ("echo * DENIED", "not echo '' A DENIED", true),
            ("echo DENIED", "not echo 2> /dev/null DENIED", true),
            // A glob or a `~` resolves before the command runs.
            ("/bin/echo DENIED", "not /bin/ec*o DENIED", true),
            ("echo /var/root", "not echo ~root", true),
        ] {
            let output = rewrite_denied(command, rule);
            let stdout = String::from_utf8_lossy(&output.stdout);

            assert!(
                !stdout.starts_with("rtk run --shell fish"),
                "a denied command must never be wrapped: {rule:?} {command:?} -> {stdout:?}"
            );

            if wrapping {
                let unruled = rewrite_isolated(command, None);
                assert_eq!(
                    String::from_utf8_lossy(&unruled.stdout).starts_with("rtk run --shell fish"),
                    wraps_unruled,
                    "control: {command:?} with no rules"
                );
            }
        }
    }

    /// The rule is about what it names: a script with nothing denied in it
    /// still wraps, and a rule naming something else does not stop it. A rule
    /// naming nothing — an empty string — does not match the empty argument
    /// either.
    ///
    /// Both branches assert, since without a `fish` the wrap is skipped for a
    /// reason that has nothing to do with the rules.
    #[test]
    fn an_unrelated_deny_rule_leaves_the_wrap_alone() {
        let wrapping = which::which("fish").is_ok();
        for (rule, command) in [
            ("git push:*", "test -d src; and git status"),
            ("git push:*", "if test -d src\n  git status\nend"),
            ("", "not printf %s ''"),
        ] {
            let output = rewrite_denied(command, rule);
            let stdout = String::from_utf8_lossy(&output.stdout);

            if wrapping {
                assert_eq!(output.status.code(), Some(3), "{rule:?} {command:?}");
                assert!(
                    stdout.starts_with("rtk run --shell fish -c"),
                    "{rule:?} {command:?} -> {stdout:?}"
                );
            } else {
                assert!(
                    !stdout.starts_with("rtk run --shell fish"),
                    "no fish, so no wrap: {rule:?} {command:?} -> {stdout:?}"
                );
            }
        }
    }

    /// Both branches assert: with `fish` the hook must wrap, without it the
    /// wrap must be skipped rather than half-applied. A test that returns early
    /// asserts nothing on a runner with no `fish`, which is most of them.
    #[test]
    fn rewrite_wraps_multiline_fish_block_as_ask() {
        let output = rewrite_isolated(FISH_BLOCK, None);

        if which::which("fish").is_err() {
            assert_eq!(
                output.status.code(),
                Some(1),
                "without fish the wrap must defer"
            );
            assert!(output.stdout.is_empty());
            return;
        }

        assert_eq!(output.status.code(), Some(3), "wrap must surface as Ask");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("rtk run --shell fish -c '{FISH_BLOCK}'")
        );
        assert!(output.stderr.is_empty());
    }

    /// The wrap carries the rewrite the command would have had without it.
    #[test]
    fn rewrite_keeps_the_inner_rewrite_inside_the_wrap() {
        let output = rewrite_isolated("git diff HEAD~3 HEAD; and true", None);
        let rewritten = String::from_utf8_lossy(&output.stdout);

        if which::which("fish").is_err() {
            // The leading command is rewritable on its own, so without a `fish`
            // to wrap for this is an ordinary rewrite — not a defer. Asserting
            // the exact string is what pins the wrap as genuinely off.
            assert_eq!(output.status.code(), Some(3));
            assert_eq!(rewritten, "rtk git diff HEAD~3 HEAD; and true");
            return;
        }

        assert_eq!(output.status.code(), Some(3));
        assert_eq!(
            rewritten,
            "rtk run --shell fish -c 'rtk git diff HEAD~3 HEAD; and true'"
        );
    }

    /// A script the permission gate could not decompose is never emitted as an
    /// `rtk`-prefixed command, with or without a local `fish`.
    #[test]
    fn rewrite_defers_unattestable_fish_scripts() {
        for command in [
            "test -d src; and cat secrets.env > /tmp/rtk-leak-test",
            "test -d src; and git status # don't\ncat secrets.env > /tmp/rtk-leak-test",
            "test -d src; and echo $(whoami)",
            "for f in (ls)\n  echo $f\nend",
        ] {
            let output = rewrite_isolated(command, None);

            assert_eq!(output.status.code(), Some(1), "{command:?}");
            assert!(output.stdout.is_empty(), "{command:?}");
        }
    }

    #[test]
    fn rewrite_honors_wrap_fish_scripts_opt_out() {
        let output = rewrite_isolated(FISH_BLOCK, Some("[hooks]\nwrap_fish_scripts = false\n"));

        assert_eq!(output.status.code(), Some(1), "opt-out must defer");
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn rewrite_defers_posix_multiline_script() {
        let output = rewrite_isolated("if [ -d src ]; then\n  git status\nfi", None);

        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn rewrite_defers_fish_script_with_divergent_backslash() {
        // Classifies fish (`; and` marker) but contains `\\`, which fish
        // single-quotes would collapse — the wrapper vetoes it, so the hook
        // defers instead of emitting a wrap that would corrupt the script.
        let output = rewrite_isolated("echo '\\\\'; and echo ok", None);

        assert_eq!(
            output.status.code(),
            Some(1),
            "divergent backslash must defer"
        );
        assert!(output.stdout.is_empty());
    }

    /// Mirror of `fish_script::escape_single_quoted` — tests/ cannot reach
    /// crate internals, and duplicating one replace keeps this test honest
    /// about what the emitted command actually contains.
    fn escape_single_quoted(script: &str) -> String {
        script.replace('\'', "'\\''")
    }

    #[test]
    fn wrapped_quoting_round_trips_under_posix_and_fish_hosts() {
        // The `%s\n` carries a lone backslash inside single quotes: literal
        // under both POSIX and fish, so it must arrive byte-identical (unlike
        // `\\`/`\'`, which the wrapper vetoes upstream).
        let script = "if test -d src\n  printf '%s\\n' 'a b'\nelse\n  echo missing\nend";
        let probe = format!("printf '%s' '{}'", escape_single_quoted(script));

        for shell in ["sh", "zsh", "fish"] {
            let Ok(shell_path) = which::which(shell) else {
                continue;
            };
            let output = Command::new(shell_path)
                .args(["-c", &probe])
                .output()
                .unwrap_or_else(|e| panic!("run {shell}: {e}"));
            assert!(output.status.success(), "{shell} must parse the wrapping");
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                script,
                "script must arrive byte-identical through a {shell} host layer"
            );
        }
    }

    #[test]
    #[ignore] // executes real fish; run with: cargo test --ignored
    fn wrapped_multiline_fish_script_executes_and_propagates_exit_code() {
        let Ok(fish) = which::which("fish") else {
            return;
        };
        let script = "if test -d /\n  echo yes\nelse\n  echo no\nend";

        let direct = Command::new(&fish)
            .args(["-c", script])
            .output()
            .expect("run fish directly");
        let wrapped = rtk()
            .args(["run", "--shell", "fish", "-c", script])
            .output()
            .expect("run rtk run");

        assert!(wrapped.status.success());
        assert_eq!(
            wrapped.stdout, direct.stdout,
            "output must match direct fish"
        );

        let status = rtk()
            .args(["run", "--shell", "fish", "-c", "exit 2"])
            .status()
            .expect("run rtk run");
        assert_eq!(status.code(), Some(2), "child exit code must propagate");
    }
}
