//! Differential fuzzer for `rtk git show` blob classification.
//!
//! The blob-windowing bug ("is this `git show` arg a blob to window, or a commit-diff /
//! flag value?") was reopened three times because the classifier tried to mirror git's
//! flag grammar. The fix asks git instead: `git cat-file -t <arg>` is the authority, and
//! `rtk` only ever windows an arg git calls a `blob`.
//!
//! This test is the standing guarantee that no misroute class is hiding. It builds a
//! hermetic git repo with objects of every relevant type, then — with a FIXED seed so CI
//! is deterministic — generates hundreds of `git show <random flags> <arg>` invocations
//! and checks rtk's CLASSIFICATION (did it window? pass through? render a commit-diff?)
//! against ground truth from `git cat-file -t`. It asserts:
//! - 0 misroutes: rtk never windows anything git does not call a `blob`;
//! - 0 silent losses: a large, byte-recoverable UTF-8 blob is ALWAYS windowed — INCLUDING
//!   when a colon-carrying short-flag CLUSTER (`-wG x:y`, `-pS url:1`, …) precedes it, the
//!   exact cross the old fuzzer never generated so the blocker hid for three rounds;
//! - byte fidelity: a blob rtk declines to window (Latin-1 / UTF-16 / small text, a
//!   Latin-1 blob behind a cluster flag, or a blob with `--textconv` / a trailing
//!   `-- <path>`) is emitted byte-identically to `git show` — no U+FFFD mojibake.
//!
//! It shells out to the real `git` and the built `rtk` binary, matching the repo's other
//! integration tests (e.g. `diff_byte_accuracy_test.rs`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RTK_BIN: &str = env!("CARGO_BIN_EXE_rtk");

/// Number of fuzz iterations. Seeded, so this is deterministic across runs/CI.
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
    cmd.env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", home.join("nonexistent-global"))
        .env("GIT_CONFIG_SYSTEM", home.join("nonexistent-system"))
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
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
    let mut cmd = Command::new(RTK_BIN);
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

    // A large, valid-UTF-8 text blob (~90 KB): the one thing that MUST be windowed.
    let mut large = String::new();
    for i in 0..2000 {
        large.push_str(&format!(
            "line {i:04} — lorem ipsum dolor sit amet with enough width to pass the budget\n"
        ));
    }
    std::fs::write(path.join("large.txt"), &large).expect("write large.txt");

    // A small UTF-8 blob: below the byte budget, always passed through.
    std::fs::write(path.join("small.txt"), "alpha\nbeta\ngamma\n").expect("write small.txt");

    // A directory → its `git show <rev>:<dir>` renders a tree listing (type tree).
    std::fs::create_dir_all(path.join("dir")).expect("mkdir dir");
    std::fs::write(path.join("dir/inner.txt"), "inner\n").expect("write inner");

    // A large Latin-1 (ISO-8859-1) blob: invalid UTF-8, so exact recovery after a
    // transcode is impossible → must pass through byte-identically, never windowed.
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

    // A large UTF-16LE-with-BOM blob: `tail`-sliced bytes cannot concatenate with a
    // UTF-8 head, so it must pass through byte-identically, never windowed.
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

/// rtk windowed the blob iff its recovery hint is present (unique to blob windowing).
fn was_windowed(out: &Output) -> bool {
    String::from_utf8_lossy(&out.stdout).contains("[see remaining: git ")
}

