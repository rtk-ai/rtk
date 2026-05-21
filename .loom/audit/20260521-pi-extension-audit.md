# Pi Extension Integration Audit

ID: audit:20260521-pi-extension-audit
Type: Audit
Status: recorded
Created: 2026-05-21
Updated: 2026-05-21
Audited: 2026-05-21
Target: ticket:20260521-pi-extension

## Summary

A bounded Ralph review audited the RTK Pi extension integration, acceptance evidence, install behavior, fail-open behavior, and docs. The final follow-up audit found no material findings within scope and gave a `clear` verdict, with residual risk limited to lack of live Pi `tool_call` end-to-end observation and the need to reload/restart Pi.

## Target

The target was `ticket:20260521-pi-extension` and the current uncommitted diff in `/Users/crlough/Code/personal/rtk` adding first-class Pi support:

- `hooks/pi/rtk.ts`
- `hooks/pi/README.md`
- `src/main.rs`
- `src/hooks/init.rs`
- `src/hooks/constants.rs`
- `README.md`
- `docs/guide/getting-started/supported-agents.md`
- `hooks/README.md`
- `src/hooks/README.md`
- `.loom/tickets/20260521-pi-extension.md`
- `.loom/evidence/20260521-pi-extension-validation.md`

## Audit Scope And Lenses

The final Ralph review challenged the current final state after prior audit follow-up fixes. Lenses used:

- acceptance and scope: whether `ACC-001` through `ACC-004` are supported now;
- evidence exactness and freshness;
- implementation quality and fail-open behavior;
- security/trust boundary;
- docs and follow-through.

Out of scope: live Pi UI/runtime end-to-end execution, every RTK rewrite pattern, and upstream Pi API changes after the audited documentation/source state.

## Context And Evidence Reviewed

- Ralph review run: headless Pi review launched from `ticket:20260521-pi-extension` and `evidence:20260521-pi-extension-validation`, with read-only scope and explicit lenses.
- `git status --short`, `git diff --stat`, and current untracked `hooks/pi/*` - current source state under review.
- `hooks/pi/rtk.ts` - Pi extension implementation and fail-open behavior.
- `hooks/pi/README.md` - Pi integration installation and failure-behavior docs.
- `src/main.rs`, `src/hooks/init.rs`, `src/hooks/constants.rs` - CLI dispatch, install/uninstall path, path resolution, and tests.
- `README.md`, `docs/guide/getting-started/supported-agents.md`, `hooks/README.md`, `src/hooks/README.md` - public and internal docs.
- `/opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent/README.md` and `docs/extensions.md` - Pi extension location and `tool_call` mutation behavior.
- `/Users/crlough/.pi/agent/extensions/rtk.ts` - installed current-user extension; Ralph reported it exists and matches `hooks/pi/rtk.ts` by `cmp`/SHA-256.
- `evidence:20260521-pi-extension-validation` - records `cargo fmt --all --check`, `cargo clippy --all-targets`, `cargo test --all`, isolated install, and current-user install/match observations.

## Findings

None - no material findings within audited scope.

The Ralph review specifically rechecked the prior blockers and reported them resolved:

- installed current-user Pi extension now exists and matches the repo artifact;
- fail-open overclaim is resolved by subprocess-level fallback plus top-level `try/catch` around the `tool_call` handler;
- Rust quality gate evidence now includes `cargo fmt --all --check`, `cargo clippy --all-targets`, and `cargo test --all`.

## Verdict

`clear` - within the audited scope, the Pi extension integration satisfies `ACC-001` through `ACC-004`, and the final review did not identify material blockers. This verdict does not itself close or accept the ticket; it supports the ticket owner making a closure/acceptance decision from the ticket, evidence, and audit records.

## Required Follow-up

No material audit follow-up is required before ticket closure.

Ticket closure should still state the live-runtime limitation explicitly: the installed extension requires Pi restart or `/reload` before it is active in a running session.

## Residual Risk

- No actual Pi end-to-end `tool_call` execution was observed; validation is source inspection plus helper/install checks.
- Pi must restart or run `/reload` before the installed extension is active in a running session.
- Extension behavior depends on the local `rtk` binary found on Pi's PATH and Pi's documented extension API remaining compatible.

## Related Records

- `ticket:20260521-pi-extension` - owns acceptance, finding disposition, and closure state.
- `evidence:20260521-pi-extension-validation` - records the validation observations consumed by this audit.
