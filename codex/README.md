# RTK — Codex CLI Integration

Use [RTK (Rust Token Killer)](https://github.com/rtk-ai/rtk) with [OpenAI Codex CLI](https://github.com/openai/codex) to save 60-90% of tokens on common dev operations.

## How It Works

Codex CLI doesn't have a pre-execution hook for command rewriting, so this integration uses a **skill** — a markdown instruction set that tells the agent to always prefix commands with `rtk`.

When Codex runs a supported command (git, cargo, docker, etc.), the skill instructs it to use the `rtk` prefix, which filters and compresses the output before it enters the LLM context.

## Quick Start

```bash
# Global (all projects):
mkdir -p ~/.codex/skills/rtk
cp codex/skills/rtk/SKILL.md ~/.codex/skills/rtk/SKILL.md

# Or per-project:
mkdir -p .codex/skills/rtk
cp codex/skills/rtk/SKILL.md .codex/skills/rtk/SKILL.md
```

## Alternative: AGENTS.md

If you prefer to inject RTK instructions directly into your project rules:

```bash
cat codex/skills/rtk/SKILL.md >> AGENTS.md
```

## Verify

After setup, run Codex and ask it to execute `git status`. It should use `rtk git status`. Then check savings:

```bash
rtk gain
```

## Requirements

- [rtk](https://github.com/rtk-ai/rtk) >= 0.23.0 on your PATH
- [Codex CLI](https://github.com/openai/codex)

## Uninstalling

```bash
rm -rf ~/.codex/skills/rtk
# or
rm -rf .codex/skills/rtk
```
