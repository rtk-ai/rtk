---
description: >
  Senior Engineer security review (CISSP/CCSP/OWASP) focused on financial systems and critical infrastructure.
  Scans code at current commit, documents findings, fixes issues, runs tests, and creates a PR.
  Args: none needed - operates on current commit.
---

# Security Scan, Fix & Review

Comprehensive security audit performed as a **Senior Engineer with CISSP, CCSP credentials** and deep OWASP expertise, with a focus on **financial systems** and **critical infrastructure** threat models.

## Security Frameworks Applied

### OWASP References (always consult)
- **LLM Prompt Injection Prevention**: https://cheatsheetseries.owasp.org/cheatsheets/LLM_Prompt_Injection_Prevention_Cheat_Sheet.html
- **MCP Security**: https://cheatsheetseries.owasp.org/cheatsheets/MCP_Security_Cheat_Sheet.html
- **Command Injection**: https://cheatsheetseries.owasp.org/cheatsheets/OS_Command_Injection_Defense_Cheat_Sheet.html
- **Input Validation**: https://cheatsheetseries.owasp.org/cheatsheets/Input_Validation_Cheat_Sheet.html
- **Error Handling**: https://cheatsheetseries.owasp.org/cheatsheets/Error_Handling_Cheat_Sheet.html
- **Logging**: https://cheatsheetseries.owasp.org/cheatsheets/Logging_Cheat_Sheet.html
- **Deserialization**: https://cheatsheetseries.owasp.org/cheatsheets/Deserialization_Cheat_Sheet.html

### Credential Lens
- **CISSP** domains: Security architecture, software development security, asset security, communication/network security
- **CCSP** domains: Cloud data security, platform/infrastructure security, application security
- **Financial systems focus**: Data integrity, audit trails, non-repudiation, regulatory compliance (SOX, PCI-DSS patterns)
- **Critical infrastructure focus**: Availability, resilience, supply chain security, defense in depth

## When to Use

- **Manual invocation**: `/security-scan-fix-review`
- **Before releases**: Comprehensive pre-release security gate
- **After dependency updates**: Check for new vulnerability vectors
- **Periodic audits**: Scheduled security hygiene

## Workflow

Execute the following phases sequentially. Do NOT skip phases.

### Phase 0 -- Setup

```bash
# Confirm working directory and state
pwd
git branch --show-current
git rev-parse --short HEAD
git status --short
```

Capture the **short commit hash** -- this will be used for the output directory name.

Create a feature branch for fixes:
```bash
git checkout -b security-review/<commit-hash>
```

### Phase 1 -- Comprehensive Security Scan

Perform a **multi-vector security audit** of ALL source files. Use parallel Agent subagents where possible to cover different threat categories simultaneously.

#### 1A. OWASP LLM Prompt Injection Analysis

RTK proxies commands for LLM consumption. Analyze for:

- **Prompt injection via command output**: Can crafted git commits, filenames, or command outputs inject instructions into the LLM context?
- **Indirect prompt injection**: Can untrusted data sources (git repos, package names, file contents) manipulate LLM behavior through RTK's output?
- **Output sanitization**: Does RTK strip or escape content that could be interpreted as LLM instructions?
- **Context boundary violations**: Can RTK output blur the boundary between system instructions and user data?

Scan patterns:
```bash
# Files that process external/untrusted input and produce LLM-consumed output
rg -l "stdout|output|println|format!" --type rust src/
# Check for any output encoding/escaping
rg "escape|sanitize|encode|strip" --type rust src/
```

#### 1B. MCP Security Analysis

If RTK integrates with MCP (Model Context Protocol) or Claude Code hooks:

- **Tool poisoning**: Can RTK be tricked into executing unintended tools?
- **Rug pulls**: Can tool descriptions change between approval and execution?
- **Excessive permissions**: Does RTK request more capabilities than needed?
- **Cross-tool contamination**: Can one command's output affect another's execution?

Scan patterns:
```bash
# Hook files and MCP integration points
find .claude/ -type f -name "*.sh" -o -name "*.json" -o -name "*.toml"
rg "hook|mcp|tool_use|claude" --type rust src/
```

#### 1C. Command Injection & Shell Security (OWASP OS Command Injection)

