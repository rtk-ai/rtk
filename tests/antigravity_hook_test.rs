use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Output, Stdio};

mod common;

fn run_antigravity_hook(command: &str, home: &Path, audit: bool) -> Output {
    let payload = json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": command
            }
        },
        "stepIdx": 1,
        "conversationId": "test-conversation-id"
    })
    .to_string();

    run_antigravity_payload(&payload, home, audit)
}

fn run_antigravity_payload(payload: &str, home: &Path, audit: bool) -> Output {
    let mut child = common::rtk_command()
        .args(["hook", "antigravity"])
        .env("HOME", home)
        .env("RTK_HOOK_AUDIT", if audit { "1" } else { "0" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn rtk hook antigravity");

    child
        .stdin
        .take()
        .expect("missing hook stdin")
        .write_all(payload.as_bytes())
        .expect("failed to write hook payload");

    child.wait_with_output().expect("hook process failed")
}

#[test]
fn antigravity_hook_rewrites_git_status() {
    let home = tempfile::tempdir().unwrap();
    let output = run_antigravity_hook("git status", home.path(), false);

    assert!(output.status.success(), "hook process failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid json: {e}\nstdout: {stdout}"));

    assert_eq!(v["decision"], "allow");
    assert_eq!(v["overwrite"]["CommandLine"], "rtk git status");
}

#[test]
fn antigravity_hook_defers_unattestable_shell_constructs() {
    let home = tempfile::tempdir().unwrap();

    for command in [
        "git status $(whoami)",
        "git status `whoami`",
        "git status <(whoami)",
        "git status > /tmp/status.txt",
    ] {
        let output = run_antigravity_hook(command, home.path(), false);
        assert!(output.status.success(), "hook failed for `{command}`");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("invalid json: {e}\nstdout: {stdout}"));

        assert_eq!(v["decision"], "allow");
        assert!(
            v.get("overwrite").is_none(),
            "unattestable command must defer without overwrite: `{command}` produced `{stdout}`"
        );
    }
}

#[test]
fn antigravity_hook_rewrites_bom_prefixed_payloads() {
    let home = tempfile::tempdir().unwrap();
    let payload = json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": "git status"
            }
        }
    })
    .to_string();

    let plain = run_antigravity_payload(&payload, home.path(), false);
    assert!(plain.status.success());
    assert!(!plain.stdout.is_empty());

    for prefix in ["\u{feff}", "\u{feff}\u{feff}"] {
        let output = run_antigravity_payload(&format!("{prefix}{payload}"), home.path(), false);
        assert!(output.status.success());
        assert_eq!(output.stdout, plain.stdout);
        assert!(output.stderr.is_empty());
    }
}

#[test]
// dirs::home_dir uses the Windows Known Folder API, so HOME cannot isolate this log.
#[cfg(unix)]
fn antigravity_hook_records_successful_rewrite_in_audit_log() {
    let home = tempfile::tempdir().unwrap();
    let output = run_antigravity_hook("git status", home.path(), true);

    assert!(output.status.success());
    assert!(
        !output.stdout.is_empty(),
        "expected an Antigravity rewrite response"
    );

    let audit_path = home.path().join(".local/share/rtk/hook-audit.log");
    let audit = std::fs::read_to_string(&audit_path)
        .unwrap_or_else(|error| panic!("missing audit log at {}: {error}", audit_path.display()));
    assert!(
        audit.contains(" | rewrite | git status | rtk git status"),
        "unexpected audit log: {audit}"
    );
}

#[test]
fn antigravity_hook_fails_open_on_corrupt_payload() {
    let home = tempfile::tempdir().unwrap();

    for bad in ["", "   ", "{invalid json"] {
        let output = run_antigravity_payload(bad, home.path(), false);
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("invalid json: {e}\nstdout: {stdout}"));
        assert_eq!(v["decision"], "allow");
        assert!(v.get("overwrite").is_none());
    }
}

#[test]
fn antigravity_hook_ignores_non_command_tools() {
    let home = tempfile::tempdir().unwrap();
    let payload = json!({
        "toolCall": {
            "name": "view_file",
            "args": {
                "AbsolutePath": "/path/to/file.rs"
            }
        }
    })
    .to_string();

    let output = run_antigravity_payload(&payload, home.path(), false);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid json: {e}\nstdout: {stdout}"));
    assert_eq!(v["decision"], "allow");
    assert!(v.get("overwrite").is_none());
}

