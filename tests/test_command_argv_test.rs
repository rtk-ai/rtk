#[cfg(target_os = "linux")]
use std::path::Path;
use std::process::Command;

fn rtk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

#[cfg(target_os = "linux")]
fn shell_quote_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "linux")]
fn shell_quote(path: &Path) -> String {
    shell_quote_text(&path.to_string_lossy())
}

#[cfg(target_os = "linux")]
#[test]
fn bash_lc_grouped_command_preserves_child_cwd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let caller = temp.path().join("caller");
    let child = temp.path().join("child");
    let evidence = temp.path().join("cwd.txt");
    std::fs::create_dir_all(&caller).expect("create caller");
    std::fs::create_dir_all(&child).expect("create child");

    let grouped = format!(
        "cd {} && readlink /proc/$$/cwd > {}",
        shell_quote(&child),
        shell_quote(&evidence)
    );
    let output = rtk()
        .current_dir(&caller)
        .args(["test", "bash", "-lc", &grouped])
        .output()
        .expect("run rtk test");

    assert!(
        output.status.success(),
        "rtk test failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&evidence)
            .expect("read child cwd evidence")
            .trim(),
        child.to_string_lossy(),
        "bash -lc grouped command must execute in the requested child directory"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn bash_lc_preserves_child_cwd_when_path_contains_spaces() {
    let temp = tempfile::tempdir().expect("tempdir");
    let caller = temp.path().join("caller");
    let child = temp.path().join("child with spaces");
    let evidence = temp.path().join("cwd with spaces.txt");
    std::fs::create_dir_all(&caller).expect("create caller");
    std::fs::create_dir_all(&child).expect("create child");

    let grouped = format!(
        "cd {} && readlink /proc/$$/cwd > {}",
        shell_quote(&child),
        shell_quote(&evidence)
    );
    let output = rtk()
        .current_dir(&caller)
        .args(["test", "bash", "-lc", &grouped])
        .output()
        .expect("run rtk test");

    assert!(output.status.success(), "rtk test must succeed");
    assert_eq!(
        std::fs::read_to_string(&evidence)
            .expect("read child cwd evidence")
            .trim(),
        child.to_string_lossy()
    );
}

#[cfg(unix)]
#[test]
fn literal_argument_with_spaces_stays_one_argument() {
    let literal = "literal argument with spaces";
    let output = rtk()
        .args(["test", "printf", "<%s>\\n", literal])
        .output()
        .expect("run rtk test");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "rtk test must succeed");
    assert!(
        stdout.contains(&format!("<{literal}>")),
        "literal argument boundary was lost: {stdout}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn shell_metacharacters_stay_inside_grouped_bash_lc_argument() {
    let temp = tempfile::tempdir().expect("tempdir");
    let caller = temp.path().join("caller");
    let child = temp.path().join("child");
    let evidence = temp.path().join("metacharacters.txt");
    let literal = "alpha && beta | gamma; delta > epsilon";
    std::fs::create_dir_all(&caller).expect("create caller");
    std::fs::create_dir_all(&child).expect("create child");

    let grouped = format!(
        "cd {} && printf '%s\\n%s\\n' \"$(readlink /proc/$$/cwd)\" {} > {}",
        shell_quote(&child),
        shell_quote_text(literal),
        shell_quote(&evidence)
    );
    let output = rtk()
        .current_dir(&caller)
        .args(["test", "bash", "-lc", &grouped])
        .output()
        .expect("run rtk test");
    let evidence = std::fs::read_to_string(&evidence).expect("read grouped shell evidence");
    let mut lines = evidence.lines();

    assert!(output.status.success(), "rtk test must succeed");
    assert_eq!(lines.next(), Some(child.to_string_lossy().as_ref()));
    assert_eq!(lines.next(), Some(literal));
}

#[cfg(unix)]
#[test]
fn quotes_and_backslashes_stay_literal_argv_bytes() {
    let literal = "single' double\" backslash:\\\\ end";
    let output = rtk()
        .args(["test", "printf", "<%s>\\n", literal])
        .output()
        .expect("run rtk test");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "rtk test must succeed");
    assert!(
        stdout.contains(&format!("<{literal}>")),
        "quoted/backslashed argv was changed: {stdout}"
    );
}

#[cfg(unix)]
#[test]
fn ordinary_non_shell_command_exit_code_is_preserved() {
    let status = rtk()
        .args(["test", "false"])
        .status()
        .expect("run rtk test");

    assert_eq!(status.code(), Some(1));
}

#[test]
fn empty_test_command_is_rejected() {
    let status = rtk().arg("test").status().expect("run empty rtk test");

    assert!(!status.success(), "rtk test without a command must fail");
}

#[cfg(unix)]
#[test]
fn pnpm_dash_c_target_behavior_is_preserved_when_pnpm_is_available() {
    if !Command::new("pnpm")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
    {
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let target = temp.path().join("pnpm target with spaces");
    let evidence = temp.path().join("pnpm-cwd.txt");
    std::fs::create_dir_all(&target).expect("create pnpm target");
    std::fs::write(
        target.join("package.json"),
        r#"{"scripts":{"probe-cwd":"node -e \"require('fs').writeFileSync(process.env.EVIDENCE, process.cwd())\""}}"#,
    )
    .expect("write package.json");

    let output = rtk()
        .args([
            "test",
            "pnpm",
            "-C",
            target.to_str().expect("utf-8 target path"),
            "run",
            "probe-cwd",
        ])
        .env("EVIDENCE", &evidence)
        .output()
        .expect("run rtk test pnpm -C");

    assert!(
        output.status.success(),
        "rtk test pnpm -C failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&evidence).expect("read pnpm cwd evidence"),
        target.to_string_lossy()
    );
}
