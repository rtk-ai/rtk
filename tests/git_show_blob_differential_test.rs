//! Differential fuzzer for `rtk git show` blob classification.
//!
//! The classifier ("is this `git show` arg a blob, or a commit-diff / flag value?") was
//! reopened three times because it tried to mirror git's flag grammar. The fix asks git
//! instead: `git cat-file -t <arg>` is the authority, and `rtk` only takes the byte-safe
//! blob path for an arg git calls a `blob`.
//!
//! This test is the standing guarantee that no misroute class is hiding. It builds a
//! hermetic git repo with objects of every relevant type, then — with a FIXED seed so CI
//! is deterministic — generates hundreds of `git show <random flags> <arg>` invocations,
//! runs each distinct one once (rtk's answer depends only on its argv in this fixed repo),
//! and checks rtk's output against ground truth from `git cat-file -t`. It asserts:
//! - 0 truncations: rtk never cuts a `git show` dump to a window with a recovery hint;
//! - byte fidelity: every blob dump (large or small UTF-8, Latin-1, UTF-16, a blob behind
//!   a colon-carrying short-flag CLUSTER such as `-wG x:y`, or with `--textconv` / a
//!   trailing `-- <path>`) is emitted byte-identically to `git show` with the same args —
//!   no prefix cut (#5070) and no U+FFFD mojibake.
//!
//! It shells out to the real `git` and the built `rtk` binary, matching the repo's other
//! integration tests (e.g. `diff_byte_accuracy_test.rs`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

/// Number of invocations drawn; the repeats among them are run once. Seeded, so this is
/// deterministic across runs/CI.
const ITERATIONS: usize = 600;
/// Fixed seed — change only intentionally.
const SEED: u64 = 0x5DEE_CE66_D3A1_2B4F;

// ---- deterministic PRNG (SplitMix64) -------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    fn chance(&mut self, n: usize, d: usize) -> bool {
        self.below(d) < n
    }
}

// ---- git / rtk helpers ---------------------------------------------------------------

/// Env that isolates git and rtk from the developer's real config/identity/home.
fn isolate(cmd: &mut Command, home: &Path) {
    common::isolate_git(cmd);
    cmd.env("HOME", home)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_TERMINAL_PROMPT", "0");
}

fn git(repo: &Path, home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo);
    // Never sign or convert line endings in the hermetic repo.
    cmd.args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"]);
    cmd.args(args);
    isolate(&mut cmd, home);
    cmd.output().expect("run git")
}

