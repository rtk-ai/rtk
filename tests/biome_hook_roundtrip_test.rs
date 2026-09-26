//! End-to-end check for rtk-ai/rtk#3117: a Biome invocation must reach the
//! Biome filter through the hook, not only when someone types `rtk lint biome`
//! by hand.
//!
//! The hook rewrite used to peel `biome` off as a strippable prefix, so
//! `biome check src/` became `rtk lint check src/` — and `rtk lint` then tried
//! to run `check` as the linter. This walks the real path: rewrite the command
//! the way the hook does, then run exactly what the rewrite produced and check
//! that the Biome report came back compressed.
//!
//! `biome` is stubbed on PATH with a script that prints a real Biome report,
//! since Biome isn't installed in this environment.

#[cfg(unix)]
#[test]
fn biome_check_reaches_the_biome_filter_through_the_rewrite() {
    use std::process::Command;

    const BIOME_REPORT: &str = r#"src/App.tsx:15:3 lint/suspicious/noDoubleEquals  FIXABLE  ━━━━━━━━━━
  ✖ Use === instead of ==.
    14 │ function check(a, b) {
  > 15 │   if (a == b) {
       │       ^^^^
    16 │     return true;
  ℹ == is only allowed when comparing against null.

src/App.tsx:22:5 lint/style/useConst  FIXABLE  ━━━━━━━━━━━━━━━━━━━━
  ✖ This let declares a variable that is only assigned once.
    21 │
  > 22 │   let count = 0;
       │       ^^^^^
    23 │   console.log(count);
  ℹ Safe fix: Use const instead.

src/utils/helpers.ts:10:1 lint/suspicious/noDoubleEquals ━━━━━━━━━━
  ✖ Use === instead of ==.
     9 │ export function isEqual(a, b) {
  > 10 │   return a == b;
       │          ^^^^
    11 │ }

src/utils/helpers.ts:45:8 lint/correctness/noUnusedVariables ━━━━━━
  ✖ This variable is unused.
    44 │ function processData() {
  > 45 │   const unused = 42;
       │         ^^^^^^
    46 │   return getData();

Checked 34 files in 21ms. No fixes applied.
Found 4 errors.
"#;

    fn shell_quote(path: &std::path::Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let report_path = dir.path().join("report.txt");
    std::fs::write(&report_path, BIOME_REPORT).expect("write report");

    // Biome exits 1 when it finds issues; the stub does the same so the test
    // also pins that rtk hands the linter's exit code back unchanged.
    let stub_path = dir.path().join("biome");
    std::fs::write(
        &stub_path,
        format!("#!/bin/sh\ncat {}\nexit 1\n", shell_quote(&report_path)),
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

    // 1. The hook's rewrite must keep the linter name.
    let rewrite = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["rewrite", "biome check src/"])
        .output()
        .expect("spawn rtk rewrite");
    let rewritten = String::from_utf8_lossy(&rewrite.stdout).trim().to_string();
    assert_eq!(
        rewritten, "rtk lint biome check src/",
        "the rewrite must not drop `biome`, or the filter is unreachable"
    );

    // 2. Running exactly what the rewrite produced must apply the Biome filter.
    let rtk_args: Vec<&str> = rewritten.split_whitespace().skip(1).collect();
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("PATH", &path_with_stub)
        .current_dir(dir.path())
        .args(&rtk_args)
        .output()
        .expect("spawn rtk lint");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        stdout.contains("Biome: 4 issues"),
        "expected the Biome summary, got: {stdout}"
    );
    assert!(
        stdout.contains("lint/suspicious/noDoubleEquals (2x)"),
        "expected rules grouped with counts, got: {stdout}"
    );
    assert!(
        !stdout.contains("Safe fix: Use const instead."),
        "the raw diagnostic bodies should be compressed away, got: {stdout}"
    );
    assert!(
        stdout.len() < BIOME_REPORT.len() / 2,
        "expected real compression: {} bytes out of {}",
        stdout.len(),
        BIOME_REPORT.len()
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "biome's exit code must survive the filter"
    );
}
