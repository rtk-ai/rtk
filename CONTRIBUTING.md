# Contributing to rtk

**Welcome!** We appreciate your interest in contributing to rtk.

## Quick Links

- [Report an Issue](../../issues/new)
- [Open Pull Requests](../../pulls)
- [Start a Discussion](../../discussions)

---

## What is rtk?

**rtk (Rust Token Killer)** is a coding agent proxy that cuts noise from command outputs. It filters and compresses CLI output before it reaches your LLM context, saving 60-90% of tokens on common operations. The vision is to make AI-assisted development faster and cheaper by eliminating unnecessary token consumption.

---

## Ways to Contribute

| Type | Examples |
|------|----------|
| **Report** | File a clear issue with steps to reproduce, expected vs actual behavior |
| **Fix** | Bug fixes, broken filter repairs |
| **Build** | New filters, new command support, performance improvements |
| **Review** | Review open PRs, test changes locally, leave constructive feedback |
| **Document** | Improve docs, add usage examples, clarify existing docs |
---

## Branch Naming Convention

Every branch **must** follow one of these prefixes to identify the level of change:

| Prefix | Semver Impact | When to Use |
|--------|---------------|-------------|
| `fix(scope): ...` | Patch | Bug fixes, corrections, minor adjustments |
| `feat(scope): ...` | Minor | New features, new filters, new command support |
| `chore(scope): ...` | Major | Breaking changes, API changes, removed functionality |

The **scope** in parentheses indicates which part of the project is concerned (e.g. `git`, `kubectl`, `filter`, `tracking`, `config`).

**Branch title must clearly describe what is affected and the goal.**

Examples:
```
fix(git): log-filter-drops-merge-commits
feat(kubectl): add-pod-list-filter
chore(proxy): remove-deprecated-flags
```

---

## Pull Request Process

### Scope Rules

**Each PR must focus on a single feature, fix, or change.** The diff must stay in-scope with the description written by the author in the PR title and body. Out-of-scope changes (unrelated refactors, drive-by fixes, formatting of untouched files) must go in a separate PR.

**For large features or refactors**, prefer multi-part PRs over one enormous PR. Split the work into logical, reviewable chunks that can each be merged independently. Examples:
- Part 1: Add data model and tests
- Part 2: Add CLI command and integration
- Part 3: Update documentation and CHANGELOG

**Why**: Small, focused PRs are easier to review, safer to merge, and faster to ship. Large PRs slow down review, hide bugs, and increase merge conflict risk.

### 1. Create Your Branch

```bash
git checkout develop
git pull origin develop
git checkout -b "feat(scope): your-clear-description"
```

### 2. Make Your Changes

**Respect the existing folder structure.** Place new files where similar files already live. Do not reorganize without prior discussion.

**Keep functions short and focused.** Each function should do one thing. If it needs a comment to explain what it does, it's probably too long -- split it.

**No obvious comments.** Don't comment what the code already says. Comments should explain *why*, never *what* to avoid noise.

**Large command files are expected.** Command modules (`*_cmd.rs`) contain the implementation, tests, and fixture in the same file. A big file is fine when it's self-contained for one command.

### 3. Add Tests

Every change **must** include tests. See [Testing](#testing) below.

### 4. Add Documentation

Every change **must** include documentation updates. See [Documentation](#documentation) below.

### Developer Certificate of Origin (DCO)

All contributions must be signed off (git commit -s) to certify
you have the right to submit the code under the project's license.

Expected format: Signed-off-by: Your Name your@email.com
https://developercertificate.org/

By signing off, you agree to the DCO.

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

### Test Types

| Type | Where | Run With |
|------|-------|----------|
| **Unit tests** | `#[cfg(test)] mod tests` in each module | `cargo test` |
| **Snapshot tests** | `assert_snapshot!()` via `insta` crate | `cargo test` + `cargo insta review` |
| **Smoke tests** | `scripts/test-all.sh` (69 assertions) | `bash scripts/test-all.sh` |
| **Integration tests** | `#[ignore]` tests requiring installed binary | `cargo test --ignored` |

### How to Write Tests

Tests for new commands live **in the module file itself** inside a `#[cfg(test)] mod tests` block (e.g. tests for `src/kubectl_cmd.rs` go at the bottom of that same file).

**1. Create a fixture from real command output** (not synthetic data):
```bash
kubectl get pods > tests/fixtures/kubectl_pods_raw.txt
```

**2. Write your test in the same module file** (`#[cfg(test)] mod tests`):
```rust
#[test]
fn test_my_filter() {
    let input = include_str!("../tests/fixtures/my_cmd_raw.txt");
    let output = filter_my_cmd(input);
    assert_snapshot!(output);
}
```

**3. Verify token savings**:
```rust
#[test]
fn test_my_filter_savings() {
    let input = include_str!("../tests/fixtures/my_cmd_raw.txt");
    let output = filter_my_cmd(input);
    let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
    assert!(savings >= 60.0, "Expected >=60% savings, got {:.1}%", savings);
}
```

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

Every change **must** include documentation updates. Update the relevant file(s) depending on what you changed:

| What you changed | Update |
|------------------|--------|
| New command or filter | [README.md](README.md) (command list + examples) and [CHANGELOG.md](CHANGELOG.md) |
| Architecture or internal design | [ARCHITECTURE.md](ARCHITECTURE.md) |
| Installation or setup | [INSTALL.md](INSTALL.md) |
| Bug fix or breaking change | [CHANGELOG.md](CHANGELOG.md) |
| Tracking / analytics | [docs/tracking.md](docs/tracking.md) |

Keep documentation concise and practical -- examples over explanations.

---

## Questions?

- **Bug reports & features**: [Issues](../../issues)
- **Discussions**: [GitHub Discussions](../../discussions)

**For external contributors**: Your PR will undergo automated security review (see [SECURITY.md](SECURITY.md)). 
This protects RTK's shell execution capabilities against injection attacks and supply chain vulnerabilities.

---

**Thank you for contributing to rtk!**