fn git_ok(repo: &Path, home: &Path, args: &[&str]) {
    let out = git(repo, home, args);
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `git cat-file -t <arg>` → object type, or None on a non-zero exit (not an object).
fn cat_file_type(repo: &Path, home: &Path, arg: &str) -> Option<String> {
    let out = git(repo, home, &["cat-file", "-t", arg]);
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn rtk_show(repo: &Path, home: &Path, args: &[&str]) -> Output {
    let mut cmd = common::rtk_command();
    cmd.arg("git").arg("show").args(args).current_dir(repo);
    isolate(&mut cmd, home);
    cmd.output().expect("run rtk git show")
}

// ---- hermetic repo -------------------------------------------------------------------

struct Repo {
    _dir: tempfile::TempDir,
    path: PathBuf,
    home: PathBuf,
}

fn setup_repo() -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_path_buf();
    let home = path.join("home");
    std::fs::create_dir_all(&home).expect("home dir");

    git_ok(&path, &home, &["init", "-q"]);

    // A large, valid-UTF-8 text blob (~90 KB): far past the old 8 KiB window (#5070).
    let mut large = String::new();
    for i in 0..2000 {
        large.push_str(&format!(
            "line {i:04} — lorem ipsum dolor sit amet with enough width to pass the budget\n"
        ));
    }
    std::fs::write(path.join("large.txt"), &large).expect("write large.txt");

    // A small UTF-8 blob.
    std::fs::write(path.join("small.txt"), "alpha\nbeta\ngamma\n").expect("write small.txt");

    // A directory → its `git show <rev>:<dir>` renders a tree listing (type tree).
    std::fs::create_dir_all(path.join("dir")).expect("mkdir dir");
    std::fs::write(path.join("dir/inner.txt"), "inner\n").expect("write inner");

    // A large Latin-1 (ISO-8859-1) blob: invalid UTF-8, so any decode would corrupt it.
    let mut latin1: Vec<u8> = Vec::new();
    for i in 0..1200 {
        latin1.extend_from_slice(b"M\xC9TODO n"); // "MÉTODO n" in Latin-1
        latin1.extend_from_slice(i.to_string().as_bytes());
        latin1.extend_from_slice(b" ADI\xD3S PARDON\n"); // "ADIÓS ..." in Latin-1
    }
    assert!(
        std::str::from_utf8(&latin1).is_err(),
        "fixture must be non-UTF-8"
    );
    std::fs::write(path.join("latin1.pck"), &latin1).expect("write latin1.pck");

    // A large UTF-16LE-with-BOM blob: also invalid UTF-8.
    let mut utf16: Vec<u8> = vec![0xFF, 0xFE]; // UTF-16LE BOM
    for i in 0..3000 {
        for u in format!("row {i} of the utf-16 blob\n").encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
    }
    assert!(
        std::str::from_utf8(&utf16).is_err(),
        "utf16 fixture must be non-UTF-8"
    );
    std::fs::write(path.join("utf16.txt"), &utf16).expect("write utf16.txt");

    git_ok(&path, &home, &["add", "-A"]);
    git_ok(&path, &home, &["commit", "-q", "-m", "seed"]);
    // A second commit so HEAD~1 exists and commit-diffs have content.
    std::fs::write(path.join("small.txt"), "alpha\nbeta\ngamma\ndelta\n").expect("edit");
    git_ok(&path, &home, &["commit", "-q", "-am", "second"]);

    Repo {
        _dir: dir,
        path,
        home,
    }
}

// ---- classification ------------------------------------------------------------------

/// rtk cut the dump iff the old blob window's recovery hint is present.
fn was_windowed(out: &Output) -> bool {
    String::from_utf8_lossy(&out.stdout).contains("[see remaining: rtk proxy git ")
}

/// The object categories the fuzzer draws from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// Large valid-UTF-8 blob → byte-identical, never cut (#5070).
    LargeUtf8,
    /// Small blob → byte-identical.
    Small,
    /// Latin-1 blob → byte-identical.
    Latin1,
    /// UTF-16 blob → byte-identical.
    Utf16,
    /// A tree (directory) → never cut.
    Tree,
    /// A commit ref, usually paired with a colon-carrying flag VALUE (the misroute
    /// stress: git re-parses `-pS`/`-wG`/… clusters, the colon token is a pickaxe/regex
    /// value, and there is NO blob) → never cut.
    CommitMisroute,
    /// A bogus `rev:path` (nonexistent object) → git errors, never cut.
    Bogus,
    /// A colon-carrying short-flag CLUSTER (`-wG x:y`, `-pS url:1`, …) placed BEFORE a
    /// large UTF-8 blob. git consumes the colon token as the flag's value and dumps ONLY
    /// the blob, so rtk must take the byte-safe blob path and match git exactly.
    ClusterLargeUtf8,
    /// The same cluster before a Latin-1 blob. The old misroute sent it through the
    /// commit-diff path's lossy UTF-8 decode → `0xF1` became U+FFFD mojibake. Must pass
    /// through BYTE-IDENTICALLY to git.
    ClusterLatin1,
    /// A content-transforming flag (`--textconv`) before a large blob → byte-identical
    /// to `git show <same args>`.
    TextconvLargeUtf8,
    /// A trailing `-- <pathspec>` beside a large blob → byte-identical to
    /// `git show <same args>`.
    PathspecLargeUtf8,
}

fn kind_object(kind: Kind) -> &'static str {
    match kind {
        Kind::LargeUtf8 => "HEAD:large.txt",
        Kind::Small => "HEAD:small.txt",
        Kind::Latin1 => "HEAD:latin1.pck",
        Kind::Utf16 => "HEAD:utf16.txt",
        Kind::Tree => "HEAD:dir",
        Kind::CommitMisroute => "HEAD",
        Kind::Bogus => "HEAD:does-not-exist.txt",
        Kind::ClusterLargeUtf8 | Kind::TextconvLargeUtf8 | Kind::PathspecLargeUtf8 => {
            "HEAD:large.txt"
        }
        Kind::ClusterLatin1 => "HEAD:latin1.pck",
    }
}

// Valueless / colon-free flags that git ignores for a blob (still dumping the file), so
// the blob stays the sole `rev:path` positional.
const BLOB_SAFE_FLAGS: &[&str] = &[
    "-p",
    "-w",
    "-b",
    "--stat",
    "--numstat",
    "--shortstat",
    "--ignore-all-space",
    "--pretty=oneline",
    "--format=medium",
];

