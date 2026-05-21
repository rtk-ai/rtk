# Add Pi Extension Integration

ID: ticket:20260521-pi-extension
Type: Ticket
Status: closed
Created: 2026-05-21
Updated: 2026-05-21
Risk: medium - adds a new agent integration path and writes into the user's Pi config, but the runtime extension is narrow, fail-open, and locally verifiable.

## Summary

Add first-class RTK support for Pi by shipping a Pi extension that intercepts Pi `bash` tool calls, delegates rewrite decisions to `rtk rewrite`, and mutates the command before execution when RTK has a compact equivalent. The single closure claim is: RTK can install a Pi extension with `rtk init --agent pi`, and the installed extension rewrites Pi bash commands through the existing RTK rewrite registry without blocking command execution when RTK is unavailable or rewrite fails.

## Related Records

- `README.md` - public quick-start and supported-agent table must mention Pi.
- `docs/guide/getting-started/supported-agents.md` - durable user-facing supported-agent guide must include Pi install behavior.
- `hooks/README.md` - hook/plugin architecture record must describe the Pi extension and fail-open behavior.
- `/opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent/README.md` - Pi extension locations and capabilities; Pi auto-discovers `~/.pi/agent/extensions/*.ts`.
- `/opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent/docs/extensions.md` - Pi `tool_call` handlers can mutate built-in `bash` tool input before execution.

## Scope

May change RTK source, docs, and hook artifacts needed for a Pi integration:

- add a `hooks/pi/` TypeScript extension artifact and README;
- add `--agent pi` CLI dispatch and installation logic that writes the extension under the Pi agent config directory;
- update supported-agent documentation and tests;
- install the extension for the current user after implementation.

Must not change RTK command filtering behavior, the rewrite registry semantics, Pi itself, or unrelated agent integrations. The extension must be a thin delegate to `rtk rewrite`, fail open on missing binary/errors/timeouts, and only mutate Pi `bash` tool calls.

Evidence posture: compile/test the touched Rust installation logic where practical, inspect installed extension location, and smoke-test the extension's rewrite helper behavior by invoking `rtk rewrite` or running the install path. Review posture: separate audit would add limited value for this local integration slice if tests and inspection cover dispatch, install path, docs, and fail-open extension behavior.

## Acceptance

- ACC-001: The repo contains a Pi extension artifact that Pi can load from its extension directory, and it rewrites only `bash` tool commands by calling `rtk rewrite` while failing open on errors.
  - Evidence: source inspection plus a lightweight test or type/format-compatible implementation review.
  - Audit: verify no unrelated tool mutation and no blocking behavior on rewrite failure.

- ACC-002: `rtk init --agent pi` installs the Pi extension idempotently into the Pi agent config directory, respecting `PI_CODING_AGENT_DIR` when set and dry-run behavior when requested.
  - Evidence: Rust tests for path resolution/install/idempotence/dry-run or an equivalent command run in an isolated config directory.
  - Audit: inspect that install writes only the expected extension path.

- ACC-003: User-facing docs list Pi as a supported plugin integration and show the install command.
  - Evidence: grep/read updated README and supported-agent docs.
  - Audit: docs match implemented command and behavior.

- ACC-004: The Pi extension is installed for the current user.
  - Evidence: installed `~/.pi/agent/extensions/rtk.ts` exists and matches the repo artifact or install output reports it.
  - Audit: note if Pi must be restarted/reloaded for activation.

## Current State

Closed. Implementation, local install, Rust validation, and audit are complete. The repo now contains a Pi TypeScript extension artifact, `--agent pi` CLI/install dispatch, docs, and Rust tests/source coverage for the install path. The extension is installed at `/Users/crlough/.pi/agent/extensions/rtk.ts` and matched the repo artifact after the final source install. Validation evidence is recorded in `evidence:20260521-pi-extension-validation`: Rust toolchain installed, `cargo fmt --all --check` passes, `cargo clippy --all-targets` passes, `cargo test --all` passes, isolated `PI_CODING_AGENT_DIR` install works, and current-user install works. Audit `audit:20260521-pi-extension-audit` returned `clear` with no material findings. Residual risk: no live Pi end-to-end `tool_call` execution was observed, and the user must restart Pi or run `/reload` for the installed extension to activate.

## Journal

- 2026-05-21: Created ticket with Status `open` from the operator request to add and install RTK Pi support.
- 2026-05-21: Set Status to `active`; current session is executing the bounded Ralph implementation slice in the main rtk worktree.
- 2026-05-21: Added Pi extension artifact, CLI install/uninstall dispatch, docs, and tests/source support; manually installed the extension to `/Users/crlough/.pi/agent/extensions/rtk.ts` because the currently installed `rtk` binary predates `--agent pi`.
- 2026-05-21: Recorded validation dossier `evidence:20260521-pi-extension-validation`. `cargo fmt` and Rust tests were not run because no Rust toolchain was available in this environment. Set Status to `review` pending Rust-toolchain verification or external review.
- 2026-05-21: Installed Rust with rustup after confirming Xcode Command Line Tools were present. Added `rustfmt` and `clippy` components. Ran `cargo fmt`, `cargo fmt --check` (pass), `cargo test` (pass: 1919 passed, 0 failed, 6 ignored), and isolated `PI_CODING_AGENT_DIR` `cargo run -- init --agent pi` with matching installed extension output. Updated `evidence:20260521-pi-extension-validation` with the results.
- 2026-05-21: Ran a bounded Ralph audit pass. Initial findings identified stale current-user install evidence, a fail-open overclaim, and missing clippy/`--all` quality-gate evidence. Fixed the extension with a top-level handler `try/catch`, reinstalled for current user with `cargo run -- init --agent pi`, ran `cargo fmt --all --check`, `cargo clippy --all-targets`, and `cargo test --all`, and updated evidence.
- 2026-05-21: Ran final bounded Ralph audit follow-up and recorded `audit:20260521-pi-extension-audit`; verdict `clear`, no material findings within audited scope. Closed ticket with residual risk limited to no live Pi end-to-end `tool_call` observation and requiring Pi restart or `/reload` for activation.
