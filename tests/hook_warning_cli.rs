//! Black-box coverage of hook warnings with a private marker per child process.

use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

const MISSING: &str = "[rtk] /!\\ No hook installed";
const OUTDATED: &str = "[rtk] /!\\ Hook outdated";

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

struct Sandbox {
    root: TempDir,
    home: PathBuf,
    claude: PathBuf,
    project: PathBuf,
}

impl Sandbox {
    fn new(git_repo: bool) -> Self {
        let root = TempDir::new().expect("tempdir");
        let home = root.path().join("home");
        let claude = root.path().join("claude");
        let project = root.path().join("project");
        for path in [&home, &claude, &project] {
            std::fs::create_dir_all(path).expect("create sandbox path");
        }
        if git_repo {
            let status = Command::new("git")
                .args(["init", "-q"])
                .current_dir(&project)
                .status()
                .expect("initialize git repository");
            assert!(status.success(), "git init failed: {status}");
        }
        Self {
            root,
            home,
            claude,
            project,
        }
    }

    fn marker(&self, name: &str) -> PathBuf {
        self.root.path().join("markers").join(name)
    }

    fn command(&self, marker: &str) -> Command {
        let marker = self.marker(marker);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
        cmd.args(["git", "status"])
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("CLAUDE_CONFIG_DIR", &self.claude)
            .env("CODEX_HOME", self.root.path().join("codex"))
            .env("HERMES_HOME", self.root.path().join("hermes"))
            .env(
                "FACTORY_HOME_OVERRIDE",
                self.root.path().join("factory-home"),
            )
            .env("COPILOT_HOME", self.root.path().join("copilot"))
            .env("RTK_HOOK_WARN_MARKER", marker)
            .env("RTK_HOOK_WARN_HOME", &self.home)
            .env("RTK_TELEMETRY_DISABLED", "1")
            .env("RTK_DB_PATH", self.root.path().join("tracking.db"))
            .env("RTK_TEE_DIR", self.root.path().join("tee"))
            .env("RTK_RECALL_DB", self.root.path().join("recall.db"));
        for name in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_SYSTEM",
        ] {
            cmd.env_remove(name);
        }
        cmd.env_remove("PI_CODING_AGENT_DIR");
        cmd
    }

    fn run(&self, marker: &str) -> Output {
        let out = self.command(marker).output().expect("spawn rtk");
        Output {
            code: out.status.code().expect("rtk exit code"),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn install_opencode(&self) {
        let path = self.home.join(".config/opencode/plugins/rtk.ts");
        std::fs::create_dir_all(path.parent().expect("OpenCode plugin parent"))
            .expect("mkdir plugin");
        std::fs::write(path, include_str!("../hooks/opencode/rtk.ts"))
            .expect("write official OpenCode plugin");
    }

    fn install_pi(&self) {
        let path = self.project.join(".pi/extensions/rtk.ts");
        std::fs::create_dir_all(path.parent().expect("Pi extension parent"))
            .expect("mkdir extension");
        std::fs::write(path, include_str!("../hooks/pi/rtk.ts"))
            .expect("write official Pi extension");
    }
}

fn warning_free(stderr: &str) -> &str {
    stderr
        .strip_prefix(MISSING)
        .and_then(|rest| rest.split_once('\n').map(|(_, rest)| rest))
        .unwrap_or(stderr)
}

#[test]
fn missing_and_opencode_controls_execute_the_warning_path() {
    let sb = Sandbox::new(true);
    let missing = sb.run("missing");
    assert_eq!(missing.code, 0, "warning must not block git status");
    assert!(
        missing.stderr.contains(MISSING),
        "stderr: {}",
        missing.stderr
    );
    assert!(
        sb.marker("missing").exists(),
        "warning marker was not isolated"
    );

    sb.install_opencode();
    let installed = sb.run("opencode");
    assert_eq!(
        (installed.code, installed.stdout),
        (missing.code, missing.stdout)
    );
    assert!(
        !installed.stderr.contains(MISSING),
        "stderr: {}",
        installed.stderr
    );
}

#[test]
fn official_pi_override_suppresses_the_missing_warning() {
    let sb = Sandbox::new(true);
    sb.install_pi();
    let out = sb.run("pi");
    assert_eq!(out.code, 0, "Pi integration must not block git status");
    assert!(!out.stderr.contains(MISSING), "stderr: {}", out.stderr);
}

#[test]
fn outdated_claude_hook_remains_visible_with_an_alternative() {
    let sb = Sandbox::new(true);
    sb.install_opencode();
    let hook = sb.claude.join("hooks/rtk-rewrite.sh");
    std::fs::create_dir_all(hook.parent().expect("hook parent")).expect("mkdir hook");
    std::fs::write(hook, "# rtk-hook-version: 2\n").expect("write outdated hook");
    let out = sb.run("outdated");
    assert_eq!(out.code, 0);
    assert!(out.stderr.contains(OUTDATED), "stderr: {}", out.stderr);
}

#[test]
fn warning_rate_limit_uses_the_private_marker() {
    let sb = Sandbox::new(true);
    assert!(sb.run("rate").stderr.contains(MISSING));
    assert!(!sb.run("rate").stderr.contains(MISSING));
}

#[test]
fn failing_git_exit_and_stderr_survive_an_alternative_integration() {
    let sb = Sandbox::new(false);
    let direct = Command::new("git")
        .args(["status"])
        .current_dir(&sb.project)
        .output()
        .expect("spawn git");
    let missing = sb.run("error-missing");
    sb.install_opencode();
    let installed = sb.run("error-opencode");
    let direct_code = direct.status.code().expect("git exit code");
    assert_ne!(direct_code, 0, "the direct Git control must fail");
    assert_eq!((missing.code, installed.code), (direct_code, direct_code));
    assert_eq!(warning_free(&missing.stderr), installed.stderr);
    assert!(!installed.stderr.is_empty(), "Git failure stderr was lost");
}