/// Runs `rtk init` in `project` with an isolated HOME and closed stdin.
fn run_init(project: &Path, home: &Path, args: &[&str]) -> Output {
    common::rtk_command()
        .arg("init")
        .args(args)
        .current_dir(project)
        .env("HOME", home)
        .stdin(Stdio::null())
        .output()
        .expect("failed to spawn rtk init")
}

#[test]
fn antigravity_init_reports_rules_and_restart() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Rules:  rules/AGENTS.md"), "{stdout}");
    assert!(stdout.contains("Restart Antigravity"), "{stdout}");
}

#[test]
fn antigravity_uninstall_with_nothing_installed_says_so() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    for args in [
        &["--agent", "antigravity", "--uninstall"][..],
        &["--agent", "antigravity", "--uninstall", "--dry-run"][..],
    ] {
        let output = run_init(project.path(), home.path(), args);
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("nothing to remove"), "{args:?}: {stdout}");
        assert!(!stdout.contains("RTK uninstalled"), "{args:?}: {stdout}");
    }
}

#[test]
fn antigravity_dry_runs_change_nothing() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let plugin_dir = project.path().join(".agents/plugins/rtk");

    let output = run_init(
        project.path(),
        home.path(),
        &["--agent", "antigravity", "--dry-run"],
    );
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("[dry-run] Nothing written."));
    assert!(!plugin_dir.exists(), "install dry run must write nothing");

    let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
    assert!(output.status.success());

    let output = run_init(
        project.path(),
        home.path(),
        &["--agent", "antigravity", "--uninstall", "--dry-run"],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[dry-run] would uninstall RTK for Google Antigravity:"),
        "{stdout}"
    );
    assert!(stdout.contains("[dry-run] Nothing written."), "{stdout}");
    assert!(
        plugin_dir.join("rules/AGENTS.md").is_file(),
        "uninstall dry run must leave the plugin in place"
    );
}

const LEGACY_V036: &str = r#"# RTK - Rust Token Killer (Google Antigravity)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk` to minimize token consumption.

Examples:

```bash
rtk git status
rtk cargo test
rtk ls src/
rtk grep "pattern" src/
rtk find "*.rs" .
rtk docker ps
rtk gh pr list
```

## Meta Commands

```bash
rtk gain              # Show token savings
rtk gain --history    # Command history with savings
rtk discover          # Find missed RTK opportunities
rtk proxy <cmd>       # Run raw (no filtering, for debugging)
```

## Why

RTK filters and compresses command output before it reaches the LLM context, saving 60-90% tokens on common operations. Always use `rtk <cmd>` instead of raw commands.
"#;

const LEGACY_V044: &str = r#"# RTK - Rust Token Killer (Google Antigravity)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk` to minimize token consumption.

Examples:

```bash
rtk git status
rtk cargo test
rtk ls src/
rtk grep "pattern" src/
rtk find "*.rs" .
rtk docker ps
rtk gh pr list
```

## Meta Commands

```bash
rtk gain              # Show token savings
rtk gain --history    # Command history with savings
rtk discover          # Find missed RTK opportunities
rtk proxy <cmd>       # Run raw (no filtering, for debugging)
```

## Why

RTK filters and compresses command output before it reaches the LLM context, cutting up to 90% of the bash output on common operations. Always use `rtk <cmd>` instead of raw commands.
"#;

const LEGACY_V049: &str = r#"# RTK

Prefix every shell command with `rtk`: `rtk git status`, `rtk cargo test`,
`rtk npm run build`, `rtk ls src/`. Keep the prefix inside chains:
`rtk git add . && rtk git commit -m "msg"`. Commands RTK has no filter for
run as-is, so the prefix is always safe.

# Command output

Command output here is condensed to save tokens, keeping every signal and
dropping costly noise. Treat it as the complete result: run commands
normally, and batch related commands into one call to avoid extra turns.
Truncated results state their recovery path in their own output. Re-run a
command as `rtk proxy <cmd>` only when its result is unusable: empty when
output was clearly expected, contradicting its exit code, or garbled.

## About RTK

RTK (Rust Token Killer) is a CLI proxy that filters command output to save
tokens; behavior and exit code are unchanged.

- `rtk gain` / `rtk gain --history` — token savings, overall and per command.
- `rtk proxy <cmd>` — run a command unfiltered, still tracked.
- `RTK_DISABLED=1 <cmd>` — skip RTK for one command.
- `rtk discover` — find past commands RTK could have condensed.
"#;

