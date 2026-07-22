# AGENTS.md

Router for coding agents (and new human contributors) working on rtk itself. Each row
is one rule, terse on purpose — this file is not a copy of the rules, it just points
you at the authoritative doc before you start coding, so the real thing can't drift
out of sync with a summary.

| If you're about to... | Rule | See |
|---|---|---|
| Write any code at all | Correctness > token savings when the user explicitly requests detail; RTK's output must be transparent (never a format the LLM wouldn't expect); every filter falls back to raw output on failure; <10ms startup, no async | [CONTRIBUTING.md § Design Philosophy](CONTRIBUTING.md#design-philosophy) |
| Decide if a feature belongs in RTK | Filtering/compression of CLI output is in scope; general-purpose tooling, config management, and anything not about token savings is not | [CONTRIBUTING.md § What Belongs in RTK?](CONTRIBUTING.md#what-belongs-in-rtk) |
| Add a new command filter | Rust module vs TOML filter decision, the six-phase execution flow, exit-code/tee/truncation contracts every filter must satisfy | [src/cmds/README.md § Adding a New Command Filter](src/cmds/README.md#adding-a-new-command-filter) |
| Write or touch Rust code | No `async`, no `unwrap()` in production, `lazy_static!` for all regex, fallback-on-failure, exit code propagation | [.claude/rules/rust-patterns.md](.claude/rules/rust-patterns.md) |
| Add or change a filter's tests | Colocated `#[cfg(test)]` unit tests; inline fixtures vs `include_str!` fixtures in `tests/fixtures/`, when to use which | [.claude/rules/cli-testing.md](.claude/rules/cli-testing.md) |
| Search the codebase | Grep → Glob → Read → Explore agent, in that order; never shell out to `find`/`grep`/`rg` directly | [.claude/rules/search-strategy.md](.claude/rules/search-strategy.md) |
| Understand the overall system | Command lifecycle, module map, filtering strategy matrix, token tracking | [docs/contributing/ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md) |
| Commit | Conventional-commit format, one logical change per commit | [CONTRIBUTING.md § Commit Messages & Changelog](CONTRIBUTING.md#commit-messages--changelog) |
| Name a branch | `type/short-description` convention | [CONTRIBUTING.md § Branch Naming Convention](CONTRIBUTING.md#branch-naming-convention) |
| Open a PR | Scope rules, required tests/docs, CLA | [CONTRIBUTING.md § Pull Request Process](CONTRIBUTING.md#pull-request-process) |
| Commit, before you do | `cargo fmt --all && cargo clippy --all-targets && cargo test --all` must pass — zero tolerance on clippy warnings | [CONTRIBUTING.md](CONTRIBUTING.md) |

Not to be confused with `rtk init --codex`, which writes RTK-awareness into a *user's own*
project `AGENTS.md` so Codex/Kimi route commands through RTK. This file is about
contributing to rtk's own codebase, not RTK's product feature of the same name.
