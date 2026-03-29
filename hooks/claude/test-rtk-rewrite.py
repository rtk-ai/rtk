#!/usr/bin/env python3
"""
Test suite for rtk-rewrite.py
Feeds mock JSON through the hook and verifies rewritten commands.
Mirrors test-rtk-rewrite.sh case-for-case so both hooks stay in sync.

Usage:
    python hooks/claude/test-rtk-rewrite.py
    HOOK=/path/to/rtk-rewrite.py python hooks/claude/test-rtk-rewrite.py
"""

import json
import os
import subprocess
import sys
from pathlib import Path

HOOK = os.environ.get(
    "HOOK",
    str(Path.home() / ".claude" / "hooks" / "rtk-rewrite.py"),
)

PASS = 0
FAIL = 0
TOTAL = 0

GREEN = "\033[32m"
RED = "\033[31m"
DIM = "\033[2m"
RESET = "\033[0m"


def run_hook(cmd: str) -> str:
    """Feed a Bash tool-call JSON through the hook and return raw stdout."""
    payload = json.dumps({"tool_name": "Bash", "tool_input": {"command": cmd}})
    try:
        result = subprocess.run(
            [sys.executable, HOOK],
            input=payload,
            capture_output=True,
            text=True,
        )
        return result.stdout.strip()
    except Exception:
        return ""


def test_rewrite(description: str, input_cmd: str, expected_cmd: str) -> None:
    """Assert that input_cmd is rewritten to expected_cmd (or not rewritten when empty)."""
    global PASS, FAIL, TOTAL
    TOTAL += 1

    output = run_hook(input_cmd)

    if not expected_cmd:
        # Expect no rewrite — hook exits 0 with no output
        if not output:
            print(f"  {GREEN}PASS{RESET} {description} {DIM}-> (no rewrite){RESET}")
            PASS += 1
        else:
            try:
                actual = json.loads(output)["hookSpecificOutput"]["updatedInput"]["command"]
            except Exception:
                actual = output
            print(f"  {RED}FAIL{RESET} {description}")
            print(f"       expected: (no rewrite)")
            print(f"       actual:   {actual}")
            FAIL += 1
    else:
        actual = ""
        if output:
            try:
                actual = json.loads(output)["hookSpecificOutput"]["updatedInput"]["command"]
            except Exception:
                actual = output

        if actual == expected_cmd:
            print(f"  {GREEN}PASS{RESET} {description} {DIM}-> {actual}{RESET}")
            PASS += 1
        else:
            print(f"  {RED}FAIL{RESET} {description}")
            print(f"       expected: {expected_cmd}")
            print(f"       actual:   {actual}")
            FAIL += 1


# ---------------------------------------------------------------------------
# SECTION 1: Existing patterns (regression tests)
# ---------------------------------------------------------------------------
print("--- Existing patterns (regression) ---")

test_rewrite("git status", "git status", "rtk git status")
test_rewrite("git log --oneline -10", "git log --oneline -10", "rtk git log --oneline -10")
test_rewrite("git diff HEAD", "git diff HEAD", "rtk git diff HEAD")
test_rewrite("git show abc123", "git show abc123", "rtk git show abc123")
test_rewrite("git add .", "git add .", "rtk git add .")
test_rewrite("gh pr list", "gh pr list", "rtk gh pr list")
test_rewrite("npx playwright test", "npx playwright test", "rtk playwright test")
test_rewrite("ls -la", "ls -la", "rtk ls -la")
test_rewrite("curl -s https://example.com", "curl -s https://example.com", "rtk curl -s https://example.com")
test_rewrite("cat package.json", "cat package.json", "rtk read package.json")
test_rewrite("grep -rn pattern src/", "grep -rn pattern src/", "rtk grep -rn pattern src/")
test_rewrite("rg pattern src/", "rg pattern src/", "rtk grep pattern src/")
test_rewrite("cargo test", "cargo test", "rtk cargo test")
test_rewrite("npx prisma migrate", "npx prisma migrate", "rtk prisma migrate")

print()

# ---------------------------------------------------------------------------
# SECTION 2: Env var prefix handling
# ---------------------------------------------------------------------------
print("--- Env var prefix handling ---")

test_rewrite(
    "env + playwright",
    "TEST_SESSION_ID=2 npx playwright test --config=foo",
    "TEST_SESSION_ID=2 rtk playwright test --config=foo",
)
test_rewrite(
    "env + git status",
    "GIT_PAGER=cat git status",
    "GIT_PAGER=cat rtk git status",
)
test_rewrite(
    "env + git log",
    "GIT_PAGER=cat git log --oneline -10",
    "GIT_PAGER=cat rtk git log --oneline -10",
)
test_rewrite(
    "multi env + vitest",
    "NODE_ENV=test CI=1 npx vitest run",
    "NODE_ENV=test CI=1 rtk vitest run",
)
test_rewrite("env + ls", "LANG=C ls -la", "LANG=C rtk ls -la")
test_rewrite(
    "env + npm run",
    "NODE_ENV=test npm run test:e2e",
    "NODE_ENV=test rtk npm run test:e2e",
)
test_rewrite(
    "env + docker compose (unsupported subcommand, NOT rewritten)",
    "COMPOSE_PROJECT_NAME=test docker compose up -d",
    "",
)
test_rewrite(
    "env + docker compose logs (supported, rewritten)",
    "COMPOSE_PROJECT_NAME=test docker compose logs web",
    "COMPOSE_PROJECT_NAME=test rtk docker compose logs web",
)

