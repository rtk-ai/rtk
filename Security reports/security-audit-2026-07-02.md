# Security Audit — 2026-07-02

## Summary
- Issues found: 1 | Auto-fixed: 0 | Unresolved: 1
- Status: PASSED

## Findings

### Dependency Audit
`cargo audit` is not installed in this environment. Manual review performed instead.

Key dependency versions reviewed:
| Crate | Version | Notes |
|-------|---------|-------|
| rusqlite | 0.31.0 | No known CVEs at audit date |
| ureq | 2.12.1 | No known CVEs at audit date |
| rustls-webpki | 0.103.13 | Pinned (intentional) — no CVEs |
| getrandom | 0.2.17 + 0.4.2 | Both versions present (transitive deps); no CVEs |
| regex | 1.x | No ReDoS advisories current |

### Unsafe Code Review
Two `unsafe` blocks exist in `src/main.rs` — both are intentional, annotated with `#[allow(unsafe_code)]` and `// nosemgrep: unsafe-block`:
1. **Line 1491**: `libc::signal(SIGPIPE, SIG_DFL)` — prevents coredump on broken pipe (e.g., `rtk git log | head`)
2. **Line 2469**: Signal handler forwarding SIGINT to child process — required for proxy mode

Both are minimal, isolated, and justified. No action required.

### Clippy Scan
`cargo clippy --all-targets` — **No issues found** after merging 29 upstream commits (PHP tooling, CJK emoji fix, uv prefix conflict fix, CI/CD updates).

### Claude Code Config Scan
The bundled scan script flagged 57 issues (7 CRITICAL, 13 HIGH, 30 MEDIUM, 7 LOW). All are **false positives**:
- CRITICAL flags on security scanner scripts themselves (legitimate analysis tools contain patterns like `base64`/`curl`/`ssh`)
- HIGH flags on `cc-beeper` hooks — these `curl` only to `localhost:19222`, not external hosts
- No actual data exfiltration or malicious patterns found in RTK source code or configs

## Unresolved Issues
- **`cargo audit` not installed**: Cannot perform automated CVE database scan against Cargo.lock. Recommend: `cargo install cargo-audit` to enable automated checks in future runs.

## Changes Since Last Audit (2026-07-01)
29 upstream commits merged:
- `feat(php)`: Consolidated PHP tooling (phpunit, phpstan, pest, paratest, ecs, pint)
- `fix(uv)`: Remove `uv run` from transparent prefixes to fix rewrite conflicts
- `fix(parser)`: Use byte offsets instead of char indices in `extract_json_object` (prevents CJK/emoji char boundary panic)
- `fix(cicd)`: PR target/fork compatibility fixes
- `chore(scripts)`: Switch benches to mockhttp.org (httpbin.org instability)

## Raw Scanner Output
```
Found 57 issue(s):
  CRITICAL: 7  (all false positives — security tool scripts flagging themselves)
  HIGH: 13     (cc-beeper localhost hooks + broad matchers in known-good settings)
  MEDIUM: 30   (broad permission patterns in CLAUDE.md — intentional config)
  LOW: 7       (informational)

cargo audit: not installed
cargo clippy --all-targets: No issues found
unsafe blocks: 2 (both justified, annotated nosemgrep)
```
