//! File operands rtk globs itself when the caller left them unexpanded (PowerShell, cmd), and
//! the fallback to the MSYS child's own globbing once the expanded list would not fit on a
//! Windows command line. Each case spawns rtk with no shell in between, so the operands reach it
//! exactly as typed, with Git for Windows' tools first on PATH; skipped when they are absent.
//!
//! `wc -- -l` runs everywhere: it needs only a `wc`.

use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::process::Output;

mod common;

/// Git for Windows' `usr\bin`, where its MSYS `ls`, `wc`, `grep` and `find` live: `git.exe` sits
/// in `<root>/cmd`, `<root>/mingw64/bin` or `<root>/bin`, so walk up to the ancestor holding them.
#[cfg(windows)]
fn msys_bin() -> Option<PathBuf> {
    let git = which::which("git").ok()?;
    let bin = git
        .ancestors()
        .map(|root| root.join("usr").join("bin"))
        .find(|bin| {
            ["ls", "wc", "grep", "find"]
                .iter()
                .all(|tool| bin.join(format!("{tool}.exe")).is_file())
        })?;
    // Only an MSYS build globs its own operands, which the fallback relies on.
    let wc = std::fs::read(bin.join("wc.exe")).ok()?;
    String::from_utf8_lossy(&wc)
        .to_ascii_lowercase()
        .contains("msys-2.0.dll")
        .then_some(bin)
}

/// `rtk args…` in `dir`, with `bin` (if any) first on PATH.
fn rtk(dir: &Path, bin: Option<&Path>, args: &[&str]) -> Output {
    let mut cmd = common::rtk_command();
    if let Some(bin) = bin {
        let mut path = bin.as_os_str().to_os_string();
        path.push(if cfg!(windows) { ";" } else { ":" });
        path.push(std::env::var_os("PATH").unwrap_or_default());
        cmd.env("PATH", path);
    }
    cmd.args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run rtk")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
}

