# Windows Claude/Codex Hook Validation

Use `scripts/rtk-windows-oracle.ps1` as the aggregated Windows gate for native
Claude Code and Codex hook changes. It is a source and release-binary oracle;
it does not install or replace an RTK binary.

## Prerequisites

- Windows with PowerShell 7 or newer.
- Rust/Cargo and Python available on `PATH`.
- A built RTK binary, normally `target\release\rtk.exe`.
- Optionally, a command-corpus CSV with `Source`, `Shell`, `Category`, and
  `Command` columns.

Build first, then run from the repository root:

```powershell
cargo build --release
pwsh -NoProfile -File .\scripts\rtk-windows-oracle.ps1 `
  -Rtk .\target\release\rtk.exe
```

To include the historical corpus gate:

```powershell
pwsh -NoProfile -File .\scripts\rtk-windows-oracle.ps1 `
  -Rtk .\target\release\rtk.exe `
  -Corpus C:\path\to\rtk-command-corpus.csv
```

By default, evidence is written to a timestamped directory under `target/`,
which is ignored by Git. Use `-OutDir` when a release process needs a stable
evidence location. Do not commit generated command payloads, databases, build
logs, or local configuration backups.

## What the oracle checks

- Claude and Codex direct-hook JSON contracts, including permission deferral.
- Bash-like, PowerShell, and `cmd.exe` rewrite coverage.
- Instruction-file passthrough and database-client no-rewrite guards.
- Raw command versus RTK output, exit codes, parse failures, and token savings.
- Documentation consistency and local Markdown links.
- Parent/PATH binary shadowing and the resolved binary version/hash boundary.
- The serial full Cargo test suite.
- An isolated gain database, so the gate never resets or contaminates real
  user usage data.
- When `-Corpus` is provided, a 2200-command `rtk hook check` sample. Corpus
  commands are classified but never executed.

The script writes `summary.json`, `cases.json`, and supporting artifacts. A
successful process exit requires every included gate to pass. Test totals and
savings values naturally change as the codebase evolves; record the actual
run values in the PR instead of hard-coding them in this document.

## Release boundaries

Treat these as separate outcomes:

1. Source tests and release build pass.
2. The source release binary passes the Windows oracle.
3. Installed binary locations match the approved source SHA256 and pass direct
   hook/runtime smoke after a recoverable backup.
4. Authenticode signing is valid for a formal release.
5. The upstream PR has the required review and remote checks.

For the Windows PR release gate, attach current real `rtk gain` data and a real
Windows `cmd.exe` screenshot, list every resolved or externally attributed
`RTK_run_log` report, and state any unexecuted cross-platform or signing gate.
