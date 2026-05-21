# Pi Extension Validation

ID: evidence:20260521-pi-extension-validation
Type: Evidence Dossier
Status: recorded
Created: 2026-05-21
Updated: 2026-05-21
Observed: 2026-05-21

## Summary

Validation observations for `ticket:20260521-pi-extension`: the Pi extension artifact was installed for the current user, the rewrite helper behavior handles RTK's exit-3 rewrite output, documentation/source references for Pi support are present, a Rust toolchain was installed, formatting/clippy/tests pass, and the new `rtk init --agent pi` path installs the extension into both isolated and current-user Pi config directories.

## Observations

- Observation: The current user's Pi extension file exists and matches the repo artifact at install time.
  - Procedure/source: Ran `mkdir -p "$HOME/.pi/agent/extensions" && cp hooks/pi/rtk.ts "$HOME/.pi/agent/extensions/rtk.ts" && cmp -s hooks/pi/rtk.ts "$HOME/.pi/agent/extensions/rtk.ts" && ls -l "$HOME/.pi/agent/extensions/rtk.ts"` from `/Users/crlough/Code/personal/rtk`.
  - Actual result: `cmp` exited successfully and `ls` showed `/Users/crlough/.pi/agent/extensions/rtk.ts` with size `1728` bytes.

- Observation: Existing installed RTK can rewrite `git status`, returning the rewritten command on stdout with exit code `3`.
  - Procedure/source: Ran `rtk rewrite "git status"; printf 'exit=%s\n' "$?"`.
  - Actual result: stdout contained `rtk git status` and `exit=3`.

- Observation: The Pi extension's Node rewrite-helper behavior returns the rewrite despite RTK exit code `3`.
  - Procedure/source: Ran a Node snippet matching `hooks/pi/rtk.ts`'s `execFile("rtk", ["rewrite", command])` catch path for `git status`.
  - Actual result: stdout was `rtk git status`.

- Observation: Source and docs contain Pi integration references.
  - Procedure/source: Ran `rg -n "Pi|--agent pi|hooks/pi|PI_CODING_AGENT_DIR|rtk.ts" README.md docs/guide/getting-started/supported-agents.md hooks/README.md src/hooks/README.md src/main.rs src/hooks/init.rs src/hooks/constants.rs hooks/pi`.
  - Actual result: matches were found in the new Pi artifact/docs and in `src/main.rs`, `src/hooks/init.rs`, and `src/hooks/constants.rs`.

- Observation: Rust formatter/tests initially could not run in this environment.
  - Procedure/source: Ran `cargo fmt` and checked for Rust toolchain commands.
  - Actual result: `/bin/bash: cargo: command not found`; `which cargo || which rustc || ls ~/.cargo/bin` produced no toolchain path.

- Observation: Rust toolchain is now installed.
  - Procedure/source: Confirmed Xcode Command Line Tools were present, ran the rustup installer, then added missing components with `rustup component add rustfmt clippy`.
  - Actual result: `rustc 1.95.0 (59807616e 2026-04-14)`, `cargo 1.95.0 (f2d3ce0bd 2026-03-21)`, `rustfmt 1.9.0-stable (59807616e1 2026-04-14)`, and `clippy 0.1.95 (59807616e1 2026-04-14)` are available after sourcing `$HOME/.cargo/env`.

- Observation: Rust formatting is clean after applying formatter output.
  - Procedure/source: Ran `cargo fmt --all`, then `cargo fmt --all --check` from `/Users/crlough/Code/personal/rtk`.
  - Actual result: `cargo fmt --all --check` exited `0` with no output.

- Observation: Full Rust clippy gate passes.
  - Procedure/source: Ran `cargo clippy --all-targets` from `/Users/crlough/Code/personal/rtk`.
  - Actual result: `clippy_exit=0`; tail output included `Finished dev profile [unoptimized + debuginfo] target(s) in 0.41s`.

- Observation: Full Rust test suite passes.
  - Procedure/source: Ran `cargo test --all` from `/Users/crlough/Code/personal/rtk`.
  - Actual result: `test_all_exit=0`; tail output included `test result: ok. 1919 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 1.74s`.