fn describe(out: &Output) -> String {
    format!(
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn write(dir: &Path, name: &str, content: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create fixture directory");
    }
    std::fs::write(path, content).expect("write fixture");
}

/// `rtk wc -- -l` counts the file named `-l`: the `--` clap consumed is restored before wc's
/// grammar reads the arguments, so `-l` is an operand and not the lines flag reading stdin.
#[test]
fn wc_counts_a_file_named_like_a_flag_after_double_dash() {
    #[cfg(windows)]
    let Some(bin) = msys_bin() else {
        eprintln!("skipping: no Git for Windows MSYS tools found");
        return;
    };
    #[cfg(windows)]
    let bin = Some(bin.as_path());
    #[cfg(not(windows))]
    let bin = None;

    let dir = common::tempdir();
    write(dir.path(), "-l", "ab\n");
    let out = rtk(dir.path(), bin, &["wc", "--", "-l"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(stdout(&out).trim(), "1L 1W 3B", "{}", describe(&out));
}

/// A tree whose globs each match something, from PowerShell's point of view: nothing expanded.
#[cfg(windows)]
fn small_tree() -> tempfile::TempDir {
    let dir = common::tempdir();
    write(dir.path(), "a.txt", "a\n");
    write(dir.path(), "c.txt", "c\nc\n");
    write(dir.path(), "b.rs", "x\n");
    write(dir.path(), "sub/x.rs", "x\n");
    write(dir.path(), "sub/d.txt", "d\n");
    dir
}

#[cfg(windows)]
#[test]
fn unexpanded_globs_are_expanded_by_rtk() {
    let Some(bin) = msys_bin() else {
        eprintln!("skipping: no Git for Windows MSYS tools found");
        return;
    };
    let dir = small_tree();
    let run = |args: &[&str]| rtk(dir.path(), Some(&bin), args);

    let out = run(&["wc", "-l", "*.txt"]);
    assert!(out.status.success(), "wc: {}", describe(&out));
    assert_eq!(
        stdout(&out).trim(),
        "1 a.txt\n2 c.txt\nΣ 3",
        "wc: {}",
        describe(&out)
    );

    let out = run(&["grep", "-c", "x", "*.rs"]);
    assert!(out.status.success(), "grep: {}", describe(&out));
    assert_eq!(stdout(&out).trim(), "1", "grep: {}", describe(&out));

    // `./s*` is a starting point, expanded; `-name *.rs` is find's pattern, kept.
    let out = run(&["find", "./s*", "-name", "*.rs"]);
    assert!(out.status.success(), "find: {}", describe(&out));
    let found = stdout(&out);
    assert!(found.contains("x.rs"), "find: {}", describe(&out));
    assert!(!found.contains("b.rs"), "find: {}", describe(&out));

    // `-I *.txt` is ls's ignore pattern, kept; `sub` is the operand.
    let out = run(&["ls", "-I", "*.txt", "sub"]);
    assert!(out.status.success(), "ls: {}", describe(&out));
    let listed = stdout(&out);
    assert!(listed.contains("x.rs"), "ls: {}", describe(&out));
    assert!(!listed.contains("d.txt"), "ls: {}", describe(&out));
}

/// Enough long names that `*.txt` expanded is ~99,000 characters, three times what a Windows
/// command line holds. Every file holds an `x`.
#[cfg(windows)]
const MANY: usize = 3000;

#[cfg(windows)]
fn many_files() -> tempfile::TempDir {
    let dir = common::tempdir();
    for n in 0..MANY {
        write(
            dir.path(),
            &format!("longer_file_name_number_{n:04}.txt"),
            "x\n",
        );
    }
    dir
}

/// Past the limit the MSYS child gets `*.txt` as typed and globs it itself, as it did before
/// rtk expanded operands; without that fallback each of these fails with `os error 206`.
#[cfg(windows)]
#[test]
fn globs_past_the_command_line_limit_are_left_to_the_msys_child() {
    let Some(bin) = msys_bin() else {
        eprintln!("skipping: no Git for Windows MSYS tools found");
        return;
    };
    let dir = many_files();
    let run = |args: &[&str]| rtk(dir.path(), Some(&bin), args);
    let first = "longer_file_name_number_0000.txt";
    let last = format!("longer_file_name_number_{:04}.txt", MANY - 1);

    let out = run(&["wc", "-l", "*.txt"]);
    assert!(out.status.success(), "wc: {}", describe(&out));
    let counted = stdout(&out);
    let lines: Vec<&str> = counted.trim().lines().collect();
    assert_eq!(lines.len(), MANY + 1, "wc: {}", describe(&out));
    assert_eq!(lines[0], format!("1 {first}"));
    assert_eq!(lines[MANY], format!("Σ {MANY}"));

    let out = run(&["grep", "-c", "x", "*.txt"]);
    assert!(out.status.success(), "grep: {}", describe(&out));
    let counts = stdout(&out);
    assert_eq!(
        counts.trim().lines().count(),
        MANY,
        "grep: {}",
        describe(&out)
    );
    assert!(
        counts.contains(&format!("{last}:1")),
        "grep: {}",
        describe(&out)
    );

    let out = run(&["ls", "*.txt"]);
    assert!(out.status.success(), "ls: {}", describe(&out));
    let listed = stdout(&out);
    assert!(listed.contains(first), "ls: {}", describe(&out));
}

/// rg has no globbing of its own to fall back to, so past the limit rtk says why it cannot
/// spawn it instead of passing on `os error 206`.
#[cfg(windows)]
#[test]
fn rg_past_the_command_line_limit_names_the_limit() {
    let Some(bin) = msys_bin() else {
        eprintln!("skipping: no Git for Windows MSYS tools found");
        return;
    };
    if which::which("rg").is_err() {
        eprintln!("skipping: no rg on PATH");
        return;
    }
    let dir = many_files();
    let out = rtk(dir.path(), Some(&bin), &["rg", "-c", "x", "*.txt"]);
    assert!(!out.status.success(), "rg: {}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("past Windows' limit of 32767"),
        "rg: {}",
        describe(&out)
    );
    assert!(!stderr.contains("os error 206"), "rg: {}", describe(&out));
}
