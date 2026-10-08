# FIX_PERF — why rtk costs tokens instead of saving them

Investigation date: 2026-07-20
Evidence: `benchmarking/benchmark-sessions/results/20260713-133110` (rtk `pr:2781`,
claude-opus-4-8, 10 paired ON/OFF VMs, task `rust-bottom-global-audit`)

---

## 1. Verdict

On our own benchmark, on the task most favourable to rtk (every prescribed command is
rtk-covered, read-only, fixed trajectory):

| Metric | OFF | ON | savings* |
|---|---|---|---|
| **Cost USD** | 0.4604 | 0.4614 | **−0.2%** |
| New input (input + cache_creation) | 22,098 | 21,322 | +3.5% |
| Cache read | 281,580 | 275,317 | +2.2% |
| API turns (`total_turns`) | 9.8 | 9.9 | −1% |
| Bash commands (`total_tool_invocations`) | 17.4 | 20.8 | **−19.5%** |
| Errors (total across 10 VMs) | 0 | 5 | — |
| Hook coverage (`is_rtk_prefixed`) | — | 57% | expected ~100% |

\* `_savings_pct(on, off) = (1 - on/off) * 100`; positive = ON cheaper = good for rtk.

**Compression works. It does not reach the invoice.** The +3.5% new-input saving is
consumed by ~3.4 extra commands per session. Net cost is a wash.

The +3.5% figure independently reproduces the ~3% ceiling that an external evaluation
derived by replaying agent transcripts. Two different methods, same number.

**Reading `bash_result_bytes` as the headline is the error.** It measures bytes out of
the filter, not dollars off the invoice — the same counterfactual substitution that makes
`rtk gain` report savings while cost rises.

---

## 2. Root cause

> rtk emits **human-readable** output where the wrapped command emits **machine-readable**
> output, and the hook rewrites commands that sit inside a pipeline.

Agents pipe constantly. In this run:

- **73%** of ON commands (152/208) use a pipe or chain
- **55%** of rewritten commands (65/119) sit inside a pipe

### 2.1 Confirmed hard failure — 100% incidence

`rtk find src -name "*.rs" | xargs wc -l` ran on **10/10 ON VMs and failed on all 10**:

```
wc: 166F: No such file or directory
wc: '39D:': No such file or directory
wc: ./: Is a directory
wc: app.rs: No such file or directory
```

`find` emits newline-separated paths. `rtk find` emits a listing with size annotations
(`166F`, `39D:`) and basenames stripped of their directories. Nothing downstream can
consume it. on-9 exited 123.

The agent recovers by re-running raw — this is the source of the extra commands:

```
[on-2] rtk find src -name "*.rs" | xargs wc -l | tail -5    → garbage
[on-2] find src -name "*.rs" | xargs wc -l | sort -rn       → 37604 total ✓

[on-9] t1 RTK ERR  rtk find src -name "*.rs" | xargs wc -l   ← fails
[on-9] t2          find src -name "*.rs" | wc -l; ...        ← raw retry
[on-9] t2          find src -name "*.rs" | xargs wc -l | tail -1  ← raw retry
```

### 2.1.1 Blast radius is narrow and specific

Counting tool occurrences across full command text (not first-token classification):

| tool | OFF | ON | ratio |
|---|---|---|---|
| **find** | 34 | 49 | **1.44x** |
| **wc** | 76 | 96 | **1.26x** |
| grep | 116 | 123 | 1.06x |
| git log / git status / tree / ls | — | — | **1.00x** |
| cargo clippy | 28 | 25 | 0.89x |
| cargo test | 21 | 14 | 0.67x |

`find` and `wc` are the only tools that move — precisely the two in the broken pipeline.
git/tree/ls are identical to the command; cargo is *lower* in ON. **There is no diffuse
"filtering makes the agent flail" effect. There is one broken pipe and its blast radius.**

### 2.1.2 Refuted: trust erosion

An earlier reading of this data claimed the agent learns to route around rtk. The
rtk-prefix rate does collapse from 100% before the first failure to ~47% after, in all 10
VMs. **The control refutes it:**

| | turns 1-3 | turns 4+ |
|---|---|---|
| ON | 63% piped | 74% piped |
| OFF | 59% piped | **93% piped** |

OFF is *more* pipeline-heavy in later turns than ON. Later commands are inherently piped
for both arms, and the hook declines to rewrite chained commands. The coverage drop is
**task structure, not agent avoidance.**

