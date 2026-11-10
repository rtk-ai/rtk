# Security Audit — 2026-07-26

## Summary
- Issues found: 93 (36 gitleaks + 48 semgrep + 9 bandit) | Auto-fixed: 0 | Unresolved: 4 categories (all triaged — see below)
- Status: ISSUES REMAINING (build failure — see Build Failure section; not a security issue, an environment gap: no Rust toolchain in this sandbox)

## Fixed Issues
None. Trivy `fs` scan found 0 vulnerable packages across all `Cargo.lock` targets (root + 3 worktrees) and 0 secrets; TruffleHog found 0 verified/unverified secrets across 13,422 chunks / 1,247 commits. No dependency CVEs to remediate.

## Unresolved Issues

### 1. GitHub Actions mutable action tags (3 workflow files) — UNRESOLVED
`cd.yml`, `ci.yml`, and a third workflow reference actions by mutable tag (`@v4`, `@v3`, `@v2`) instead of pinned commit SHA (e.g. `actions/checkout@v4`, `actions/create-github-app-token@v3`, `googleapis/release-please-action@v4`, `actions/download-artifact@v4`, `softprops/action-gh-release@v2`).
**Reason unresolved:** a correct fix requires looking up and pinning the exact 40-char commit SHA for each action's currently-used version. Guessing or fabricating a SHA would either break CI (wrong ref) or silently pin to the wrong commit — worse than the current state. Needs manual verification against each action's release page before pinning.

### 2. Gitleaks — 36 hits, all false positives (documentation/test fixture strings)
Every hit is a `sk_live_...` / `sk-...` / pagination-token-style **example string** inside historical `Security reports/security-audit-*.md` files (illustrating what a secret pattern looks like), `src/cmds/cloud/aws_cmd.rs` test fixtures (synthetic CloudWatch pagination tokens like `f/abcdef1234...`, not credentials), or `scripts/benchmark/cloud-init.yaml` fixture data. Cross-checked against TruffleHog, which found 0 verified secrets over the same history — confirms no live/real secret present.

### 3. Semgrep — 48 hits: 3 CI-hardening (see #1) + 45 inherent-to-CLI-design
| # | Component | Rule | Why not a fix |
|---|-----------|------|----------------|
| 1-4 | `dotnet_cmd.rs`, `git.rs`, `tracking.rs`, `hook_cmd.rs` | `temp-dir.temp-dir` | `std::env::temp_dir()` used only for test-fixture / scratch-file paths, not security-sensitive file creation |
| 5 | `args_utils.rs`, `main.rs` | `args.args` / `args-os.args-os` | `std::env::args()`/`args_os()` used for RTK's own CLI arg parsing (its actual job as a CLI proxy), not a security decision |
| 6 | `telemetry.rs` | `current-exe.current-exe` | `std::env::current_exe()` used to label telemetry pings with the binary path, not for auth/trust decisions |
| 7 | `registry.rs` | `current-exe.current-exe` | Used to check binary mtime for the `discover` cache invalidation, not security-relevant |
| 8-9 | `main.rs` | `unsafe-usage.unsafe-usage` | Two pre-existing `unsafe { libc::signal(...) }` blocks registering SIGPIPE/SIGINT/SIGTERM handlers — standard, narrowly-scoped libc FFI, no new code, no memory-safety issue introduced |

Rules run: 606 total (11 rust + owasp-top-ten + secrets + rust rulesets active), 390 files scanned, ~99.9% parsed. No OWASP Top 10 web-style findings (no injection, XSS, deserialization — expected: RTK is a local CLI proxy with no network-facing input surface).

### 4. Bandit — 9 hits, all `B603 subprocess_without_shell_equals_true` (Low/High-confidence) — no fix needed
All 9 findings are in `scripts/benchmark-sessions/lib/runner.py`, calling `subprocess.run([...], check=True)` with a list-form argv and no `shell=True`. This is already the **secure** pattern (no shell interpolation, no injection surface) — Bandit's B603 rule flags any `subprocess.run` call generically regardless of whether `shell=True` is actually set, since it can't fully prove absence of untrusted input. Manually reviewed: all argv lists are built from static strings and typed path variables, not raw user/network input.

## Raw Scanner Output

### Pre-flight
```
OK  bandit 1.9.4      (ran — repo has .py files under scripts/benchmark-sessions/ and hooks/hermes/)
OK  semgrep 1.170.0   (ran: p/owasp-top-ten + p/rust + p/secrets)
OK  trivy 0.72.0      (ran: fs scan, all Cargo.lock + pom.xml fixture targets)
OK  trufflehog 3.95.9 (ran: git history, full JSON)
OK  gitleaks 8.30.1   (ran: git history, 1247 commits)
OK  gh                (available; no CodeQL workflow present in .github/workflows/ — skipped)
```

### Trivy (fs scan)
```
All targets (Cargo.lock x4 worktrees + root, pom.xml fixtures): 0 vulnerabilities, 0 secrets
```

### TruffleHog (git history, full)
```
finished scanning: chunks=13422 bytes=9624331 verified_secrets=0 unverified_secrets=0
scan_duration=2.295705416s
```

### Gitleaks (git history, 1247 commits)
```
leaks found: 36  (all confirmed false-positive placeholder/fixture strings — see Unresolved #2 above)
```

### Semgrep (full repo, p/owasp-top-ten + p/rust + p/secrets)
```
Ran 606 rules on 390 files: 48 findings (3 CI-hardening + 45 triaged above — no code fix required)
```

### Bandit (Python files under scripts/ and hooks/)
```
Total issues: 9 Low-severity / High-confidence (B603 subprocess_without_shell_equals_true)
Total lines of code: 467
All 9 in scripts/benchmark-sessions/lib/runner.py — list-form subprocess.run, no shell=True (secure pattern)
```

## Build Failure

`cargo install --path . --force` failed: no Rust toolchain (`cargo`/`rustc`) present in this sandboxed session — `$HOME/.cargo/bin` and `$HOME/.rustup` do not exist, `which cargo`/`which rustc` resolve empty. This is the same environment gap noted in the 2026-07-24 run — the sandbox this scheduled task executes in has no Rust install. Binary was not rebuilt or reinstalled this run; the previously installed `rtk` at `$HOME/.local/bin/rtk` / `$HOME/.cargo/bin/rtk` remains in place, unchanged.

## Cross-Tool Observations
No cross-tool overlaps detected — gitleaks (secrets-pattern), semgrep (code-pattern), and bandit (Python subprocess) flagged disjoint categories. All were independently confirmed as non-issues: gitleaks via TruffleHog verified-secret cross-check (0 hits), semgrep via manual review of call sites, bandit via manual review of argv construction.

## Coverage Gaps
- CodeQL: not run — no `.github/workflows/codeql.yml` present in this repo; would need to be added as a GitHub Actions workflow to run in CI.
- mcps-audit: N/A — no `SKILL.md`/`.mcp*` files found outside `.claude/worktrees/` (which are excluded, being ephemeral worktree copies).
- Business logic / IDOR: not covered by SAST tooling — RTK has no auth or multi-tenant surface, low relevance.
- Runtime behavior / fuzzing: not covered by this static scan pass.
- GitHub Actions mutable-tag pinning (Unresolved #1): flagged but not auto-fixed this run — recommend a manual pass to pin action SHAs.
