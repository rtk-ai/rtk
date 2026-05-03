# Contributing to rtk

**Welcome!** We appreciate your interest in contributing to rtk.

## Quick Links

- [Report an Issue](../../issues/new)
- [Open Pull Requests](../../pulls)
- [Start a Discussion](../../discussions)
- [Technical Documentation](docs/contributing/TECHNICAL.md) — Architecture, end-to-end flow, folder map, how to write tests

---

## What is rtk?

**rtk (Rust Token Killer)** is a coding agent proxy that cuts noise from command outputs. It filters and compresses CLI output before it reaches your LLM context, saving 60-90% of tokens on common operations. The vision is to make AI-assisted development faster and cheaper by eliminating unnecessary token consumption.

---

## Ways to Contribute

| Type | Examples |
|------|----------|
| **Report** | File a clear issue with steps to reproduce, expected vs actual behavior |
| **Fix** | Bug fixes, broken filter repairs |
| **Build** | New filters, new command support, new features (for core features, discuss with maintainers before) |
| **Review** | Review open PRs, test changes locally, leave constructive feedback |
| **Document** | Improve docs, clarify |
---

## Design Philosophy

Four principles guide every RTK design decision. Understanding them helps you write contributions that fit naturally into the project.

### Correctness VS Token Savings

When a user or LLM explicitly requests detailed output via flags (e.g., `git log --comments`, `cargo test -- --nocapture`, `ls -la`), respect that intent. Compressing explicitly-requested detail defeats the purpose — the LLM asked for it because it needs it.

Filters should be flag-aware: default output (no flags) gets aggressively compressed, but verbose/detailed flags should pass through more content. When in doubt, preserve correctness.

> Example: `rtk cargo test` shows failures only (90% savings). But `rtk cargo test -- --nocapture` preserves all output because the user explicitly asked for it.

### Transparency

The LLM doesn't know RTK is involved for which commands, hooks rewrite commands silently. RTK's output must be a valid, useful subset of the original tool's output, not a different format the LLM wouldn't expect. If an LLM parses `git diff` output, RTK's filtered version must still look like `git diff` output.

Don't invent new output formats. Don't add RTK-specific headers or markers in the default output. The filtered output should be indistinguishable from "a shorter version of the real command."

### Never Block

If a filter fails, fall back to raw output. RTK should never prevent a command from executing or producing output. Better to pass through unfiltered than to error out. Same for hooks: exit 0 on all error paths so the agent's command runs unmodified.

Every filter needs a fallback path. Every hook must handle malformed input gracefully.

### Zero Overhead

<10ms startup. No async runtime. No config file I/O on the critical path. If developers perceive any delay, they'll disable RTK. Speed is the difference between adoption and abandonment.

`lazy_static!` for all regex. No network calls. No disk reads in the hot path. Benchmark before/after with `hyperfine`.

### Extensibility

Always use components already in place to avoid duplication, also use extensible modules when this is possible.
If you want to submit a new core feature, this is an important point to watch.

---

## What Belongs in RTK?

### In Scope

Commands that produce **text output** (typically 100+ tokens) and can be compressed **60%+** without losing essential information for the LLM.

- Test runners (vitest, pytest, cargo test, go test)
- Linters and type checkers (eslint, ruff, tsc, mypy)
- Build tools (cargo build, dotnet build, make, next build)
- VCS operations (git status/log/diff, gh pr/issue)
- Package managers (pnpm, pip, cargo install, brew)
- File operations (ls, tree, grep, find, cat/head/tail)
- Infrastructure tools with text output (docker, kubectl, terraform)

