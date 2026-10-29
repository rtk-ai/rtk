# Contributing to the RTK engine and proxy

**Status: issues for `rtk-engine` and `rtk-proxy` are open. Pull requests open on 18 October 2026.**

Pull requests against `engine/` or `proxy/` opened before 18 October 2026 will be closed with a link to this page — not because they are unwelcome, but because the code underneath them is still moving.

The rest of RTK — filters, commands, hooks — is unaffected. It keeps following the root [`CONTRIBUTING.md`](../CONTRIBUTING.md) as before.

## What you can do now

- Read the design: [`README.md`](README.md) (the grammar, normative). The Rust contracts and reserved slots land here when pull requests open.
- Open an issue: a bug, or a **method proposal** — what it transforms, on which outputs, measured before/after tokens, and what the agent does when it needs the removed detail (the recovery path). A proposal with numbers is worth more than a PR without them.
- Prepare a method against the `Method` contract so it is ready on 18 October. The trait, `MethodInfo` and `Block` are stable; the checklist below will not change.
- Add a filter for a command RTK does not know — that path is open today and unchanged (`filters.toml`, root `CONTRIBUTING.md`).

## What we accept from 18 October

| You contribute | It must include |
|---|---|
| **A method** (`impl Method`) | a frozen name; a complete `MethodInfo` (`lossy`, `recovery`, `idempotent`, `cache_stable`, `cost`, `consumes`/`produces`); before/after fixtures on real outputs; measured savings **and** recovery rate; an idempotency test if lossless (`apply(apply(x)) == apply(x)`); fail-open on every path |
| **A chain or an option** on a built-in | the guide example that exercises it, validating |
| **A directive** | the text; its measured standing cost per turn; measured output reduction on real sessions |
| **A task kind** (`impl TaskKind`) | interface, prompt template, output handling; credit if it comes from a published method |
| **A harness profile** | evidence for every capability (a link to the harness's hook documentation or a recorded transcript); `TODO: VERIFY` on anything not proven — never a guess |
| **A reference pipeline** (`rtk:*`) | the literal file; a provenance section with numbers and source credit; every rule justified; a golden file for `rtk pipeline show` |
| **A proxy transform** | everything a method needs, plus: a pure function of its block (`cache_stable`), and `CacheHealth.hit_ratio` unchanged on the fixture sessions |
| **Docs** | grammar changes are guide changes in the same PR; the guide is normative |

## Benchmarking a pipeline

Every `.toml` in `engine/builtin/` opens with a header, and `rtk pipeline show
<name>` prints the file verbatim — so the header is what someone reads when they
are deciding whether a pipeline is worth adopting. A contributed pipeline
carries the same four parts:

```toml
# rtk:<name> — <one line: what it does>
#
# <two to five lines: what it changes, what it costs, what stays recoverable>
#
# Benchmark: <the headline number, the workload, and how many requests>
# Source:    <link to the run, or a path in this repo>
```

**"not measured" is an acceptable value.** Most of the built-ins say exactly
that, because the per-method fixtures measure methods and nobody has yet
measured the pipelines end to end. Writing it down is the point: an unmeasured
pipeline that admits it is honest, and the empty slot is an invitation. Inventing
a number to fill the slot is the one thing that will get a PR closed.

**Say which number you measured.** They are not interchangeable:

| lever | the number that matters |
|---|---|
| hook-site filtering | bash output bytes before/after, per command |
| `after_tool` compression | the same, plus how often the agent asks for the original back |
| directives | the agent's **output** tokens — a different budget, billed higher |
| `proxy.request` | cost against a live prompt cache, not tokens removed |

For that last row, fewer tokens is not the same as lower cost: an edit that
invalidates a cached prefix re-bills everything after it. Report cost, and say
what the cache hit rate was while you measured.

**State the shape of the run**: how many requests, against which model, with or
without a warm cache, and whether the comparison was paired. A single run on one
model is a direction, not a result, and should say so.

## Rules every PR must satisfy

1. **No TOML file ever contains a command to run.** New behaviour is Rust, composed by name in TOML.
2. **Nothing enters the agent's session** except transformations and configured directives. No diagnostics, warnings or hints in hook output.
3. **Fail open.** A load error, a panic, a timeout: the command runs and the content passes through unchanged.
4. **Hook budget.** `hook.before_tool` under 10 ms. Nothing printed from a hook path. No I/O in `engine/`.
5. **Agent-agnostic.** No harness or tool names in the engine or in any pipeline file; subjects are the generic categories (`shell`, `read`, `web`, …).
6. **Proxy methods are pure functions of their block.** The cache invariant is enforced; a turn-varying condition on a cached part is a load error.
7. **Existing semantics stay.** `toml_filter`, the built-in filters, `rewrite_command`, the proxy's `Transform`s are called as they are, never re-implemented.
8. **`engine/` imports nothing from `src/` or `proxy/`.**
9. **Numbers, or it did not happen.** Every saving claim ships with tokens before/after and the recovery rate on real outputs.

## Process

Same as the root `CONTRIBUTING.md`: Conventional Commits (`feat(engine): …`, `fix(proxy): …`), `feat/` / `fix/` / `chore/` branches, one change per PR, `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt` — and the same three against `--manifest-path proxy/Cargo.toml` for a proxy change, since `proxy/` is its own workspace root and `--workspace` does not reach it. Additionally for this area: an engine PR touches `engine/` only, unless it is a site adapter; a new method is registered in `engine/src/methods/mod.rs` and nowhere else; a grammar change comes with its guide section, its error or warning code, and a validation test.

## Not accepted

Harness-specific rules or methods; quota or licence gating of any kind; JSON lens, TOON session mirrors and edit translation; optical methods.

## Licensing

`rtk-engine` and `rtk-proxy` are both BUSL-1.1 (Business Source License). A contribution is licensed under BUSL-1.1. Whether a sign-off (DCO) is required will be announced when pull requests open.

## Until pull requests open

We will post the frozen API, the public fixture suite and the PR templates in this file and in the repository discussions before 18 October.