print()

# ---------------------------------------------------------------------------
# SECTION 3: New patterns
# ---------------------------------------------------------------------------
print("--- New patterns ---")

test_rewrite("npm run test:e2e", "npm run test:e2e", "rtk npm run test:e2e")
test_rewrite("npm run build", "npm run build", "rtk npm run build")
test_rewrite("npm test", "npm test", "")
test_rewrite("vue-tsc -b", "vue-tsc -b", "")
test_rewrite("npx vue-tsc --noEmit", "npx vue-tsc --noEmit", "rtk npx vue-tsc --noEmit")
test_rewrite("docker compose up -d (NOT rewritten)", "docker compose up -d", "")
test_rewrite("docker compose logs postgrest", "docker compose logs postgrest", "rtk docker compose logs postgrest")
test_rewrite("docker compose ps", "docker compose ps", "rtk docker compose ps")
test_rewrite("docker compose build", "docker compose build", "rtk docker compose build")
test_rewrite("docker compose down (NOT rewritten)", "docker compose down", "")
test_rewrite(
    "docker compose -f file.yml up (NOT rewritten — flag before subcommand)",
    "docker compose -f docker-compose.preview.yml --project-name myapp up -d --build",
    "",
)
test_rewrite("docker run --rm postgres", "docker run --rm postgres", "rtk docker run --rm postgres")
test_rewrite("docker exec -it db psql", "docker exec -it db psql", "rtk docker exec -it db psql")
test_rewrite("find", "find . -name '*.ts'", "rtk find . -name '*.ts'")
test_rewrite("tree", "tree src/", "rtk tree src/")
test_rewrite("wget", "wget https://example.com/file", "rtk wget https://example.com/file")
test_rewrite("gh api repos/owner/repo", "gh api repos/owner/repo", "rtk gh api repos/owner/repo")
test_rewrite("gh release list", "gh release list", "rtk gh release list")
test_rewrite("kubectl describe pod foo", "kubectl describe pod foo", "rtk kubectl describe pod foo")
test_rewrite("kubectl apply -f deploy.yaml", "kubectl apply -f deploy.yaml", "rtk kubectl apply -f deploy.yaml")

print()

# ---------------------------------------------------------------------------
# SECTION 3b: RTK_DISABLED and redirect fixes
# ---------------------------------------------------------------------------
print("--- RTK_DISABLED ---")

test_rewrite("RTK_DISABLED=1 git status (no rewrite)", "RTK_DISABLED=1 git status", "")
test_rewrite("RTK_DISABLED=1 cargo test (no rewrite)", "RTK_DISABLED=1 cargo test", "")
test_rewrite("FOO=1 RTK_DISABLED=1 git status (no rewrite)", "FOO=1 RTK_DISABLED=1 git status", "")

print()
print("--- Redirect operators ---")

test_rewrite("cargo test 2>&1 | head", "cargo test 2>&1 | head", "rtk cargo test 2>&1 | head")
test_rewrite("cargo test 2>&1", "cargo test 2>&1", "rtk cargo test 2>&1")
test_rewrite("cargo test &>/dev/null", "cargo test &>/dev/null", "rtk cargo test &>/dev/null")
test_rewrite(
    "cargo test & git status (both sides rewritten)",
    "cargo test & git status",
    "rtk cargo test & rtk git status",
)

print()

# ---------------------------------------------------------------------------
# SECTION 4: Vitest edge cases
# ---------------------------------------------------------------------------
print("--- Vitest run dedup ---")

test_rewrite("vitest (no args)", "vitest", "rtk vitest")
test_rewrite("vitest run (no double run)", "vitest run", "rtk vitest run")
test_rewrite("vitest run --reporter", "vitest run --reporter=verbose", "rtk vitest run --reporter=verbose")
test_rewrite("npx vitest run", "npx vitest run", "rtk vitest run")
test_rewrite("pnpm vitest run --coverage", "pnpm vitest run --coverage", "rtk vitest run --coverage")

print()

# ---------------------------------------------------------------------------
# SECTION 5: Should NOT rewrite
# ---------------------------------------------------------------------------
print("--- Should NOT rewrite ---")

test_rewrite("already rtk", "rtk git status", "")
test_rewrite("heredoc", "cat <<'EOF'\nhello\nEOF", "")
test_rewrite("echo (no pattern)", "echo hello world", "")
test_rewrite("cd (no pattern)", "cd /tmp", "")
test_rewrite("mkdir (no pattern)", "mkdir -p foo/bar", "")
test_rewrite("python3 (no pattern)", "python3 script.py", "")
test_rewrite("node (no pattern)", "node -e 'console.log(1)'", "")

print()

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
print("============================================")
if FAIL == 0:
    print(f"  {GREEN}ALL {TOTAL} TESTS PASSED{RESET}")
else:
    print(f"  {RED}{FAIL} FAILED{RESET} / {TOTAL} total ({PASS} passed)")
print("============================================")

sys.exit(FAIL)