When implementing a new filter/cmds, be aware of the [Design Philosophy](#design-philosophy) above.

### Out of Scope

- Interactive TUIs (htop, vim, less): not batch-mode compatible
- Binary output (images, compiled artifacts): no text to filter
- Trivial commands: not worth the overhead and may loose important informations
- Commands with no text output: nothing to compress
- Others features not related to a LLM-proxy like RTK

### TOML vs Rust: Which One?

| Use **TOML filter** when | Use **Rust module** when |
|--------------------------|--------------------------|
| Output is plain text with predictable line structure | Output is structured (JSON, NDJSON) |
| Regex line filtering achieves 60%+ savings | Needs state machine parsing (e.g., pytest phases) |
| No need to inject CLI flags | Needs to inject flags like `--format json` |
| No cross-command routing | Routes to other commands (lint → ruff/mypy) |
| Examples: brew, df, shellcheck, rsync, ping | Examples: vitest, pytest, golangci-lint, gh |

See [`src/filters/README.md`](src/filters/README.md) for TOML filter guidance and [`src/cmds/README.md`](src/cmds/README.md) for Rust module guidance.

### Adding a Filter

For the step-by-step checklist (create filter, register rewrite pattern, register in main.rs, write tests, update docs), see [src/cmds/README.md — Adding a New Command Filter](src/cmds/README.md#adding-a-new-command-filter).

---

## Commit Messages & Changelog

RTK uses [Conventional Commits](https://www.conventionalcommits.org/) and [release-please](https://github.com/googleapis/release-please) to **auto-generate CHANGELOG.md, version bumps, and GitHub releases**. Never edit `CHANGELOG.md` manually — it is fully managed by release-please from your commit messages.

### Commit format

```
<type>(<scope>): <short description>
```

| Type | Semver Impact | When to Use |
|------|---------------|-------------|
| `feat` | Minor | New features, new filters, new command support |
| `fix` | Patch | Bug fixes, corrections |
| `perf` | Patch | Performance improvements |
| `refactor` | — | Code restructuring (no changelog entry) |
| `docs` | — | Documentation only |
| `chore` | — | Maintenance, CI, deps |
| `feat!` / `fix!` | Major | Breaking changes (add `!` after type) |

**Scope** should match the module or area: `git`, `cargo`, `gh`, `hook`, `tracking`, `cicd`, etc.

### Examples

```
feat(kubectl): add pod log filtering
fix(git): preserve merge commit messages in log filter
perf(cargo): lazy-compile clippy regex patterns
feat!(hook): change rewrite config format
```

These commit messages directly become CHANGELOG entries when release-please creates a release PR. Write them as if they will be read by users.

---

## Branch Naming Convention

Git branch names cannot include spaces or colons, so we use slash-prefixed names. Pick the prefix that matches your change type and follow it with an optional scope and a short, kebab-case description.

| Prefix | When to Use |
|--------|-------------|
| `fix/` | Bug fixes, corrections, minor adjustments |
| `feat/` | New features, new filters, new command support |
| `chore/` | CI/CD, deps, maintenance, breaking changes |

Combine the prefix with a scope if it adds clarity (e.g. `git`, `kubectl`, `filter`, `tracking`, `config`) and finish with a descriptive slug: `fix/<scope>-<description>` or `feat/<description>`.

Examples:
```
fix/git-log-filter-drops-merge-commits
feat/kubectl-add-pod-list-filter
chore/release-pipeline-cleanup
```

---

## Pull Request Process

### Scope Rules

**Each PR must focus on a single feature, fix, or change.** The diff must stay in-scope with the description written by the author in the PR title and body. Out-of-scope changes (unrelated refactors, drive-by fixes, formatting of untouched files) must go in a separate PR.

**For large features or refactors**, prefer multi-part PRs over one enormous PR. Split the work into logical, reviewable chunks that can each be merged independently. Examples:
- feat(Part 1): Add data model and tests
- feat(Part 2): Add CLI command and integration
- feat(Part 3): Update documentation

**Why**: Small, focused PRs are easier to review, safer to merge, and faster to ship. Large PRs slow down review, hide bugs, and increase merge conflict risk.


### 1. Create Your Branch

```bash
git checkout develop
git pull origin develop
git checkout -b feat/scope-your-clear-description
```

### 2. Make Your Changes

**Respect the existing folder structure.** Place new files where similar files already live. Do not reorganize without prior discussion.

**Keep functions short and focused.** Each function should do one thing. If it needs a comment to explain what it does, it's probably too long -- split it.

**No obvious comments.** Don't comment what the code already says. Comments should explain *why*, never *what* to avoid noise.

**Large command files are expected.** Command modules (`*_cmd.rs`) contain the implementation, tests, and fixture in the same file. A big file is fine when it's self-contained for one command. This will be moved in the future.

### 3. Add Tests

Every change **must** include tests. See [Testing](#testing) below.

### 4. Add Documentation

Documentation updates are required for new filters, new features, and changes that affect already-documented behavior. Bug fixes and refactors typically don't need doc updates. See [Documentation](#documentation) below.

### Contributor License Agreement (CLA)

All contributions require signing our [Contributor License Agreement (CLA)](CLA.md) before being merged.

By signing, you certify that:
- You have authored 100% of the contribution, or have the necessary rights to submit it.
- You grant **rtk-ai** and **rtk-ai Labs** a perpetual, worldwide, royalty-free license to use your contribution — including in commercial products such as **rtk Pro** — under the [Apache License 2.0](LICENSE).
- If your employer has rights over your work, you have obtained their permission.

**This is automatic.** When you open a Pull Request, [CLA Assistant](https://cla-assistant.io) will post a comment asking you to sign. Click the link in that comment to sign with your GitHub account. You only need to sign once.

### 5. Merge into `develop`

Once your work is ready, open a Pull Request targeting the **`develop`** branch.

### 6. Review Process

1. **Maintainer review** -- A maintainer reviews your code for quality and alignment with the project
2. **CI/CD checks** -- Automated tests and linting must pass
3. **Resolution** -- Address any feedback from review or CI failures

### 7. Integration & Release

Once merged, your changes are tested on the `develop` branch alongside other features. When the maintainer is satisfied with the state of `develop`, they release to `master` under a specific version.

```
your branch --> develop (review + CI + integration testing) --> version branch --> master (versioned release)
```

---

## Testing

Every change **must** include tests. We follow **TDD (Red-Green-Refactor)**: write a failing test first, implement the minimum to pass, then refactor.

For how to write tests (fixtures, snapshots, token savings verification), see [docs/contributing/TECHNICAL.md — Testing](docs/contributing/TECHNICAL.md#testing).

### Test Types

| Type | Where | Run With |
|------|-------|----------|
| **Unit tests** | `#[cfg(test)] mod tests` in each module | `cargo test` |
| **Snapshot tests** | `assert_snapshot!()` via `insta` crate | `cargo test` + `cargo insta review` |
| **Smoke tests** | `scripts/test-all.sh` (69 assertions) | `bash scripts/test-all.sh` |
| **Integration tests** | `#[ignore]` tests requiring installed binary | `cargo test --ignored` |

### Pre-Commit Gate (mandatory)

All three must pass before any PR:

```bash
cargo fmt --all --check && cargo clippy --all-targets && cargo test
```

### PR Testing Checklist

- [ ] Unit tests added/updated for changed code
- [ ] Snapshot tests reviewed (`cargo insta review`)
- [ ] Token savings >=60% verified
- [ ] Edge cases covered
- [ ] `cargo fmt --all --check && cargo clippy --all-targets && cargo test` passes
- [ ] Manual test: run `rtk <cmd>` and inspect output

---

## Documentation

Documentation updates are required for new filters, new features, and changes that affect already-documented behavior. Use this table to find which docs to update:

| What you changed | Update these docs |
|------------------|-------------------|
| New Rust filter (`src/cmds/`) | Ecosystem `README.md` (e.g., `src/cmds/git/README.md`), [README.md](README.md) command list |
| New TOML filter (`src/filters/`) | [src/filters/README.md](src/filters/README.md) if naming conventions change, [README.md](README.md) command list |
| New rewrite pattern | `src/discover/rules.rs` — see [Adding a New Command Filter](src/cmds/README.md#adding-a-new-command-filter) |
| Core infrastructure (`src/core/`) | [src/core/README.md](src/core/README.md), [docs/contributing/TECHNICAL.md](docs/contributing/TECHNICAL.md) if flow changes |
| Hook system (`src/hooks/`) | [src/hooks/README.md](src/hooks/README.md), [hooks/README.md](hooks/README.md) for agent-facing docs |
| Architecture or design change | [ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md), [docs/contributing/TECHNICAL.md](docs/contributing/TECHNICAL.md) |

> **Note**: Do NOT edit `CHANGELOG.md` manually — it is auto-generated by [release-please](https://github.com/googleapis/release-please) from your commit messages. See [Commit Messages & Changelog](#commit-messages--changelog).

**Navigation**: [CONTRIBUTING.md](CONTRIBUTING.md) (you are here) → [docs/contributing/TECHNICAL.md](docs/contributing/TECHNICAL.md) (architecture + flow) → each folder's `README.md` (implementation details).

Keep documentation concise and practical -- examples over explanations.

---

## Questions?

- **Bug reports & features**: [Issues](../../issues)
- **Discussions**: [GitHub Discussions](../../discussions)

**For external contributors**: Your PR will undergo automated security review (see [SECURITY.md](SECURITY.md)). 
This protects RTK's shell execution capabilities against injection attacks and supply chain vulnerabilities.

---

**Thank you for contributing to rtk!**
