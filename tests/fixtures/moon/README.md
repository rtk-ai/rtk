# Moon Filter Fixtures

Captured from a real moon 2.0.4 workspace (`yulii/ops-platform`) on 2026-05-28.
Ground truth for the `rtk moon` filter's snapshot tests (Tasks 3, 5, 6).

## Files

| File | Description |
|------|-------------|
| `run_typecheck_success.txt` | 44 lines — cold-cache `:typecheck` across 18 projects (parallel) |
| `run_cache_hit.txt` | 7 lines — single cached task (`audit:format`) |
| `run_test_failure.txt` | 32 lines — `audit:test` failure with `bun test` output |
| `run_summary_detailed.txt` | 50 lines — `MOON_SUMMARY=detailed` with `audit:format` (cached) |
| `run_tsc_failure.txt` | 19 lines — `audit:typecheck` failure with 4 TypeScript errors (parallel-mode body) |
| `query_tasks.json` | 5844 lines — full `moon query tasks` output for 16 projects |

---

## Exact byte format of moon chrome

### Chrome prefix

The plan expected `▮▮▮▮ ` (4 × U+25AE BLACK VERTICAL RECTANGLE + SPACE).
**Confirmed.** Exact bytes:

```
e2 96 ae  e2 96 ae  e2 96 ae  e2 96 ae  20
  ▮           ▮           ▮           ▮     SPACE
```

`xxd` evidence (first 30 bytes of `run_typecheck_success.txt`):
```
00000000: e296 aee2 96ae e296 aee2 96ae 2065 3265  ............ e2e
00000010: 3a74 7970 6563 6865 636b 2028 6264       :typecheck (bd
```

Regex to match chrome lines: `^▮{4} ` or equivalently `^\xe2\x96\xae{4}\x20` (UTF-8 bytes).

### Chrome line types

Moon emits three chrome line patterns:

1. **Task start** — `▮▮▮▮ <project>:<task> (<8-hex-hash>)`
   - Example: `▮▮▮▮ e2e:typecheck (bd958ad9)`
   - 8-character lowercase hex hash

2. **Task complete (cold-cache)** — `▮▮▮▮ <project>:<task> (<duration>, <8-hex-hash>)`
   - Duration format: `Xms` or `Xs Yms` (e.g. `404ms`, `3s 283ms`, `850ms`)
   - Example: `▮▮▮▮ e2e:typecheck (404ms, bd958ad9)`

3. **Task complete (cached)** — `▮▮▮▮ <project>:<task> (cached, <8-hex-hash>)`
   - Example: `▮▮▮▮ audit:format (cached, 7b3060c4)`
   - No duration in cached hits — just `cached` + hash

4. **Notification banners** — also prefixed with `▮▮▮▮ `, plain text
   - Example: `▮▮▮▮ There's a new version of moon available, 2.2.5 (currently on 2.0.4)!`
   - Distinguish from task lines: no `(...)` suffix, contains no colon+taskname pattern

### Hash format

8 lowercase hex characters: `[0-9a-f]{8}`.
- Cold-cache: `(<duration>, <hash>)` — parentheses, comma-separated
- Cache hit: `(cached, <hash>)` — parentheses, `cached` literal
- Task start: `(<hash>)` — parentheses only (no comma, no duration)

---

## Summary line format

### Single-task success with cache hit
```
Tasks: 1 completed (1 cached)
 Time: 43ms ❯❯❯❯ to the moon
```
The `❯❯❯❯ to the moon` footer **appears on single-task runs** when all tasks complete (cached or not).

### Multi-task success (no footer)
```
Tasks: 18 completed
 Time: 3s 283ms
```
**No `❯❯❯❯ to the moon` footer on multi-task runs.** The footer only appeared on single-task invocations in captured fixtures.

### Failure
```
Error: task_runner::run_failed

  × Task audit:test failed to run.
  ╰─▶ Process bun failed: exit code 1
```
No `Tasks:` block emitted on failure.

---

## Parallel task output format

When tasks run in parallel, moon does **not** add `[project:task]` prefixes to the streamed output. Instead:

- Task body lines are prefixed with ` <project>:<task> | ` (one space + project:task + pipe + space)
- Example from `worker-py:typecheck`:
  ```
   worker-py:typecheck | Success: no issues found in 16 source files
  ```
- Most tasks produce no body output (just chrome start/complete lines)
- The per-task prefix uses a **single leading space** then `<project>:<task> | `

Tasks that produce no stdout have only chrome start and chrome complete lines (no body lines between them).

---

## `MOON_SUMMARY=detailed` format

The `MOON_SUMMARY=detailed` env var produces an additional section after task output:

