# Codex PreToolUse Adapter

This guide adds an optional Codex Desktop/CLI `PreToolUse` hook that rewrites
Bash commands through RTK before Codex executes them.

## Why use this

`rtk init --codex` writes lightweight RTK awareness into Codex's
`AGENTS.md`/`RTK.md` context. That is useful, but it still depends on the model
remembering to prefix shell commands with `rtk`.

Codex hooks can enforce the same behavior at tool-call time. A `PreToolUse` hook
can inspect Bash commands, ask RTK to rewrite them, and return Codex's native
`permissionDecision: "allow"` + `updatedInput` response shape.

## Quick install

Use this when setting up Codex Desktop or Codex CLI on a machine that may or may
not already have RTK installed. It reuses an existing working RTK binary and only
installs RTK when `rtk gain` is unavailable.

```bash
set -eu

if ! command -v rtk >/dev/null 2>&1 || ! rtk gain >/dev/null 2>&1; then
  curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh | sh
fi

export PATH="$HOME/.local/bin:$PATH"

CODEX_HOME="${CODEX_HOME:-$HOME/.codex}"
mkdir -p "$CODEX_HOME/hooks"

cat > "$CODEX_HOME/hooks/rtk-pretooluse.py" <<'PY'
#!/usr/bin/env python3
import json
import os
import shutil
import subprocess
import sys


def find_rtk():
    if os.environ.get("RTK_BIN"):
        return os.environ["RTK_BIN"]
    local_bin = os.path.expanduser("~/.local/bin/rtk")
    if os.path.exists(local_bin):
        return local_bin
    return shutil.which("rtk")


def main():
    try:
        payload = json.loads(sys.stdin.read() or "{}")
    except json.JSONDecodeError:
        return 0

    if payload.get("hook_event_name") not in (None, "PreToolUse"):
        return 0
    if payload.get("tool_name") not in ("Bash", "bash"):
        return 0

    tool_input = payload.get("tool_input") or {}
    command = tool_input.get("command")
    if not isinstance(command, str) or not command.strip():
        return 0

    rtk = find_rtk()
    if not rtk:
        return 0

    try:
        proc = subprocess.run(
            [rtk, "rewrite", command],
            text=True,
            capture_output=True,
            timeout=4,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return 0

    rewritten = proc.stdout.strip()
    if proc.returncode not in (0, 3) or not rewritten or rewritten == command:
        return 0

    updated_input = dict(tool_input)
    updated_input["command"] = rewritten
    sys.stdout.write(json.dumps({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": updated_input,
        }
    }, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
PY

chmod +x "$CODEX_HOME/hooks/rtk-pretooluse.py"

python3 - "$CODEX_HOME/hooks.json" "$CODEX_HOME/hooks/rtk-pretooluse.py" <<'PY'
import json
import os
import sys

hooks_json, script = sys.argv[1:]
command = f"python3 {script}"
entry = {
    "matcher": "^Bash$",
    "hooks": [{
        "type": "command",
        "command": command,
        "timeout": 5,
        "statusMessage": "Rewriting Bash command with RTK",
    }],
}

try:
    with open(hooks_json, "r", encoding="utf-8") as f:
        root = json.load(f)
except (FileNotFoundError, json.JSONDecodeError):
    root = {}

hooks = root.setdefault("hooks", {})
pre_tool_use = hooks.setdefault("PreToolUse", [])
already_installed = any(
    hook.get("command") == command
    for group in pre_tool_use
    for hook in group.get("hooks", [])
)
if not already_installed:
    pre_tool_use.append(entry)

os.makedirs(os.path.dirname(hooks_json), exist_ok=True)
with open(hooks_json, "w", encoding="utf-8") as f:
    json.dump(root, f, indent=2)
    f.write("\n")
PY
```

Restart Codex or reload hooks, then trust the new hook from `/hooks` or the
Codex settings panel.

## Behavior

- Handles Bash tool calls only.
- Uses `rtk rewrite <command>` so new RTK rewrite rules are picked up by the
  installed binary.
- Preserves the rest of Codex's original `tool_input`.
- Produces no output when RTK has no rewrite, is unavailable, or times out, so
  Codex continues with the original command.
- Returns `updatedInput` only with `permissionDecision: "allow"`, matching the
  Codex hook contract.

Example rewrite:

```text
git status -> rtk git status
```

## Verify

```bash
python3 -m py_compile "$CODEX_HOME/hooks/rtk-pretooluse.py"

printf '%s' '{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"git status"}}' \
  | python3 "$CODEX_HOME/hooks/rtk-pretooluse.py"
```

Expected output includes:

```json
"permissionDecision":"allow"
"command":"rtk git status"
```

Non-Bash tools should be ignored:

```bash
printf '%s' '{"hook_event_name":"PreToolUse","tool_name":"apply_patch","tool_input":{"command":"x"}}' \
  | python3 "$CODEX_HOME/hooks/rtk-pretooluse.py" \
  | wc -c
```

Expected output:

```text
0
```

## Uninstall

Remove the hook script:

```bash
rm -f "${CODEX_HOME:-$HOME/.codex}/hooks/rtk-pretooluse.py"
```

If `hooks.json` contains only this RTK hook, it is also safe to remove the file:

```bash
rm -f "${CODEX_HOME:-$HOME/.codex}/hooks.json"
```

If other hooks are present, keep `hooks.json` and remove only the RTK
`PreToolUse` entry.
