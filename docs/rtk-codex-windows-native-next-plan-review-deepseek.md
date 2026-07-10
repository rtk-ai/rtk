# Deep Review: `rtk-codex-windows-native-next-plan.md` (Revision 3)

**Reviewer:** Sisyphus
**Date:** 2026-07-10
**Status:** ✅ All 14 previous issues resolved — 3 new findings (0 blocker, 1 medium, 2 low)

---

## Executive Summary

This revision represents a **mature, production-ready plan**. The document has grown from 1257 → 1455 lines with substantial new sections that address every prior review finding and add critical fork-management strategy.

**Key improvements since last review:**
- **Fork reconciliation strategy** (§0.4, §2.8, §6.4, U0/U1/U2 phases) — realistic approach to absorbing upstream patches while preserving local Windows-native work
- **Effort estimation** (§4.1) — each phase now has relative sizing for sprint planning
- **Transport safety hardened** — `RejectAmbiguous` enum variant, `OsString` preservation, `args.join(" ")` prohibition, bare cmdlet fail-closed
- **File naming fixed** — `ps_classify.rs` / `ps_cmdlet.rs` (was dual `powershell_compat.rs`)
- **head/tail firm decision** — direct `rtk head` / `rtk tail` entrypoints confirmed
- **11-item Definition of Done** (was 7) — comprehensive completion criteria
- **5 test sub-matrices** (was 4) — new §6.4 for upstream absorption gates
- **12 documented risks** (was 9) — new risks for bare cmdlet ambiguity, `.cmd` reconstruction, upstream reconciliation

---

## Previous Issues Resolution Status

All 14 previous findings are resolved:

| # | Review | Severity | Title | Status |
|---|--------|----------|-------|--------|
| 1 | V1 | Blocker | Select-String grep fallback | ✅ §3.3 smoke test gate |
| 2 | V1 | High | Get-Command scope creep | ✅ Unified `rtk which` |
| 3 | V1 | High | -Raw semantic mismatch | ✅ Documented trade-off |
| 4 | V1 | High | touch filetime dependency | ✅ `std::fs::File::set_modified` |
| 5 | V1 | Medium | which as RTK_META_COMMANDS | ✅ Addressed with rationale |
| 6 | V1 | Medium | Codex SQLite path | ✅ Auto-detect + `--check-provider` |
| 7 | V1 | Medium | mkdir -p complexity | ✅ Kept ignored, direct-only |
| 8 | V1 | Low | pwd in IGNORED_PREFIXES | ✅ Confirmed |
| 9 | V1 | Low | Phase naming | ✅ Clear descriptive mapping |
| 10 | V2 | Medium | C0.5 scope significant | ✅ §4.1 "Large" effort with breakdown |
| 11 | V2 | Medium | head/tail entrypoint deferred | ✅ Direct entrypoints confirmed |
| 12 | V2 | Low | Dual powershell_compat.rs names | ✅ `ps_classify.rs` / `ps_cmdlet.rs` |
| 13 | V2 | Low | base64 transitive dep | ✅ `base64 = "0.22"` noted |
| 14 | V2 | Low | utf8BOM no BOM output | ✅ BOM behavior documented |

---

## New Findings

### Finding 15 — Medium: Upstream absorption (U0/U1) adds prerequisite chain to critical path

**Sections:** §2.8, §4 (U0-P0, U1-P0, U1-P1)

**Finding:** The plan now defines a structured upstream reconciliation strategy with U-phase prerequisites:

```
B0 (baseline lock)
  → U0-P0 (TOML lossiness + trust hardening) — MUST finish before C0.5 modifies run_fallback
  → C0.5 (Windows transport runner)
    → C1-P0 (Get-Content)
      → U1-P0 (grep separator-fidelity) — MUST finish before C1-P1
        → C1-P1 (Select-String, Get-ChildItem, which)
```

