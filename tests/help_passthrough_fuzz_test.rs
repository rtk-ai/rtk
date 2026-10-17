//! Differential fuzz for #4198: what a wrapped tool prints for its own `-h`/`--help`
//! must survive RTK.
//!
//! Two failure modes are covered, because the bug had two halves. RTK must not answer the
//! flag itself (its usage is recognisable by the `Usage: rtk` line), and the filter must
//! not compress the page the tool printed -- `cargo build --help` came back as
//! `cargo build (0 crates compiled)`, 31 bytes of 3135.
//!
//! Tools that are not installed are skipped rather than failing, so the suite runs
//! anywhere and covers whatever the machine has. Commands are spawned directly, never
//! through a shell, so a `grep` shell function cannot stand in for the real binary.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn on_path(tool: &str) -> Option<PathBuf> {
    let dirs: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH")?).collect();
    if cfg!(windows) {
        // A bare `npm` on PATH is an extensionless shell script; spawning it fails with
        // "not a valid Win32 application". PATHEXT names the ones that are programs.
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into());
        for dir in &dirs {
            for ext in pathext.split(';').filter(|e| !e.is_empty()) {
                let candidate = dir.join(format!("{tool}{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        return None;
    }
    dirs.into_iter()
        .find(|d| d.join(tool).is_file())
        .map(|d| d.join(tool))
}

/// A tool prints its own name from argv[0], and on Windows this harness reaches `pip.EXE`
/// where rtk reaches `pip`. The page is the same; only that spelling differs.
fn without_exe(text: &str) -> String {
    text.replace(".EXE", "").replace(".exe", "")
}

/// The first line of a tool's page that identifies it: `GIT-LOG(1)`, `Usage: tsc`. A page
/// from the wrong subcommand fails on this where a length check would pass.
fn first_substantial_line(text: &str) -> &str {
    text.lines()
        .map(str::trim_end)
        .find(|l| l.trim().len() > 8)
        .unwrap_or("")
}

/// Clap names the program from argv[0], which is `rtk.exe` on Windows, so the marker
/// stops at the stem. Getting this wrong made the "RTK did not answer" check below pass
/// for the wrong reason on Windows.
fn is_rtk_usage(text: &str) -> bool {
    text.contains("Usage: rtk")
}

/// Windows ships a `find.exe` and a `tree.com` that are unrelated programs, and `ls`,
/// `grep` and `wc` only exist there if a Unix toolchain is on PATH. Comparing RTK against
/// those proves nothing about the filters, which assume the Unix tools.
#[cfg(windows)]
const UNIX_ONLY: &[&str] = &["grep", "ls", "tree", "wc", "find"];
#[cfg(not(windows))]
const UNIX_ONLY: &[&str] = &[];

struct Output {
    text: String,
    code: i32,
}

/// `LC_ALL=C` on both sides, since a tool's usage is translated and the two are compared
/// byte for byte. `MANWIDTH`/`COLUMNS` pin the width `man` renders a git page at.
fn finish(mut cmd: Command, args: &[&str], label: &str) -> Output {
    let out = cmd
        .args(args)
        .env("LC_ALL", "C")
        .env("MANWIDTH", "80")
        .env("COLUMNS", "80")
        .output()
        .unwrap_or_else(|e| panic!("spawn {label}: {e}"));
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Output {
        text,
        code: out.status.code().unwrap_or(-1),
    }
}

fn run(program: &Path, args: &[&str]) -> Output {
    let label = program.display().to_string();
    let mut cmd = common::native_command(&label);
    if let Some(dir) = common::rtk_command().get_current_dir() {
        cmd.current_dir(dir);
    }
    finish(cmd, args, &label)
}

/// rtk with its tracking database and tee spool redirected to this test's scratch
/// directory; `core::test_isolation` fails the suite on a spawn that skips this.
fn run_rtk(args: &[&str]) -> Output {
    finish(common::rtk_command(), args, "rtk")
}

/// `rtk <rtk_args>` must answer with what `<tool> <native_args>` answers.
struct Case {
    rtk_args: &'static [&'static str],
    tool: &'static str,
    native_args: &'static [&'static str],
}

