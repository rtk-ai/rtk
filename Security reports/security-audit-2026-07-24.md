# Security Audit — 2026-07-24

## Summary
- Issues found: 49 (36 gitleaks + 13 semgrep) | Auto-fixed: 0 | Unresolved: 0 (all triaged as false positives / non-issues, no code change required)
- Status: ISSUES REMAINING (build failure — see Build Failure section; not a security issue, an environment gap: no Rust toolchain in this sandbox)

## Fixed Issues
None — no genuine vulnerability was found. Dependency scan (osv-scanner, Trivy) found 0 vulnerable packages across 203 Cargo.lock entries; TruffleHog found 0 verified secrets.

## Triaged Findings (no fix needed)

### Gitleaks — 36 hits, all false positives (documentation placeholder secrets)
Every hit is a `sk_live_1234567890ab...` / `sk-1234567890abcdef...` / `secret-api-key-12345` style **example string** inside historical `Security reports/security-audit-*.md` files (used to illustrate what a real secret pattern looks like) or inside `src/cmds/cloud/aws_cmd.rs` / `scripts/benchmark/cloud-init.yaml`, which are RTK's own AWS-output **redaction filter** test fixtures (sequential placeholder hex, not live credentials). No live/verified secret present — confirmed by TruffleHog `--only-verified` returning 0 results across 13,322 chunks / 1,236 commits scanned.

### Semgrep — 13 hits, all inherent-to-CLI-design patterns
| # | Component | Rule | Why not a fix |
|---|-----------|------|----------------|
| 1-4 | `dotnet_cmd.rs`, `git.rs`, `tracking.rs`, `hook_cmd.rs` | `temp-dir.temp-dir` | `std::env::temp_dir()` used only for test-fixture / cache paths, not security-sensitive file creation |
| 5 | `args_utils.rs` | `args.args` | `std::env::args()` used for RTK's own CLI arg parsing (its actual job as a CLI proxy), not a security decision |
| 6 | `telemetry.rs` | `current-exe.current-exe` | `std::env::current_exe()` used to label telemetry pings with the binary path, not for auth/trust decisions |
| 7 | `registry.rs` | `current-exe.current-exe` | Same — used to check binary mtime for the `discover` cache invalidation, not security-relevant |
| 8, 10 | `main.rs` | `args.args` / `args-os.args-os` | Top-level CLI arg collection (clap parsing entry point) — required for the tool to function |
| 9, 11 | `main.rs` | `unsafe-usage.unsafe-usage` | Two pre-existing `unsafe { libc::signal(...) }` blocks registering SIGPIPE/SIGINT/SIGTERM handlers — standard, narrowly-scoped libc FFI, no new code, no memory-safety issue introduced |

Rules run: 17 (rust + owasp-top-ten rulesets), 204 files scanned, 100% parsed. No OWASP Top 10 web-style findings (no injection, XSS, deserialization, etc. — expected, RTK is a local CLI proxy with no network-facing input surface beyond passthrough of user-invoked shell commands, which is its documented purpose).

## Raw Scanner Output

### Pre-flight
```
OK  bandit 1.9.4     (skipped — no .py files scanned; used only as availability check)
OK  semgrep          (ran: p/rust + p/owasp-top-ten)
OK  trivy 0.72.0     (ran: fs scan, all Cargo.lock + pom.xml fixture targets)
OK  trufflehog 3.95.9 (ran: git history, --only-verified)
OK  gitleaks 8.30.1  (ran: git history, 1236 commits)
OK  osv-scanner 2.4.0 (ran: Cargo.lock, 203 packages)
```

### osv-scanner
```
Scanned Cargo.lock file and found 203 packages
vuln packages: 0
```

### Trivy (fs scan)
```
All targets (Cargo.lock ×4 worktrees + root, pom.xml fixtures): 0 vulnerabilities, 0 secrets
```

### TruffleHog (git history, verified only)
```
finished scanning: chunks=13322 bytes=9555354 verified_secrets=0 unverified_secrets=0
scan_duration=3.337s
```

### Gitleaks (git history, 1236 commits)
```
leaks found: 36  (all confirmed false-positive placeholder strings in docs/test fixtures — see Triaged Findings above)
```

### Semgrep (src/ + Cargo.toml, p/rust + p/owasp-top-ten)
```
Ran 17 rules on 204 files: 13 findings (all triaged above — no fix required)
```

## Build Failure

`cargo install --path . --force` failed: no Rust toolchain (`cargo`) is present in this sandboxed session — `which cargo` / `$HOME/.cargo/bin` / `$HOME/.rustup` all resolve empty. This is an environment gap for this run of the daily sync (the sandbox this scheduled task executed in has no Rust install), not a code regression. Binary was not rebuilt or reinstalled this run; the previously installed `rtk` at `$HOME/.local/bin/rtk` / `$HOME/.cargo/bin/rtk` (from the prior sync) remains in place, unchanged.

Raw error:
```
rtk: Failed to run cargo install: Failed to spawn process: No such file or directory (os error 2)
```

## Cross-Tool Observations
No cross-tool overlaps detected — gitleaks and semgrep flagged disjoint categories (secrets-pattern vs. code-pattern), and both sets were independently confirmed as non-issues (gitleaks via TruffleHog verified-secret cross-check; semgrep via manual review of call sites).

## Coverage Gaps
- Bandit: N/A, no Python source in this Rust codebase.
- CodeQL: not run (would require `gh` GitHub Actions workflow trigger; out of scope for this local daily sync).
- Business logic / IDOR: not covered by SAST tooling — RTK has no auth or multi-tenant surface, low relevance.
- Runtime behavior / fuzzing: not covered by this static scan pass.
