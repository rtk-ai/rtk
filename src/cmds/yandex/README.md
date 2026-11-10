# Yandex / Arcadia ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

Filters for Arcadia meta-tools: **envelope + inner-runner** for `ya`, compact VCS filters for `arc`.

**Design docs:** [ya-roadmap.md](../../../ya-roadmap.md) (stages 0–10) · [ya-analitycs.md](../../../ya-analitycs.md) (token ROI) · [ACCEPTANCE.md](../../../tests/fixtures/ya/ACCEPTANCE.md) (golden criteria)

| Module | Role |
|--------|------|
| [`ya_cmd.rs`](ya_cmd.rs) | Classify argv → test / build / passthrough; `run_streamed` |
| [`ya_build.rs`](ya_build.rs) | Build-mode progress collapse (`YaBuildStreamFilter`) |
| [`envelope.rs`](envelope.rs) | Test-mode dispatch + live `YaTestStreamFilter` |
| [`framing.rs`](framing.rs) | Suite / chunk / totals keep rules |
| [`detect.rs`](detect.rs) | Fingerprint inner runner from output text |
| [`adapters/`](adapters/) | `py3test`, `go_test`, `generic_fail` |
| [`arc_cmd.rs`](arc_cmd.rs) | `arc status\|log\|diff\|show` (+ other passthrough) |

```
rtk ya make -t … / rtk ya test …
        │
        ▼
┌─ YaTestStreamFilter (live) ─┐
│ fat Expected / Ok / [PB] drop │
│ [fail] → S0-T5 compact live   │
│ no-fail → on_exit envelope    │
└────────────┬─────────────────┘
             ▼
   framing + Log: / Logsdir:
   (overflow → fail_overflow_tee_hint)
             │
             ▼  runner with_tee("ya")
        full raw recovery
```

**Stream vs envelope (important):**

| Path | When | Behavior |
|------|------|----------|
| **Live fail stream** (production) | Suites that emit `[fail]` | **Generic S0-T5 compact only** (`compact_fail_block`) as blocks close — **does not** call py3test/go adapters |
| **No-fail `on_exit`** (production) | Suites with no `[fail]` | [`filter_ya_envelope`](envelope.rs) → may dispatch py3test / go adapters |
| **Buffered oracle** (unit tests) | `filter_ya_envelope` / adapter unit tests | Full adapter pipeline (py3test → pytest reuse; go_test → ya-framed compact) |

In short: **live failure streaming = generic compact**; **language adapters run on the no-fail / oracle path**. Adapter ROI still covers goldens via `filter_ya_envelope` asserts; production fail UX prioritizes low-latency streaming + tee recovery.

## Demo & presentation (token savings)

Contributor design docs above are complete for v1. For **slides / live demos**, use fixtures
(always works) or a real Arcadia tree (`ya` / `arc` on PATH).

### Offline fixture demo (recommended for presentations)

```bash
cargo build
chmod +x scripts/demo-ya-savings.sh
./scripts/demo-ya-savings.sh
```

Or one-liners (`chars/4` ≈ tokens, same metric as analytics):

```bash
# Before/after sizes
RAW=tests/fixtures/ya/make_t_py_fail_logsdir_raw.txt
wc -c "$RAW"
cargo run -q -- pipe -f ya <"$RAW" | wc -c

# Show what the agent would see
cargo run -q -- pipe -f ya <"$RAW" | head -50

# Build spam
cargo run -q -- pipe -f ya-build <tests/fixtures/ya/make_python_py_build_large_proto_raw.txt | head -40

# arc
cargo run -q -- pipe -f arc-status <tests/fixtures/arc/status_raw.txt
cargo run -q -- pipe -f arc-log <tests/fixtures/arc/log_raw.txt
```

Pipe filters: `ya` / `ya-test` · `ya-build` · `arc-status` · `arc-log` · `arc-diff` · `arc-show`.

### Live against real `ya` / `arc`

```bash
cargo install --path .   # or: cargo build --release && export PATH="$PWD/target/release:$PATH"

# Side-by-side (pick a small failing target you already know)
ya make -t path/to/tests 2>&1 | tee /tmp/ya-raw.txt | wc -c
rtk ya make -t path/to/tests 2>&1 | tee /tmp/ya-rtk.txt | wc -c
# Inspect keep invariants
grep -E '\[fail\]|Logsdir:' /tmp/ya-rtk.txt | head

# Build-only
rtk ya make python -r path/to/pkg

# arc
rtk arc status
rtk arc log -n 20
rtk arc diff

# After a few commands — presentation slide for tracked savings
rtk gain
rtk gain -H
rtk gain -g
```

**Hooks (agents auto-rewrite):** `rtk init` / install hooks, then bare `ya make -t …` becomes `rtk ya make -t …`. Confirm with `rtk gain -H`.

**Bypass:** `rtk proxy ya make -t …` — full raw output, still tracked (0% savings).

## Modes

| Invoked as | Pipeline |
|------------|----------|
| `rtk ya test …` | test stream (`YaTestStreamFilter`) |
| `rtk ya make -t` / `-tt` / `-ttX` / `--test …` | test stream |
| `rtk ya make …` (no test flags) | build stream (`YaBuildStreamFilter`) |
| `rtk ya tool …` | passthrough + track |
| other `rtk ya …` | passthrough + track |

### Flag passthrough

User filters and verbosity are **never rewritten** — args reach bare `ya` byte-for-byte:

```bash
rtk ya test -F '*order*' -r path/to/tests
rtk ya make -ttX -F '*sku*' path
rtk ya make python -r
```

