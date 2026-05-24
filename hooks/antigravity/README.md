# Google Antigravity Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Supported Modes

RTK supports two separate Google Antigravity integrations:

### 1. Antigravity CLI (`agy`) — Programmatic Hook

The `agy` CLI (successor to Gemini CLI) supports a JSON hook protocol.  RTK registers as a `PreToolUse` hook that rewrites shell commands before execution.

**Install (global):**
```bash
rtk init -g --agent agy
```

**Install (project-local):**
```bash
rtk init --agent agy
```

**Hook location:**
- Global: `~/.agents/hooks.json`
- Project: `.agents/hooks.json`

**Hook entry written:**
```json
{
  "hooks": {
    "PreToolUse": [
      {
        "type": "command",
        "command": "rtk hook antigravity",
        "toolNameMatcher": "^(run_command|Bash)$"
      }
    ]
  }
}
```

**JSON protocol:**

Input (`run_command` tool):
```json
{"toolCall": {"name": "run_command", "args": {"CommandLine": "git status"}}}
```

Output (rewritten):
```json
{"decision": "allow", "toolCall": {"args": {"CommandLine": "rtk git status"}}}
```

Input (`Bash` tool):
```json
{"toolCall": {"name": "Bash", "args": {"command": "cargo test"}}}
```

Output (rewritten):
```json
{"decision": "allow", "toolCall": {"args": {"command": "rtk cargo test"}}}
```

No output is produced for non-shell tools or commands that don't match any RTK filter — the tool runs unchanged.

**Uninstall:**
```bash
rtk init --uninstall --agent agy          # project-local
rtk init -g --uninstall --agent agy       # global
```

### 2. Antigravity IDE — Prompt-Level Guidance

The Antigravity IDE (formerly "Antigravity") reads project rules files.  RTK installs its awareness instructions as a rules file.

**Install (project-local only):**
```bash
rtk init --agent antigravity
```

- Installs `.agents/rules/antigravity-rtk-rules.md` in the project root
- Instructs Antigravity to prefix shell commands with `rtk`
- No programmatic hook — relies on the model following the rules

## Gemini CLI (Legacy)

The Gemini CLI is still supported but is now considered **legacy**.  New users should use `agy`.

```bash
rtk init -g --gemini    # Gemini CLI (legacy)
```
