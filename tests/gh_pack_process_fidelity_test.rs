#![cfg(unix)]
//! Process-level fidelity contract for the packed `gh` paths.
//!
//! `jsonpack`'s unit tests prove the *encoding* is lossless. They cannot
//! prove the *plumbing* is: capturing a child's stdout to filter it can
//! silently break things no JSON test would notice. A pre-submission
//! review found exactly that class — with a line-oriented capture, `gh
//! api` lost stdin (`--input -` sent empty bodies), dropped any single
//! line beyond the buffer cap (empty stdout, exit 0), and mangled binary
//! bodies through lossy UTF-8 decoding.
//!
//! These tests run the real rtk binary against a fake `gh` on PATH and
//! assert byte-level outcomes, so those three failures can never return
//! unnoticed.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// A fake `gh` on PATH plus an isolated tracking DB, so tests never touch
/// the developer's real rtk analytics.
struct FakeGh {
    dir: TempDir,
}

impl FakeGh {
    /// `body` is the shell script body of the fake gh (without shebang).
    fn new(body: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir bin");
        let gh = bin.join("gh");
        fs::write(&gh, format!("#!/usr/bin/env bash\n{body}\n")).expect("write fake gh");
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).expect("chmod");
        Self { dir }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let path = format!(
            "{}:{}",
            self.dir.path().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut c = Command::new(env!("CARGO_BIN_EXE_rtk"));
        c.args(args)
            .env("PATH", path)
            .env("RTK_DB_PATH", self.dir.path().join("tracking.db"));
        c
    }

    /// Run rtk and return (stdout_bytes, exit_code).
    fn run(&self, args: &[&str]) -> (Vec<u8>, Option<i32>) {
        let out = self
            .cmd(args)
            .stdin(Stdio::null())
            .output()
            .expect("run rtk");
        (out.stdout, out.status.code())
    }

    /// Run rtk with `input` on stdin; returns stdout as a String.
    fn run_stdin(&self, args: &[&str], input: &[u8]) -> String {
        let mut child = self
            .cmd(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn rtk");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input)
            .expect("write stdin");
        let out = child.wait_with_output().expect("wait");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

#[test]
fn gh_api_forwards_stdin_to_gh() {
    // `gh api graphql --input -` reads the request body from stdin. A
    // capture that nulls stdin makes gh send an empty body — the request
    // fails or, worse, a mutation silently does nothing.
    let fake = FakeGh::new(r#"n=$(cat | wc -c | tr -d ' '); printf '{"stdin_bytes":%s}' "$n""#);
    let body = br#"{"query":"query{viewer{login}}"}"#;
    let out = fake.run_stdin(&["gh", "api", "graphql", "--input", "-"], body);
    assert_eq!(
        out,
        format!("{{\"stdin_bytes\":{}}}", body.len()),
        "gh must receive the {} stdin bytes rtk was given",
        body.len()
    );
}

#[test]
fn gh_api_emits_huge_single_line_body_intact() {
    // GitHub returns minified (single-line) JSON when piped, and
    // --paginate concatenations get large. A line-oriented capture drops
    // an over-long line wholesale: empty stdout with exit 0, the worst
    // possible failure — silent, and indistinguishable from "no data".
    // Must exceed the line-oriented capture's 10MiB cap, or the test
    // cannot tell a fixed build from a broken one (verified by mutation:
    // a 2.7MB payload passes even with the dropping capture in place).
    let payload = format!(
        "[{}]",
        (0..1_800_000)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("big.json");
    fs::write(&path, &payload).expect("write payload");
    let fake = FakeGh::new(&format!("cat {}", path.display()));

    let (stdout, code) = fake.run(&["gh", "api", "repos/o/r/nums"]);
    assert_eq!(code, Some(0));
    // Scalar array: unpackable, so the bytes must arrive exactly as gh
    // produced them — any drop or truncation shows up here.
    assert_eq!(
        stdout.len(),
        payload.len(),
        "expected {} bytes, got {}",
        payload.len(),
        stdout.len()
    );
    assert_eq!(stdout, payload.as_bytes());
}

#[test]
fn gh_api_streams_body_beyond_pack_cap_verbatim() {
    // Past the pack cap rtk stops buffering and streams the remainder
    // through. The switch must stay byte-faithful and must not truncate.
    let unit = "0123456789abcdef".repeat(4); // 64 B
    let mut payload = String::from("\"");
    while payload.len() < 34 * 1024 * 1024 {
        payload.push_str(&unit);
    }
    payload.push('"');
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("huge.json");
    fs::write(&path, &payload).expect("write payload");
    let fake = FakeGh::new(&format!("cat {}", path.display()));

    let (stdout, code) = fake.run(&["gh", "api", "repos/o/r/huge"]);
    assert_eq!(code, Some(0));
    assert_eq!(
        stdout.len(),
        payload.len(),
        "streamed output must keep every byte"
    );
    assert_eq!(stdout, payload.as_bytes());
}

#[test]
fn gh_api_keeps_binary_bodies_byte_exact() {
    // `gh api repos/o/r/tarball > f.tgz` is gh's documented download
    // pattern. Lossy UTF-8 decoding replaces every invalid byte with
    // U+FFFD, producing a file that is not a gzip at all.
    let gzip: Vec<u8> = vec![
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x2b, 0x2e, 0x49, 0x2c, 0xe6,
        0x02, 0x00, 0xed, 0x64, 0x6f, 0x9c, 0x06, 0x00, 0x00, 0x00,
    ];
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("blob.tgz");
    fs::write(&path, &gzip).expect("write blob");
    let fake = FakeGh::new(&format!("cat {}", path.display()));

    let (stdout, code) = fake.run(&["gh", "api", "repos/o/r/tarball"]);
    assert_eq!(code, Some(0));
    assert_eq!(stdout, gzip, "binary body must survive byte-for-byte");
}

#[test]
fn gh_api_packs_tabular_bodies() {
    // Control for the three fidelity tests above: the feature must still
    // do its job, or they would all pass on a no-op implementation.
    let fake = FakeGh::new(
        r#"printf '[{"id":1,"name":"alice-the-first","tag":"x"},{"id":2,"name":"bob-the-second","tag":"y"},{"id":3,"name":"carol-the-third","tag":"z"}]'"#,
    );
    let (stdout, code) = fake.run(&["gh", "api", "repos/o/r/list"]);
    assert_eq!(code, Some(0));
    let text = String::from_utf8(stdout).expect("utf8");
    assert_eq!(
        text.lines().next(),
        Some("[3]{id:int,name:string,tag:string}"),
        "got: {text}"
    );
    assert!(text.contains("alice-the-first"), "got: {text}");
}

#[test]
fn gh_api_error_body_passes_through_with_exit_code() {
    // On failure the caller needs gh's own message verbatim, and the
    // exit code must survive so scripts and CI still branch correctly.
    let fake = FakeGh::new(r#"printf '{"message":"Not Found","status":"404"}'; exit 1"#);
    let (stdout, code) = fake.run(&["gh", "api", "repos/o/nope"]);
    assert_eq!(code, Some(1), "gh's exit code must propagate");
    assert_eq!(stdout, br#"{"message":"Not Found","status":"404"}"#);
}