// Colon-carrying flag VALUES: a cluster/flag consumes the NEXT token as its value, so a
// colon there must not be read as a blob. These are the exact shapes KuSh's fuzzer hit.
const COLON_VALUE_FLAGS: &[(&str, &str)] = &[
    ("-pS", "url:1"),
    ("-wG", "x:y"),
    ("-pI", "a:b"),
    ("-pwG", "q:r"),
    ("-wpG", "m:n"),
    ("-S", "needle:1"),
    ("-G", "pat:2"),
    ("-I", "re:3"),
    ("--ignore-matching-lines", "a:b"),
];

// The subset of `COLON_VALUE_FLAGS` the cluster-aware walker resolves WITHOUT needing a
// long-flag table entry: short single-letter value flags and their clusters. For these,
// the walker skips the colon value and the following blob is the sole positional.
// (`--ignore-matching-lines` is a long flag absent from the value table, so its
// space-form value stays a phantom positional; it is exercised via `CommitMisroute`.)
const CLUSTER_FLAGS: &[(&str, &str)] = &[
    ("-pS", "url:1"),
    ("-wG", "x:y"),
    ("-pI", "a:b"),
    ("-pwG", "q:r"),
    ("-wpG", "m:n"),
    ("-S", "needle:1"),
    ("-G", "pat:2"),
    ("-I", "re:3"),
];

fn build_invocation(rng: &mut Rng, kind: Kind) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    match kind {
        Kind::CommitMisroute => {
            // A colon-carrying flag value with NO real blob — the misroute trap.
            let (flag, val) = *rng.pick(COLON_VALUE_FLAGS);
            args.push(flag.to_string());
            args.push(val.to_string());
        }
        Kind::LargeUtf8 | Kind::Small | Kind::Latin1 | Kind::Utf16 => {
            // Optionally sprinkle blob-safe flags before the object; they must not
            // change the blob classification.
            let n = rng.below(4);
            for _ in 0..n {
                args.push(rng.pick(BLOB_SAFE_FLAGS).to_string());
            }
        }
        Kind::Tree | Kind::Bogus => {
            if rng.chance(1, 2) {
                args.push(rng.pick(BLOB_SAFE_FLAGS).to_string());
            }
        }
        Kind::ClusterLargeUtf8 | Kind::ClusterLatin1 => {
            // A colon-carrying short-flag cluster the walker skips, then the blob.
            // Optionally sprinkle blob-safe flags around it — they must not change the
            // outcome. This is the exact cross that was NEVER generated before (the old
            // fuzzer only paired these clusters with a blob-less HEAD), so the blocker hid.
            if rng.chance(1, 2) {
                args.push(rng.pick(BLOB_SAFE_FLAGS).to_string());
            }
            let (flag, val) = *rng.pick(CLUSTER_FLAGS);
            args.push(flag.to_string());
            args.push(val.to_string());
        }
        Kind::TextconvLargeUtf8 => {
            args.push("--textconv".to_string());
        }
        Kind::PathspecLargeUtf8 => {} // trailing `-- <path>` appended after the object
    }
    args.push(kind_object(kind).to_string());
    if let Kind::PathspecLargeUtf8 = kind {
        args.push("--".to_string());
        args.push("large.txt".to_string());
    }
    args
}

