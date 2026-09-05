# RTK agent capability baseline

Recorded 2026-09-05 on the Windows checkout `D:\src\rtk`.

## Environment

| Item | Observed value |
|---|---|
| Source branch | `RTK-Global-Optimization` |
| Source commit | `d296b2b` plus intentionally uncommitted implementation work |
| Installed RTK | `0.46.1-dev.10` |
| Installed RTK executable | `C:\Users\dmitr\.cargo\bin\rtk.exe` |
| Codex executable configured by the host | `C:\Users\dmitr\AppData\Local\OpenAI\Codex\bin\27d6a192e9c98618\codex.exe` |
| Codex profile | `C:\Users\dmitr\.codex\config.toml` |
| RTK MCP registration | One absolute `[mcp_servers.rtk]` entry using `rtk.exe mcp` |
| Global instructions | `C:\Users\dmitr\.codex\AGENTS.md` references `C:\Users\dmitr\.codex\RTK.md` |
| Project trust | `D:\src\rtk` is trusted by the active Codex profile |

`rtk init -g --codex` was run after a private backup of the Codex config. A
second `rtk init -g --codex --dry-run` reported `Nothing written` and the MCP
registration was `already up to date`.

## Protocol checks

The current Codex hook output schema accepts a `PreToolUse` response with
`hookSpecificOutput`, `hookEventName`, optional `updatedInput`, and optional
permission fields. The source of truth used for this baseline is the
[Codex pre-tool-use output schema](https://github.com/openai/codex/blob/main/codex-rs/hooks/schema/generated/pre-tool-use.command.output.schema.json).

Claude Code’s current hook contract uses JSON settings with a `PreToolUse`
matcher group and command handlers. A handler may return
`hookSpecificOutput.updatedInput` or remain silent with exit code 0 so the
normal permission flow continues. See the [Claude Code hooks reference](https://code.claude.com/docs/en/hooks).

RTK's Codex adapter only returns `updatedInput` when the event is the canonical
`Bash` tool and Codex supplies `permission_mode = "bypassPermissions"`. In the
normal `default` approval mode the adapter is intentionally silent: Codex runs
its own per-command approval after the hook, and RTK cannot safely recreate that
decision for a renamed `rtk ...` command.

Live child-agent and follow-up probes were not available through the current
desktop execution interface. They remain an explicit compatibility gate; no
fixture result below is presented as live worker evidence.

## Deterministic fixture manifest

`tests/fixtures/agent_capabilities.json` defines the baseline cases and
`tests/agent_capability_fixture_test.rs` validates that every case has:

- typed argv without a duplicate leading `rtk`;
- a unique ID, route, consumer, exit code, and output contract;
- a checked-in fixture file; and
- exact output for machine consumers.

The fixture values are sanitized contract examples, not measured model token
usage. Paired benchmark measurements must record raw bytes, complete
model-visible bytes, estimated tokens, recovery bytes, fact preservation, and
exit-code preservation separately.

Run the validator with:

```text
cargo test --test agent_capability_fixture_test
```