const CASES: &[Case] = &[
    // `-h` spelled by a tool that means something else by it.
    Case {
        rtk_args: &["grep", "--help"],
        tool: "grep",
        native_args: &["--help"],
    },
    Case {
        rtk_args: &["ls", "--help"],
        tool: "ls",
        native_args: &["--help"],
    },
    Case {
        rtk_args: &["tree", "--help"],
        tool: "tree",
        native_args: &["--help"],
    },
    Case {
        rtk_args: &["wc", "--help"],
        tool: "wc",
        native_args: &["--help"],
    },
    Case {
        rtk_args: &["find", "--help"],
        tool: "find",
        native_args: &["--help"],
    },
    Case {
        rtk_args: &["rg", "--help"],
        tool: "rg",
        native_args: &["--help"],
    },
    // `-h` as a genuine help request, the half that only the filter guard catches.
    Case {
        rtk_args: &["cargo", "build", "-h"],
        tool: "cargo",
        native_args: &["build", "-h"],
    },
    Case {
        rtk_args: &["cargo", "test", "--help"],
        tool: "cargo",
        native_args: &["test", "--help"],
    },
    Case {
        rtk_args: &["cargo", "clippy", "--help"],
        tool: "cargo",
        native_args: &["clippy", "--help"],
    },
    Case {
        rtk_args: &["git", "log", "--help"],
        tool: "git",
        native_args: &["log", "--help"],
    },
    Case {
        rtk_args: &["git", "status", "--help"],
        tool: "git",
        native_args: &["status", "--help"],
    },
    Case {
        rtk_args: &["git", "diff", "--help"],
        tool: "git",
        native_args: &["diff", "--help"],
    },
    Case {
        rtk_args: &["go", "vet", "-h"],
        tool: "go",
        native_args: &["vet", "-h"],
    },
    Case {
        rtk_args: &["gh", "pr", "diff", "--help"],
        tool: "gh",
        native_args: &["pr", "diff", "--help"],
    },
    Case {
        rtk_args: &["kubectl", "get", "--help"],
        tool: "kubectl",
        native_args: &["get", "--help"],
    },
    Case {
        rtk_args: &["docker", "--help"],
        tool: "docker",
        native_args: &["--help"],
    },
    Case {
        rtk_args: &["npm", "--help"],
        tool: "npm",
        native_args: &["--help"],
    },
    Case {
        rtk_args: &["tsc", "--help"],
        tool: "tsc",
        native_args: &["--help"],
    },
    Case {
        rtk_args: &["prettier", "--help"],
        tool: "prettier",
        native_args: &["--help"],
    },
    Case {
        rtk_args: &["mvn", "--help"],
        tool: "mvn",
        native_args: &["--help"],
    },
    Case {
        rtk_args: &["pip", "list", "--help"],
        tool: "pip",
        native_args: &["list", "--help"],
    },
    Case {
        rtk_args: &["uv", "run", "--help"],
        tool: "uv",
        native_args: &["run", "--help"],
    },
    Case {
        rtk_args: &["deno", "test", "--help"],
        tool: "deno",
        native_args: &["test", "--help"],
    },
    Case {
        rtk_args: &["dotnet", "build", "--help"],
        tool: "dotnet",
        native_args: &["build", "--help"],
    },
    Case {
        rtk_args: &["dotnet", "format", "--help"],
        tool: "dotnet",
        native_args: &["format", "--help"],
    },
    // Spellings a POSIX scan misreads: MSBuild's `-?`, Go's single-dash long name, and a
    // request addressed past the tool's own boundary.
    Case {
        rtk_args: &["dotnet", "build", "-?"],
        tool: "dotnet",
        native_args: &["build", "-?"],
    },
    Case {
        rtk_args: &["go", "vet", "-help"],
        tool: "go",
        native_args: &["vet", "-help"],
    },
    Case {
        rtk_args: &["pnpm", "outdated", "--help"],
        tool: "pnpm",
        native_args: &["outdated", "--help"],
    },
    Case {
        rtk_args: &["jest", "--help"],
        tool: "jest",
        native_args: &["--help"],
    },
    // A whole command the caller named (#4548). Its `-h` stays unread, see
    // `an_arbitrary_commands_flags_are_not_rtks_to_read`.
    Case {
        rtk_args: &["err", "cargo", "build", "--help"],
        tool: "cargo",
        native_args: &["build", "--help"],
    },
    Case {
        rtk_args: &["test", "cargo", "build", "--help"],
        tool: "cargo",
        native_args: &["build", "--help"],
    },
    Case {
        rtk_args: &["summary", "cargo", "build", "--help"],
        tool: "cargo",
        native_args: &["build", "--help"],
    },
];

