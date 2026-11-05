# Security Audit — 2026-07-23

## Summary
- Issues found: 0 real (84 raw tool hits, all triaged as false positive / not applicable) | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

## Fixed Issues
(none — no real vulnerabilities found)

## Unresolved Issues
(none)

## Triage Notes

- **Gitleaks** — 36 hits, all example/placeholder secrets (fake `stripe-access-token` and `generic-api-key` literals — redacted here per policy) embedded in historical `Security reports/*.md` audit docs, `scripts/benchmark/cloud-init.yaml` sample config, `src/cmds/cloud/aws_cmd.rs` test fixtures, and `SECURITY.md` documentation examples. No real credentials.
- **Semgrep** (p/rust + p/owasp-top-ten, 307 rules / 387 files) — 48 findings, all in these classes, none applicable to RTK's threat model (local CLI proxy, no network-facing security boundary):
  - `temp-dir` (std::env::temp_dir() in test-only code paths — dotnet_cmd.rs, git.rs, tracking.rs, hook_cmd.rs)
  - `args` / `args-os` (std::env::args() used for CLI parsing, not security decisions — main.rs, args_utils.rs)
  - `current-exe` (std::env::current_exe() used for self-update/mtime checks, not auth — telemetry.rs, discover/registry.rs)
  - `unsafe-usage` (libc::signal SIGPIPE/SIGINT/SIGTERM handlers in main.rs — standard, audited pattern)
  - GitHub Actions `uses:` pin-by-tag warnings (informational, CI workflow)
- **Trivy** (fs scan) — 0 vulnerabilities, 0 secrets across all Cargo.lock / pom.xml fixture targets.
- **TruffleHog** (git history, 1219 commits, 13114 chunks) — 0 verified, 0 unverified secrets.
- **Bandit** (only `.py` file in repo: `scripts/benchmark-sessions/lib/runner.py`) — 3 Low/High-confidence `subprocess_without_shell_equals_true` (tar/git commands with `shell=False`, already the safe pattern) — informational, dev tooling script not shipped in the binary.

## Raw Scanner Output

### Gitleaks
```
1219 commits scanned.
scanned ~8500365 bytes (8.50 MB) in 881ms
leaks found: 36
```
All 36 findings are `stripe-access-token` / `generic-api-key` rule matches against literal placeholder strings (fake Stripe-style and generic API key examples — values redacted per policy, pattern only) in:
- `Security reports/security-audit-*.md` (18 prior daily audit docs — self-referential example text)
- `scripts/benchmark/cloud-init.yaml` (2 hits — sample cloud-init doc)
- `src/cmds/cloud/aws_cmd.rs` (4 hits — AWS CLI output test fixtures, e.g. CloudWatch `nextForwardToken`)
- `SECURITY.md` (1 hit — documentation example)

### Semgrep
```
Scanning 387 files tracked by git with 571 Code rules
Ran 307 rules on 387 files: 48 findings (48 blocking)
```
Rule breakdown: `rust.lang.security.temp-dir.temp-dir`, `rust.lang.security.args.args`, `rust.lang.security.args-os.args-os`, `rust.lang.security.current-exe.current-exe`, `rust.lang.security.unsafe-usage.unsafe-usage`, plus GitHub Actions `uses:` version-pin advisories in `.github/workflows/*`.

### Trivy
```
Cargo.lock (root + 3 worktrees): 0 vulnerabilities
tests/fixtures/**/pom.xml (4 files, root + 3 worktrees): 0 vulnerabilities
Secrets: 0 across all targets
```

### TruffleHog
```
chunks: 13114, bytes: 9507588, verified_secrets: 0, unverified_secrets: 0
scan_duration: 2.357269042s
```

### Bandit
```
Total lines of code: 133
Total issues: Low: 3, Medium: 0, High: 0
Confidence: High: 3
```
All 3 are `B603/B607 subprocess_without_shell_equals_true` in `runner.py:28` (`tar czf` invocation via list-args, `shell=False` — the bandit-recommended safe form; flagged only because it doesn't pass `shell=True` explicitly to disable it).

## Coverage Gaps
- CodeQL: not run (no `.github/workflows/codeql.yml` present in this scan; GitHub Actions-based deep SAST not configured for this repo).
- mcps-audit: no MCP/skill manifest files found in target path — not applicable.
- Business logic / IDOR: out of scope for a local CLI tool with no network auth boundary.

## Build Failure

`cargo install --path . --force` failed: no Rust toolchain available in this sandbox environment (`cargo`/`rustc`/`rustup` all absent from PATH, `~/.cargo` does not exist). Same gap noted in the 2026-07-22 audit. Cargo.toml version at merge time: `0.42.4`. Binary was not rebuilt or reinstalled this run — commit/push proceeded per protocol regardless.
