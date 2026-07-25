# RTK - Rust Token Killer

**Usage**: Token-optimized CLI proxy (cuts up to 90% of bash output).

## How Devin CLI uses RTK

A `PreToolUse` hook intercepts every `exec` (shell) command and rewrites it to `rtk <command>` whenever RTK has a filter. Use normal command names (`git status`, `cargo test`, `cat file.md`, `grep pattern .`, `find . -name '*.rs'`, `ls`, etc.) — the hook converts them automatically. Only call `rtk` directly for meta commands (`rtk gain`, `rtk proxy`, etc.) or when the hook has no matching rewrite.

## Meta / always use RTK directly

```bash
rtk gain                 # Show token savings summary
rtk gain --history       # Show savings + command history
rtk discover             # Find missed RTK opportunities (Claude Code history)
rtk proxy <cmd>          # Run raw command without filtering, still tracked
rtk run <cmd>            # Run via sh -c (raw, no filtering or tracking)
rtk pipe                 # Read stdin, apply RTK filter, print
rtk trust                # Trust project-local .rtk/filters.toml
rtk untrust              # Revoke trust for project filters
rtk verify               # Verify hook integrity / TOML filter tests
rtk config               # Show or create RTK config
```

## Common commands auto-rewritten by the hook

```bash
git status, git log, git diff, git add, git commit, git push
cargo test, cargo build, cargo clippy, cargo install
npm run, npx, pnpm, bun, yarn
jest, vitest, pytest, playwright
tsc, eslint, prettier, lint, format, ruff, mypy
docker ps, docker logs, docker build, docker compose
kubectl get, kubectl logs, kubectl describe
gh pr view, gh run list, gh issue list
glab ...
aws ...
psql ...
curl ...
find . -name '*.rs'
grep pattern . -r
cat file.md
ls, tree, wc, wget
```

For the full command list run `rtk --help`.

## Permission Sync

RTK reads `permissions.allow/ask/deny` from Devin CLI's own config files (`.devin/config.json`, `.devin/config.local.json`, `~/.config/devin/config.json`). Allowed commands are auto-approved and rewritten; denied commands are blocked; everything else is rewritten so Devin CLI can prompt on the rewritten command.

## Escape hatches

- `RTK_DISABLED=1 <cmd>` — run raw command without RTK rewrite.
- `rtk proxy <cmd>` — run raw but track usage in `rtk gain --history`.
- `rtk run <cmd>` — fully raw, no tracking.

When you call `rtk` directly, prefix a shell command with `rtk` to get compact output. If RTK has no filter for it, the command runs unchanged.