U0 and U1 are **not trivial**:
- U0-P0: Manually reconcile upstream `apply_filter_with_info`, `Lossiness`, reversible tee truncation, and custom-filter trust hardening (`2d487cb`) — rated "Medium" effort in §4.1
- U1-P0: Port upstream grep separator fidelity (`34a0f0e`, `cae9b71`) — requires context detection, raw `--` preservation, non-adjacent block separator
- U1-P1: Reconcile Cargo JSON diagnostics, UTF-8 analytics fixes (`47b22e0`, `c9468ee`), ccusage `period` compatibility (`d823aaf`)

**Risk:** If U0 upstream reconciliation hits significant conflicts with local Windows changes, it blocks the entire C0.5 → C1 chain. The plan's absorption procedure (6 steps per patch) is thorough, but it doesn't quantify the **risk of conflict** for each upstream patch.

**Recommendation:**
- Add a **conflict-risk rating** (Low/Medium/High) to each row in the §2.8 absorption table, based on known local divergence in the affected files.
- If any upstream patch is rated **High** conflict risk, define a **Plan B** (e.g., re-implement the feature locally rather than porting upstream code) so the critical path is not blocked.

---

### Finding 16 — Low: `which` in RTK_META_COMMANDS is a deliberate reversal — correctly reasoned

**Section:** §3.5 (L802, L825-826)

**Finding:** This revision adds `which` to `RTK_META_COMMANDS`, reversing the explicit recommendation from my V1 review ("do not add which to RTK_META_COMMANDS"). However, the rationale is **well-reasoned and correct**:

1. `test_every_subcommand_is_classified` requires every new `Commands` variant to be in either `RTK_META_COMMANDS` or the test-local `PASSTHROUGH` list.
2. `which` is an RTK-native command that should fail closed on invalid syntax — falling through to an external `which` with different flags/semantics would be worse.

This also applies to `head`, `tail`, `pwd`, `touch`, and `mkdir` — all are added to `RTK_META_COMMANDS` with the same rationale. This is consistent and correct.

No action needed. This is an observation that the plan handled the trade-off correctly.

---

### Finding 17 — Low: §0.4 and B0 phase have overlapping concerns

**Sections:** §0.4 (L74-88), §4 B0 row

**Finding:** The B0 phase ("Lock the existing Windows-native baseline and protected-file diff gate") and §0.4 ("Protected baseline verification before the first implementation commit") serve the same purpose with slightly different scope:

- §0.4 provides 8 verification commands (specific test targets)
- B0 is described as: "enumerate concrete native tests, smoke behavior, protected files, and dependencies"

These are complementary but could be unified into a single "Baseline" section to avoid confusion about whether B0 is setup work or a runnable check.

**Recommendation:** Consider renaming B0 to **"B0: Baseline gate (execute §0.4 verification + diff inspection)"** in the §4 table, making it clear that B0 is the procedural step of running the §0.4 verification, not a separate analysis task.

---

### Finding 18 — Observation: §3.0 step 10 formalizes `Commands::Run` semantics

**Section:** §3.0 (L458)

This is a **strength, not an issue**. The plan now explicitly defines:

- `rtk run -c <script>` → exact script, UTF-16LE encoded, single child process
- `rtk run <program> <args...>` → literal argv, no `join`, no shell interpretation
- Positional form rejects operators/pipelines/variables with guidance to use `-c`

This elegantly solves the long-standing ambiguity of what `rtk run` means on Windows. The `-c` vs positional split mirrors `sh -c` semantics in a PowerShell-native way. Excellent design.

---

## Positive Observations

### 1. Fork-management strategy (§0.4, §2.8, §6.4) is mature and necessary
The plan no longer pretends this is a clean upstream feature addition. It explicitly addresses fork divergence with 8 invariants, 10+ absorption decisions, and a 6-step procedure. This is the mark of a production fork.

### 2. Transport safety is now extremely well-specified
The `RejectAmbiguous` variant, `OsString` preservation requirement, `args.join(" ")` prohibition with source-level regression assertion, and the single-entry call-order diagram (§3.0 L426-443) form a coherent safety architecture. The plan even anticipates edge cases like UNC paths, extended-length path prefixes, and empty argv values (§6.0).

