# Claude Code Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

Two hook implementations are provided. Both are thin delegates: they parse the
Claude Code JSON input, call `rtk rewrite`, and return the result in the
`updatedInput` format. All rewrite logic lives in the Rust binary.

## rtk-rewrite.sh (Linux / macOS)

- Requires `bash` and `jq`
- Installed automatically by `rtk init -g` on Unix systems
- Returns `updatedInput` JSON for transparent command rewrite (agent doesn't know RTK is involved)
- Exits silently (exit 0) on any failure: jq missing, rtk missing, rtk too old (< 0.23.0), no match
- Version guard checks `rtk --version` against minimum 0.23.0

## rtk-rewrite.py (Windows / cross-platform)

- Requires Python 3 (no third-party packages, no `jq`)
- Drop-in equivalent of `rtk-rewrite.sh` — identical exit code protocol and JSON output
- Works on Windows via Claude Code's Git Bash environment (`settings.json` hook)
- Same graceful degradation: exits 0 on all error paths so commands always run

### Manual installation on Windows

```powershell
# 1. Copy the hook
Copy-Item hooks\claude\rtk-rewrite.py "$env:USERPROFILE\.claude\hooks\rtk-rewrite.py"
```

Add the hook to `%USERPROFILE%\.claude\settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "python C:/Users/<you>/.claude/hooks/rtk-rewrite.py"
          }
        ]
      }
    ]
  }
}
```

Then restart Claude Code. Commands like `git status` will be transparently
rewritten to `rtk git status` before execution.

## Testing

```bash
# Shell hook — full test suite (60+ assertions)
bash hooks/claude/test-rtk-rewrite.sh

# Shell hook — test against a specific path
HOOK=/path/to/rtk-rewrite.sh bash hooks/claude/test-rtk-rewrite.sh

# Python hook — same assertions, cross-platform
python hooks/claude/test-rtk-rewrite.py

# Python hook — test against a specific path
HOOK=/path/to/rtk-rewrite.py python hooks/claude/test-rtk-rewrite.py
```

Both test scripts share the same test cases so regressions in either hook are caught.

## rtk-awareness.md

A slim 10-line instructions file embedded into `CLAUDE.md` by `rtk init`.
Used as a fallback on systems where the hook cannot be installed.