#[test]
fn a_wrapped_tools_own_help_survives_rtk() {
    let mut checked = 0usize;
    let mut skipped = Vec::new();

    for case in CASES {
        if UNIX_ONLY.contains(&case.tool) {
            skipped.push(case.tool);
            continue;
        }
        let Some(tool) = on_path(case.tool) else {
            skipped.push(case.tool);
            continue;
        };
        let label = format!("rtk {}", case.rtk_args.join(" "));
        let mine = run_rtk(case.rtk_args);
        let native = run(&tool, case.native_args);
        checked += 1;

        assert!(
            !is_rtk_usage(&mine.text),
            "{label} answered with RTK's own usage:\n{}",
            &mine.text[..mine.text.len().min(200)]
        );
        assert_eq!(
            mine.code, native.code,
            "{label} exit code drifted from the tool's"
        );
        // Content, not length: a length check passes on a long page from the wrong
        // subcommand, which is exactly what a bad `git_name` arm would return. Exact bytes
        // are still not asserted, because `man` renders to the terminal it thinks it has
        // and a tool may print its own name from argv[0].
        let anchor = without_exe(first_substantial_line(&native.text));
        assert!(
            without_exe(&mine.text).contains(&anchor),
            "{label} does not contain the tool's own first line {anchor:?}:\n{}",
            &mine.text[..mine.text.len().min(200)]
        );
        assert!(
            mine.text.len() * 2 >= native.text.len(),
            "{label} returned {} bytes of the tool's {}",
            mine.text.len(),
            native.text.len()
        );
    }

    assert!(
        checked > 0,
        "no wrapped tool was available to compare against"
    );
    if !skipped.is_empty() {
        eprintln!("skipped (not installed): {}", skipped.join(", "));
    }
}

/// The reported bug, end to end: `-h` is grep's `--no-filename`, so the output must be
/// grep's matches, byte for byte, and not a help page.
// Unix only: these assert GNU/BSD flag semantics that the Windows namesakes do not share.
#[cfg(unix)]
#[test]
fn grep_dash_h_searches_and_drops_the_filename() {
    let Some(grep) = on_path("grep") else {
        eprintln!("skipped: grep not installed");
        return;
    };
    let tmp = common::tempdir();
    let dir = tmp.path();
    let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&a, "alpha\nbeta\n").expect("write a");
    std::fs::write(&b, "alpha\ngamma\n").expect("write b");
    let (a, b) = (
        a.to_string_lossy().into_owned(),
        b.to_string_lossy().into_owned(),
    );

    for flags in [
        vec!["-h", "alpha", &a, &b],
        vec!["-rh", "alpha", &a, &b],
        vec!["alpha", "-h", &a, &b],
        vec!["-h", "-n", "alpha", &a, &b],
    ] {
        let mine = run_rtk(&[&["grep"], flags.as_slice()].concat());
        let native = run(&grep, &flags);
        assert_eq!(
            mine.text,
            native.text,
            "rtk grep {} diverged from grep",
            flags.join(" ")
        );
        assert_eq!(mine.code, native.code, "exit code for {}", flags.join(" "));
    }
}