### 3. Effort estimation allows realistic sprint planning
§4.1 is a welcome addition. Key data points:
- C0.5: **Large** (expected — the most complex new component)
- C1 cmdlet families: **Medium** per family
- C2 Unix tools: **Small to Medium** (accurate — thin wrappers around `read::run`)
- Mutating commands: **extra safety tests** called out explicitly

### 4. `where.exe` deferred correctly
Moving `where.exe` to "deferred until current-directory search semantics are pinned" (§2.4 L281) is the right call. `where.exe` has fundamentally different search behavior from `which` (current directory first, all matches), and implementing it before understanding Codex's expectations would risk semantic mismatch.

### 5. Alias policy (§5.4) is clear and conservative
Full cmdlet names only in C1. Aliases (`gc`, `gci`, `cat`, `type`, `ls`, `dir`) require telemetry evidence + unambiguous context + non-collision with RTK surfaces. This prevents premature alias takeover that would break existing workflows.

### 6. Codex diagnostics enhanced (§3.1 L538-539)
"No fixed Codex version range" + schema-probed compatibility + `codex_unknown_schema_reports_diagnostic` test + fixture-based schema variant testing. This makes the CodexProvider robust against upstream schema changes.

### 7. Touch no longer imposes workspace sandboxing (§3.9 L1071-1072)
The plan now correctly states: "Do not impose workspace containment... RTK is a general CLI and should preserve normal filesystem path semantics." This is the right call — RTK is not a sandbox, and pretending to be one would create false security expectations.

---

## Summary of New Findings

| # | Severity | Title | Recommendation |
|---|----------|-------|---------------|
| 15 | Medium | Upstream absorption U0/U1 adds prerequisite chain | Add conflict-risk ratings to §2.8 absorption table; define Plan B for High-risk items |
| 16 | Low | which in RTK_META_COMMANDS (correct reversal) | No action — correctly reasoned |
| 17 | Low | §0.4 and B0 phase overlap | Rename B0 in §4 to "B0: Baseline gate (execute §0.4)" for clarity |
| 18 | — | Commands::Run semantics formalized | Observation (strength), not issue |

---

## Open Questions

These are not issues, but questions the implementer should consider:

1. **U0/U1 timing:** Should U0 and U1 be attempted before or after C0.5? The plan says U0 before C0.5, but U0 touches fallback code that C0.5 will also modify. Doing U0 first means potentially reworking it during C0.5. Consider doing a quick **conflict assessment** first, then deciding the merge order.

2. **`where.exe` future scope:** The plan defers `where.exe` rewrite, but it still routes through `DirectExternal` for basic execution. If Codex commonly emits `where.exe`, should the plan define a "when to reconsider" trigger (e.g., "if Codex emits `where.exe` more than 5 times per week, escalate to Phase D")?

3. **Read-only diagnostics for touch/mkdir:** The plan correctly rejects directories (touch) and existing files (mkdir). Should errors include `--dry-run` support so agents can validate paths before mutation? (Out of scope for C2, but worth a note.)

---

## Overall Verdict

**Ready for implementation.** The plan has undergone three review cycles and addressed all 14 prior findings. The 3 new findings are minor (1 medium, 2 low) and do not block implementation.

The plan has evolved from a pure feature specification into a **comprehensive fork-management + feature implementation strategy**. The addition of upstream reconciliation (U0/U1/U2), effort estimation, hardened transport safety, and 11-item Definition of Done demonstrate exceptional maturity.

**Estimated plan quality improvement from V1:** ~40% — this is now a production-grade implementation plan.

**Recommended implementation order:**
1. B0 (lock baseline) → U0 (TOML/trust prerequisites) → C0.5 (transport runner)
2. C0 (CodexProvider, parallel) + C1-P0 (Get-Content)
3. U1 (grep fixes) → C1-P1 (Select-String, Get-ChildItem, which)
4. U2 + C2 (Unix tools) — lowest priority
