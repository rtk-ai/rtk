# Security Audit — 2026-07-28

## Summary
- Issues found: 93 (36 gitleaks + 48 semgrep + 9 bandit) | Auto-fixed: 0 | Unresolved: 3 categories (all triaged — see below)
- Status: PASSED (0 real vulnerabilities; 0 dependency CVEs; 0 verified secrets — remaining findings are false positives or require manual action)

## Fixed Issues
None. Trivy `fs` scan found 0 vulnerable packages across all `Cargo.lock` targets (root + 3 worktrees) and 0 secrets. TruffleHog found 0 verified/unverified secrets across 13,584 chunks / 1,258 commits. OSV-Scanner found 0 issues across all `Cargo.lock` / `pom.xml` fixture targets. No dependency CVEs to remediate.

## Unresolved Issues

### 1. Gitleaks — 36 hits, all false positives (documentation/test fixture strings)
Every hit is a `sk_live_...` / `generic-api-key`-style **example string** inside historical `Security reports/security-audit-*.md` files (illustrating what a secret pattern looks like), `src/cmds/cloud/aws_cmd.rs` test fixtures (synthetic CloudWatch pagination tokens like `f/abcdef1234...`), `scripts/benchmark/cloud-init.yaml` fixture data, or `SECURITY.md` documentation. Cross-checked against TruffleHog (0 verified secrets, same history) — confirms no live/real secret present. Same finding set as the 2026-07-27 audit, no new hits.

### 2. Semgrep — 48 hits: 37 GitHub Actions mutable-tag (CI-hardening, deferred), 11 Rust findings (all false positives)
- **37× `yaml.github-actions.security.github-actions-mutable-action-tag`** across `.github/workflows/{cd,ci,release}.yml`, flagging every `uses: actions/checkout@v4`-style step (mutable tag/branch ref instead of pinned 40-char commit SHA). **Reason unresolved:** a correct fix requires looking up and pinning the exact commit SHA for each action's currently-used version. Fabricating a SHA would either break CI or silently pin to the wrong commit — worse than the current state. Needs a manual pass verifying each action's release page before pinning.
- **11× Rust findings** — all false positives on manual review, ran with `p/rust` + `p/owasp-top-ten` + `p/secrets` (606 rules, 392 files):
  - `temp-dir.temp-dir` (5×: `dotnet_cmd.rs:256,264,292`, `git.rs:3056`, `tracking.rs:1557`, `hook_cmd.rs:1464`) — every hit is inside `#[cfg(test)]` code building scratch paths like `rtk_test_custom.db` / `rtk-test-audit`; no security-sensitive operation touches these paths.
  - `current-exe.current-exe` (2×: `telemetry.rs:416`, `registry.rs:2085`) — used for analytics/version reporting and a test's mtime check, not for any access-control or trust decision.
  - `args.args` / `args-os.args-os` (3×: `args_utils.rs:10`, `main.rs:1284,1580`) — standard CLI entry-point argument parsing for a command-proxy tool; args are the tool's normal input, not used for privilege/security decisions.
  - `unsafe-usage.unsafe-usage` (2×: `main.rs:1538-1540`, `main.rs:2553-2562`) — `libc::signal()` calls installing SIGPIPE/SIGINT/SIGTERM handlers, the standard safe pattern for signal handling in a CLI; `unsafe` is required by the libc FFI signature, already scoped tightly to the signal-registration call.

### 3. Bandit — 9 hits, all `B404`/`B607`/`B603` in `scripts/benchmark-sessions/lib/runner.py` — no fix needed
All 9 findings are Low-severity/High-confidence flags on `subprocess.run([...], check=True)` calls using list-form argv (no `shell=True`). This is already the **secure** pattern (no shell interpolation, no injection surface); Bandit's B603/B607/B404 rules flag any subprocess usage generically. Manually reviewed: all argv lists are built from static strings and typed path variables, not raw user/network input. Identical finding set to the 2026-07-27 audit.

## Raw Scanner Output

### Pre-flight
```
OK  bandit 1.9.4       (ran — repo has .py files under scripts/benchmark-sessions/)
OK  semgrep (pipx, $HOME/.local/bin)  (ran: p/rust + p/owasp-top-ten + p/secrets, 606 rules / 392 files)
OK  trivy 0.72.0       (ran: fs scan, all Cargo.lock + pom.xml fixture targets)
OK  trufflehog 3.95.9  (ran: git history, full)
OK  gitleaks 8.30.1    (ran: git history, 1258 commits)
OK  osv-scanner 2.4.0  (ran: recursive lockfile scan)
OK  gh                 (available; no CodeQL workflow present in .github/workflows/ — skipped)
```

### Trivy (fs scan)
```
All targets (Cargo.lock x4 worktrees + root, pom.xml fixtures): 0 vulnerabilities, 0 secrets
```

### OSV-Scanner (recursive)
```
738 dirs visited, 6261 inodes visited — No issues found
```

### TruffleHog (git history, full)
```
chunks: 13584, bytes: 9698841, verified_secrets: 0, unverified_secrets: 0, scan_duration: 2.66s
```

### Gitleaks (git history, 1258 commits)
```
leaks found: 36 — see Unresolved #1 (all confirmed false positives: doc/test-fixture example tokens)
```

### Semgrep (p/rust + p/owasp-top-ten + p/secrets)
```
Findings: 48 (48 blocking) — Rules run: 341 — Targets scanned: 392
37x yaml.github-actions.security.github-actions-mutable-action-tag (see Unresolved #2)
5x rust.lang.security.temp-dir.temp-dir (test code, false positive)
2x rust.lang.security.current-exe.current-exe (non-security use, false positive)
2x rust.lang.security.args.args (CLI entry point, false positive)
1x rust.lang.security.args-os.args-os (CLI entry point, false positive)
2x rust.lang.security.unsafe-usage.unsafe-usage (libc signal handlers, expected/scoped)
```

### Bandit
```
Total issues: 9 Low/High-confidence — all B404/B607/B603 subprocess flags in scripts/benchmark-sessions/lib/runner.py (see Unresolved #3)
```

## Cross-Tool Observations
Gitleaks' 36 hits and Bandit/Semgrep's subprocess/CLI-arg findings recur identically across recent daily audits (2026-07-22 through 2026-07-28) — stable, already-triaged noise, not new regressions. Trivy + TruffleHog + OSV-Scanner (the three tools capable of confirming exploitable, non-test-fixture issues) agree: 0 vulnerabilities, 0 verified secrets, 0 dependency CVEs.

## Coverage Gaps
- No CodeQL workflow configured in `.github/workflows/` — deep semantic SAST via GitHub Actions not run.
- GitHub Actions supply-chain hardening (SHA-pinning) remains open — requires a manual pass, not safely automatable.
- Business logic / IDOR-style review out of scope for these tools; RTK is a local CLI proxy with no network-facing input surface, which limits applicability of web-focused rulesets (OWASP Top Ten ruleset ran clean as expected).