#[cfg(unix)]
#[test]
fn a_caller_value_that_spells_a_help_token_is_not_a_request() {
    let tmp = common::tempdir();
    let dir = tmp.path();
    let file = dir.join("flags.txt");
    std::fs::write(&file, "use --help for usage\nplain\nuse --version here\n").expect("write");
    let f = file.to_string_lossy().into_owned();

    for (tool, args) in [
        ("grep", vec!["-e", "--help", &f]),
        ("grep", vec!["--", "--version", &f]),
        ("grep", vec!["-F", "--", "--help", &f]),
        ("rg", vec!["-e", "--help", &f]),
        ("rg", vec!["--", "-h", &f]),
    ] {
        let Some(native) = on_path(tool) else {
            continue;
        };
        let mine = run_rtk(&[&[tool], args.as_slice()].concat());
        let theirs = run(&native, &args);
        assert_eq!(
            mine.text,
            theirs.text,
            "rtk {tool} {} searched for the token instead of matching it",
            args.join(" ")
        );
        assert_eq!(mine.code, theirs.code, "{tool} {}", args.join(" "));
    }
}

#[cfg(unix)]
#[test]
fn go_test_help_is_not_replayed_through_the_injected_json() {
    let Some(go) = on_path("go") else { return };
    let tmp = common::tempdir();
    let dir = tmp.path();
    std::fs::write(dir.join("go.mod"), "module t\n\ngo 1.21\n").expect("write go.mod");
    std::fs::write(
        dir.join("t_test.go"),
        "package t\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) {}\n",
    )
    .expect("write test");

    let args = ["test", "./...", "-args", "-help"];
    let mut rtk_cmd = common::rtk_command();
    rtk_cmd.current_dir(dir).arg("go");
    let mine = finish(rtk_cmd, &args, "rtk");

    let mut go_cmd = common::native_command(&go.display().to_string());
    go_cmd.current_dir(dir);
    let theirs = finish(go_cmd, &args, "go");

    assert!(
        !mine.text.contains("\"Action\":"),
        "the usage came back as -json events:\n{}",
        &mine.text[..mine.text.len().min(200)]
    );
    assert!(
        mine.text.contains("Usage of") && theirs.text.contains("Usage of"),
        "expected the test binary's usage from both sides"
    );
}

#[cfg(unix)]
#[test]
fn an_arbitrary_commands_flags_are_not_rtks_to_read() {
    let Some(du) = on_path("du") else { return };

    // Generated rather than `df -h`, whose 436 bytes on a CI runner are small enough that
    // RTK rightly returns them raw and the marker below cannot tell guard from size.
    let tmp = common::tempdir();
    let dir = tmp.path();
    for i in 0..60 {
        let leaf = dir.join(format!("d{i}"));
        std::fs::create_dir_all(&leaf).expect("create temp tree");
        std::fs::write(leaf.join("f"), vec![b'x'; 2000]).expect("write leaf");
    }
    let tree = dir.display().to_string();

    let raw = run(&du, &["-h", &tree]);
    for sub in ["err", "summary"] {
        let mine = run_rtk(&[sub, "du", "-h", &tree]);
        assert_ne!(
            mine.text, raw.text,
            "rtk {sub} du -h <tree> passed du's output through instead of filtering it"
        );
        assert!(
            mine.text.starts_with("[ok]"),
            "rtk {sub} du -h <tree> did not produce RTK's own summary; got: {}",
            mine.text.chars().take(120).collect::<String>()
        );
    }

    // `rtk test` reports the tail of what it ran rather than a copy of it.
    let mine = run_rtk(&["test", "du", "-h", &tree]);
    assert_ne!(
        mine.text, raw.text,
        "rtk test du -h <tree> passed du's output through instead of filtering it"
    );
    assert!(
        mine.text.starts_with("OUTPUT (last 5 lines):"),
        "rtk test du -h <tree> did not produce RTK's own report; got: {}",
        mine.text.chars().take(120).collect::<String>()
    );
}