#[test]
fn git_show_blob_classification_differential() {
    let repo = setup_repo();
    let (path, home) = (repo.path.as_path(), repo.home.as_path());

    let kinds = [
        Kind::LargeUtf8,
        Kind::Small,
        Kind::Latin1,
        Kind::Utf16,
        Kind::Tree,
        Kind::CommitMisroute,
        Kind::Bogus,
        Kind::ClusterLargeUtf8,
        Kind::ClusterLatin1,
        Kind::TextconvLargeUtf8,
        Kind::PathspecLargeUtf8,
    ];

    // The draws repeat (the flag pools are small) and every process costs tens of
    // milliseconds on Windows, so each distinct invocation runs once.
    let mut rng = Rng(SEED);
    let mut invocations: Vec<(Kind, Vec<String>)> = Vec::new();
    for _ in 0..ITERATIONS {
        let kind = *rng.pick(&kinds);
        let invocation = (kind, build_invocation(&mut rng, kind));
        if !invocations.contains(&invocation) {
            invocations.push(invocation);
        }
    }

    // Ground truth per object, from `git cat-file -t`.
    let mut truths: HashMap<&str, Option<String>> = HashMap::new();
    for &kind in &kinds {
        let object = kind_object(kind);
        truths
            .entry(object)
            .or_insert_with(|| cat_file_type(path, home, object));
    }

    // Sanity-check the fixtures against ground truth before fuzzing.
    assert_eq!(truths["HEAD:large.txt"].as_deref(), Some("blob"));
    assert_eq!(truths["HEAD:dir"].as_deref(), Some("tree"));
    assert_eq!(truths["HEAD"].as_deref(), Some("commit"));
    assert_eq!(truths["HEAD:does-not-exist.txt"], None);

    // Collect failures instead of panicking mid-loop, so the incidence is measured
    // across ALL invocations (KuSh: a measured incidence is what proves no class hides).
    let mut truncations: Vec<String> = Vec::new();
    let mut fidelity_breaks: Vec<String> = Vec::new();

    for (kind, args) in &invocations {
        let kind = *kind;
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

        let object = kind_object(kind);
        let truth = &truths[object];
        let out = rtk_show(path, home, &arg_refs);

        // 0 truncations: `git show` output is never cut to a window, blob or not.
        if was_windowed(&out) {
            truncations.push(format!(
                "cut the dump (type {truth:?}) for `git show {}`",
                args.join(" ")
            ));
        }

        // Byte fidelity: a blob dump is the file itself, so rtk must print exactly what
        // git prints for the SAME args -- the strongest invariant is "rtk == git for this
        // exact command". Trees, commit-misroutes and bogus objects take the other routes.
        if truth.as_deref() == Some("blob") {
            let mut full: Vec<&str> = vec!["show"];
            full.extend(arg_refs.iter().copied());
            let plain = git(path, home, &full);
            if out.stdout != plain.stdout || out.status.code() != plain.status.code() {
                fidelity_breaks.push(format!(
                    "`git show {}` differs from git (rtk {} bytes, exit {:?} vs git {} bytes, exit {:?})",
                    args.join(" "),
                    out.stdout.len(),
                    out.status.code(),
                    plain.stdout.len(),
                    plain.status.code()
                ));
            }
        }
    }

    // Report incidence (KuSh: the fuzzer establishes there are no other classes hiding).
    eprintln!(
        "differential fuzzer: {} distinct of {ITERATIONS} drawn invocations — truncations={}, fidelity_breaks={}",
        invocations.len(),
        truncations.len(),
        fidelity_breaks.len()
    );
    assert!(truncations.is_empty(), "truncations: {truncations:#?}");
    assert!(
        fidelity_breaks.is_empty(),
        "fidelity breaks: {fidelity_breaks:#?}"
    );
}

/// #5070: `git show <rev>:<path>` of a file past the old 8 KiB window, read as data by a
/// script, must come back whole and exit 0 -- the reporter's exact fixtures (700 short
/// lines, 50 long lines) and a JSON document that must still parse.
#[test]
fn large_blob_show_is_not_truncated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();
    let home = path.join("home");
    std::fs::create_dir_all(&home).expect("home dir");
    git_ok(path, &home, &["init", "-q"]);
    std::fs::write(
        path.join("C.txt"),
        format!("{}\n", "y".repeat(19)).repeat(700),
    )
    .expect("write C.txt");
    std::fs::write(
        path.join("A.txt"),
        format!("{}\n", "x".repeat(400)).repeat(50),
    )
    .expect("write A.txt");
    let entries: Vec<String> = (0..450)
        .map(|i| format!("  \"key{i}\": \"{}\"", "v".repeat(20)))
        .collect();
    std::fs::write(
        path.join("snapshot.json"),
        format!("{{\n{}\n}}\n", entries.join(",\n")),
    )
    .expect("write snapshot.json");
    git_ok(path, &home, &["add", "-A"]);
    git_ok(path, &home, &["commit", "-q", "-m", "seed"]);

    for object in ["HEAD:C.txt", "HEAD:A.txt", "HEAD~0:snapshot.json"] {
        let plain = git(path, &home, &["show", object]);
        assert!(
            plain.stdout.len() > 8192,
            "{object} must exceed the old window"
        );
        let out = rtk_show(path, &home, &[object]);
        assert!(out.status.success(), "{object}: rtk should exit 0");
        assert_eq!(
            out.stdout.len(),
            plain.stdout.len(),
            "{object}: rtk truncated the blob"
        );
        assert!(
            out.stdout == plain.stdout,
            "{object}: bytes differ from git"
        );
    }
}
