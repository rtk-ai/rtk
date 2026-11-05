# Security Audit — 2026-07-27

## Summary
- Issues found: 80 (36 gitleaks + 35 semgrep + 9 bandit) | Auto-fixed: 0 | Unresolved: 3 categories (all triaged — see below)
- Status: ISSUES REMAINING (build failure — see Build Failure section; not a security issue, an environment gap: no Rust toolchain in this sandbox)

## Fixed Issues
None. Trivy `fs` scan found 0 vulnerable packages across all `Cargo.lock` targets (root + 3 worktrees) and 0 secrets; TruffleHog found 0 verified/unverified secrets across 13,440 chunks / 1,250 commits. No dependency CVEs to remediate.

## Unresolved Issues

### 1. Gitleaks — 36 hits, all false positives (documentation/test fixture strings)
Every hit is a `sk_live_...` / `sk-...` / pagination-token-style **example string** inside historical `Security reports/security-audit-*.md` files (illustrating what a secret pattern looks like), `src/cmds/cloud/aws_cmd.rs` test fixtures (synthetic CloudWatch pagination tokens like `f/abcdef1234...`, `secret-api-key-12345` — not credentials), or `scripts/benchmark/cloud-init.yaml` fixture data (`sk-1234567890abcdef`). Cross-checked against TruffleHog, which found 0 verified secrets over the same history — confirms no live/real secret present.

### 2. Semgrep — 35 hits, all GitHub Actions mutable-tag (CI-hardening, not exploitable today)
All 35 findings are `yaml.github-actions.security.github-actions-mutable-action-tag` across `.github/workflows/*.yml`, flagging every `uses: actions/checkout@v4`-style step (mutable tag/branch ref instead of pinned 40-char commit SHA).
**Reason unresolved:** a correct fix requires looking up and pinning the exact commit SHA for each action's currently-used version. Fabricating a SHA would either break CI or silently pin to the wrong commit — worse than the current state. Needs a manual pass to verify each action's release page before pinning. Ran `p/python` + `p/owasp-top-ten` rulesets (301 files scanned) — no OWASP Top 10 web-style findings (expected: RTK is a local CLI proxy, no network-facing input surface).

### 3. Bandit — 9 hits, all `B603 subprocess_without_shell_equals_true` (Low/High-confidence) — no fix needed
All 9 findings are in `scripts/benchmark-sessions/lib/runner.py`, calling `subprocess.run([...], check=True)` with a list-form argv and no `shell=True`. This is already the **secure** pattern (no shell interpolation, no injection surface) — Bandit's B603 rule flags any `subprocess.run` call generically regardless of whether `shell=True` is actually set. Manually reviewed: all argv lists are built from static strings and typed path variables, not raw user/network input.

## Raw Scanner Output

### Pre-flight
```
OK  bandit 1.9.4      (ran — repo has .py files under scripts/benchmark-sessions/ and hooks/hermes/)
OK  semgrep (pipx, $HOME/.local/bin)  (ran: p/python + p/owasp-top-ten)
OK  trivy 0.72.0      (ran: fs scan, all Cargo.lock + pom.xml fixture targets)
OK  trufflehog 3.95.9 (ran: git history, full JSON)
OK  gitleaks 8.30.1   (ran: git history, 1250 commits)
OK  gh                (available; no CodeQL workflow present in .github/workflows/ — skipped)
```

### Trivy (fs scan)
```
All targets (Cargo.lock x4 worktrees + root, pom.xml fixtures): 0 vulnerabilities, 0 secrets
```

### TruffleHog (git history, full)
```
finished scanning: chunks=13440 bytes=9654030 verified_secrets=0 unverified_secrets=0
scan_duration=2.346316292s
```

### Gitleaks (git history, 1250 commits)
```
leaks found: 36  (all confirmed false-positive placeholder/fixture strings — see Unresolved #1 above)
```

### Semgrep (repo excl. worktrees/target, p/python + p/owasp-top-ten)
```
Ran 300 rules on 351 files: 35 findings (all github-actions-mutable-action-tag — triaged above)
```

### Bandit (Python files under scripts/ and hooks/, excl. worktree dupes)
```
Total issues: 9 Low-severity / High-confidence (B603 subprocess_without_shell_equals_true)
Total lines of code: 467
All 9 in scripts/benchmark-sessions/lib/runner.py — list-form subprocess.run, no shell=True (secure pattern)
```

## Build Failure

`cargo install --path . --force` failed: no Rust toolchain (`cargo`/`rustc`) present in this sandboxed session — `which cargo`/`which rustc` resolve empty. Same environment gap noted in prior daily runs (2026-07-24, 2026-07-26) — the sandbox this scheduled task executes in has no Rust install. Binary was not rebuilt or reinstalled this run; the previously installed `rtk` at `$HOME/.local/bin/rtk` / `$HOME/.cargo/bin/rtk` remains in place, unchanged.

## Cross-Tool Observations
No cross-tool overlaps detected — gitleaks (secrets-pattern), semgrep (CI YAML), and bandit (Python subprocess) flagged disjoint categories. All independently confirmed as non-issues: gitleaks via TruffleHog verified-secret cross-check (0 hits), semgrep via manual review (mutable tags are a hardening recommendation, not an active exploit), bandit via manual review of argv construction.

## Coverage Gaps
- CodeQL: not run — no `.github/workflows/codeql.yml` present in this repo.
- mcps-audit: N/A — no `SKILL.md`/`.mcp*` files found outside `.claude/worktrees/` (ephemeral, excluded).
- Business logic / IDOR: not covered by SAST tooling — RTK has no auth or multi-tenant surface, low relevance.
- Runtime behavior / fuzzing: not covered by this static scan pass.
- GitHub Actions mutable-tag pinning (Unresolved #2): flagged but not auto-fixed this run — recommend a manual pass to pin action SHAs.