const LEGACY_TEXTS: [&str; 3] = [LEGACY_V036, LEGACY_V044, LEGACY_V049];
const LEGACY_RULES_PATH: &str = ".agents/rules/antigravity-rtk-rules.md";

#[test]
fn antigravity_init_removes_exact_match_legacy_rules() {
    for legacy_text in LEGACY_TEXTS {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let legacy_file = project.path().join(LEGACY_RULES_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        fs::write(&legacy_file, legacy_text).unwrap();

        let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("  Legacy: removed ") && stdout.contains(LEGACY_RULES_PATH),
            "{stdout}"
        );
        assert!(
            !stdout.contains("awareness text from"),
            "exact match removal must not print suffix-strip message: {stdout}"
        );
        assert!(!legacy_file.exists(), "Legacy file must be removed");
        assert!(
            project
                .path()
                .join(".agents/plugins/rtk/plugin.json")
                .is_file(),
            "Plugin must be installed"
        );
    }
}

#[test]
fn antigravity_init_removes_crlf_legacy_rules() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let legacy_file = project.path().join(LEGACY_RULES_PATH);
    fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
    let crlf_text = LEGACY_V036.replace('\n', "\r\n");
    fs::write(&legacy_file, crlf_text).unwrap();

    let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
    assert!(output.status.success());
    assert!(!legacy_file.exists(), "CRLF legacy file must be removed");
}

#[test]
fn antigravity_init_strips_suffix_preserving_user_content() {
    let user_content = "# Team rules\n\nAlways write tests.";
    for legacy_text in LEGACY_TEXTS {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let legacy_file = project.path().join(LEGACY_RULES_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        fs::write(&legacy_file, format!("{user_content}\n\n{legacy_text}")).unwrap();

        let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("  Legacy: removed awareness text from ")
                && stdout.contains(LEGACY_RULES_PATH),
            "{stdout}"
        );
        assert!(legacy_file.exists(), "File with user content must remain");
        let remaining = fs::read_to_string(&legacy_file).unwrap();
        assert_eq!(remaining, user_content);
    }
}

#[test]
fn antigravity_init_strips_crlf_suffix_preserving_crlf_user_content() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let legacy_file = project.path().join(LEGACY_RULES_PATH);
    fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
    let user_content = "# Team rules\r\n\r\nAlways write tests.";
    let crlf_legacy = LEGACY_V036.replace('\n', "\r\n");
    fs::write(&legacy_file, format!("{user_content}\r\n\r\n{crlf_legacy}")).unwrap();

    let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
    assert!(output.status.success());
    let remaining = fs::read_to_string(&legacy_file).unwrap();
    assert_eq!(remaining, user_content);
}

#[test]
fn antigravity_init_strips_windows_crlf_suffix_preserving_crlf_user_content() {
    let user_content = "# Team rules\r\n\r\nAlways write tests.";
    for legacy_text in LEGACY_TEXTS {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let legacy_file = project.path().join(LEGACY_RULES_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        let crlf_legacy = legacy_text.replace('\n', "\r\n");
        // Windows pre-plugin releases appended with "\n\n" separator followed by CRLF text
        fs::write(&legacy_file, format!("{user_content}\n\n{crlf_legacy}")).unwrap();

        let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("  Legacy: removed awareness text from ")
                && stdout.contains(LEGACY_RULES_PATH),
            "{stdout}"
        );
        let remaining = fs::read_to_string(&legacy_file).unwrap();
        assert_eq!(remaining, user_content);

        // Also test uninstall
        fs::write(&legacy_file, format!("{user_content}\n\n{crlf_legacy}")).unwrap();
        let uninst_out = run_init(
            project.path(),
            home.path(),
            &["--agent", "antigravity", "--uninstall"],
        );
        assert!(uninst_out.status.success());
        let uninst_remaining = fs::read_to_string(&legacy_file).unwrap();
        assert_eq!(uninst_remaining, user_content);
    }
}

#[test]
fn antigravity_init_leaves_single_newline_separator_untouched() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let legacy_file = project.path().join(LEGACY_RULES_PATH);
    fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
    let single_nl = format!("# Team rules\n\nAlways write tests.\n{LEGACY_V036}");
    fs::write(&legacy_file, &single_nl).unwrap();

    let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("Legacy:"), "{stdout}");
    assert_eq!(fs::read_to_string(&legacy_file).unwrap(), single_nl);
}

