# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in RTK, please report it to the maintainers privately:

- **Email**: security@rtk-ai.dev (or create a private security advisory on GitHub)
- **Response time**: We aim to acknowledge reports within 48 hours
- **Disclosure**: We follow responsible disclosure practices (90-day embargo)

**Please do NOT:**
- Open public GitHub issues for security vulnerabilities
- Disclose vulnerabilities on social media or forums before we've had a chance to address them

---

## Security Review Process for Pull Requests

RTK is a CLI tool that executes shell commands and handles user input. PRs from external contributors undergo enhanced security review to protect against:

- **Shell injection** (command execution vulnerabilities)
- **Supply chain attacks** (malicious dependencies)
- **Backdoors** (logic bombs, exfiltration code)
- **Data leaks** (tracking.db exposure, telemetry abuse)

---

## Automated Security Checks

Every PR triggers our [`security-check.yml`](.github/workflows/security-check.yml) workflow:

1. **Dependency audit** (`cargo audit`) - Detects known CVEs
2. **Critical files alert** - Flags modifications to high-risk files
3. **Dangerous pattern scan** - Regex-based detection of:
   - Shell execution (`Command::new("sh")`)
   - Environment manipulation (`.env("LD_PRELOAD")`)
   - Network operations (`reqwest::`, `std::net::`)
   - Unsafe code blocks
   - Panic-inducing patterns (`.unwrap()` in production)
4. **Clippy security lints** - Enforces Rust best practices

Results are posted in the PR's GitHub Actions summary.

---

## Critical Files Requiring Enhanced Review

The following files are considered **high-risk** and trigger mandatory 2-reviewer approval:

### Tier 1: Shell Execution & System Interaction
- **`src/runner.rs`** - Shell command execution engine (primary injection vector)
- **`src/summary.rs`** - Command output aggregation (data exfiltration risk)
- **`src/tracking.rs`** - SQLite database operations (privacy/telemetry concerns)

### Tier 2: Input Validation
- **`src/pnpm_cmd.rs`** - Package name validation (prevents injection via malicious names)
- **`src/container.rs`** - Docker/container operations (privilege escalation risk)

### Tier 3: Supply Chain & CI/CD
- **`Cargo.toml`** - Dependency manifest (typosquatting, backdoored crates)
- **`.github/workflows/*.yml`** - CI/CD pipelines (release tampering, secret exfiltration)

**If your PR modifies ANY of these files**, expect:
- Detailed manual security review
- Request for clarification on design choices
- Potentially slower merge timeline

---

## Review Workflow

### For External Contributors

1. **Submit PR** → Automated `security-check.yml` runs
2. **Review automated results** → Fix any flagged issues
3. **Manual review** → Maintainer performs comprehensive security audit
4. **Approval** → Merge (or request for changes)

### For Maintainers

Use the comprehensive security review process:

```bash
# If Claude Code available, run the dedicated skill:
/rtk-pr-security <PR_NUMBER>

# Manual review (without Claude):
gh pr view <PR_NUMBER>
gh pr diff <PR_NUMBER> > /tmp/pr.diff
bash scripts/detect-dangerous-patterns.sh /tmp/pr.diff
```

**Review checklist:**
- [ ] No critical files modified OR changes justified + reviewed by 2 maintainers
- [ ] No dangerous patterns OR patterns explained + safe
- [ ] No new dependencies OR deps audited on crates.io (downloads, maintainer, license)
- [ ] PR description matches actual code changes (intent vs reality)
- [ ] No logic bombs (time-based triggers, conditional backdoors)
- [ ] Code quality acceptable (no unexplained complexity spikes)

---

## Dangerous Patterns We Check For

| Pattern | Risk | Example |
|---------|------|---------|
| `Command::new("sh")` | Shell injection | Spawns shell with user input |
| `.env("LD_PRELOAD")` | Library hijacking | Preloads malicious shared libraries |
| `reqwest::`, `std::net::` | Data exfiltration | Unexpected network operations |
| `unsafe {` | Memory safety | Bypasses Rust's guarantees |
| `.unwrap()` in `src/` | DoS via panic | Crashes on invalid input |
| `SystemTime::now() > ...` | Logic bombs | Delayed malicious behavior |
| Base64/hex strings | Obfuscation | Hides malicious URLs/commands |

See [Dangerous Patterns Reference](https://github.com/rtk-ai/rtk/wiki/Dangerous-Patterns) for exploitation examples.

---

## Dependency Security

New dependencies added to `Cargo.toml` must meet these criteria:

- **Downloads**: >10,000 on crates.io (or strong justification if lower)
- **Maintainer**: Verified GitHub profile + track record of other crates
- **License**: MIT or Apache-2.0 compatible
- **Activity**: Recent commits (within 6 months)
- **No typosquatting**: Manual verification against similar crate names

**Red flags:**
- Brand new crate (<1 month old) with low downloads
- Anonymous maintainer with no GitHub history
- Crate name suspiciously similar to popular crate (e.g., `serid` vs `serde`)
- License change in recent versions

---

## Security Best Practices for Contributors

### Avoid These Anti-Patterns

**❌ DON'T:**
```rust
// Shell injection risk
let user_input = get_arg();
Command::new("sh").arg("-c").arg(format!("echo {}", user_input)).output();

// Panic on invalid input
let path = std::env::args().nth(1).unwrap();

// Hardcoded secrets
const API_KEY: &str = "sk_live_1234567890abcdef";
```

**✅ DO:**
```rust
// No shell, direct binary execution
let user_input = get_arg();
Command::new("echo").arg(user_input).output();

// Graceful error handling
let path = std::env::args().nth(1).context("Missing path argument")?;

// Env vars or config files for secrets
let api_key = std::env::var("API_KEY").context("API_KEY not set")?;
```

### Error Handling Guidelines

- Use `anyhow::Result<T>` with `.context()` for all error propagation
- NEVER use `.unwrap()` in `src/` (tests are OK)
- Prefer `.expect("descriptive message")` over `.unwrap()` if unavoidable
- Use `?` operator instead of `unwrap()` for propagation

### Input Validation

- Validate all user input before passing to `Command`
- Use allowlists for command flags (not denylists)
- Canonicalize file paths to prevent traversal attacks
- Sanitize package names with strict regex patterns

---

## Disclosure Timeline

When vulnerabilities are reported:

1. **Day 0**: Acknowledgment sent to reporter
2. **Day 7**: Maintainers assess severity and impact
3. **Day 14**: Patch development begins
4. **Day 30**: Patch released + CVE filed (if applicable)
5. **Day 90**: Public disclosure (or earlier if patch is deployed)

Critical vulnerabilities (remote code execution, data exfiltration) may be fast-tracked.

---

## Security Tooling

- **`cargo audit`** - Automated CVE scanning (runs in CI)
- **`cargo deny`** - License compliance + banned dependencies
- **`cargo clippy`** - Lints for unsafe patterns
- **GitHub Dependabot** - Automated dependency updates
- **GitHub Code Scanning** - Static analysis via CodeQL (planned)

---

## Contact

- **Security issues**: security@rtk-ai.dev
- **General questions**: https://github.com/rtk-ai/rtk/discussions
- **Maintainers**: @FlorianBruniaux (active fork maintainer)

---

**Last updated**: 2026-02-02
