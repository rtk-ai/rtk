# Output-route inventory after semantic migration

Recorded 2026-09-05 from the working tree for Task 10. This is a route
inventory, not a claim that every command in the ecosystem is supported.

## Current measured baseline

The local RTK database was queried with `rtk gain -f json` after the focused
implementation checks:

| Counter | Value |
|---|---:|
| Commands | 11,957 |
| Producer input estimate | 551,655,542 |
| Displayed output estimate | 527,034,677 |
| Saved estimate | 24,629,619 |
| Aggregate savings | 4.4647% |

These values include the current work session and are only a workload snapshot;
they are not used as a historical performance claim. The highest-impact rows
were `rtk test cargo test ...`, `rtk rg`, `rtk read`, and `rtk find`.

## Migrated command-module routes

The production command modules no longer call `runner::run_streamed()`. Existing
specialized string filters are adapted by
`runner::run_ai_from_filter()` until they can be replaced with native semantic
parsers. The adapter adds:

- native status and exit facts, including an explicit failed/no-diagnostic case;
- severity ordering (`error`, `warning`, informational and success records);
- omitted-item accounting from the filter result;
- the shared budget, recovery, and never-worse emission contract.

The migrated families are Git status/log/diff/show/worktree/branch,
GitHub and GitLab human-facing list/view/check/trace/release routes, Cargo
build/check/test/clippy, Go test/build/vet, JVM Gradle/Maven/SBT, TypeScript,
Python test/type/lint, PHP test/lint, Ruby test/lint, JavaScript package/build
routes, directory/tree/wc listings, CTest, psql, container logs/builds, and
the existing specialized semantic routes.

Explicit machine-readable, binary, blob, interactive, word-diff, and
caller-selected format routes remain exact through
`run_passthrough_with_reason()` or their existing native adapter. Examples
include Cargo `--message-format=json`, Go `-json`/benchmark output, Gradle
diagnostic verbosity, Git `--porcelain`, `git show rev:path`, and GitHub/GitLab
user-selected JSON or web output.

## Remaining intentional capture or direct-output paths

These paths remain outside the generic semantic runner for a concrete reason;
they are follow-up candidates only when their native contract can be modeled
without changing behavior.

| Location / route | Contract reason | Verification or guard |
|---|---|---|
| `src/cmds/git/git.rs` `checkout` | Established action acknowledgement is deliberately stable (`ok <branch>` / restored-file count); adding semantic status/facts would change a small, non-high-volume user contract. | Existing checkout integration tests; native exit code remains authoritative. |
| `src/cmds/git/git.rs` mutation and explicit-format branches | Writes, explicit stat/word/blob output, and caller-selected formats must retain native output and timing. | `ExactReason` passthrough branches plus Git unit suite. |
| `src/cmds/git/gt_cmd.rs` | `gt` has no stable RTK semantic parser in this checkout; unknown output must remain transparent. | Native capture/passthrough and exit propagation tests. |
| `src/cmds/go/go_cmd.rs` `run_other` and `go tool golangci-lint` | Unknown Go subcommands and version-dependent golangci v1/v2 JSON have different executable/exit contracts. | Version parser and golangci fixture tests; unsupported commands stay native. |
| `src/cmds/cloud/aws_cmd.rs` | Several actions have command-specific JSON/text/machine modes and provider-specific failures; the direct adapter preserves AWS's selected mode. | AWS filter/parser tests and machine-format bypasses. |
| `src/cmds/cloud/container.rs` Docker ps/images and compose passthrough | Table formatting may require a second native `--format` query, while compose exec/interactive/action routes must preserve native streams. | Container formatter tests and explicit passthrough routing. |
| `src/cmds/python/pip_cmd.rs` package list/outdated and write commands | `pip`/`uv pip` selection is environment-sensitive; writes and unknown subcommands are transparent, while JSON list handling retains its provider fallback. | Existing pip JSON/parser tests and tool-selection checks. |
| `src/cmds/js/{lint,playwright_cmd,pnpm_cmd,prisma_cmd,vitest_cmd}.rs` | Structured reports, test reporters, browser processes, install actions, and JSON intended for another program are exact or tool-specific. | Existing structured-output and passthrough tests. |
| `src/cmds/system/{format_cmd,summary,search}.rs` | Generic formatter/summary wrappers and search JSON/inventory/replace modes cannot assume human text; the default human `rtk rg` route uses bounded streaming separately. | Search large-line tests, machine-mode classifiers, and exact replace/JSON routes. |
| `src/core/runner.rs` legacy wrappers | Public compatibility APIs remain for external adapters and tests; `git checkout` also retains its established exit-aware acknowledgement path. | `rtk rg` inventory check and runner contract tests. |

## Large-output invariant

The default `rtk rg` match routes use
`StreamingStdout` with a 64 KiB retained line prefix. The producer line is
fully drained, total consumed bytes are tracked, and a truncation marker plus
match/omission facts are emitted. When the raw output exceeds the configured
tee limit, the output is bounded and explicitly reports unavailable recovery;
the previous behavior of replaying a huge line as an unbounded raw fallback is
not used for this route.

The structured `rg --json`, count, and inventory modes retain their explicit
machine/structured contracts and are not silently converted to human semantic
text.

## Reproducible checks

```text
cargo fmt --all -- --check
cargo check --bin rtk
cargo test --test semantic_route_migration_test
cargo test --test search_large_output_test
RUST_MIN_STACK=8388608 cargo test --all    # required on Windows for the expanded CLI parser
cargo clippy --all-targets
cargo build --release
```

The full suite passed on 2026-09-05 with 3,357 tests passed and 8 ignored.
The stack override is a test-process setting only; it does not change the
release binary. This document should be updated if a remaining path is
migrated or its exactness reason changes.
