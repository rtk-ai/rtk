# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk`.

If Codex hooks are enabled, raw supported Bash commands may be blocked with a message telling you to rerun the `rtk ...` form. Follow that guidance rather than retrying the raw command.

On Windows, or when Codex does not load project hooks such as untrusted local `.codex` config, this prompt guidance may be the only active RTK layer.

Examples:

```bash
rtk git status
rtk cargo test
rtk npm run build
rtk pytest -q
```

## Meta Commands

```bash
rtk gain            # Token savings analytics
rtk gain --history  # Recent command savings history
rtk proxy <cmd>     # Run raw command without filtering
```

## Verification

```bash
rtk --version
rtk gain
which rtk
```