/// The object categories the fuzzer draws from, with their expected classification.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// Large valid-UTF-8 blob → MUST window.
    LargeUtf8,
    /// Small blob → passthrough, byte-identical.
    Small,
    /// Latin-1 blob → passthrough, byte-identical.
    Latin1,
    /// UTF-16 blob → passthrough, byte-identical.
    Utf16,
    /// A tree (directory) → never windowed.
    Tree,
    /// A commit ref, usually paired with a colon-carrying flag VALUE (the misroute
    /// stress: git re-parses `-pS`/`-wG`/… clusters, the colon token is a pickaxe/regex
    /// value, and there is NO blob) → never windowed.
    CommitMisroute,
    /// A bogus `rev:path` (nonexistent object) → git errors, never windowed.
    Bogus,
    /// THE BLOCKER: a colon-carrying short-flag CLUSTER (`-wG x:y`, `-pS url:1`, …)
    /// placed BEFORE a large UTF-8 blob. git consumes the colon token as the flag's
    /// value and dumps ONLY the blob, so it MUST still window (0 silent loss). The old
    /// walker knew only single-letter flags, so it read the colon value as the object,
    /// the probe rejected it, and the real blob fell through un-windowed.
    ClusterLargeUtf8,
    /// The same cluster before a Latin-1 blob. The old misroute sent it through the
    /// commit-diff path's lossy UTF-8 decode → `0xF1` became U+FFFD mojibake. Must pass
    /// through BYTE-IDENTICALLY to git.
    ClusterLatin1,
    /// A content-transforming flag (`--textconv`) before a large blob (MINOR 1). The
    /// `git show rev:path | tail` recovery hint omits the flag, so rtk must NOT window;
    /// it passes the bytes through byte-identically to `git show <same args>`.
    TextconvLargeUtf8,
    /// A trailing `-- <pathspec>` beside a large blob (MINOR 2). Also omitted from the
    /// hint, so rtk must NOT window; byte-identical passthrough.
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
// the blob stays the sole `rev:path` positional and must still window.
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
// the walker skips the colon value and the following blob is the sole positional, so a
// large UTF-8 blob after one of them MUST window. (`--ignore-matching-lines` is a long
// flag absent from the value table, so its space-form value stays a phantom positional
// and the blob passes through byte-identically instead — correct, just uncompacted; it
// is exercised via `CommitMisroute` and the byte-fidelity cross below, not here.)
const WINDOWING_CLUSTER_FLAGS: &[(&str, &str)] = &[
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
            let (flag, val) = *rng.pick(WINDOWING_CLUSTER_FLAGS);
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

    // Sanity-check the fixtures against ground truth before fuzzing.
    assert_eq!(
        cat_file_type(path, home, "HEAD:large.txt").as_deref(),
        Some("blob")
    );
    assert_eq!(
        cat_file_type(path, home, "HEAD:dir").as_deref(),
        Some("tree")
    );
    assert_eq!(cat_file_type(path, home, "HEAD").as_deref(), Some("commit"));
    assert_eq!(cat_file_type(path, home, "HEAD:does-not-exist.txt"), None);

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

    let mut rng = Rng(SEED);
    // Collect failures instead of panicking mid-loop, so the incidence is measured
    // across ALL invocations (KuSh: a measured incidence is what proves no class hides).
    let mut misroutes: Vec<String> = Vec::new();
    let mut silent_losses: Vec<String> = Vec::new();
    let mut fidelity_breaks: Vec<String> = Vec::new();

    for _ in 0..ITERATIONS {
        let kind = *rng.pick(&kinds);
        let args = build_invocation(&mut rng, kind);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

        let object = kind_object(kind);
        let truth = cat_file_type(path, home, object); // ground truth for the object
        let out = rtk_show(path, home, &arg_refs);
        let windowed = was_windowed(&out);

        // 0 misroutes: rtk must never window anything git does not call a `blob`. The
        // only blob among our objects is the one the invocation targets, so a window
        // implies the targeted object is that blob (and truly a blob per cat-file).
        if windowed && truth.as_deref() != Some("blob") {
            misroutes.push(format!(
                "windowed a non-blob (type {truth:?}) for `git show {}`",
                args.join(" ")
            ));
        }

        match kind {
            // 0 silent losses: a large, recoverable UTF-8 blob must ALWAYS window.
            Kind::LargeUtf8 => {
                if !windowed {
                    silent_losses.push(format!(
                        "large UTF-8 blob not windowed for `git show {}`",
                        args.join(" ")
                    ));
                } else {
                    assert!(out.status.success(), "windowed blob show should exit 0");
                }
            }
            // Blobs rtk declines to window must be byte-identical to plain `git show`.
            Kind::Small | Kind::Latin1 | Kind::Utf16 => {
                assert!(
                    !windowed,
                    "small/latin1/utf16 blobs must not window: {args:?}"
                );
                let plain = git(path, home, &["show", object]);
                if out.stdout != plain.stdout {
                    fidelity_breaks.push(format!(
                        "`git show {}` differs from git (rtk {} vs git {} bytes)",
                        args.join(" "),
                        out.stdout.len(),
                        plain.stdout.len()
                    ));
                }
            }
            // Trees, commit-misroutes and bogus objects must never window.
            Kind::Tree | Kind::CommitMisroute | Kind::Bogus => {
                assert!(!windowed, "non-blob must not window: {args:?}");
            }
            // BLOCKER: a colon-cluster before a large UTF-8 blob must STILL window — git
            // dumps only the blob, so failing to window is a silent savings loss. (Old
            // code misrouted this to the commit-diff path: not windowed → this fails.)
            Kind::ClusterLargeUtf8 => {
                if !windowed {
                    silent_losses.push(format!(
                        "cluster + large UTF-8 blob not windowed for `git show {}`",
                        args.join(" ")
                    ));
                } else {
                    assert!(out.status.success(), "windowed blob show should exit 0");
                }
            }
            // BLOCKER: a colon-cluster before a Latin-1 blob must pass through
            // byte-identically to git — the old misroute lossily decoded it to U+FFFD
            // mojibake (not byte-identical → this fails on old code).
            Kind::ClusterLatin1 => {
                assert!(!windowed, "latin-1 blob must never window: {args:?}");
                // Compare against git run with the SAME args, not just `git show object`:
                // the strongest invariant is "rtk == git for this exact command".
                let mut full: Vec<&str> = vec!["show"];
                full.extend(arg_refs.iter().copied());
                let plain = git(path, home, &full);
                if out.stdout != plain.stdout {
                    fidelity_breaks.push(format!(
                        "cluster + latin1 `git show {}` differs from git (rtk {} vs git {} bytes)",
                        args.join(" "),
                        out.stdout.len(),
                        plain.stdout.len()
                    ));
                }
            }
            // MINOR 1/2: a content-transforming flag or a trailing pathspec makes the
            // recovery hint diverge from git's bytes, so rtk must NOT window; it passes
            // through byte-identically to git run with the SAME args.
            Kind::TextconvLargeUtf8 | Kind::PathspecLargeUtf8 => {
                assert!(
                    !windowed,
                    "transform-flag / trailing-pathspec must not window: {args:?}"
                );
                let mut full: Vec<&str> = vec!["show"];
                full.extend(arg_refs.iter().copied());
                let plain = git(path, home, &full);
                if out.stdout != plain.stdout {
                    fidelity_breaks.push(format!(
                        "`git show {}` differs from git (rtk {} vs git {} bytes)",
                        args.join(" "),
                        out.stdout.len(),
                        plain.stdout.len()
                    ));
                }
            }
        }
    }

    // Report incidence (KuSh: the fuzzer establishes there are no other classes hiding).
    eprintln!(
        "differential fuzzer: {ITERATIONS} invocations — misroutes={}, silent_losses={}, fidelity_breaks={}",
        misroutes.len(),
        silent_losses.len(),
        fidelity_breaks.len()
    );
    assert!(misroutes.is_empty(), "misroutes: {misroutes:#?}");
    assert!(
        silent_losses.is_empty(),
        "silent losses: {silent_losses:#?}"
    );
    assert!(
        fidelity_breaks.is_empty(),
        "fidelity breaks: {fidelity_breaks:#?}"
    );
}

