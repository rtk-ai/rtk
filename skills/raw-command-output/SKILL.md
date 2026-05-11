---
name: raw-command-output
description: Use when shell command output appears compressed, summarized, or mangled, or when you need exact raw output (JSON/XML/CSV) for data analysis. Teaches RTK_DISABLED=1 prefix to bypass rtk output filtering.
allowed-tools: Bash
---

# Raw Command Output

## When to Use

- Shell command output appears compressed, summarized, or truncated
- You need to parse raw JSON/XML/CSV data from a command
- A command succeeded but the output is unusable for analysis

## How to Use

Prefix your shell command with `RTK_DISABLED=1` to bypass output filtering:

    RTK_DISABLED=1 aws ec2 describe-spot-price-history --output json
    RTK_DISABLED=1 kubectl get pods -o json
    RTK_DISABLED=1 curl -s https://api.example.com/data
    RTK_DISABLED=1 terraform show -json

This is a standard shell environment variable prefix. It is recognised by
the rtk binary and all agent hooks (OpenCode, Claude Code, Cursor, etc.).

The system also automatically detects retries and disables filtering,
but use this prefix proactively when you know you need raw data.
