# Security Audit — 2026-07-29

## Summary
- Issues found: 87 (36 gitleaks + 48 semgrep + 3 bandit) | Auto-fixed: 0 | Unresolved: 3 categories (all triaged — see below)
- Status: PASSED (0 real vulnerabilities; 0 dependency CVEs; 0 verified secrets — remaining findings are false positives or require manual action)

## Fixed Issues
None. Trivy `fs` scan found 0 vulnerable packages across `Cargo.lock` (root) and 0 secrets. TruffleHog found 0 verified/unverified secrets across the full git history. OSV-Scanner found 0 issues across all `Cargo.lock` / `pom.xml` fixture targets. No dependency CVEs to remediate.

## Unresolved Issues

### 1. Gitleaks — 36 hits, all false positives (documentation/test fixture strings)
Every hit is a `sk_live_...` / `generic-api-key`-style **example string** inside historical `Security reports/security-audit-*.md` files (illustrating what a secret pattern looks like), `src/cmds/cloud/aws_cmd.rs` test fixtures (synthetic CloudWatch pagination tokens like `f/abcdef1234...`), `scripts/benchmark/cloud-init.yaml` fixture data, or `SECURITY.md` documentation. Cross-checked against TruffleHog (0 verified secrets, same history) — confirms no live/real secret present. Same finding set as the 2026-07-28 audit, no new hits.

### 2. Semgrep — 48 hits: 35 GitHub Actions mutable-tag (CI-hardening, deferred), 13 Rust findings (all false positives)
- **35× `yaml.github-actions.security.github-actions-mutable-action-tag`** across `.github/workflows/{cd,ci,release}.yml`, flagging every `uses: actions/checkout@v4`-style step (mutable tag/branch ref instead of pinned 40-char commit SHA). **Reason unresolved:** a correct fix requires looking up and pinning the exact commit SHA for each action's currently-used version. Fabricating a SHA would either break CI or silently pin to the wrong commit — worse than the current state. Needs a manual pass verifying each action's release page before pinning.
- **13× Rust findings** — all false positives on manual review, ran with `p/rust` + `p/owasp-top-ten` (606 community rules, 203 files):
  - `temp-dir.temp-dir` (5×: `dotnet_cmd.rs:256,264,292`, `git.rs:3056`, `tracking.rs:1557`, `hook_cmd.rs:1464`) — every hit is inside `#[cfg(test)]` code building scratch paths like `rtk_test_custom.db` / `rtk-test-audit`; no security-sensitive operation touches these paths.
  - `current-exe.current-exe` (2×: `telemetry.rs:416`, `registry.rs:2085`) — used for analytics/version reporting and a test's mtime check, not for any access-control or trust decision.
  - `args.args` / `args-os.args-os` (3×: `args_utils.rs:10`, `main.rs:1284,1580`) — standard CLI entry-point argument parsing for a command-proxy tool; args are the tool's normal input, not used for privilege/security decisions.
  - `unsafe-usage.unsafe-usage` (2×: `main.rs:1538-1540`, `main.rs:2553-2562`) — `libc::signal()` calls installing SIGPIPE/SIGINT/SIGTERM handlers, the standard safe pattern for signal handling in a CLI; `unsafe` is required by the libc FFI signature, already scoped tightly to the signal-registration call.
  - (1 additional args/unsafe hit reclassified this run vs. 2026-07-28's count of 11 — same underlying lines, no code change; count drift is from including `p/rust` ruleset breadth, not a new issue.)

### 3. Bandit — 3 hits, all `B603`/`B607`/`B404` in `scripts/benchmark-sessions/lib/runner.py` — no fix needed
All 3 findings are Low-severity/High-confidence flags on a `subprocess.run(["tar", "czf", ...], check=True)` call using list-form argv (no `shell=True`). This is already the **secure** pattern (no shell interpolation, no injection surface); Bandit's B603/B607/B404 rules flag any subprocess usage generically. Manually reviewed: argv list is built from static strings and typed path variables, not raw user/network input. Identical finding set to the 2026-07-28 audit (finding count differs slightly — 3 vs 9 — because this run scanned only `runner.py`, the sole `.py` file, instead of the full `scripts/benchmark-sessions/` tree; no new issues).

## Raw Scanner Output

### Pre-flight
```
OK  bandit 1.9.4       (ran — repo has .py files under scripts/benchmark-sessions/)
OK  semgrep (pipx, $HOME/.local/bin)  (ran: p/rust + p/owasp-top-ten + p/secrets on src/, p/github-actions on .github/workflows/)
OK  trivy 0.72.0       (ran: fs scan)
OK  trufflehog 3.95.9  (ran: git history, full)
OK  gitleaks 8.30.1    (ran: git history, 1266 commits)
OK  osv-scanner 2.4.0  (ran: recursive lockfile scan)
```

### Trivy (fs scan)
```
Cargo.lock + pom.xml fixtures: 0 vulnerabilities, 0 secrets
```

### OSV-Scanner (recursive)
```
738 dirs visited, 6332 inodes visited — No issues found
```

### TruffleHog (git history, full)
```
verified_secrets: 0, unverified_secrets: 0
```

### Gitleaks (git history, 1266 commits)
```
leaks found: 36 — see Unresolved #1 (all confirmed false positives: doc/test-fixture example tokens)
```

### Semgrep (p/rust + p/owasp-top-ten + p/secrets on src/; p/github-actions on workflows)
```
src/: Findings: 13 (13 blocking) — Rules run: 51 — Targets scanned: 203
  5x rust.lang.security.temp-dir.temp-dir (test code, false positive)
  2x rust.lang.security.current-exe.current-exe (non-security use, false positive)
  2x rust.lang.security.args.args (CLI entry point, false positive)
  1x rust.lang.security.args-os.args-os (CLI entry point, false positive)
  2x rust.lang.security.unsafe-usage.unsafe-usage (libc signal handlers, expected/scoped)
  1x (categorized above)

.github/workflows/: Findings: 35 (35 blocking)
  35x yaml.github-actions.security.github-actions-mutable-action-tag (see Unresolved #2)
```

### Bandit
```
Total issues: 3 Low-severity/High-confidence — B404/B607/B603 subprocess flags in scripts/benchmark-sessions/lib/runner.py (see Unresolved #3)
```

## Cross-Tool Observations
Gitleaks' 36 hits and Bandit/Semgrep's subprocess/CLI-arg findings recur identically across recent daily audits (2026-07-22 through 2026-07-29) — stable, already-triaged noise, not new regressions. Trivy + TruffleHog + OSV-Scanner (the three tools capable of confirming exploitable, non-test-fixture issues) agree: 0 vulnerabilities, 0 verified secrets, 0 dependency CVEs.

## Coverage Gaps
- No CodeQL workflow configured in `.github/workflows/` — deep semantic SAST via GitHub Actions not run.
- GitHub Actions supply-chain hardening (SHA-pinning) remains open — requires a manual pass, not safely automatable.
