# RTK — Warp (Oz) Integration

Use [RTK (Rust Token Killer)](https://github.com/rtk-ai/rtk) with [Warp](https://www.warp.dev) to save 60-90% of tokens on common dev operations.

## How It Works

Warp's Oz agent doesn't have a pre-execution hook for command rewriting, so this integration uses a **skill** — a markdown instruction set that tells the agent to always prefix commands with `rtk`.

When Oz runs a supported command (git, cargo, docker, etc.), the skill instructs it to use the `rtk` prefix, which filters and compresses the output before it enters the LLM context.

## Quick Start

```bash
# Global (all projects):
mkdir -p ~/.agents/skills/rtk
cp warp/skills/rtk/SKILL.md ~/.agents/skills/rtk/SKILL.md

# Or per-project:
mkdir -p .agents/skills/rtk
cp warp/skills/rtk/SKILL.md .agents/skills/rtk/SKILL.md
```

Restart Warp. The skill is auto-discovered.

## Verify

Ask Oz: "What skills do I have?" — RTK should appear in the list.

Then run a command like `git status` and check that Oz uses `rtk git status`.

## Supported Skill Directories

Warp discovers skills from many directories. You can place the skill in whichever you prefer:

- `.agents/skills/` (recommended)
- `.warp/skills/`
- `.claude/skills/`
- `.codex/skills/`
- `.opencode/skills/`

The same applies for global skills (`~/.agents/skills/`, `~/.warp/skills/`, etc.).

## Comparison with Hook-Based Integrations

| Aspect | Claude Code / OpenCode | Warp |
|---|---|---|
| Mechanism | Pre-execution hook rewrites commands transparently | Skill instructs agent to use `rtk` prefix |
| Adoption | 100% (automatic) | Depends on agent adherence to skill (~85-95%) |
| Context cost | 0 tokens (hook) or ~10 tokens (RTK.md) | ~200 tokens (skill loaded on demand) |
| Setup | `rtk init -g` / plugin copy | Copy SKILL.md to skills directory |

## Uninstalling

```bash
rm -rf ~/.agents/skills/rtk
# or
rm -rf .agents/skills/rtk
```
