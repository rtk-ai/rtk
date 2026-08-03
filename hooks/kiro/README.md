# Kiro CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Uses the `rtk hook kiro` Rust binary (not a shell script) — no `jq` dependency, matches the Copilot/Gemini/Droid binary-hook pattern rather than the Claude Code shell-script pattern.
- Kiro's documented `preToolUse` hook contract ([kiro.dev CLI docs, "Hooks System"](https://kiro.dev/docs/cli/experimental/hooks/)) supports only two outcomes: **exit 0** (allow, unmodified) or **exit 2** (block, STDERR text is returned to the LLM). There is no `updatedInput`-equivalent transparent-rewrite mechanism — unlike Claude Code, Cursor, and VS Code Copilot Chat, a Kiro hook cannot silently substitute a different command for the agent to run.
- This makes the integration **deny-with-suggestion**, structurally identical to the JetBrains Copilot IDE integration: on a real rewrite match, exit 2 with `[rtk] Use \`<rewritten>\` instead for reduced token usage.` on stderr. Kiro surfaces that stderr text back to the LLM, which can retry with the suggested command on its next turn.
- Input JSON shape (per Kiro's docs): `{"hook_event_name":"preToolUse","cwd":"...","tool_name":"shell","tool_input":{"command":"..."}}`. The `shell` tool's documented legacy aliases (`execute_bash`, `execute_cmd`) are also matched, since older `tool_input` payloads may still use them.
- Matches Copilot CLI's pattern for the exit-code contract specifically (§ "Exit Code Contract" in `../README.md`): all infrastructure failures (missing binary, bad JSON, non-shell tool, empty command) exit 0 — never block a command over a bug in this hook itself. Exit 2 is reserved exclusively for genuine, confirmed rewrite suggestions.

## Kiro agent configuration wiring

Kiro has no dedicated hook-installer directory convention like `~/.claude/settings.json` — hooks are declared per-agent inside `.kiro/agents/<name>.json` (or `~/.kiro/agents/<name>.json` for global agents). Add:

```json
{
  "hooks": {
    "preToolUse": [
      {
        "matcher": "shell",
        "command": "rtk hook kiro",
        "timeout_ms": 5000
      }
    ]
  }
}
```

`rtk init` does not yet automate this file edit for Kiro (unlike its `~/.claude/settings.json` JSON-patching for Claude Code) — this is a manual, one-line addition to whichever agent config(s) you want token-optimized. See `../../src/hooks/README.md` for the installer architecture if automating this is added later.

## Testing

```bash
echo '{"hook_event_name":"preToolUse","cwd":"/tmp","tool_name":"shell","tool_input":{"command":"git status"}}' | rtk hook kiro
echo "exit: $?"   # expect 2, stderr suggests `rtk git status`

echo '{"hook_event_name":"preToolUse","cwd":"/tmp","tool_name":"shell","tool_input":{"command":"rtk git status"}}' | rtk hook kiro
echo "exit: $?"   # expect 0, silent — already using rtk
```
