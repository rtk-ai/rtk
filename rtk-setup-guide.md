## RTK Token Optimizer — Setup & Configuration Guide

RTK (Rust Token Killer) is a CLI proxy that sits between Claude Code and your shell. It intercepts command output, strips noise, and returns only what the AI needs — cutting token usage by 25–60% per command.

---

## Installation

```bash
# Install binary
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh | sh

# Add to PATH permanently
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc

# Generate default config
rtk config --create

# Register global hook (works with Claude Code, Cursor, Codex)
rtk init -g
```

---

## Wire into Claude Code

RTK hooks into Claude Code via `~/.claude/settings.json`. Add this manually if `rtk init -g` doesn't patch it automatically:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "/Users/<you>/.local/bin/rtk hook claude"
          }
        ]
      }
    ]
  }
}
```

Use the **full path** to the binary (not just `rtk`) to ensure it works regardless of shell PATH in Claude's environment.

Restart Claude Code after editing `settings.json`.

---

## Global Config — `~/Library/Application Support/rtk/config.toml` (macOS)

This config applies to **every project** automatically. No per-project setup needed.

```toml
[tracking]
enabled = true
history_days = 90

[display]
colors = true
emoji = true
max_width = 120

[filters]
ignore_dirs = [
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "vendor",
]
ignore_files = [
    "*.lock",
    "*.min.js",
    "*.min.css",
]

[tee]
enabled = true
mode = "failures"
max_files = 20
max_file_size = 1048576

[telemetry]
enabled = false

[hooks]
# Commands that should pass through without rtk filtering (interactive ops)
exclude_commands = [
    "git rebase",
    "git merge",
    "git cherry-pick",
    "open ",
]
# Commands rtk rewrites transparently to its optimized versions
transparent_prefixes = [
    "grep",
    "cat",
    "find",
    "ls",
    "curl",
    "psql",
    "npm",
    "npx",
    "gh",
    "git",
    "wc",
    "vercel",
]

[limits]
grep_max_results = 200
grep_max_per_file = 25
status_max_files = 15
status_max_untracked = 10
passthrough_max_chars = 2000
```

### Key decisions
- **`transparent_prefixes`** — the most impactful setting. These are the commands that generate the most token noise. RTK intercepts and compacts their output before Claude sees it.
- **`exclude_commands`** — interactive git ops (rebase, merge, cherry-pick) are excluded because filtering mid-conflict output breaks the flow.
- **`telemetry: false`** — disabled by default for privacy.

---

## Per-project override (optional)

Create `.rtk/filters.toml` in any project root to override or extend global filters for that project only. Useful for suppressing framework-specific noise (Next.js build output, Supabase verbose logs, etc.).

```bash
# Trust project-local filters (required once per project)
rtk trust
```

---

## Check savings

```bash
# Current session savings
rtk gain

# Discover what you could save from command history
rtk discover

# Session-by-session breakdown
rtk session
```

---

## Real numbers (30-day history, one developer)

| Command | Count | Potential savings |
|---------|-------|-------------------|
| `grep` | 3,753 | ~481K tokens |
| `git` | 4,393 | ~448K tokens |
| `cat` | 1,001 | ~371K tokens |
| `npx tsc` | 1,160 | ~320K tokens |
| `find` | 878 | ~198K tokens |
| `gh` | 1,374 | ~184K tokens |
| **Total** | **14,671 cmds** | **~2.4M tokens/month** |

---

## Works with

- Claude Code (Desktop + CLI)
- Cursor
- GitHub Copilot / Codex
- Any AI IDE that supports shell hooks
