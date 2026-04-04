---
title: Security Policy
description: How to report vulnerabilities, the PR security review process, and dangerous patterns to avoid
sidebar:
  order: 2
---

# Security Policy

## Reporting a vulnerability

Report security issues privately — do not open public GitHub issues for vulnerabilities.

- **Email**: security@rtk-ai.dev (or create a private GitHub security advisory)
- **Response time**: Acknowledgment within 48 hours
- **Disclosure**: 90-day embargo, responsible disclosure

## Automated security checks

Every PR triggers `security-check.yml`:

1. **Dependency audit** (`cargo audit`) — detects known CVEs
2. **Critical files alert** — flags modifications to high-risk files
3. **Dangerous pattern scan** — regex detection of shell execution, env manipulation, network ops, unsafe code, `.unwrap()` in production
4. **Clippy security lints**

## High-risk files

These files require enhanced review (2 maintainers for Tier 1):

**Tier 1 — Shell execution & system interaction:**
- `src/runner.rs` — shell command execution engine
- `src/tracking.rs` — SQLite database operations
- `src/discover/registry.rs` — command rewrite logic
- `hooks/rtk-rewrite.sh` — intercepts all Claude Code commands

**Tier 2 — Input validation:**
- `src/pnpm_cmd.rs` — package name validation
- `src/container.rs` — Docker/container operations

**Tier 3 — Supply chain & CI/CD:**
- `Cargo.toml` — dependency manifest
- `.github/workflows/*.yml` — CI/CD pipelines

## Dangerous patterns

| Pattern | Risk |
|---------|------|
| `Command::new("sh")` | Shell injection |
| `.env("LD_PRELOAD")` | Library hijacking |
| `reqwest::`, `std::net::` | Data exfiltration |
| `unsafe {` | Memory safety bypass |
| `.unwrap()` in `src/` | DoS via panic |
| `SystemTime::now() > ...` | Logic bombs |

**Avoid — shell injection:**
```rust
// ❌ Never do this
Command::new("sh").arg("-c").arg(format!("echo {}", user_input)).output();

// ✅ Direct binary execution
Command::new("echo").arg(user_input).output();
```

## Dependency criteria for new crates

- Downloads: >10,000 on crates.io
- Maintainer: verified GitHub profile + track record
- License: MIT or Apache-2.0
- Activity: commits within 6 months
- No typosquatting (verify against similar crate names)

## Disclosure timeline

| Day | Action |
|-----|--------|
| 0 | Acknowledgment to reporter |
| 7 | Severity assessment |
| 14 | Patch development |
| 30 | Patch released + CVE filed (if applicable) |
| 90 | Public disclosure |

Critical vulnerabilities (RCE, data exfiltration) may be fast-tracked.