/// `rtk git log` probes git again with the patch letters stripped from the caller's flags,
/// which turns `-hp` into a bare `-h`. The command has already gone to git, so git's answer,
/// usage or rejection, is the only one, with git's own code.
#[test]
fn a_clustered_h_is_answered_by_git_once() {
    let Some(git) = on_path("git") else {
        eprintln!("skipped: git not installed");
        return;
    };
    let repo = common::temp_git_repo();
    let flags = ["log", "-hp"];
    let mut rtk_cmd = common::rtk_command();
    rtk_cmd.current_dir(repo.path()).arg("git");
    let mine = finish(rtk_cmd, &flags, "rtk");
    let mut git_cmd = Command::new(&git);
    common::isolate_git(&mut git_cmd);
    git_cmd.current_dir(repo.path());
    let theirs = finish(git_cmd, &flags, "git");
    assert_eq!(mine.code, theirs.code, "rtk git log -hp exit code");
    assert_eq!(mine.text, theirs.text, "rtk git log -hp output");
}

#[test]
fn a_usage_request_keeps_a_non_zero_exit() {
    let mut checked = 0usize;
    for (tool, args) in [
        ("wget", vec!["--zzz", "--help"]),
        ("pnpm", vec!["outdated", "--zzz", "--help"]),
    ] {
        let Some(native) = on_path(tool) else {
            continue;
        };
        let theirs = run(&native, &args);
        if theirs.code == 0 {
            continue;
        }
        let mine = run_rtk(&[&[tool], args.as_slice()].concat());
        assert_eq!(
            mine.code,
            theirs.code,
            "rtk {tool} {} must end with the tool's code",
            args.join(" ")
        );
        assert!(!is_rtk_usage(&mine.text));
        checked += 1;
    }
    if checked == 0 {
        eprintln!("skipped: no tool available that rejects a flag before printing usage");
    }
}

/// The inverse of the passthrough: a tool whose `-h` is its own flag must still be
/// filtered. Dropping `tree` from `DASH_H_IS_NOT_HELP` fails here, which is what stops a
/// future edit from silently turning compression off with every other check still green.
/// `ls` is along for the ride rather than proving the table: it renders human sizes itself
/// and strips `-h` before building the child, so the guard never sees that flag.
// Unix only: these assert GNU/BSD flag semantics that the Windows namesakes do not share.
#[cfg(unix)]
#[test]
fn a_tools_own_dash_h_is_still_filtered() {
    let tmp = common::tempdir();
    let dir = tmp.path();
    std::fs::write(dir.join("one.txt"), "x\n").expect("write");
    std::fs::write(dir.join("two.txt"), "y\n").expect("write");
    let path = dir.to_string_lossy().into_owned();

    for (tool, args) in [("ls", vec!["-h", &path]), ("tree", vec!["-h", &path])] {
        let Some(native) = on_path(tool) else {
            continue;
        };
        let mine = run_rtk(&[&[tool], args.as_slice()].concat());
        let theirs = run(&native, &args);
        assert!(
            !mine.text.is_empty(),
            "rtk {tool} {} produced nothing",
            args.join(" ")
        );
        assert_ne!(
            mine.text,
            theirs.text,
            "rtk {tool} {} was passed through unfiltered; is `{tool}` still in \
             DASH_H_IS_NOT_HELP?",
            args.join(" ")
        );
    }
}

/// RTK's own commands keep their help: releasing everything would satisfy the checks
/// above and leave no way to read RTK's usage.
#[test]
fn rtks_own_help_is_still_rtks() {
    for args in [
        vec!["--help"],
        vec!["-h"],
        vec!["gain", "--help"],
        vec!["proxy", "-h"],
        vec!["init", "--help"],
        vec!["help", "grep"],
    ] {
        let out = run_rtk(&args);
        assert!(
            is_rtk_usage(&out.text),
            "rtk {} should print RTK's own usage, got:\n{}",
            args.join(" "),
            &out.text[..out.text.len().min(200)]
        );
    }
}
