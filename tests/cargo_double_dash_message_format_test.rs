//! Regression test for `--message-format` detection in cargo_cmd.rs: everything after the
//! user's `--` belongs to rustc, not to cargo, so `cargo clippy -- --message-format=json`
//! must still take the human filter. The detection ran on the clap-parsed args (whose leading
//! `--` clap strips), so the rustc flag looked exactly like cargo's own.
//!
//! Stubs `cargo` on PATH with a script that records its argv and replays human clippy output.

#[cfg(unix)]
fn run_with_cargo_stub(args: &[&str]) -> (Vec<String>, String) {
    use std::process::Command;

    fn shell_quote(path: &std::path::Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let argv_file = dir.path().join("argv.txt");
    let stub_path = dir.path().join("cargo");

    std::fs::write(
        &stub_path,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > {}
cat <<'EOF'
warning: the loop variable `i` is only used to index `x`
 --> src/main.rs:3:14
  |
3 |     for i in 0..x.len() {{
  |              ^^^^^^^^^^
  |
  = note: `#[warn(clippy::needless_range_loop)]` on by default

warning: `demo` (bin "demo") generated 1 warning
EOF
exit 0
"#,
            shell_quote(&argv_file)
        ),
    )
    .expect("write stub");
    let mut perms = std::fs::metadata(&stub_path)
        .expect("stat stub")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&stub_path, perms).expect("chmod stub");

    let path_with_stub = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("PATH", path_with_stub)
        .current_dir(dir.path())
        .args(args)
        .output()
        .expect("spawn rtk");
    assert!(
        out.status.success(),
        "rtk {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let argv = std::fs::read_to_string(&argv_file)
        .expect("read captured argv")
        .lines()
        .map(str::to_string)
        .collect();
    (argv, String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(unix)]
#[test]
fn clippy_message_format_after_double_dash_keeps_human_filter() {
    let (argv, stdout) = run_with_cargo_stub(&["cargo", "clippy", "--", "--message-format=json"]);

    assert_eq!(
        argv,
        vec!["clippy", "--", "--message-format=json"],
        "the user's -- must survive to cargo"
    );
    assert!(
        stdout.contains("1 warnings") || stdout.contains("1 warning"),
        "a rustc-side --message-format must not switch rtk to the JSON filter, \
         which reads nothing out of human clippy output: {stdout:?}"
    );
}

#[cfg(unix)]
#[test]
fn clippy_message_format_before_double_dash_still_selects_json_filter() {
    let (argv, stdout) = run_with_cargo_stub(&[
        "cargo",
        "clippy",
        "--message-format=json",
        "--",
        "-D",
        "warnings",
    ]);

    assert_eq!(
        argv,
        vec!["clippy", "--message-format=json", "--", "-D", "warnings"],
        "cargo's own flags and the forwarded rustc flags both keep their side of --"
    );
    assert!(
        !stdout.contains("1 warning"),
        "cargo's own --message-format=json must still select the JSON filter, \
         which finds no diagnostics in human output: {stdout:?}"
    );
}
