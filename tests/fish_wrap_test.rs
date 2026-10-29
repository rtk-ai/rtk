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

    /// An isolated home carrying the rule files the test names, each written at
    /// `(relative path, contents)`.
    ///
    /// The wrap is opt-in, so every home here turns it on: these tests are
    /// about what the wrap does once a user asks for it.
    /// `the_hook_does_not_wrap_unless_asked_to` covers the default.
    fn home_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        home_with_wrap(files, Some(true))
    }

    /// [`home_with`], with `hooks.wrap_fish_scripts` set to `enabled` — or the
    /// config left absent entirely when it is `None`.
    fn home_with_wrap(files: &[(&str, &str)], enabled: Option<bool>) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("create isolated home");
        if let Some(enabled) = enabled {
            // dirs::config_dir() resolves differently per platform; cover both.
            for dir in [
                home.path().join("Library/Application Support/rtk"),
                home.path().join("rtk"),
            ] {
                std::fs::create_dir_all(&dir).expect("create config dir");
                std::fs::write(
                    dir.join("config.toml"),
                    format!("[hooks]\nwrap_fish_scripts = {enabled}\n"),
                )
                .expect("write config");
            }
        }
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

    /// Builds one host's payload around a command.
    type PayloadFor = fn(&str) -> String;

    /// `value` as a JSON string literal, for payloads built by hand.
    fn json_string(value: &str) -> String {
        serde_json::to_string(value).expect("encode string")
    }

    /// A Claude Code `PreToolUse` payload carrying `command`.
    fn claude_payload(command: &str) -> String {
        format!(
            r#"{{"tool_name":"Bash","tool_input":{{"command":{}}}}}"#,
            serde_json::to_string(command).expect("encode command")
        )
    }

    fn wrapped(output: &Output) -> bool {
        String::from_utf8_lossy(&output.stdout).contains("rtk run --shell fish")
    }

    /// The command of a wrap the Claude hook emitted, or `None` when it did
    /// not wrap.
    fn wrapped_command(output: &Output) -> Option<String> {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")?
            .as_str()?;
        cmd.starts_with("rtk run --shell fish")
            .then(|| cmd.to_string())
    }

    /// `rtk hook claude` on `command`, with the rule files the test names.
    fn claude_hook(command: &str, files: &[(&str, &str)]) -> Output {
        let home = home_with(files);
        hook_stdin("claude", &claude_payload(command), &home)
    }

    fn claude_denies(rule: &str) -> String {
        format!(r#"{{"permissions":{{"deny":["Bash({rule})"]}}}}"#)
    }

    const FISH_CHAIN: &str = "test -d src; and git status";

    /// Every placement the previous rounds swept, plus the ones that defeated
    /// a scanner reading the script: a `?` fish 3.x expands as a glob, a
    /// whitespace-only argument, a program named through `alias`, and a word
    /// carrying a `~`, a `\r` or an escape. None of them is read any more —
    /// the rule's *presence* keeps the wrap off, so there is nothing left for
    /// a spelling to slip past.
    ///
    /// Each row carries its control: the same script wraps when the host has
    /// no rule at all, so no row can pass by being unwrappable for an
    /// unrelated reason.
    #[test]
    fn any_deny_rule_keeps_the_wrap_off() {
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
            ("rm:*", "if not 'rm' -rf victim\n  echo x\nend", true),
            ("rm:*", "if not r'm' -rf victim\n  echo x\nend", true),
            ("rm:*", "if not \"rm\" -rf victim\n  echo x\nend", true),
            ("rm:*", "echo hi  # don't worry\nnot rm -rf victim", true),
            ("git push:*", "not git push; echo blocked", true),
            ("git push:*", "not git push|cat", true),
            (
                "echo * DENIED",
                "if not echo ARG DENIED\n  echo BODY\nend",
                true,
            ),
            ("echo * DENIED", "not echo ARG DENIED extra; and true", true),
            ("rtk ls:*", "ls; and true", true),
            ("pwd:*", "if not pwd\r\n  echo BODY\r\nend", true),
            ("pwd:*", "if not p\\\nwd\n  echo BODY\nend", true),
            ("pwd:*", "if not \\x70wd\n  echo BODY\nend", true),
            ("echo * DENIED", "not echo A 2>&1 DENIED", true),
            ("echo DENIED", "not echo 2> /dev/null DENIED", true),
            ("echo DENIED", "not echo '' A DENIED", true),
            // A `?` is a filename glob in fish before 4.0, so `ec?o` runs
            // `echo` there and not on 4.x — a difference no reading of the
            // text can settle.
            ("/bin/echo DENIED", "not /bin/ec?o DENIED", true),
            ("/bin/echo DENIED", "not /bin/ec*o DENIED", true),
            ("echo /var/root", "not echo ~root", true),
            // The program is named only by an alias defined in the script.
            ("echo DENIED", "alias e echo; e DENIED; and true", true),
            // A rule that names nothing in the script keeps it off all the
            // same: RTK cannot say which commands a fish would run.
            ("git push:*", FISH_CHAIN, true),
            ("curl:*", "if test -d src\n  git status\nend", true),
            ("*", "begin\n  git status\nend", true),
        ] {
            let denied = claude_hook(command, &[(".claude/settings.json", &claude_denies(rule))]);
            assert!(
                !wrapped(&denied),
                "a deny rule must keep the wrap off: {rule:?} {command:?} -> {}",
                String::from_utf8_lossy(&denied.stdout)
            );

            if wrapping {
                assert_eq!(
                    wrapped(&claude_hook(command, &[])),
                    wraps_unruled,
                    "control: {command:?} with no rules"
                );
            }
        }
    }

    /// A settings file RTK reads but cannot use may be the one naming the rule
    /// that matters, so the wrap is withheld: not parseable JSON, not
    /// decodable as UTF-8, or not readable at all.
    #[test]
    fn a_settings_file_rtk_could_not_read_keeps_the_wrap_off() {
        if which::which("fish").is_err() {
            return;
        }
        assert!(
            wrapped(&claude_hook(FISH_CHAIN, &[])),
            "control: no settings at all must wrap"
        );
        assert!(
            wrapped(&claude_hook(
                FISH_CHAIN,
                &[(".claude/settings.json", r#"{"permissions":{"deny":[]}}"#)]
            )),
            "control: a parsed file with no deny rule must wrap"
        );
        for (file, contents) in [
            (".claude/settings.json", "{ not json"),
            (".claude/settings.local.json", "{ not json"),
        ] {
            assert!(
                !wrapped(&claude_hook(FISH_CHAIN, &[(file, contents)])),
                "{file} did not parse, so RTK has not seen every rule"
            );
        }

        // Bytes that are not UTF-8 never reach the JSON parser.
        let home = home_with(&[]);
        let settings = home.path().join(".claude/settings.json");
        std::fs::create_dir_all(settings.parent().expect("has a parent")).expect("create .claude");
        std::fs::write(
            &settings,
            b"{\"permissions\":{\"deny\":[\"Bash(rm:*)\"]},\"n\":\"\xff\"}",
        )
        .expect("write settings");
        assert!(
            !wrapped(&hook_stdin("claude", &claude_payload(FISH_CHAIN), &home)),
            "a file RTK could not decode must keep the wrap off"
        );
    }

    /// "No deny rule" means no entry at all, not "no entry this gate
    /// understands": `Bash` on its own denies every Bash call, and the Bash
    /// extractor keeps only the `Bash(…)` forms.
    #[test]
    fn a_deny_entry_this_gate_does_not_parse_still_keeps_the_wrap_off() {
        if which::which("fish").is_err() {
            return;
        }
        for rule in ["Bash", "Read(*)", "WebFetch"] {
            let settings = format!(r#"{{"permissions":{{"deny":["{rule}"]}}}}"#);
            assert!(
                !wrapped(&claude_hook(
                    FISH_CHAIN,
                    &[(".claude/settings.json", &settings)]
                )),
                "a deny rule is a deny rule: {rule}"
            );
        }
    }

    /// Only `rtk hook claude` wraps. Every other agent either reads a rule
    /// source that is not the one enforced, or hands the rewritten command to
    /// a host that judges it again — so flipping any single one turns this red.
    #[test]
    fn only_the_claude_hook_wraps() {
        if which::which("fish").is_err() {
            // Without a fish nothing wraps for any agent, so no row separates
            // them; `decision.rs`'s unit tests pin the mapping itself.
            return;
        }
        let empty = home_with(&[]);
        assert!(
            wrapped(&hook_check("claude", FISH_CHAIN, &empty)),
            "control: Claude Code's own hook wraps when it has no deny rule"
        );

        // Each agent with no rule file of its own — the case most likely to
        // look like "nothing is denied" — and, where it has one, with a rule.
        for (agent, rule_file, denies) in [
            // Reads `~/.claude` for its verdict, then judges the returned
            // command with its own allowlist flow.
            ("copilot", None, None),
            // Reads only the global config; the host also applies the project's.
            (
                "cursor",
                Some(".cursor/cli-config.json"),
                Some(r#"{"permissions":{"deny":["Shell(rm)"]}}"#),
            ),
            // The loader produces no deny rule at all, so exclude lists are
            // invisible here.
            (
                "gemini",
                Some(".gemini/settings.json"),
                Some(r#"{"tools":{"exclude":["run_shell_command(rm)"]}}"#),
            ),
            // Built-in defaults are deliberately not mirrored.
            (
                "droid",
                Some(".factory/settings.json"),
                Some(r#"{"commandDenylist":["rm"]}"#),
            ),
            ("codex", None, None),
            ("trae", None, None),
            ("vibe", None, None),
            ("antigravity", None, None),
            ("opencode", None, None),
            // No hook at all: `rtk hook check` must not show a wrap nothing
            // would ever emit.
            ("kimi", None, None),
        ] {
            assert!(
                !wrapped(&hook_check(agent, FISH_CHAIN, &empty)),
                "{agent} must never wrap, even with no rule file"
            );
            if let (Some(file), Some(contents)) = (rule_file, denies) {
                let home = home_with(&[(file, contents)]);
                assert!(
                    !wrapped(&hook_check(agent, FISH_CHAIN, &home)),
                    "{agent} must never wrap, rule file or not"
                );
            }
        }

        // …and the diagnostic is not the path that runs. Every host with a
        // payload hook of its own gets the same row through that hook, so a
        // host flipped there — and not in `AgentPath` — is caught too. Each
        // row carries its own control: the same payload with a plain command
        // must come back rewritten, so a row cannot pass because the payload
        // never reached the decision.
        let payloads: [(&str, PayloadFor); 10] = [
            ("cursor", |cmd| {
                format!(
                    r#"{{"tool_name":"Bash","tool_input":{{"command":{}}}}}"#,
                    json_string(cmd)
                )
            }),
            ("gemini", |cmd| {
                format!(
                    r#"{{"tool_name":"run_shell_command","tool_input":{{"command":{}}}}}"#,
                    json_string(cmd)
                )
            }),
            ("droid", |cmd| {
                format!(
                    r#"{{"hook_event_name":"PreToolUse","tool_name":"Execute","tool_input":{{"command":{}}}}}"#,
                    json_string(cmd)
                )
            }),
            // Codex's shell tool is `Bash`, and the payload must name a
            // permission mode; `"tool_name":"shell"` is ignored outright, so a
            // row sending that would pass whatever the Codex path decides.
            ("codex", |cmd| {
                format!(
                    r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","permission_mode":"default","tool_input":{{"command":{}}}}}"#,
                    json_string(cmd)
                )
            }),
            ("vibe", |cmd| {
                format!(
                    r#"{{"hook_event_name":"pre_tool","tool_name":"bash","tool_input":{{"command":{}}}}}"#,
                    json_string(cmd)
                )
            }),
            ("trae", |cmd| {
                format!(
                    r#"{{"tool_name":"RunCommand","tool_input":{{"command":{}}}}}"#,
                    json_string(cmd)
                )
            }),
            ("antigravity", |cmd| {
                format!(
                    r#"{{"toolCall":{{"name":"run_command","args":{{"CommandLine":{}}}}}}}"#,
                    json_string(cmd)
                )
            }),
            // Copilot reaches RTK through three shapes, and each has its own
            // call into the decision: the VS Code hook, the IDE tool call and
            // the CLI's `toolArgs`.
            ("copilot", |cmd| {
                format!(
                    r#"{{"tool_name":"Bash","tool_input":{{"command":{}}}}}"#,
                    json_string(cmd)
                )
            }),
            ("copilot", |cmd| {
                format!(
                    r#"{{"toolName":"run_in_terminal","toolArgs":{}}}"#,
                    json_string(&format!(
                        r#"{{"command":{},"explanation":"x","isBackground":false}}"#,
                        json_string(cmd)
                    ))
                )
            }),
            ("copilot", |cmd| {
                format!(
                    r#"{{"toolName":"bash","toolArgs":{}}}"#,
                    json_string(&format!(r#"{{"command":{}}}"#, json_string(cmd)))
                )
            }),
        ];
        for (agent, payload) in payloads {
            let home = home_with(&[]);
            assert!(
                !wrapped(&hook_stdin(agent, &payload(FISH_CHAIN), &home)),
                "rtk hook {agent} must never wrap: {}",
                payload(FISH_CHAIN)
            );

            let control = hook_stdin(agent, &payload("git status"), &home);
            assert!(
                String::from_utf8_lossy(&control.stdout).contains("rtk git status"),
                "control: this payload must reach the decision for {agent}: {}",
                String::from_utf8_lossy(&control.stdout)
            );
        }
    }

    /// The pipeline's `Swap` gate asks the shared decision whether RTK would
    /// touch the command at all, and uses the answer as a yes/no on the
    /// pipeline's *own* replacement command. A wrap must never be that answer:
    /// the gate would then swap a script it otherwise defers into `sh -c`.
    #[test]
    fn the_pipeline_swap_gate_never_sees_a_wrap() {
        if which::which("fish").is_err() {
            return;
        }
        const RULE: &str = r#"[upstream.haiku]
provider = "anthropic"
model = "claude-haiku-4-5"

[task.check-build]
kind = "digest"
upstream = "haiku"

[[hook.before_tool.shell]]
name = "swap-git"
when = { command_matches = "git status" }
action = "route"
task = "check-build"
mode = "swap"
"#;
        // dirs::config_dir() resolves differently per platform; cover both.
        let home = home_with(&[
            ("rtk/pipeline.toml", RULE),
            ("Library/Application Support/rtk/pipeline.toml", RULE),
        ]);

        // The control: a plain command the rule matches is swapped, so the
        // rule is live and the gate is the thing being exercised.
        let swapped = hook_stdin("claude", &claude_payload("git status"), &home);
        assert!(
            String::from_utf8_lossy(&swapped.stdout).contains("rtk check-build"),
            "control: the swap rule must fire: {}",
            String::from_utf8_lossy(&swapped.stdout)
        );

        // A fish chain the same rule matches is not swapped into `sh -c`.
        let output = hook_stdin("claude", &claude_payload(FISH_CHAIN), &home);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains("rtk check-build"),
            "a fish script must not be swapped: {stdout}"
        );
        assert!(wrapped(&output), "{stdout}");
    }

    /// `rtk hook check` for a delegate agrees with `rtk rewrite`, which is the
    /// path that actually runs for it.
    #[test]
    fn the_delegate_diagnostic_never_shows_a_wrap() {
        if which::which("fish").is_err() {
            return;
        }
        let empty = home_with(&[]);
        for agent in ["openclaw", "omp", "pi", "hermes"] {
            assert!(
                !wrapped(&hook_check(agent, FISH_CHAIN, &empty)),
                "{agent} shells out to rtk rewrite, which never wraps"
            );
        }
    }

    /// `rtk rewrite` never wraps: the delegates that shell out to it apply
    /// exit 3 without prompting, and OpenClaw matches its own exec checks
    /// against the returned string.
    #[test]
    fn rtk_rewrite_never_wraps() {
        for host in [
            None,
            Some("openclaw"),
            Some("omp"),
            Some("pi"),
            Some("hermes"),
        ] {
            let home = home_with(&[]);
            let mut cmd = rtk();
            cmd.args(["rewrite", FISH_CHAIN])
                .current_dir(home.path())
                .env("HOME", home.path())
                .env("XDG_CONFIG_HOME", home.path())
                .env("RTK_TELEMETRY_DISABLED", "1");
            if let Some(host) = host {
                cmd.env("RTK_REWRITE_HOST", host);
            }
            let output = cmd.output().expect("run rtk rewrite");
            assert!(
                !wrapped(&output),
                "rtk rewrite must never wrap ({host:?}): {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }

    /// OpenCode decides with its own patterns against the string RTK returns,
    /// so a wrap would hide the script from them. It never wraps, and the
    /// denial it reports is its own.
    #[test]
    fn opencode_never_wraps_and_keeps_its_own_denial() {
        // OpenCode reads its rules in file order and the last match wins, so
        // the deny comes after the blanket allow.
        let home = home_with(&[(
            "opencode.json",
            r#"{"permission":{"bash":{"*":"allow","rm *":"deny"}}}"#,
        )]);
        let config = home.path().join("opencode.json");

        for command in [FISH_CHAIN, "if test -d src\n  rm -rf victim\nend"] {
            let output = rtk()
                .args(["hook", "opencode", "--", command])
                .current_dir(home.path())
                .env("HOME", home.path())
                .env("XDG_CONFIG_HOME", home.path())
                .env("OPENCODE_CONFIG", &config)
                .env("RTK_TELEMETRY_DISABLED", "1")
                .output()
                .expect("run rtk hook opencode");
            assert!(
                !wrapped(&output),
                "opencode must never wrap: {command:?} -> {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }

        // The verdict still comes from OpenCode's own rules, which live
        // outside the settings-file loader.
        let denied = rtk()
            .args([
                "hook",
                "check",
                "--agent",
                "opencode",
                "--",
                "rm -rf victim",
            ])
            .current_dir(home.path())
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("OPENCODE_CONFIG", &config)
            .env("RTK_TELEMETRY_DISABLED", "1")
            .output()
            .expect("run rtk hook check");
        assert!(
            String::from_utf8_lossy(&denied.stderr).contains("Denied by a permission rule"),
            "hook check must report OpenCode's own denial: {}",
            String::from_utf8_lossy(&denied.stderr)
        );
    }

    /// Both branches assert: with `fish` the hook must wrap, without it the
    /// wrap must be skipped rather than half-applied. A test that returns early
    /// asserts nothing on a runner with no `fish`, which is most of them.
    #[test]
    fn claude_hook_wraps_multiline_fish_block() {
        let output = claude_hook(FISH_BLOCK, &[]);

        if which::which("fish").is_err() {
            assert!(!wrapped(&output), "without fish the wrap must be skipped");
            return;
        }

        assert_eq!(
            wrapped_command(&output).as_deref(),
            Some(format!("rtk run --shell fish -c '{FISH_BLOCK}'").as_str())
        );
    }

    /// The wrap carries the rewrite the command would have had without it.
    #[test]
    fn the_wrap_keeps_the_inner_rewrite() {
        let output = claude_hook("git diff HEAD~3 HEAD; and true", &[]);

        if which::which("fish").is_err() {
            // The leading command is rewritable on its own, so without a `fish`
            // to wrap for this is an ordinary rewrite — not a defer.
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                stdout.contains("rtk git diff HEAD~3 HEAD; and true"),
                "{stdout}"
            );
            assert!(!wrapped(&output));
            return;
        }

        assert_eq!(
            wrapped_command(&output).as_deref(),
            Some("rtk run --shell fish -c 'rtk git diff HEAD~3 HEAD; and true'")
        );
    }

    /// A script the permission gate could not decompose is never emitted as an
    /// `rtk`-prefixed command, with or without a local `fish`.
    #[test]
    fn the_hook_defers_unattestable_fish_scripts() {
        for command in [
            "test -d src; and cat secrets.env > /tmp/rtk-leak-test",
            "test -d src; and git status # don't\ncat secrets.env > /tmp/rtk-leak-test",
            "test -d src; and echo $(whoami)",
            "for f in (ls)\n  echo $f\nend",
        ] {
            let output = claude_hook(command, &[]);

            assert!(
                output.stdout.trim_ascii().is_empty(),
                "{command:?} -> {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }

    /// The wrap is opt-in. Without `hooks.wrap_fish_scripts = true` — whether
    /// the config says `false` or says nothing at all — the hook answers as it
    /// does on `develop`.
    ///
    /// It is opt-in because the host judges what RTK hands back, and what it
    /// sees is one `rtk run` command rather than the script's own.
    #[test]
    fn the_hook_does_not_wrap_unless_asked_to() {
        // No config at all (the `Default` impl), a config that says `false`,
        // and a config that exists but does not name the field (serde's
        // default) — all three must leave the command alone.
        for home in [
            home_with_wrap(&[], None),
            home_with_wrap(&[], Some(false)),
            home_with_wrap(
                &[
                    ("rtk/config.toml", "[hooks]\nexclude_commands = []\n"),
                    (
                        "Library/Application Support/rtk/config.toml",
                        "[hooks]\nexclude_commands = []\n",
                    ),
                ],
                None,
            ),
        ] {
            let output = hook_stdin("claude", &claude_payload(FISH_BLOCK), &home);

            assert!(
                output.stdout.trim_ascii().is_empty(),
                "the wrap is opt-in: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }

        // The control: the same script wraps once a user asks for it.
        if which::which("fish").is_ok() {
            assert!(wrapped(&claude_hook(FISH_BLOCK, &[])));
        }
    }

    /// Every wrap carries its own `ask`. Left to the host, the permission check
    /// would run against `rtk run --shell fish -c '…'`, where an allow rule
    /// matching `rtk` covers the whole script and a rule naming one of its
    /// commands matches nothing — including rules RTK cannot read at all
    /// (`--settings`, `--disallowedTools`, managed settings, a project the
    /// agent has moved away from).
    #[test]
    fn a_wrap_carries_an_explicit_ask() {
        if which::which("fish").is_err() {
            return;
        }
        for settings in [
            None,
            Some(r#"{"permissions":{"allow":["Bash(rtk:*)"]}}"#),
            Some(r#"{"permissions":{"allow":["Bash(rtk:*)"],"ask":["Bash(git:*)"]}}"#),
        ] {
            let files: Vec<(&str, &str)> = settings
                .map(|s| vec![(".claude/settings.json", s)])
                .unwrap_or_default();
            let output = claude_hook(FISH_BLOCK, &files);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let v: serde_json::Value =
                serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{stdout}: {e}"));

            assert!(wrapped(&output), "{settings:?}");
            assert_eq!(
                v.pointer("/hookSpecificOutput/permissionDecision")
                    .and_then(serde_json::Value::as_str),
                Some("ask"),
                "a wrap must ask, whatever the host's own rules would say: {settings:?}"
            );
        }

        // An ordinary rewrite is unaffected: an allow rule still auto-allows.
        let allowed = claude_hook(
            "git status",
            &[(
                ".claude/settings.json",
                r#"{"permissions":{"allow":["Bash(git status:*)"]}}"#,
            )],
        );
        let stdout = String::from_utf8_lossy(&allowed.stdout);
        let v: serde_json::Value = serde_json::from_str(stdout.trim()).expect("json");
        assert_eq!(
            v.pointer("/hookSpecificOutput/permissionDecision")
                .and_then(serde_json::Value::as_str),
            Some("allow")
        );
    }

    /// Only a wrap RTK built asks explicitly. An `rtk run --shell fish -c …` the
    /// agent typed itself, which the rules only rewrote around, keeps the
    /// decision it gets without the feature: an allow where the rules allow
    /// every part, and no `permissionDecision` at all where they leave it to
    /// the host. With the wrap off that holds for any spelling. With it on, a
    /// typed form behind a leading line continuation is still asked for: the
    /// rules drop the continuation, so the hook cannot tell it from a wrap.
    #[test]
    fn a_typed_wrap_form_keeps_its_decision() {
        for (enabled, command) in [
            (None, "rtk run --shell fish -c 'echo ok' && git status"),
            (
                Some(true),
                "rtk run --shell fish -c 'echo ok' && git status",
            ),
            // The rules drop a leading line continuation, so the rewritten
            // text starts in the wrap form while the typed one does not.
            (None, "\\\nrtk run --shell fish -c 'echo ok' && git status"),
            // Leading whitespace is not a different command.
            (
                Some(true),
                "  rtk run --shell fish -c 'echo ok' && git status",
            ),
        ] {
            for (allow, expected) in [
                (r#"["Bash(rtk:*)","Bash(git:*)"]"#, Some("allow")),
                (r#"["Bash(rtk:*)"]"#, None),
            ] {
                let settings = format!(r#"{{"permissions":{{"allow":{allow}}}}}"#);
                let home = home_with_wrap(&[(".claude/settings.json", settings.as_str())], enabled);
                let output = hook_stdin("claude", &claude_payload(command), &home);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let v: serde_json::Value = serde_json::from_str(stdout.trim())
                    .unwrap_or_else(|e| panic!("{command:?}: {stdout}: {e}"));

                assert_eq!(
                    v.pointer("/hookSpecificOutput/updatedInput/command")
                        .and_then(serde_json::Value::as_str),
                    Some("rtk run --shell fish -c 'echo ok' && rtk git status"),
                    "control: the rules rewrite around the typed form ({enabled:?}, {allow}, {command:?})"
                );
                assert_eq!(
                    v.pointer("/hookSpecificOutput/permissionDecision")
                        .and_then(serde_json::Value::as_str),
                    expected,
                    "{enabled:?}, {allow}, {command:?}"
                );
            }
        }
    }

    /// With the wrap off, a fish script costs the hook and `rtk hook check`
    /// nothing they did not cost before: the settings are walked as often as
    /// for any other command, so a file RTK cannot parse warns as often too.
    #[test]
    fn a_fish_script_reads_the_settings_no_more_often_when_the_wrap_is_off() {
        const FISH: &str = "git diff HEAD~3 HEAD; and true";
        let count = |output: Output| {
            String::from_utf8_lossy(&output.stderr)
                .matches("failed to parse permissions")
                .count()
        };
        let hook = |home: &tempfile::TempDir, command: &str| {
            count(hook_stdin("claude", &claude_payload(command), home))
        };
        let check =
            |home: &tempfile::TempDir, command: &str| count(hook_check("claude", command, home));

        let off = home_with_wrap(&[(".claude/settings.json", "{ not json")], None);
        let plain = hook(&off, "git status");
        assert!(plain > 0, "control: the broken file must warn at all");
        assert_eq!(hook(&off, FISH), plain, "rtk hook claude");
        assert_eq!(
            check(&off, FISH),
            check(&off, "git status"),
            "rtk hook check"
        );
        assert_eq!(check(&off, "git status"), plain, "rtk hook check");

        // The control: with the wrap on, the same script is attested, which
        // walks the settings again, so the count is what tells them apart.
        let on = home_with_wrap(&[(".claude/settings.json", "{ not json")], Some(true));
        assert!(
            hook(&on, FISH) > plain,
            "control: {FISH} classifies as fish"
        );
        assert!(
            check(&on, FISH) > check(&on, "git status"),
            "control: rtk hook check attests only what the hook attests"
        );
    }

    #[test]
    fn the_hook_defers_a_posix_multiline_script() {
        let output = claude_hook("if [ -d src ]; then\n  git status\nfi", &[]);

        assert!(output.stdout.trim_ascii().is_empty());
    }

    #[test]
    fn the_hook_defers_a_fish_script_with_a_divergent_backslash() {
        // Classifies fish (`; and` marker) but contains `\\`, which fish
        // single-quotes would collapse — the wrapper vetoes it, so the hook
        // defers instead of emitting a wrap that would corrupt the script.
        let output = claude_hook("echo '\\\\'; and echo ok", &[]);

        assert!(
            output.stdout.trim_ascii().is_empty(),
            "divergent backslash must defer: {}",
            String::from_utf8_lossy(&output.stdout)
        );
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
            // Through `native_command`: `fish -c` sources the user's
            // `config.fish`, and anything it prints would land on the stdout
            // compared here byte for byte.
            let output =
                crate::common::native_command(shell_path.to_str().expect("shell path is UTF-8"))
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

    /// Both branches assert: with `fish` the wrapped script executes and
    /// propagates its exit code, without it `rtk run --shell fish` reports
    /// exit 127 rather than appear to have run. This is the one case that runs
    /// a wrapped script instead of only emitting one, and CI installs `fish` on
    /// Linux and macOS.
    #[test]
    fn wrapped_multiline_fish_script_executes_and_propagates_exit_code() {
        let script = "if test -d /\n  echo yes\nelse\n  echo no\nend";
        let Ok(fish) = which::which("fish") else {
            let output = rtk()
                .args(["run", "--shell", "fish", "-c", script])
                .output()
                .expect("run rtk run");
            assert_eq!(
                output.status.code(),
                Some(127),
                "an unresolvable shell must report, not run"
            );
            assert!(output.stdout.is_empty());
            return;
        };

        let direct = crate::common::native_command(fish.to_str().expect("fish path is UTF-8"))
            .args(["-c", script])
            .output()
            .expect("run fish directly");
        let wrapped = rtk()
            .args(["run", "--shell", "fish", "-c", script])
            .output()
            .expect("run rtk run");

        assert!(wrapped.status.success());
        assert!(
            !direct.stdout.is_empty(),
            "the compared output must be real"
        );
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