```
 SUMMARY 

pass SyncWorkspace 
pass SyncProject(core) (2ms)
pass SyncProject(db) (2ms)
pass SyncProject(config) (1ms)
pass SyncProject(audit) (1ms)
pass RunTask(audit:format) (cached, 35ms, 7b3060c4)

 STATS 

Actions: 6 completed (1 cached)
   Time: 41ms
```

The summary also triggers a "changed files" preamble (loading the VCS diff for affected project detection):
```
▮▮▮▮ Loading changed files
Base revision: N/A
Head revision: HEAD
Changed files:
	...
▮▮▮▮ Building action graph
Action count: 6
...
▮▮▮▮ Executing action pipeline
```

This preamble is **not** present in regular (non-summary) runs.

---

## `moon query tasks` JSON structure

The JSON has two top-level keys (not just `tasks` as the plan expected):

```json
{
  "tasks": {
    "<project>": {
      "<task>": {
        "command": "...",
        "args": [...],
        "id": "...",
        "inputs": [...],
        "options": {...},
        "target": "<project>:<task>",
        "toolchains": [...],
        "type": "..."
      }
    }
  },
  "options": { ... }
}
```

**Deviation from plan:** plan expected only `{"tasks": {...}}` but actual JSON is `{"tasks": {...}, "options": {...}}`. The `tasks` key structure is exactly as expected: `tasks.<project>.<task>.command`. Confirmed: `tasks.audit.format.command == "prettier"`.

Note: `moon query tasks --no-color` is **not a valid flag** in moon 2.0.4 — it errors with `error: unexpected argument '--no-color' found`. Use plain `moon query tasks` to capture JSON.

---

## Test runner for failure fixture

**`run_test_failure.txt` uses `bun test` (Bun's built-in test runner).**

Evidence from fixture header:
```
bun test v1.3.14 (0d9b296a)
```

Failure output format (bun test):
```
test/action-extractor.test.ts:
16 | }
17 | 
18 | describe('AuditAction/AuditResource decorators', () => {
19 | 	it('records action metadata', () => {
20 | 		const action = Reflect.getMetadata(AUDIT_ACTION_KEY, MyController.prototype.getOne);
21 | 		expect(action).toBe('INTENTIONALLY_BROKEN_FOR_RTK_FIXTURE');
                      ^
error: expect(received).toBe(expected)

Expected: "INTENTIONALLY_BROKEN_FOR_RTK_FIXTURE"
Received: "customers:read"

      at <anonymous> (/path/to/file.ts:21:18)
(fail) <describe> > <it name> [2.29ms]

 6 pass
 1 fail
 10 expect() calls
Ran 7 tests across 3 files. [874.00ms]
```

**Task 5 implication:** rtk does NOT currently have a dedicated `bun test` filter. The closest existing filter is `cmds/js/vitest_cmd.rs`, but it parses JSON output from `--reporter json` — bun test emits plain text, so the vitest filter won't apply. Task 5 must either (a) add a small `filter_bun_test_output()` helper inside `moon_cmd.rs` that compresses bun test plain-text output, or (b) leave `bun` unmapped in `filter_for_tool` so bun test bodies pass through unchanged (chrome stripping still applies, but the underlying test output is verbatim). If (b) is chosen, the Task 6 ≥60% savings assertion on this fixture may need to use a different (failing) fixture from a tool with a real rtk filter (e.g. eslint).

The `bun test` failure body is NOT prefixed with `[project:task]` — it is streamed raw between the chrome start line and chrome complete line.

---

## `run_tsc_failure.txt`

Captured by injecting 4 TypeScript errors into `packages/audit/src/index.ts` then running `moon run audit:typecheck --force`. Errors include type-mismatch (TS2322), missing return (TS2355), and unknown identifier (TS2304). Reverted after capture.

The fixture's task body appears in **parallel-mode prefix** (` audit:typecheck | <line>`) because moon runs dependencies (core, config, db) in parallel with the target. This means the per-task tool filter routing (Task 5) does NOT compress this fixture's body — it's a chrome-only savings case. A future PR adding per-task buffering in parallel mode would unlock the tsc filter's compression for this shape.

---

## Deviations from plan expectations

| Expectation | Actual |
|-------------|--------|
| Cache hit: `(cached, Xms, hash)` | `(cached, hash)` — **no duration in cached format** |
| Task start: `(hash)` only | Confirmed — hash only, no duration |
| Task complete: `(duration, hash)` | Confirmed |
| `❯❯❯❯ to the moon` on multi-task success | NOT present — only on single-task runs |
| `--no-color` flag | Invalid flag in moon 2.0.4; omit it |
| `query_tasks.json` structure: `{"tasks":{}}` | Actually `{"tasks":{}, "options":{}}` |
| Parallel output: `[project:task]` prefix | Uses ` project:task | ` prefix (space + pipe, not brackets) |