#[test]
fn antigravity_init_leaves_foreign_rules_untouched() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let legacy_file = project.path().join(LEGACY_RULES_PATH);
    fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
    let foreign = "Prefer RTK Query for data fetching; do not add axios.";
    fs::write(&legacy_file, foreign).unwrap();

    let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("Legacy:"), "{stdout}");
    assert_eq!(fs::read_to_string(&legacy_file).unwrap(), foreign);
}

#[test]
fn antigravity_uninstall_removes_legacy_rules() {
    let user_content = "# Team rules\n\nAlways write tests.";
    for legacy_text in LEGACY_TEXTS {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let legacy_file = project.path().join(LEGACY_RULES_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        fs::write(&legacy_file, legacy_text).unwrap();

        let output = run_init(
            project.path(),
            home.path(),
            &["--agent", "antigravity", "--uninstall"],
        );
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("RTK uninstalled for Google Antigravity:"),
            "{stdout}"
        );
        assert!(stdout.contains("legacy rules file:"), "{stdout}");
        assert!(!legacy_file.exists());

        fs::write(&legacy_file, format!("{user_content}\n\n{legacy_text}")).unwrap();
        let output = run_init(
            project.path(),
            home.path(),
            &["--agent", "antigravity", "--uninstall"],
        );
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("legacy awareness text in:"), "{stdout}");
        assert_eq!(fs::read_to_string(&legacy_file).unwrap(), user_content);
    }
}

#[test]
// dirs::home_dir uses the Windows Known Folder API, so HOME cannot isolate ~/.gemini/config on Windows.
#[cfg(unix)]
fn antigravity_global_uninstall_cleans_workspace_legacy_rules() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let legacy_file = project.path().join(LEGACY_RULES_PATH);
    fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
    fs::write(&legacy_file, LEGACY_V036).unwrap();

    let output = run_init(
        project.path(),
        home.path(),
        &["-g", "--agent", "antigravity", "--uninstall"],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("RTK uninstalled for Google Antigravity:"),
        "{stdout}"
    );
    assert!(stdout.contains("legacy rules file:"), "{stdout}");
    assert!(!legacy_file.exists());
}

#[test]
fn antigravity_dry_run_leaves_legacy_rules_in_place() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let legacy_file = project.path().join(LEGACY_RULES_PATH);
    fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
    fs::write(&legacy_file, LEGACY_V036).unwrap();

    let output = run_init(
        project.path(),
        home.path(),
        &["--agent", "antigravity", "--dry-run"],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[dry-run] would remove legacy rules file:"),
        "{stdout}"
    );
    assert!(stdout.contains("[dry-run] Nothing written."), "{stdout}");
    assert!(legacy_file.exists(), "Dry-run must not remove legacy file");

    let output = run_init(
        project.path(),
        home.path(),
        &["--agent", "antigravity", "--uninstall", "--dry-run"],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[dry-run] would uninstall RTK for Google Antigravity:"),
        "{stdout}"
    );
    assert!(stdout.contains("legacy rules file:"), "{stdout}");
    assert!(
        !stdout.contains("[dry-run] would remove legacy rules file:"),
        "uninstall dry-run must not list legacy file twice: {stdout}"
    );
    assert!(stdout.contains("[dry-run] Nothing written."), "{stdout}");
    assert!(
        legacy_file.exists(),
        "Uninstall dry-run must not remove legacy file"
    );

    // Pin mutation: dry-run on an appended file must not rewrite disk contents
    let user_content = "# Team rules\n\nAlways write tests.";
    let appended_content = format!("{user_content}\n\n{LEGACY_V036}");
    fs::write(&legacy_file, &appended_content).unwrap();
    let output = run_init(
        project.path(),
        home.path(),
        &["--agent", "antigravity", "--dry-run"],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[dry-run] would remove legacy awareness text from"),
        "{stdout}"
    );
    assert_eq!(
        fs::read_to_string(&legacy_file).unwrap(),
        appended_content,
        "Dry run must not rewrite appended file"
    );
}

