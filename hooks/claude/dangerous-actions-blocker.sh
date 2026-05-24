#!/bin/bash
# dangerous-actions-blocker.sh - Block dangerous CLI operations
# Hook: PreToolUse (Bash)
# Blocks: rm -rf, force-push, secret exposure, destructive git ops

set -euo pipefail

# Read the tool input from stdin
INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

if [ -z "$COMMAND" ]; then
  exit 0
fi

# === DESTRUCTIVE FILE OPERATIONS ===
# Skip host-path checks when `rm` runs inside a container.
# `docker exec <ctr> rm /path` and `kubectl exec <pod> -- rm /path` operate on the
# container's filesystem, not the host — blocking them is a false positive.
_in_container_ctx=false
if echo "$COMMAND" | grep -qE '^(docker|kubectl)\s+exec\s+'; then
  _in_container_ctx=true
fi

if [ "$_in_container_ctx" = "false" ] && echo "$COMMAND" | grep -qE 'rm\s+(-[a-zA-Z]*f[a-zA-Z]*\s+|--force\s+)*(\/|~|\$HOME|\.\.)'; then
  echo '{"decision":"block","reason":"BLOCKED: rm -rf on root/home/parent directory. Use a safer path."}'
  exit 0
fi

if [ "$_in_container_ctx" = "false" ] && echo "$COMMAND" | grep -qE 'rm\s+-[a-zA-Z]*r[a-zA-Z]*f|rm\s+-[a-zA-Z]*f[a-zA-Z]*r'; then
  # Allow rm -rf on safe paths (node_modules, dist, build, tmp, cache, .next, __pycache__)
  if echo "$COMMAND" | grep -qE 'rm\s+-rf\s+(node_modules|dist|build|\.next|__pycache__|\.cache|tmp|\.tmp|coverage|\.nyc_output)'; then
    exit 0
  fi
  echo '{"decision":"ask","reason":"rm -rf detected. Please confirm this is intentional."}'
  exit 0
fi

# === DESTRUCTIVE GIT OPERATIONS ===
# Block --force but allow --force-with-lease
if echo "$COMMAND" | grep -qE 'git\s+push\s+.*--force'; then
  if ! echo "$COMMAND" | grep -qE 'force-with-lease'; then
    echo '{"decision":"block","reason":"BLOCKED: git push --force. Use --force-with-lease instead."}'
    exit 0
  fi
fi

if echo "$COMMAND" | grep -qE 'git\s+push\s+.*\s-f(\s|$)'; then
  echo '{"decision":"block","reason":"BLOCKED: git push -f. Use --force-with-lease instead."}'
  exit 0
fi

if echo "$COMMAND" | grep -qE 'git\s+reset\s+--hard'; then
  echo '{"decision":"ask","reason":"git reset --hard will discard uncommitted changes. Confirm?"}'
  exit 0
fi

if echo "$COMMAND" | grep -qE 'git\s+clean\s+-[a-zA-Z]*f'; then
  echo '{"decision":"ask","reason":"git clean -f will permanently delete untracked files. Confirm?"}'
  exit 0
fi

if echo "$COMMAND" | grep -qE 'git\s+checkout\s+--\s+\.'; then
  echo '{"decision":"ask","reason":"git checkout -- . will discard all unstaged changes. Confirm?"}'
  exit 0
fi

if echo "$COMMAND" | grep -qE 'git\s+branch\s+-D'; then
  echo '{"decision":"ask","reason":"git branch -D force-deletes a branch even if not merged. Confirm?"}'
  exit 0
fi

# === SECRETS EXPOSURE ===
if echo "$COMMAND" | grep -qE '(cat|echo|printf|head|tail|less|more)\s+.*\.(env|pem|key|secret|credentials|token)'; then
  echo '{"decision":"block","reason":"BLOCKED: Reading a potential secrets file. Use environment variables instead."}'
  exit 0
fi

if echo "$COMMAND" | grep -qiE '(ANTHROPIC_API_KEY|AWS_SECRET|OPENAI_API_KEY|DATABASE_URL|PRIVATE_KEY|TOKEN|PASSWORD)='; then
  echo '{"decision":"block","reason":"BLOCKED: Secret/credential detected in command. Use env vars or .env files."}'
  exit 0
fi

# === DATABASE DESTRUCTIVE OPS ===
if echo "$COMMAND" | grep -qiE '(DROP\s+(TABLE|DATABASE|SCHEMA)|TRUNCATE\s+TABLE|DELETE\s+FROM\s+\w+\s*;)'; then
  echo '{"decision":"block","reason":"BLOCKED: Destructive database operation (DROP/TRUNCATE/DELETE ALL). Do this manually."}'
  exit 0
fi

# === DOCKER DESTRUCTIVE OPS ===
if echo "$COMMAND" | grep -qE 'docker\s+system\s+prune\s+(-a|--all)'; then
  echo '{"decision":"ask","reason":"docker system prune -a will remove ALL unused images, containers, networks. Confirm?"}'
  exit 0
fi

if echo "$COMMAND" | grep -qE 'docker\s+(rm|rmi)\s+-f\s+\$\(docker'; then
  echo '{"decision":"ask","reason":"Mass docker removal detected. Confirm?"}'
  exit 0
fi

# All clear
exit 0
