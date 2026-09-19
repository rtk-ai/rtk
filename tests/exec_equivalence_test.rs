//! Does the rewritten command run the same programs, on the same input, as the
//! one it replaced?
//!
//! Every other test here compares the rewriter against an expectation somebody
//! wrote down, so a mistake in the author's model of shell grammar sits on both
//! sides and agrees with itself. Here bash decides. Recording stubs sit ahead of
//! everything on `PATH`, the original and the rewritten command are each run,
//! and what actually executed has to match.
//!
//! The `rtk` stub runs the command it wraps and marks its output, which is what
//! the real one does in miniature. So a rewrite at a command position changes
//! nothing that is compared, while a rewrite reaching inside a substitution
//! changes the text the outer command was built from — visibly.
//!
//! What this does not check: whether a rewrite happened at all (a command RTK
//! declines runs identically, so coverage belongs to other tests), and plumbing
//! (stubs record their arguments, not where their file descriptors point, so a
//! dropped redirect is invisible here).
//!
//! Unix only, skipped where there is no `bash`.
#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `echo` is a shell builtin, so a stub by that name is never reached — the
/// outer command of a substitution has to be one bash looks up on `PATH`, which
/// is what `sink` is for.
const STUBS: &[&str] = &[
    "git", "cargo", "ls", "grep", "rm", "tail", "cat", "diff", "tee", "sink",
];

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct Oracle {
    bin: PathBuf,
    root: PathBuf,
}

impl Oracle {
    fn new() -> Option<Self> {
        Command::new("bash").arg("-c").arg("true").output().ok()?;

        // One directory per instance: cargo runs these tests concurrently, and
        // rewriting a stub another test is executing fails with ETXTBSY.
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "exec-oracle-{}",
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("create stub dir");

        for name in STUBS {
            // An argument naming something readable is recorded by its contents
            // rather than its name: a process substitution arrives as
            // `/dev/fd/63`, where the number bash picked says nothing and what
            // it points at says everything.
            //
            // Bounded, because `>( )` hands over the writing end of a pipe: it
            // opens but never delivers, so an unbounded read waits for a writer
            // that is itself waiting for this process to finish.
            write_stub(
                &bin.join(name),
                &format!(
                    "#!/bin/bash\n\
                     rec=\"\"\n\
                     for a in \"$@\"; do\n\
                     \x20 if [ -r \"$a\" ] && [ ! -d \"$a\" ]; then\n\
                     \x20   rec=\"$rec <$(timeout 1 tr '\\n' ' ' < \"$a\")>\"\n\
                     \x20 else\n\
                     \x20   rec=\"$rec $a\"\n\
                     \x20 fi\n\
                     done\n\
                     printf '%s\\t%s\\t%s\\n' \"{name}\" \"$PWD\" \"$rec\" >> \"$RTK_ORACLE_LOG\"\n\
                     echo \"{name}-out\"\n"
                ),
            );
        }

        // The real `rtk` runs the command and reshapes its output. This does the
        // same in miniature and records nothing of itself, so a rewrite at a
        // command position is invisible to the comparison while one that reaches
        // into a substitution is not.
        write_stub(
            &bin.join("rtk"),
            "#!/bin/bash\nout=$(\"$@\")\necho \"filtered:$out\"\n",
        );

        Some(Self { bin, root })
    }

    /// Which programs `cmd` ran, with what arguments and working directory.
    ///
    /// A set, because the stages of a pipeline run concurrently and reach the
    /// log in whatever order they are scheduled; the question is what ran, not
    /// what finished first.
    fn trace(&self, cmd: &str) -> BTreeSet<String> {
        let log = self
            .root
            .join(format!("trace-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::write(&log, "").expect("create log");
        Command::new("bash")
            .arg("-c")
            .arg(cmd)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("RTK_ORACLE_LOG", &log)
            .output()
            .expect("run bash");
        fs::read_to_string(&log)
            .expect("read log")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn rewrite(&self, cmd: &str) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["rewrite", cmd])
            .env("RTK_DISABLE_TRACKING", "1")
            .output()
            .expect("run rtk rewrite");
        let rewritten = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if rewritten.is_empty() {
            cmd.to_string()
        } else {
            rewritten
        }
    }
}

fn write_stub(path: &Path, body: &str) {
    fs::write(path, body).expect("write stub");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

#[test]
fn a_rewrite_runs_the_same_programs() {
    let Some(oracle) = Oracle::new() else {
        return;
    };
    for cmd in [
        "git status",
        "git status && cargo build",
        "git status; cargo build",
        "(git status; cargo build)",
        "(git status)",
        "cargo build | grep x",
        "cargo build 2>&1 | tail -1",
        "git status & wait",
        "sink $(git status && cargo build)",
        "sink $( (ls) && git status )",
        "diff <(git status) <(cargo build)",
        "tee >(cargo build) < /dev/null",
        "D='# shellcheck disable=SC2034'; sink \"$D\"",
        "D='# shellcheck disable=SC2034'; git status",
        "ls {a,b}.txt",
        "time (cargo build)",
        "cd / && git status",
    ] {
        let rewritten = oracle.rewrite(cmd);
        assert_eq!(
            oracle.trace(cmd),
            oracle.trace(&rewritten),
            "rewriting {cmd:?} to {rewritten:?} changed what ran"
        );
    }
}

/// A test that can only pass proves nothing. Each of these rewrites is wrong in
/// a way this repository has actually shipped, and the oracle has to object.
#[test]
fn the_oracle_objects_to_a_rewrite_that_changes_what_runs() {
    let Some(oracle) = Oracle::new() else {
        return;
    };
    for (original, corrupted, what) in [
        (
            "sink $(git status && cargo build)",
            "sink $(git status && rtk cargo build)",
            "filtering inside a substitution changes the text it captures",
        ),
        (
            "D='# shellcheck disable=SC2034'; sink \"$D\"",
            "D='# rtk shellcheck disable=SC2034'; sink \"$D\"",
            "editing inside a quoted assignment changes what a later command reads",
        ),
        (
            "diff <(git status) <(cargo build)",
            "diff <(rtk git status) <(cargo build)",
            "filtering a process substitution changes what the outer command compares",
        ),
        (
            "git status && cargo build",
            "git status && rtk cargo test",
            "rewriting one command into a different one",
        ),
    ] {
        assert_ne!(
            oracle.trace(original),
            oracle.trace(corrupted),
            "the oracle did not notice that {what}"
        );
    }
}

/// A command RTK leaves alone runs identically, and so does one it wraps at a
/// command position. Without this the test above could be satisfied by an
/// oracle that simply calls everything different.
#[test]
fn the_oracle_is_quiet_when_nothing_really_changed() {
    let Some(oracle) = Oracle::new() else {
        return;
    };
    assert_eq!(
        oracle.trace("(git status; cargo build)"),
        oracle.trace("(git status; rtk cargo build)"),
        "a missed rewrite runs the same programs and is not this test's business"
    );
    assert_eq!(
        oracle.trace("git status && cargo build"),
        oracle.trace("rtk git status && rtk cargo build"),
        "wrapping a command is what a rewrite is for"
    );
}