### Compact policy (S0-T5)

Per `[fail]` keep: headline (node id) · first location · first error head · `Log:` · `Logsdir:` (dedupe OK).

Drop: Expected/but bodies · long stacks · PEERDIR / `[PB]` / `Ok [n/m]` progress when compressing.

### Truncation recovery

- **Full raw:** runner `RunOptions::with_tee("ya")` only — filters do **not** call `force_tee_hint` on full output (avoids double tee).
- **Capped fail lists** (`CAP_ERRORS`): `fail_overflow_tee_hint` — `force_tee_tail_hint` on the **full** headline list with offset `shown + 1`.

For `rtk ya test` / `rtk ya make -t*`, saved output also includes a short index
of `[fail]` headlines and their **1-based physical lines in that saved file**.
`rtk pipe -f ya` / `ya-test` saves stdin and provides the same index when failures
are present. On Unix, a quoted `sed` command previews 20 lines at the first failure;
use another listed line number to inspect a different failure.

Locations follow the saved file's order: stdin verbatim in pipe mode, stdout
followed by stderr in live mode. They are not offsets in the compact summary
or in the interleaved live display. Only complete lines retained by the tee cap
are indexed; a partial log is identified explicitly. The index uses
`MAX_FAIL_BLOCKS` and reports the first remaining saved failure when capped.
`RTK_TEE=0` disables the saved log and its locations.
The recovery hints respect the output budget: a short live result may show only
the saved-log path; pipe mode retains the usual `never_worse` fallback.

## Adapters

| Fingerprint | Adapter | Notes |
|-------------|---------|--------|
| `py3test` / `test-results/py3test` | `py3test` | **Oracle / no-fail `on_exit` only** (`filter_ya_envelope`): synthetic FAILURES → `filter_pytest_output`. Live fail stream = generic S0-T5 compact. |
| `<go_test>` / `/gotest/` | `go_test` | **Oracle / no-fail `on_exit` only**: ya-framed compact — **not** `go test -json`. Live fail stream = generic S0-T5 compact. |
| vitest / jest | detect only | Fingerprint exists in [`detect.rs`](detect.rs); **no adapter / no fixtures** (follow-up F3). Detected → `generic_fail` / same live compact. |
| unknown (test) | `generic_fail` | Same S0-T5 rules as live fail stream |
| build `ya make` | `ya_build` | Allowlist errors/warnings; collapse progress (`run_streamed`) |

## `arc`

| Invoked as | Pipeline |
|------------|----------|
| `rtk arc status …` | compact like `git status` |
| `rtk arc log …` | oneline bias; default `-n 10` if unset |
| `rtk arc diff …` | normalize → `compact_diff` + tighten |
| `rtk arc show …` | compact header + tightened patch |
| `rtk arc <other> …` | passthrough + track |

```bash
rtk arc status
rtk arc log -n 20
rtk arc diff
rtk arc show HEAD
rtk arc info   # passthrough
```

## Hooks & discover

With RTK hooks installed:

| Raw command | Rewritten to |
|-------------|----------------|
| `ya make …` | `rtk ya make …` |
| `ya test …` | `rtk ya test …` |
| `arc status\|log\|diff\|show …` | `rtk arc …` |
| `ya tool …` | **not rewritten** |
| other `arc …` | **not rewritten** |

**Hook scope (Q4):** always rewrite when hooks are active — same as git/cargo. No PATH or arc-mount gate.

**Known gap — absolute binary paths:** `/usr/local/bin/ya make` classifies as supported but is not rewritten (same gap as `/usr/bin/git status`). Prefer bare `ya` / `arc` on `PATH`, or call `rtk ya` / `rtk arc` explicitly.

Preferred note for Arcadia `AGENTS.md`:

```markdown
## RTK (token-optimized CLI)

Prefer `rtk ya make -t …` / `rtk ya test …` and `rtk arc status|log|diff|show`
over bare `ya` / `arc` when RTK is installed. Do not wrap arbitrary `ya tool *`
unless a dedicated filter exists.
```

Rules: [`src/discover/rules.rs`](../../discover/rules.rs).

## Status

**v1 complete (stages 0–10).** Test + build `ya` streamed; `arc` filtered subset; hooks rewrite safe set; Logsdir / tee / cross-platform / fuzz hardened.

## Non-goals (v1)

- Rewriting RECIPE / parsing all Makefile dialects
- Injecting pytest flags through `ya`
- Blanket `ya tool *` hook rewrite
- JS/vitest under `ya` without fixtures

## Follow-ups (post-v1)

| Item | Notes |
|------|--------|
| Allowlisted `ya tool <name>` | New mini-stage per tool when ROI + fixtures exist; keep wildcard out of hooks |
| Absolute-path rewrite | Platform-wide discover normalization (`/usr/bin/git`, `/usr/local/bin/ya`, …) |
| JS under `ya` | Collect fixtures → `jest_vitest` adapter |
| Dual `ya test` corpus | Only if real dumps diverge from `ya make -t` |

## Related

- Fixtures: [`tests/fixtures/ya/`](../../../tests/fixtures/ya/), [`tests/fixtures/arc/`](../../../tests/fixtures/arc/)
- Acceptance: [`tests/fixtures/ya/ACCEPTANCE.md`](../../../tests/fixtures/ya/ACCEPTANCE.md)
- Roadmap: [`ya-roadmap.md`](../../../ya-roadmap.md)
- Analytics: [`ya-analitycs.md`](../../../ya-analitycs.md)
