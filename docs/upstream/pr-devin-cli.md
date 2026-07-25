# Upstream PR draft: Add Devin CLI support

## Title
feat(hooks): Add first-class Devin CLI integration

## Summary

Adds native support for [Devin CLI](https://docs.devin.ai/get-started) via a `PreToolUse` hook and lifecycle context hooks. Devin CLI shell commands are rewritten to `rtk <command>` whenever RTK has a filter, with decisions (`approve` / `block`) synced to Devin CLI's own permission settings.

Closes #3205

## Changes

- `src/hooks/hook_cmd.rs`
  - New `process_devin_payload`, `devin_decide_for_command`, `devin_decide_explicit_rtk`, and helpers for explicit `rtk <meta>` / `rtk proxy <cmd>` / `rtk <allowed>` handling.
- `src/hooks/init.rs`
  - `run_devin_mode`, `uninstall_devin`, `show_devin_config`, `devin_rtk_hook_dir`, `write_devin_hook_files`, and helpers to patch/remove Devin CLI hook config.
  - Writes `rtk-devin.js`, `rtk-instructions.md`, and `.gitignore` to the RTK hook directory.
- `src/hooks/integrity.rs`
  - `run_verify_devin` and `verify_devin_hook_dir` verify installed Devin lifecycle files against the source-of-truth content embedded in the binary.
- `src/hooks/permissions.rs`
  - `load_devin_rules` and `get_devin_settings_paths` read Devin CLI `permissions.allow/ask/deny` from `~/.config/devin/config.json` and `.devin/config.json*`.
- `src/hooks/constants.rs`
  - Adds `DEVIN_*` constants (matcher, hook command, config dir env).
- `src/main.rs`
  - Adds `AgentTarget::Devin` and `HookCommands::Devin`, wires `rtk verify` to run Devin integrity checks.
- `hooks/devin/rtk-devin.js`, `hooks/devin/rtk-instructions.md`, `hooks/devin/rtk-awareness.md`
  - New source-of-truth lifecycle hook files, included at build time via `include_str!`.
- `hooks/devin/test-rtk-devin.sh`
  - 34-test regression suite for the Devin CLI hook.
- `docs/guide/getting-started/supported-agents.md`, `README.md`, `INSTALL.md`
  - Update Devin CLI docs and supported-agents table.
- `docs/guide/devin-cli.md`
  - New dedicated Devin CLI setup and troubleshooting guide.
- `.github/hooks/rtk-rewrite.json`
  - Fix stale `rtk hook` command to `rtk hook copilot` and add the `preToolUse` schema used by Copilot CLI.

## Test plan

- [ ] `cargo test` passes (2505+ tests).
- [ ] `cargo clippy --all-targets` passes with no warnings.
- [ ] `bash hooks/devin/test-rtk-devin.sh` passes (34/34).
- [ ] Clean install: clone fork, run `bash install-devin.sh`, restart Devin CLI, run `git status` → should execute `rtk git status`.
- [ ] `rtk verify` reports `PASS` for Devin CLI global and project hook files.
- [ ] `rtk init -g --agent devin --uninstall` removes RTK entries and hook files without touching other hooks.

## Usage

```bash
# One-command install from the fork
git clone https://github.com/warelik/rtk.git
cd rtk
bash install-devin.sh

# Or, if rtk is already installed
rtk init -g --agent devin
```

Then restart Devin CLI and test with `git status`.

## Backwards compatibility

- No existing integrations are changed.
- Devin CLI is opt-in via `--agent devin`.
- Lifecycle hooks are idempotent; re-running `rtk init -g --agent devin` is safe.

## Notes for reviewers

- This PR intentionally takes a hook-first approach: no `AGENTS.md`/`RTK.md` injection is written, matching Devin CLI's native hook architecture.
- The lifecycle script uses a small state file (`.rtk-active`) to avoid re-injecting the full instruction block on every user prompt while still re-injecting after context compaction.
- Devin CLI config can be overridden with `$DEVIN_CONFIG_DIR`, and project hook files use `$DEVIN_PROJECT_DIR` for portability.