#[test]
#[cfg(unix)] // dirs::home_dir uses Windows Known Folder API, so HOME cannot isolate ~/.gemini/config on Windows.
fn antigravity_global_init_cleans_workspace_legacy_rules() {
    let user_content = "# Team rules\n\nAlways write tests.";
    for legacy_text in LEGACY_TEXTS {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let legacy_file = project.path().join(LEGACY_RULES_PATH);
        fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        fs::write(&legacy_file, legacy_text).unwrap();

        let output = run_init(
            project.path(),
            home.path(),
            &["-g", "--agent", "antigravity"],
        );
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("  Legacy: removed ") && stdout.contains(LEGACY_RULES_PATH),
            "{stdout}"
        );
        assert!(
            !legacy_file.exists(),
            "Global init in workspace must remove legacy rules"
        );
        assert!(
            home.path()
                .join(".gemini/config/plugins/rtk/plugin.json")
                .is_file(),
            "Global plugin must be installed"
        );

        fs::write(&legacy_file, format!("{user_content}\n\n{legacy_text}")).unwrap();
        let output = run_init(
            project.path(),
            home.path(),
            &["-g", "--agent", "antigravity"],
        );
        assert!(output.status.success());
        assert_eq!(fs::read_to_string(&legacy_file).unwrap(), user_content);
    }
}

#[test]
#[cfg(unix)]
fn antigravity_readonly_legacy_rules_warns_and_succeeds() {
    use std::os::unix::fs::PermissionsExt;

    // 1. Test Install under read-only rules directory
    {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let legacy_file = project.path().join(LEGACY_RULES_PATH);
        let rules_dir = legacy_file.parent().unwrap();
        fs::create_dir_all(rules_dir).unwrap();
        fs::write(&legacy_file, LEGACY_V036).unwrap();

        let mut perms = fs::metadata(rules_dir).expect("meta").permissions();
        perms.set_mode(0o555);
        fs::set_permissions(rules_dir, perms.clone()).expect("chmod");

        // Probe write permission first to detect if we're running as root
        let probe = rules_dir.join(".rtk-perm-probe");
        if fs::write(&probe, b"x").is_ok() {
            let _ = fs::remove_file(&probe);
            perms.set_mode(0o755);
            let _ = fs::set_permissions(rules_dir, perms);
            eprintln!("skipped: filesystem does not enforce write permissions (running as root?)");
            return;
        }

        let install_out = run_init(project.path(), home.path(), &["--agent", "antigravity"]);

        perms.set_mode(0o755);
        fs::set_permissions(rules_dir, perms).expect("chmod back");

        assert!(
            install_out.status.success(),
            "install must succeed even when legacy cleanup fails"
        );
        assert!(
            project
                .path()
                .join(".agents/plugins/rtk/plugin.json")
                .is_file(),
            "plugin must be installed"
        );
        let install_stdout = String::from_utf8_lossy(&install_out.stdout);
        assert!(
            !install_stdout.contains("Legacy:"),
            "failed removal must not report Legacy on stdout: {install_stdout}"
        );
        let install_stderr = String::from_utf8_lossy(&install_out.stderr);
        assert!(
            install_stderr.contains("[warn]")
                && install_stderr.contains("antigravity-rtk-rules.md"),
            "expected warning naming legacy file, got: {install_stderr}"
        );
    }

    // 2. Test Uninstall under read-only rules directory
    {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let legacy_file = project.path().join(LEGACY_RULES_PATH);
        let rules_dir = legacy_file.parent().unwrap();
        fs::create_dir_all(rules_dir).unwrap();
        fs::write(&legacy_file, LEGACY_V036).unwrap();

        // First install cleanly so there is an existing plugin directory to uninstall
        let pre_install = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
        assert!(pre_install.status.success());
        // Re-write legacy rules file in case pre_install cleaned it
        fs::write(&legacy_file, LEGACY_V036).unwrap();

        let mut perms = fs::metadata(rules_dir).expect("meta").permissions();
        perms.set_mode(0o555);
        fs::set_permissions(rules_dir, perms.clone()).expect("chmod");

        let uninstall_out = run_init(
            project.path(),
            home.path(),
            &["--agent", "antigravity", "--uninstall"],
        );

        perms.set_mode(0o755);
        fs::set_permissions(rules_dir, perms).expect("chmod back");

        assert!(
            uninstall_out.status.success(),
            "uninstall must succeed even when legacy cleanup fails"
        );
        assert!(
            !project.path().join(".agents/plugins/rtk").exists(),
            "plugin directory must be removed"
        );
        let uninstall_stdout = String::from_utf8_lossy(&uninstall_out.stdout);
        assert!(
            uninstall_stdout.contains("RTK uninstalled for Google Antigravity:"),
            "{uninstall_stdout}"
        );
        let uninstall_stderr = String::from_utf8_lossy(&uninstall_out.stderr);
        assert!(
            uninstall_stderr.contains("[warn]")
                && uninstall_stderr.contains("antigravity-rtk-rules.md"),
            "expected warning naming legacy file, got: {uninstall_stderr}"
        );
    }
}