- Observation: The implemented `rtk init --agent pi` install path writes the extension into an isolated Pi config directory and the installed file matches the repo artifact.
  - Procedure/source: Ran `TMP_PI_DIR=$(mktemp -d); PI_CODING_AGENT_DIR="$TMP_PI_DIR" cargo run --quiet -- init --agent pi; cmp -s hooks/pi/rtk.ts "$TMP_PI_DIR/extensions/rtk.ts"; ls -l "$TMP_PI_DIR/extensions/rtk.ts"; PI_CODING_AGENT_DIR="$TMP_PI_DIR" cargo run --quiet -- init --agent pi`.
  - Actual result: `cargo run` reported `Pi extension installed`; `cmp` exited successfully; `ls` showed `$TMP_PI_DIR/extensions/rtk.ts` with size `1728` bytes; second run completed successfully.

- Observation: The implemented `rtk init --agent pi` install path was run for the current user after audit follow-up, and the installed file matches the repo artifact.
  - Procedure/source: Ran `cargo run --quiet -- init --agent pi; cmp -s hooks/pi/rtk.ts /Users/crlough/.pi/agent/extensions/rtk.ts; ls -l /Users/crlough/.pi/agent/extensions/rtk.ts`.
  - Actual result: `cargo run` reported `Pi extension installed`; `cmp` exited successfully; `ls` showed `/Users/crlough/.pi/agent/extensions/rtk.ts` with size `1958` bytes.

## Artifacts

- `hooks/pi/rtk.ts` - repo Pi extension artifact installed for the current user.
- `/Users/crlough/.pi/agent/extensions/rtk.ts` - installed Pi extension file, matching `hooks/pi/rtk.ts` at observation time.
- Command excerpt: `rtk rewrite "git status"` produced `rtk git status` with exit code `3`, which the extension catch path handles.
- Command excerpt: `cargo fmt --all --check` exited `0` with no output.
- Command excerpt: `cargo clippy --all-targets` exited `0`.
- Command excerpt: `cargo test --all` ended with `test result: ok. 1919 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 1.74s`.
- Command excerpt: isolated `PI_CODING_AGENT_DIR="$TMP_PI_DIR" cargo run --quiet -- init --agent pi` wrote `$TMP_PI_DIR/extensions/rtk.ts`, and `cmp -s hooks/pi/rtk.ts "$TMP_PI_DIR/extensions/rtk.ts"` succeeded.
- Command excerpt: current-user `cargo run --quiet -- init --agent pi` wrote `/Users/crlough/.pi/agent/extensions/rtk.ts`, and `cmp -s hooks/pi/rtk.ts /Users/crlough/.pi/agent/extensions/rtk.ts` succeeded.

## What This Shows

- `ticket:20260521-pi-extension#ACC-001` - supports - the extension artifact targets Pi `bash` tool calls and the observed helper logic rewrites `git status` through `rtk rewrite`, including RTK's exit-code-3 path; full Rust tests also pass after adding the integration code.
- `ticket:20260521-pi-extension#ACC-002` - supports - source inspection, Rust tests, formatting, and an isolated `PI_CODING_AGENT_DIR` `cargo run -- init --agent pi` install confirmed the path writes the expected extension file and handles a repeat run.
- `ticket:20260521-pi-extension#ACC-003` - supports - README, supported-agent guide, hook docs, and Pi README all contain Pi install/behavior documentation.
- `ticket:20260521-pi-extension#ACC-004` - supports - the extension is installed at `/Users/crlough/.pi/agent/extensions/rtk.ts` and matched the repo artifact when copied.

## What This Does Not Show

- Does not prove Pi has reloaded the extension in the currently running session; restart Pi or run `/reload` to activate the installed file.
- Does not exercise an actual Pi `tool_call` event end-to-end; it only verifies the installed artifact and rewrite helper behavior.
- Does not prove behavior for every RTK rewrite pattern or every shell edge case.

## Related Records

- `ticket:20260521-pi-extension` - owns the executable work and acceptance criteria this dossier supports.