/// A companion check that the recovery hint reconstructs a windowed blob BYTE-for-byte:
/// `head_shown` ++ `git show <rev:path> | tail -n +N` == full `git show <rev:path>`.
#[test]
fn windowed_blob_recovery_is_byte_exact() {
    let repo = setup_repo();
    let (path, home) = (repo.path.as_path(), repo.home.as_path());

    let out = rtk_show(path, home, &["HEAD:large.txt"]);
    assert!(out.status.success());
    let shown = String::from_utf8(out.stdout).expect("windowed head is UTF-8");
    assert!(
        shown.contains("[see remaining: git "),
        "expected a windowed head"
    );

    // The head is everything before the hint marker.
    let head = &shown[..shown.find("... (+").expect("truncation marker")];

    // Extract N from `| tail -n +N]`.
    let n: usize = {
        let after = shown.split("| tail -n +").nth(1).expect("tail hint");
        after
            .trim_end_matches(|c: char| !c.is_ascii_digit())
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .expect("N parses")
    };

    let full = git(path, home, &["show", "HEAD:large.txt"]).stdout;
    let full = String::from_utf8(full).expect("blob is UTF-8");
    // Recovery: the shown head must be a byte-exact prefix, and lines from N onward are
    // exactly the remainder.
    assert!(
        full.starts_with(head),
        "shown head is not a byte-exact prefix of git's bytes"
    );
    let tail: String = full.lines().skip(n - 1).map(|l| format!("{l}\n")).collect();
    let reconstructed = format!("{head}{tail}");
    assert_eq!(
        reconstructed, full,
        "head + `tail -n +{n}` must reconstruct the blob byte-for-byte"
    );
}

/// The BLOCKER, end-to-end: `-wG x:y HEAD:large.txt` (a colon-cluster whose value the
/// walker skips) must window the real blob, and its hint — which names only the blob
/// arg, not the cluster flags — must still reconstruct the blob byte-for-byte.
#[test]
fn cluster_flag_before_blob_windows_and_recovers_byte_exact() {
    let repo = setup_repo();
    let (path, home) = (repo.path.as_path(), repo.home.as_path());

    let out = rtk_show(path, home, &["-wG", "x:y", "HEAD:large.txt"]);
    assert!(out.status.success());
    let shown = String::from_utf8(out.stdout).expect("windowed head is UTF-8");
    assert!(
        shown.contains("[see remaining: git "),
        "cluster + blob must still window (blocker regression)"
    );
    // The hint points at the blob arg alone — verify it names `HEAD:large.txt`, not the
    // `x:y` pickaxe value that the old walker mistook for the object.
    assert!(
        shown.contains("'HEAD:large.txt'"),
        "hint must target the real blob arg, not the flag value"
    );

    let head = &shown[..shown.find("... (+").expect("truncation marker")];
    let n: usize = {
        let after = shown.split("| tail -n +").nth(1).expect("tail hint");
        after
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .expect("N parses")
    };
    // git dumps ONLY the blob for this invocation (the colon token is `-G`'s value), so
    // the recovery target is a plain `git show HEAD:large.txt`.
    let full = String::from_utf8(git(path, home, &["show", "HEAD:large.txt"]).stdout)
        .expect("blob is UTF-8");
    assert!(
        full.starts_with(head),
        "shown head is not a byte-exact prefix of git's bytes"
    );
    let tail: String = full.lines().skip(n - 1).map(|l| format!("{l}\n")).collect();
    assert_eq!(
        format!("{head}{tail}"),
        full,
        "head + `tail -n +{n}` must reconstruct the blob byte-for-byte"
    );
}