- **Direct injection**: User input passed to shell commands without sanitization
- **Indirect injection**: Environment variables, config files, filenames used in commands
- **Argument injection**: Flags/arguments that alter command behavior (e.g., `--exec`, `-o`)
- **Shell metacharacter handling**: `;`, `|`, `&`, `$()`, backticks, newlines

Scan patterns:
```bash
rg "Command::new|\.arg\(|\.args\(" --type rust src/
rg "format!.*Command|format!.*arg" --type rust src/
rg "sh.*-c|bash.*-c|cmd.*\/c" --type rust src/
rg "env::var|env::args" --type rust src/
```

#### 1D. Input Validation & Data Integrity (Financial Systems Focus)

- **Untrusted data parsing**: JSON, TOML, YAML deserialization without validation
- **SQL injection** in tracking database (SQLite)
- **Path traversal**: File operations with user-controlled paths
- **Integer overflow/underflow**: Token counting, size calculations
- **Buffer handling**: String slicing, array indexing without bounds checks
- **Data integrity**: Can tracking data be tampered with? Are audit trails reliable?

Scan patterns:
```bash
rg "serde_json::from|toml::from|serde::Deserialize" --type rust src/
rg "rusqlite|execute|prepare|query" --type rust src/
rg "\.unwrap\(\)|\.expect\(" --type rust src/
rg "\[.*\.\." --type rust src/  # array/string slicing
rg "as usize|as i32|as u32" --type rust src/  # numeric casts
```

#### 1E. Supply Chain & Dependency Security

```bash
# Check for known vulnerabilities
cargo audit 2>&1 || echo "cargo-audit not installed -- note in report"

# Review dependency tree for suspicious or unnecessary deps
cargo tree --depth 1

# Check for unsafe code
rg "unsafe" --type rust src/
```

#### 1F. Information Disclosure & Error Handling

- **Verbose errors**: Stack traces, internal paths, or system info leaked to output
- **Timing attacks**: Constant-time comparisons where needed
- **Debug artifacts**: Debug prints, TODO comments with security implications
- **Sensitive data in logs/tracking**: Does the SQLite DB store sensitive command content?

Scan patterns:
```bash
rg "dbg!|eprintln!|println!.*err|println!.*error" --type rust src/
rg "TODO|FIXME|HACK|XXX|SECURITY" --type rust src/
rg "password|secret|token|key|credential" --type rust src/
```

#### 1G. Availability & Resilience (Critical Infrastructure Focus)

- **DoS vectors**: Can crafted input cause excessive CPU, memory, or disk usage?
- **Resource exhaustion**: Unbounded allocations, regex backtracking (ReDoS)
- **Graceful degradation**: Does the tool fail safely under adverse conditions?
- **File descriptor leaks**: Unclosed files, processes, or connections

Scan patterns:
```bash
rg "\.collect\(\)|Vec::new|String::new" --type rust src/  # unbounded allocations
rg "Regex::new" --type rust src/  # potential ReDoS
rg "loop|while" --type rust src/  # potential infinite loops
rg "File::open|File::create" --type rust src/  # resource management
```

### Phase 2 -- Document Findings

Create the security review document:

```bash
mkdir -p docs/security-review
```

Write `docs/security-review/<commit-hash>.md` with this structure:

```markdown
# Security Review: <commit-hash>

**Date**: <today's date>
**Reviewer**: Claude (Senior Engineer, CISSP/CCSP/OWASP)
**Scope**: Full codebase at commit <full-hash>
**Branch**: <branch-name>
**Focus**: Financial systems and critical infrastructure threat model

## Executive Summary

<2-3 sentence overview of findings>

## Findings

### Critical (Must Fix)

#### FINDING-001: <Title>
- **Category**: <OWASP category>
- **CVSS Estimate**: <score>
- **File**: <file:line>
- **Description**: <what the issue is>
- **Impact**: <what could happen if exploited>
- **Recommendation**: <how to fix>
- **Financial/Infra Impact**: <specific impact for financial systems or critical infrastructure>

### High (Should Fix)
...

### Medium (Consider Fixing)
...

### Low / Informational
...

## OWASP LLM Prompt Injection Assessment
<Specific findings related to LLM prompt injection vectors>

## OWASP MCP Security Assessment
<Specific findings related to MCP security>

## Supply Chain Assessment
<Dependency audit results>

## Positive Security Observations
<What the codebase does well from a security perspective>
```

