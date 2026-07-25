# Upstream PR draft: Fix stale command in `.github/hooks/rtk-rewrite.json`

## Title
fix(github): update copilot hook json to use `rtk hook copilot`

## Summary

The repository's `.github/hooks/rtk-rewrite.json` still references the legacy `rtk hook` command. This PR updates it to `rtk hook copilot` and adds the `preToolUse` entry used by Copilot CLI, matching the format written by `rtk init -g --copilot`.

## Changes

- `.github/hooks/rtk-rewrite.json`
  - `command` changed from `rtk hook` to `rtk hook copilot`.
  - Added `version: 1`.
  - Added `preToolUse` block with `bash` / `powershell` variants for Copilot CLI.

## Test plan

- [ ] `rtk init -g --copilot` produces the same JSON structure as `.github/hooks/rtk-rewrite.json`.
- [ ] `rtk verify` (if checking copilot hooks) passes.