#[test]
#[cfg(unix)]
fn antigravity_readonly_appended_rules_warns_and_succeeds() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let legacy_file = project.path().join(LEGACY_RULES_PATH);
    let rules_dir = legacy_file.parent().unwrap();
    fs::create_dir_all(rules_dir).unwrap();
    let user_content = "# Team rules\n\nAlways write tests.";
    let full_content = format!("{user_content}\n\n{LEGACY_V036}");
    fs::write(&legacy_file, &full_content).unwrap();

    let mut perms = fs::metadata(rules_dir).expect("meta").permissions();
    perms.set_mode(0o555);
    fs::set_permissions(rules_dir, perms.clone()).expect("chmod");

    let probe = rules_dir.join(".rtk-perm-probe");
    if fs::write(&probe, b"x").is_ok() {
        let _ = fs::remove_file(&probe);
        perms.set_mode(0o755);
        let _ = fs::set_permissions(rules_dir, perms);
        return;
    }

    let install_out = run_init(project.path(), home.path(), &["--agent", "antigravity"]);

    perms.set_mode(0o755);
    fs::set_permissions(rules_dir, perms).expect("chmod back");

    assert!(install_out.status.success());
    let install_stdout = String::from_utf8_lossy(&install_out.stdout);
    assert!(
        !install_stdout.contains("Legacy:"),
        "failed write must not report Legacy on stdout: {install_stdout}"
    );
    let install_stderr = String::from_utf8_lossy(&install_out.stderr);
    assert!(
        install_stderr.contains("[warn]") && install_stderr.contains("antigravity-rtk-rules.md"),
        "{install_stderr}"
    );
    assert_eq!(
        fs::read_to_string(&legacy_file).unwrap(),
        full_content,
        "failed write must leave file untouched"
    );
}

#[test]
fn antigravity_init_ordering_leaves_legacy_rules_on_install_failure() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let legacy_file = project.path().join(LEGACY_RULES_PATH);
    fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
    fs::write(&legacy_file, LEGACY_V036).unwrap();

    // Create .agents/plugins as a regular file so fs::create_dir_all(&rules_dir) fails on install
    let plugins_path = project.path().join(".agents/plugins");
    fs::write(&plugins_path, "not a directory").unwrap();

    let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
    assert!(
        !output.status.success(),
        "install must fail when plugin directory cannot be created"
    );
    assert!(
        legacy_file.exists(),
        "legacy file must be untouched when plugin installation fails"
    );
    assert_eq!(
        fs::read_to_string(&legacy_file).unwrap(),
        LEGACY_V036,
        "legacy file content must be unchanged"
    );
}

#[test]
fn antigravity_init_ignores_non_utf8_legacy_file() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let legacy_file = project.path().join(LEGACY_RULES_PATH);
    fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
    let invalid_bytes = b"\xff\xfe\x00not utf8 content";
    fs::write(&legacy_file, invalid_bytes).unwrap();

    let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("Legacy:"), "{stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("[warn]"),
        "non-UTF-8 file must be ignored silently: {stderr}"
    );
    assert_eq!(
        fs::read(&legacy_file).unwrap(),
        invalid_bytes,
        "non-UTF-8 file must remain untouched"
    );
}

#[test]
fn antigravity_init_skips_legacy_path_that_is_not_a_regular_file() {
    for name in ["directory", "parent is a file"] {
        for args in [
            &["--agent", "antigravity"][..],
            &["--agent", "antigravity", "--uninstall"][..],
        ] {
            let home = tempfile::tempdir().unwrap();
            let project = tempfile::tempdir().unwrap();
            if name == "directory" {
                fs::create_dir_all(project.path().join(LEGACY_RULES_PATH)).unwrap();
            } else {
                fs::create_dir_all(project.path().join(".agents")).unwrap();
                fs::write(project.path().join(".agents/rules"), "not a directory").unwrap();
            }

            let output = run_init(project.path(), home.path(), args);
            assert!(output.status.success(), "{name} {args:?}");
            let stdout = String::from_utf8_lossy(&output.stdout);
            for marker in ["Legacy:", "legacy rules file", "legacy awareness text"] {
                assert!(!stdout.contains(marker), "{name} {args:?}: {stdout}");
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!stderr.contains("[warn]"), "{name} {args:?}: {stderr}");
        }
    }
}
