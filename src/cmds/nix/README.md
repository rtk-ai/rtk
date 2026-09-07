# Nix

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `nix_cmd.rs` handles `nix`, plus legacy `nix-build`, `nix-shell`, and `nix-env` through one generic filter: download/copy progress is counted into summary lines, store-path lists collapse to counts, eval traces are dropped, errors and warnings survive with compressed hashes
- When the invocation wraps another command, rtk respawns through nix with a nested rtk so the inner command is filtered by its own module while still running inside the nix environment:

  ```text
  nix develop -c cargo test   →   nix develop -c <rtk-exe> rtk cargo test
  ```

- Detected wrap points: `-c|--command` on modern `nix` verbs, and `-c|--command|--run` on `nix-shell` where the value is one shell-string token. Post-`--` positionals on `nix run` and `nix shell` are not wrap points (they go to the flake's app or parse as more installables), so they stay on the generic path
- The outer layer streams passthrough and tracks ~0% reduction; the inner rtk does the real filtering and tracking, so nested runs produce two entries in `rtk gain --history`
- Delegation only happens when rewriting the wrapped command is a pure `rtk ` prefix insertion and re-splitting reproduces the original tokens plus that prefix. Compound commands, unknown tools such as `zsh`, output-tool specials, and arguments whose spaces would not survive the round trip all fall back to the generic nix filter

## Cross-command

- Hook routing uses the existing `nix*` rules (`src/discover/rules.rs`), so no new registry entries were needed; `rewrite_command` from `src/discover/registry.rs` decides whether the wrapped part is delegatable
