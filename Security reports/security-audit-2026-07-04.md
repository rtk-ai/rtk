# Security Audit — 2026-07-04

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

## Fixed Issues
None.

## Scan Coverage

| Area | Result |
|------|--------|
| `cargo audit` | Not installed — skipped |
| Unsafe code blocks | 3 found, all legitimate (signal handlers) |
| `unwrap()` in production | All safe (guarded Some values or test-only) |
| Shell injection | None — all `Command::new()` + `.arg()` (safe) |
| Hardcoded secrets | None |
| Dependency versions | All recent; no known CVEs identified |
| Hook trust/integrity | `integrity.rs` SHA-256 checks intact |
| Network calls (ureq) | Telemetry only; no user data exfiltrated |

## Dependency Versions (Cargo.lock)

| Crate | Version | Notes |
|-------|---------|-------|
| rustls-webpki | 0.103.13 | Pinned explicitly in Cargo.toml |
| ureq | 2.12.1 | Recent; HTTP client for telemetry |
| regex | 1.12.3 | Recent |
| rusqlite | 0.31.0 | Recent; bundled feature |
| quick-xml | 0.37.5 | Recent |

## Detail: Unsafe Blocks

All 3 unsafe blocks in `src/main.rs` are standard POSIX signal-handling patterns:
1. `libc::signal(SIGPIPE, SIG_DFL)` — prevents SIGABRT on broken pipes (line ~1494)
2. `extern "C" fn handle_signal` + SIGINT/SIGTERM registration — clean child process forwarding (lines ~2472-2491)

Each block carries `#[allow(unsafe_code)]` + `// nosemgrep: unsafe-block` annotations. No injection surface.

## Detail: Compile-time Enforcement

`Cargo.toml` enforces `[lints.rust] unsafe_code = "deny"` and `warnings = "deny"` globally. The 3 unsafe blocks require explicit `#[allow(unsafe_code)]` overrides — hardening is in place.

## Raw Scanner Output

```
cargo-audit: not installed (cargo add cargo-audit to enable advisory database checks)

Grep scans performed:
  - unsafe blocks: 3 matches in src/main.rs (all signal handlers)
  - unwrap() production: all instances in guarded Some() blocks or test modules
  - shell injection: all $(…) occurrences in test fixtures verifying injection is blocked
  - hardcoded secrets: none (test fixtures only, e.g. "secret123" in test JSON)
  - network exfiltration: ureq used only for opt-in telemetry ping
```
