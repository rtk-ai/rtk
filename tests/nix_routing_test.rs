//! `rtk nix` filters the stdout of `nix search`, forwards its stderr, and
//! passes everything else through: other subcommands, `--json` and `-v`. Stubs
//! `nix` on PATH with a script that replays real nix output (2.36; the
//! progress lines are nix 2.35's). One test, so the stubs are never written
//! while another thread of this binary forks (an executable open for writing
//! cannot be run).

#![cfg(unix)]

mod common;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/nix/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn read(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("read fixture")
}

fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// Runs `rtk nix <args>` against a stub that replays, by its arguments, the
/// first branch that matches:
/// - first `build`: a failed build's log on stderr, exit 1;
/// - first `-v`: the `^jq` results on stdout, progress and a no-match error
///   on stderr, exit 1;
/// - third `nomatch`: progress and a no-match error on stderr, exit 1;
/// - third `everything`: one JSON line of 11 MiB on stdout;
/// - second `-f`: its stdin on stdout (`nix search -f - …`);
/// - any `--json`: a dirty flake's JSON on stdout, its warning on stderr;
/// - otherwise the `^jq` results on stdout, the warning on stderr.
///
/// `stdin` is written to rtk's stdin.
fn rtk_nix(args: &[&str], stdin: &str) -> std::process::Output {
    let dir = common::tempdir();
    let stub = dir.path().join("nix");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = build ]; then cat {build} >&2; exit 1; fi\n\
             if [ \"$1\" = -v ]; then cat {jq}; cat {progress} >&2; exit 1; fi\n\
             if [ \"$3\" = nomatch ]; then cat {progress} >&2; exit 1; fi\n\
             if [ \"$3\" = everything ]; then printf '{{\"a\":\"'; head -c 11534336 /dev/zero | tr '\\0' x; printf '\"}}\\n'; exit 0; fi\n\
             if [ \"$2\" = -f ]; then cat; exit 0; fi\n\
             for a in \"$@\"; do\n\
             \x20 if [ \"$a\" = --json ]; then cat {json}; cat {warning} >&2; exit 0; fi\n\
             done\n\
             cat {jq}; cat {warning} >&2\n",
            build = shell_quote(&fixture("build_fail_stderr.txt")),
            jq = shell_quote(&fixture("search_jq_stdout.txt")),
            json = shell_quote(&fixture("search_dirty_flake_json_stdout.json")),
            progress = shell_quote(&fixture("search_nix235_no_match_stderr.txt")),
            warning = shell_quote(&fixture("search_dirty_flake_stderr.txt")),
        ),
    )
    .expect("write stub");
    let mut perms = std::fs::metadata(&stub).expect("stat stub").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&stub, perms).expect("chmod stub");

    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut child = common::rtk_command()
        .env("PATH", path)
        .current_dir(dir.path())
        .arg("nix")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn rtk");
    std::io::Write::write_all(&mut child.stdin.take().expect("stdin"), stdin.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait rtk")
}

#[test]
fn search_is_filtered_everything_else_passes_through() {
    let out = rtk_nix(&["build", "--no-link", "-f", "."], "");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        read("build_fail_stderr.txt"),
        "builder log and `nix log` hint must pass through"
    );

    let out = rtk_nix(&["search", "git+file:///repo", "demo", "--json"], "");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        read("search_dirty_flake_json_stdout.json")
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        read("search_dirty_flake_json_stderr.txt")
    );

    // `nix search nixpkgs ^ --json` is one line of 19 MB: past the runner's
    // 10 MiB capture it would be dropped whole, so `--json` passes through.
    let out = rtk_nix(&["search", "nixpkgs", "everything", "--json"], "");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(out.stdout.len(), 11_534_336 + "{\"a\":\"\"}\n".len());

    // `-v` prints a progress line per attribute that can push the error nix
    // prints last past the runner's capture: such a search is left as printed.
    let out = rtk_nix(&["-v", "search", "nixpkgs", "^jq"], "");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        read("search_jq_stdout.txt")
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        read("search_nix235_no_match_stderr.txt")
    );

    // A search: stdout compacted, stderr forwarded on stderr.
    let out = rtk_nix(&["search", "nixpkgs", "^jq"], "");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout.starts_with("Nix: 24 results (showing 20)\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\n… +4 more, refine the query\n"),
        "{stdout}"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        read("search_dirty_flake_stderr.txt")
    );

    // A failed search: its stderr, the error included, reaches the agent whole.
    let out = rtk_nix(&["search", "nixpkgs", "nomatch"], "");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        read("search_nix235_no_match_stderr.txt")
    );

    // `nix search -f - …` reads its expression from the agent's stdin.
    let out = rtk_nix(
        &["search", "-f", "-", "", "jq"],
        &read("search_jq_stdout.txt"),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout.starts_with("Nix: 24 results (showing 20)\n"),
        "{stdout}"
    );
}