Assign each finding a unique ID (FINDING-001, FINDING-002, etc.) for traceability.

### Phase 3 -- Fix Issues

For each finding rated **Critical** or **High**:

1. Create the fix in the relevant source file
2. Follow RTK coding standards (anyhow, context, no unwrap, lazy_static regex)
3. Add or update tests to cover the security fix
4. Document the fix in the security review markdown

For **Medium** findings:
- Fix if the change is low-risk and straightforward
- Otherwise, document as "Accepted Risk" with justification

For **Low/Informational**:
- Document only, do not change code unless trivially fixable

### Phase 4 -- Verify Fixes

Run the full quality gate repeatedly until clean:

```bash
cargo fmt --all
cargo clippy --all-targets
cargo test --all
```

If tests fail:
1. Analyze the failure
2. Fix the issue (may be in test or in code)
3. Re-run the full gate
4. Repeat until ALL THREE pass cleanly

Do NOT proceed to Phase 5 until the gate is fully green.

### Phase 5 -- Update Security Review Document

Add a **Remediation Log** section to the security review document:

```markdown
## Remediation Log

### FINDING-001: <Title>
- **Status**: Fixed / Accepted Risk / Deferred
- **Commit**: <short-hash of fix>
- **Changes**:
  - `src/file.rs:42` - <description of change>
  - `src/other.rs:17` - <description of change>
- **Tests Added/Updated**:
  - `src/file.rs::tests::test_name` - <what it verifies>
- **Verification**: cargo test passing, clippy clean

### FINDING-002: ...
```

### Phase 6 -- Commit & PR

Stage and commit all changes:

```bash
git add -A
git commit -m "security: comprehensive OWASP security review and fixes

- Reviewed codebase at commit <original-hash>
- Applied CISSP/CCSP/OWASP frameworks
- Focus: financial systems and critical infrastructure
- Findings documented in docs/security-review/<hash>.md
- <N> issues found, <M> fixed, <K> accepted risk

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Push and create PR:

```bash
git push -u origin security-review/<commit-hash>
```

Create PR with `gh pr create` including:
- **Title**: `security: OWASP security review and fixes for <commit-hash>`
- **Body**: Full summary with:
  - Executive summary
  - Bulleted list of ALL findings with severity, status, and description
  - Bulleted list of ALL code changes made
  - Test verification results
  - Frameworks applied (OWASP LLM, OWASP MCP, CISSP, CCSP)
  - Link to full report: `docs/security-review/<hash>.md`

### Phase 7 -- Summary

Print a final summary to the user:
- Total findings by severity
- Issues fixed vs accepted risk vs deferred
- Files modified
- Tests added/updated
- PR number and link

## If No Issues Found

If the scan reveals no security issues:

1. Still create `docs/security-review/<commit-hash>.md` documenting:
   - What was scanned
   - Which frameworks were applied
   - Positive security observations (what the codebase does well)
   - Conclusion: "No actionable findings at this time"
2. Commit the review document
3. Create PR with the clean report
4. Summarize to user what was checked and that no issues were found

## Severity Classification

| Severity | Criteria | Action |
|----------|----------|--------|
| **Critical** | RCE, data breach, authentication bypass | Must fix immediately |
| **High** | Privilege escalation, significant data exposure, DoS | Should fix before release |
| **Medium** | Information disclosure, limited impact vulnerabilities | Fix if low effort |
| **Low** | Best practice deviations, hardening opportunities | Document only |
| **Informational** | Observations, suggestions for defense in depth | Document only |

## Financial Systems Threat Considerations

When evaluating findings, weigh additional impact for:
- **Data integrity**: Can transaction records, audit trails, or tracking data be tampered with?
- **Non-repudiation**: Can actions be performed without proper attribution?
- **Regulatory**: Would this finding trigger SOX, PCI-DSS, or similar compliance concerns?
- **Availability**: Could exploitation cause service disruption during critical processing windows?

## Critical Infrastructure Threat Considerations

- **Cascading failures**: Could exploitation propagate to dependent systems?
- **Supply chain**: Could compromised dependencies affect downstream consumers?
- **Resilience**: Does the system degrade gracefully under attack?
- **Recovery**: Can the system be restored to a known-good state after compromise?