Also refuted: an earlier first-token classification appeared to show `grep` +18 in ON. That
was an artifact — OFF prefixes commands with `echo "=== TODO ==="; grep ...`, which
classified as `echo`. Corrected, grep is 1.06x. Flat.

### 2.2 Confirmed silent corruption — worse than the failure

```
[on-5] cargo clippy --all-targets 2>&1 | tee /tmp/clippy.log | tail -5; \
       grep -c "^warning" /tmp/clippy.log
  → cargo clippy: No issues found
  → ===WARN COUNT=== 0
```

The tee'd log holds rtk's filtered summary, so `grep -c "^warning"` counts zero. The agent
reports **0 clippy warnings** with no error and no retry. Same pattern on on-7.

This is a correctness bug, not a token bug, and it justifies the fix on its own.

### 2.3 The guard exists and does not fire

`src/discover/registry.rs:690-700` already exempts `find`/`fd` when they precede a pipe:

```rust
TokenKind::Pipe => {
    let seg = cmd[seg_start..tok.offset].trim();
    let is_pipe_incompatible = seg.starts_with("find ") || seg == "find"
        || seg.starts_with("fd ") || seg == "fd";
```

Added in v0.31.0 (#666), so it was present in the `pr:2781` build. Yet all 10 VMs show
`is_rtk_prefixed=True` on `rtk find ... | sort`. `src/discover/rules.rs:118-124` performs
an unconditional `^find\s+` → `rtk find` regex rewrite. **Two rewrite paths; the live hook
appears to take the one without the guard.** Tracing that dispatch is the first task.

---

## 3. Why `isatty` is NOT the fix

Initial recommendation was to gate filtering on `std::io::stdout().is_terminal()`, as
`src/cmds/system/ls.rs:102` and `src/cmds/cloud/curl_cmd.rs:76` already do. **This does not
work under an agent harness.**

Claude Code's Bash tool captures stdout. From rtk's perspective:

| Invocation | stdout | `is_terminal()` |
|---|---|---|
| `rtk find src` (agent reads output) | pipe to harness | **false** |
| `rtk find src \| xargs wc -l` (program reads output) | pipe to xargs | **false** |

Identical. `isatty` cannot distinguish "consumed by the LLM" from "consumed by another
program" — both are pipes. Gating passthrough on it would make rtk a **no-op under Claude
Code**, destroying the filtering entirely while fixing nothing.

Corollary worth noting: `ls.rs:102` only appends its summary line when `is_tty`, so under
Claude Code **that summary is dead code** — the agent never sees it.

`isatty` is still correct for interactive human use (`$ rtk find src | xargs wc -l` at a
real terminal does have a non-TTY stdout). It is simply blind in the deployment that matters.

**Only the hook has the syntactic context needed to make this decision.** It sees the full
command line before execution and can tell whether the rtk-rewritten segment feeds another
program.

---

## 3bis. The theoretical fix

### The invariant being violated

Every command has an **output contract**: `find` emits newline-separated paths, `wc -l`
emits counts, `git status --porcelain` emits a stable machine format. rtk substitutes a
lossy human-readable summary for that contract.

That substitution is sound only when the consumer tolerates format changes — i.e. when the
consumer is the language model. It is catastrophic when the consumer is another program.

> **Invariant: a filter may only be applied when rtk's stdout is terminal — when nothing
> but the agent reads it.**

rtk currently has no representation of this invariant anywhere in its design.

### Where consumer identity is knowable

| Layer | Can it tell who consumes the output? |
|---|---|
| Filter function | No — sees only bytes |
| rtk binary at runtime | No — stdout is a pipe in both cases under an agent harness |
| **Hook, pre-execution** | **Yes — the full command line is visible as text** |

Consumer identity is a *syntactic* property of the command line and is destroyed the moment
the shell forks. It must be decided in the hook.

### The decision rule

Per pipeline segment, given `rtk`-eligible command `C`:

| Shell form | Consumer of C's stdout | Rewrite? |
|---|---|---|
| `C` | agent | **yes** |
| `C \| prog` | prog | **no** |
| `C > file` / `C \| tee f` | file (unknown reader) | **no** |
| `prog \| C` | agent (C is last) | **yes** |
| `C1 ; C2` | agent, both | **yes, both** |
| `C1 && C2` | agent, both | **yes, both** |

The key structural point: **`;` and `&&` are safe; `|` and `>` are not.** Each
semicolon-separated segment writes independently to the agent. A pipe redirects one
segment's stdout into another program.

Current behaviour is wrong in *both* directions — it declines on `;`/`&&` (safe, costs
coverage: 57% vs ~100%) and rewrites inside `|` (unsafe, causes the bug). So this is not a
safety-vs-coverage tradeoff: **the correct rule is simultaneously safer and higher-coverage
than what ships today.**

### Two strategies, both needed

**A. Contract preservation** — reduce volume, preserve shape. `rtk find` should emit
newline-separated paths, just fewer of them, with an elision marker on stderr rather than
stdout. Then piping is safe by construction and needs no gating.

Applies to line-oriented filters: `find`, `ls`, `grep`, `wc`, `du`, `tree`. These are also
the filters with the *least* to gain from reformatting — a file listing is already terse —
so the cost of the constraint is near zero.

**B. Consumer gating** — for filters whose entire value *is* reformatting (`cargo test`,
`git log`, `cargo clippy`), the contract cannot be preserved. These must be hook-gated per
the decision rule above.

Strategy A is strictly stronger where it applies: it removes the failure mode rather than
detecting it. A filter that preserves its contract cannot break a pipeline no matter where
it appears.

### Why this is the whole problem

rtk's founding bet is that a lossy human summary can be substituted for a machine data
stream, because the consumer is a model that prefers prose. That bet holds only when the
model is the *terminal* consumer.

In this benchmark, **73% of commands use a pipe or chain** — agents use the shell as a
computation engine (counting, sorting, filtering), not merely as a way to produce text to
read. rtk cannot see downstream, so it applies a human-facing transformation to data that
is on its way to a program. Everything in §2 follows from that single blind spot.

---

## 4. Fixes, in order

1. **Trace and unify the rewrite dispatch.** Determine why `rules.rs` regex rewriting wins
   over the `registry.rs` pipe-aware path. Make the pipe guard authoritative.
2. **Generalise the guard beyond `find`/`fd`.** Any rewritten command whose stdout feeds
   another program must not be filtered. The current allowlist is two commands; the failure
   class covers every filter with a bespoke output format.
3. **Fail open, never closed.** `find_cmd.rs:88` `bail!`s on 23 flags (`-o -exec -size
   -mtime -not …`) instead of exec'ing real `find`; `main.rs:1494-1500` turns any escaped
   `Err` into exit 1 with no fallback layer. `pipe_cmd.rs:258` bails on >10 MiB stdin and
   **discards it**.
4. **Fix `rtk gain`'s counterfactual.** It credits the full raw byte count as "saved" even
   when the harness would have truncated the output anyway. Cap the counterfactual at what
   the agent would actually have received.
5. **`pnpm outdated` masks failure.** `pnpm_cmd.rs:434-470` — `run_outdated` lacks the
   `!result.success()` guard its two siblings have, so a failed run yields empty stdout →
   parse passthrough → literal `"All packages up-to-date"`, exit 0.
6. **`mvn` warning dedup drops cross-file instances.** `mvn_cmd.rs:670-679` strips the file
   coordinate *before* computing the dedup key, collapsing the same warning across N files
   to one line with no `… +N more`. Recovery is dead here too: `RunOptions::with_tee` is
   mode-gated to `TeeMode::Failures`, so a successful build writes no tee file.

---

## 5. Benchmark methodology notes

The harness itself is sound — paired ON/OFF VMs, real `cost_usd`, and `WARNING_BENCH.md`
already anticipates retry spirals (#6) and over-filtering (#7). Two adjustments:

- **Report `cost_usd` as the headline, not `bash_result_bytes`.** Bytes saved is an input
  to the question, not the answer.
- **`WARNING_BENCH.md` #2 vs #3 is a real tension.** Prescribing exact commands (the fix
  for #2, behavioural divergence) removes the degree of freedom where trajectory-divergence
  cost lives. All 10 tasks currently use "MUST exercise the following". Worth adding one
  unconstrained task to measure what the prescribed ones cannot.
- The read-only constraint makes edit→re-run→re-edit thrash loops structurally impossible,
  so the `mvn`-class silent-loss failure cannot appear in any current task.

---

## 6. Open questions

- Which code path does the live hook actually use for rewriting? (blocks fix #1)
- Does the turn penalty persist at low reasoning effort? This run is opus-4-8; the external
  evaluation found the penalty at low effort and null at high effort.
- What is the pipe rate in real agent sessions vs this benchmark's 73%? The task's mandatory
  command list inflates it; the true rate determines the size of the win from fix #2.
