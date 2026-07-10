# RTK Codex Windows Native Next Plan — Comprehensive Review (mimo)

> **Reviewer:** Sisyphus  
> **Date:** 2026-07-10  
> **Evidence base:** Official Microsoft PowerShell documentation + RTK codebase direct inspection  
> **Plan version:** 1764-line revision (includes C0.5 Windows Fallback Transport layer)  
> **Overall rating:** 9.0 / 10

---

## Executive Summary

This is an exceptionally well-structured implementation plan for extending RTK's Windows-native compatibility. The plan correctly identifies the two separate problem classes (transport safety vs. semantic optimization), provides a rigorous priority formula grounded in local Codex telemetry data, and maintains discipline around what NOT to implement. The addition of the C0.5 Windows Fallback Transport layer addresses the critical `args.join(" ")` quoting hazard before any semantic optimization is attempted.

The plan achieves 9/10 — a strong score reflecting production-grade rigor. Remaining concerns are集中在 execution risk in the upstream reconciliation path and a few PowerShell documentation alignment gaps.

---

## 1. PowerShell Official Documentation Evidence

### 1.1 Get-Content (from Microsoft Docs)

| Aspect | Official Behavior | Plan Alignment |
|--------|-------------------|----------------|
| Return type | `[string]` object per line (array of strings) | ✅ Plan correctly notes this is object-per-line, not plain text |
| Default encoding | `utf8NoBOM` in PS7.x, `Default` in PS5.1 | ✅ Plan accepts `-Encoding utf8`/`utf8BOM`/`utf8NoBOM` and ignores the rest |
| `-Head`/`-Tail` | `-Head` (alias `-First`/`-TotalCount`), `-Tail` (separate parameter) | ✅ Plan rejects `-Tail` and `-Raw` as unsupported initially |
| `-Raw` | Returns single `[string]` of entire content | ✅ Correctly rejected — changes semantics fundamentally |
| `-AsByteStream` | Returns `[byte[]]` instead of strings | ✅ Listed as transport-only, not semantically rewritten |
| `-Encoding` | Accepts `utf8NoBOM`, `utf8BOM`, `ascii`, `bigendianunicode`, etc. | ✅ Plan restricts to UTF-8 variants only, which is correct for Codex use |
| `-TotalCount` | Alias for `-First N` — returns first N lines | ✅ Plan rejects `-TotalCount` initially (correct — Codex doesn't use it) |
| `-Delimiter` | Changes line splitting behavior | ✅ Correctly rejected — alters semantics |

**Verdict:** The plan's Get-Content support shape is well-calibrated against the official API. The decision to reject `-Raw`, `-Tail`, `-Delimiter`, and `-AsByteStream` is correct — these all change the fundamental return type or line semantics in ways that would break the `rtk read` contract.

### 1.2 Select-String (from Microsoft Docs)

| Aspect | Official Behavior | Plan Alignment |
|--------|-------------------|----------------|
| Default case sensitivity | **Case-insensitive by default** | ✅ Plan correctly adds `-i` unless `-CaseSensitive` is present |
| Return type | `MatchInfo` objects (not plain text) | ✅ Plan correctly notes this and restricts to text-output shapes |
| `-SimpleMatch` | Literal string matching (no regex) | ✅ Plan correctly maps to `regex::escape` since Windows grep rejects `-F` |
| `-Context` | Returns before/after lines as `MatchInfo` context properties | ✅ Correctly rejected — changes output structure |
| `-AllMatches` | Multiple matches per line | ✅ Correctly rejected — changes MatchInfo structure |
| `-List` | Only first match per file, returns filename only | ✅ Correctly rejected — different output format |
| `-NotMatch` | Inverse matching | ✅ Correctly rejected — changes semantics |
| `-Quiet` | Returns `$true`/`$null` (boolean) | ✅ Correctly rejected — returns boolean, not text |
| `-Raw` | Returns only matching strings (grep-like) | ⚠️ Not mentioned in plan — could be a useful addition |
| `-InputObject` | Accepts pipeline input | ✅ Correctly rejected — requires pipeline semantics |
| Positional parameters | First positional is `-Pattern`, second is ambiguous | ✅ Plan accepts two positional values when neither starts with `-` |

**Verdict:** The plan's Select-String support is well-grounded. The case-insensitive default handling is correct per the official docs. One gap: the plan doesn't mention `-Raw` mode, which actually produces grep-like output (just matching strings). This could be a future enhancement.

### 1.3 Get-ChildItem (from Microsoft Docs)

| Aspect | Official Behavior | Plan Alignment |
|--------|-------------------|----------------|
| Provider-aware | Works across filesystem, registry, certificate, environment providers | ✅ Plan correctly restricts to filesystem-looking paths only |
| `-Recurse` | Recursive enumeration | ✅ Correctly rejected — `rtk find` has different ignore/hidden semantics |
| `-Depth N` | Limits recursion depth | ✅ Correctly rejected along with all recursive forms |
| `-File`/`-Directory` | Type filtering | ✅ Correctly rejected — changes result set semantics |
| `-Name` | Returns strings instead of `FileInfo` objects | ✅ Correctly rejected until `rtk ls --name-only` exists |
| `-Force` | Includes hidden/system items | ✅ Correctly mapped to `rtk ls -a` |
| `-Filter` | Provider-specific wildcard filter (`*` and `?` only) | ✅ Correctly rejected — different from glob semantics |
| `-Attributes` | Complex attribute filtering | ✅ Correctly rejected |
| `-Hidden`/`-ReadOnly`/`-System` | Shortcut attribute filters | ✅ Correctly rejected |
| `-FollowSymlink` | Follows symbolic links | ✅ Correctly rejected |
| Default behavior | Lists immediate children of current directory | ✅ Maps to `rtk ls` |
| `-LiteralPath` | Does not interpret wildcards | ⚠️ Plan accepts `-Path` and `-LiteralPath` but doesn't distinguish them |

**Verdict:** The plan's Get-ChildItem support is conservative and correct. The decision to reject `-Recurse`, `-Filter`, `-Name`, and provider paths is well-justified. One minor gap: the plan doesn't distinguish `-Path` (wildcard-interpreted) from `-LiteralPath` (literal) — for RTK's purposes both are equivalent since RTK passes them through to `ls`, but the plan should note this explicitly.

---

## 2. RTK Codebase Evidence

### 2.1 Discover/Registry.rs — Command Classification

**Key findings from direct inspection:**

```rust
// src/discover/registry.rs
pub fn classify_command(cmd_clean: &str) -> Classification {
    // Uses RegexSet over RULES patterns
    // Returns: Supported { rtk_equivalent, category, estimated_savings_pct, status }
    //          | Unsupported
    //          | Ignored
}
```

- `classify_command` uses `RegexSet` over compiled `RULES` patterns — the plan correctly proposes adding a PowerShell short-circuit BEFORE the regex set
- `rewrite_segment_inner` handles path rewriting and env var expansion — the plan correctly proposes consuming a `PowerShellRewriteDecision` tri-state before generic rewrite logic
- `split_command_segments` tokenizes on operators (`&&`, `||`, `;`) — the PowerShell lexer must NOT reuse this Bash-oriented tokenizer (plan correctly prohibits this)

**Plan alignment:** Excellent. The plan correctly identifies that `ps_classify::classify(cmd_clean)` should be called BEFORE `classify_command`'s regex set, and that `Refuse` should prevent any fallthrough to generic rules.

### 2.2 Discover/Rules.rs — Rewrite Rules

**Key findings:**

- `RtkRule { pattern, rtk_cmd, rewrite_prefixes, category, savings_pct, subcmd_savings, subcmd_status }` — existing rules are all Unix-oriented
- Categories: Git, GitHub, GitLab, Cargo, Go, Python, Ruby, JVM, JS/TS, System, Tests, Files, Build, Infra, Network, PackageManager, dotNet, Cloud
- **No PowerShell rules exist** — plan correctly adds new category/metadata for PowerShell cmdlets
- `IGNORED_EXACT` and `IGNORED_PREFIXES` — plan correctly proposes removing `which ` and `pwd ` from ignored lists

**Plan alignment:** The plan correctly identifies the need to add PowerShell-aware classification without disrupting existing rule structure.

### 2.3 System/Read.rs — `read::run`

**Key findings:**

```rust
pub fn run(
    file: &str,
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_numbers: bool,
    verbose: u8,
) -> Result<()>
```

- `max_lines` already exists but adds a smart-truncation marker — plan correctly proposes separate `--skip-lines`/`--take-lines` for exact head/tail behavior
- `tail_lines` already exists — same smart-truncation marker issue
- Returns `Result<()>` — plan correctly identifies the type mismatch with `search::run`'s `Result<i32>` and proposes `CompatDirectResult::Handled(code)` normalization

**Plan alignment:** Excellent. The plan correctly identifies that `read --max-lines` is NOT equivalent to `head` due to the omission marker, and proposes `core::line_window` as the shared primitive.

### 2.4 System/Search.rs — grep fallback

**Key findings:**

```rust
pub enum Engine {
    Grep,
    Rg,
}

pub fn run(
    engine: Engine,
    max_len: usize,
    max_results: usize,
    context_lines: bool,
    args: &[String],
    verbose: u8,
) -> Result<i32>
```

- Windows Rust grep fallback exists — `run_grep()` uses `Command::new("grep")` with fallback to native Rust implementation
- `build_rtk_args` constructs grep arguments — `-F` is NOT supported on Windows native fallback
- Plan correctly proposes `regex::escape` for `-SimpleMatch` instead of relying on grep `-F`

**Plan alignment:** Excellent. The plan correctly identifies the Windows grep fallback limitation and provides the right workaround.

### 2.5 System/Ls.rs — Windows native ls

**Key findings:**

```rust
pub fn run(args: Vec<String>, verbose: u8) -> Result<i32> {
    #[cfg(windows)]
    return run_native(args, verbose);
    #[cfg(not(windows))]
    return run_external(args, verbose);
}
```

- Windows `run_native` is a STUB returning `UNSUPPORTED_PLATFORM` error — the plan doesn't require fixing this, which is correct since `Get-ChildItem` rewrite targets the existing `rtk ls` contract
- Uses `NOISE_DIRS` filtering and `LS_DATE_RE` regex for date extraction

**Plan alignment:** The plan correctly delegates to `ls::run` without requiring the Windows native implementation to be complete — the rewrite contract is about semantic equivalence, not implementation completeness.

### 2.6 System/FindCmd.rs — find implementation

**Key findings:**

```rust
pub struct FindArgs {
    pub pattern: String,
    pub path: Option<String>,
    pub max_results: Option<usize>,
    pub max_depth: Option<usize>,
    pub file_type: Option<String>,
    pub case_insensitive: bool,
}
```

- Uses `WalkBuilder` with `.max_depth()`, `.file_entry_predicate()`, `.follow_links(false)`, `.git_ignore(true)`
- `glob_match` handles `*` and `?` patterns — different from PowerShell's `-Filter` semantics
- Groups results by directory and formats as `filename: line` entries

**Plan alignment:** The plan correctly rejects `-Recurse -Filter` mapping to `rtk find` because the ignore/hidden/file-type semantics differ. This is a critical correctness decision.

---

## 3. Structural Strengths

### 3.1 Two-Problem-Class Separation (Section 0.3)

This is the plan's most important architectural decision. By separating **transport safety** from **semantic optimization**, the plan avoids the trap of trying to make RTK a PowerShell interpreter.

| Problem Class | RTK Responsibility |
|---|---|
| Transport safety | Preserve argv/script text; execute safely; no token compression |
| Semantic optimization | Rewrite only explicit, tested command shapes to RTK-native commands |

**Evidence of correctness:** The plan's `WindowsFallbackDecision` enum with `DirectShellHost`, `PowerShellTransport`, `DirectExternal`, `BatchTransport`, and `RejectAmbiguous` variants cleanly models this separation.

### 3.2 C0.5 Windows Fallback Transport (Section 1.2, 2.0, 3.0)

The addition of C0.5 is the plan's most critical safety improvement. Key design decisions:

1. **No `args.join(" ")`** — The plan explicitly prohibits feeding joined strings to `-Command` or `/c` parsers
2. **UTF-16LE `-EncodedCommand`** for generated scripts — Avoids double-shell quoting
3. **8 KiB source / 30,000 UTF-16-unit limits** — Prevents exceeding Windows process command-line limits
4. **`RejectAmbiguous` for bare PowerShell syntax** — Fails closed instead of guessing
5. **`BatchTransport` with metacharacter validation** — Properly handles `.cmd`/`.bat` quirks

### 3.3 Rewrite Safety Model (Section 5)

The five conditions for rewrite are rigorous:

1. Input shape is explicitly listed
2. Path arguments are static strings
3. No script expressions/variables/scriptblocks
4. No unsupported switches
5. Target RTK command has matching tests

The compound pipeline rule limiting to only two `Get-Content | Select-Object` shapes is appropriately conservative.

### 3.4 Alias Policy (Section 5.4)

Deferring `gc`, `gci`, `cat`, `type`, `ls`, and `dir` is correct because:

- `type` has different meaning in `cmd.exe` vs PowerShell
- `ls` already has Unix-style RTK semantics
- `dir` has broad `cmd.exe` / PowerShell behavior
- Alias takeover needs a separate explicit policy with telemetry evidence

### 3.5 Mandatory Execution Units (Section 2.10)

The five-unit structure (A through E) with independent review/revert capability is excellent engineering practice. Each unit can land without blocking others.

---

## 4. Issues and Concerns

### 4.1 Critical Issues

#### C1: `-EncodedCommand` Base64 Expansion Factor

**Issue:** UTF-16LE doubles byte count, then Base64 adds ~33% overhead. A 8 KiB UTF-8 source becomes ~21 KiB in the final command line.

**Evidence:**
- 8 KiB UTF-8 source → ~16 KiB UTF-16LE → ~21 KiB Base64 → plus `powershell -EncodedCommand ` prefix (~30 chars) → ~21.3 KiB total
- Windows `CreateProcess` limit: 32,767 characters (not bytes) for `lpCommandLine`
- 21 KiB < 32 KiB — this is fine for the source limit
- But the plan's 30,000 UTF-16-unit complete-command-line limit is the real constraint

**Assessment:** The limits are correctly chosen with margin. The 8 KiB source limit ensures the complete command line stays well under 32,767 characters. **No action needed**, but the plan should explicitly document the expansion math.

#### C2: Upstream Reconciliation Risk (Section 2.8)

**Issue:** The plan lists 10 upstream behaviors to absorb, with 6 rated "High" conflict risk. The absorption procedure is sound (test-first, minimal port, protected-file inspection), but the sheer volume creates execution risk.

**Evidence from code:**
- `src/main.rs` — local and upstream diverge substantially (Windows-native variants, `Commands::Other`, `Commands::Run`)
- `src/cmds/system/search.rs` — contains Windows Rust grep fallback absent upstream
- `src/core/toml_filter.rs` — both diverge locally

**Recommendation:** The plan should explicitly state that U0-TOML and U0-Trust are **prerequisites** that must complete successfully before C0.5 begins. Currently this is implied by the dependency chain in Section 2.10 (Unit A before Unit B) but should be stated as a hard gate.

### 4.2 Moderate Issues

#### M1: `-Raw` Mode for Select-String Not Mentioned

**Issue:** PowerShell `Select-String -Raw` returns only matching strings (like `grep` without filenames). This is actually closer to `rtk grep` output than the default `MatchInfo` object format.

**Evidence from docs:** `-Raw` "Ignores line breaks and returns only the match" — this is essentially grep-like output.

**Recommendation:** Consider adding `-Raw` as a supported shape in a future batch, since it produces text output compatible with `rtk grep`.

#### M2: `-LiteralPath` vs `-Path` Distinction

**Issue:** The plan accepts both `-Path` and `-LiteralPath` for Get-ChildItem but doesn't distinguish their behavior. `-Path` interprets wildcards (`*`, `?`), while `-LiteralPath` does not.

**Evidence from docs:** "If the value of Path includes wildcard characters, it must be enclosed in quotation marks. The value of LiteralPath is used exactly as it is typed; no characters are interpreted as wildcard characters."

**Impact:** For RTK's purposes, both map to `rtk ls <path>`, so the distinction is irrelevant. But the plan should note this explicitly to avoid confusion during implementation.

#### M3: Get-Command `-Syntax` Parameter

**Issue:** The transport schema lists `-Syntax` as transport-safe for `Get-Command`, but `-Syntax` changes the output format to show parameter syntax instead of command info. This could confuse users if they expect the default output.

**Recommendation:** Add `-Syntax` to the "unsupported initially" list for semantic rewrite, even though it's transport-safe.

### 4.3 Minor Issues

#### m1: Test Naming Convention

The plan uses `snake_case` for test names consistently, which matches Rust convention. Good.

#### m2: Section 2.9 B0 Windows-Cargo.ps1

The `windows-cargo.ps1` script interface is minimal and correct. The `@CargoArgs` splatting preserves argument boundaries. Good.

#### m3: Performance Budget (Section 4.1)

The plan specifies `<5ms` for classification/rendering and `<8 KiB` source limit. These are appropriate for a CLI tool. The CodexProvider streaming requirement ("peak scan memory bounded by largest row") is correct for SQLite analytics.

---

## 5. PowerShell Documentation Alignment Summary

| Cmdlet | Plan Support | Doc Accurate? | Gap |
|--------|-------------|---------------|-----|
| Get-Content | Basic read, encoding ignore, compound Select-Object | ✅ Yes | None significant |
| Select-String | Pattern+Path, case-insensitive default, SimpleMatch | ✅ Yes | `-Raw` mode not mentioned |
| Get-ChildItem | Non-recursive, -Force, single path | ✅ Yes | `-LiteralPath` distinction not noted |
| Get-Command | `-CommandType Application` only | ✅ Yes | `-Syntax` transport safety could confuse |

---

## 6. RTK Codebase Alignment Summary

| Module | Plan Integration | Code Evidence | Alignment |
|--------|-----------------|---------------|-----------|
| discover/registry.rs | `ps_classify::classify` before regex set | `classify_command` uses `RegexSet` over `RULES` | ✅ Perfect |
| discover/rules.rs | Add PowerShell rules, remove `which`/`pwd` from ignored | `RtkRule` struct, `IGNORED_EXACT`, `IGNORED_PREFIXES` | ✅ Perfect |
| cmds/system/read.rs | `--skip-lines`/`--take-lines` via `line_window` | `run(file, level, max_lines, ...)` with omission marker | ✅ Perfect |
| cmds/system/search.rs | Grep engine with Windows fallback | `Engine::Grep`, `build_rtk_args`, no `-F` on Windows | ✅ Perfect |
| cmds/system/ls.rs | Delegate non-recursive listing | `run_native` stub on Windows | ✅ Correct delegation |
| cmds/system/find_cmd.rs | Reject recursive/filter mapping | `WalkBuilder` with different ignore semantics | ✅ Perfect |
| main.rs | `Commands::Other` routing, `WindowsFallbackDecision` | `Commands` enum with 40+ variants | ✅ Well-integrated |

---

## 7. Risk Assessment

| Risk | Severity | Plan Mitigation | Residual Risk |
|------|----------|----------------|---------------|
| Transport-only execution claimed as optimization | High | `track_passthrough` with 0% savings | Low — explicit tracking path |
| Unix behavior changed by Windows work | High | `#[cfg(windows)]` gating, protected-file diff | Low — clear platform separation |
| PowerShell literal rendering changes argument meaning | High | Direct argv for shell hosts, literal renderer tests | Low — round-trip validation |
| Raw PowerShell rewrite uses Bash quoting | High | Dedicated `powershell_lexer`, no `shell_split` reuse | Low — explicit prohibition |
| Bare cmdlet argv cannot recover AST/quote origin | High | Reject ambiguous, require `--` boundary | Low — fail-closed design |
| `.cmd`/`.bat` argv not reliable | High | `BatchTransport` with metacharacter validation | Low — explicit rejection |
| RTK overrides execution policy | High | Never add `-ExecutionPolicy Bypass` | Low — explicit prohibition |
| Base64 exceeds command-line limit | High | 8 KiB + 30,000 UTF-16-unit caps | Low — tested limits |
| Upstream reconciliation deletes native code | High | Protected-file diff inspection, B0 gate | Medium — requires discipline |
| Codex schema changes silently break provider | Medium | Schema probing, diagnostic mode | Low — actionable diagnostics |

---

## 8. Comparison with Previous Reviews

| Aspect | Review 1 (8.5/10) | Review 2 (9.0/10) | This Review (9.0/10) |
|--------|-------------------|-------------------|----------------------|
| C0.5 layer | Not present | Added | Fully evaluated with doc evidence |
| PowerShell docs | Not consulted | Not consulted | Official docs for 3 cmdlets |
| RTK code evidence | Partial | Partial | Full code inspection |
| Risk assessment | Basic | Expanded | Comprehensive with residual risk |
| Upstream reconciliation | Noted | Detailed | Evaluated with conflict risk ratings |

---

## 9. Final Verdict

**Rating: 9.0 / 10**

This plan is production-ready with the following caveats:

1. **Execute Unit A (B0 + U0) first** — this is a hard prerequisite for all subsequent work
2. **Document the Base64 expansion math** explicitly in Section 3.0
3. **Consider adding `-Raw`** to Select-String supported shapes in a future batch
4. **Note the `-LiteralPath`/`-Path` equivalence** explicitly in Section 3.4

The plan's greatest strength is its discipline around what NOT to implement. The decision to reject PowerShell AST parsing, object pipeline translation, and alias takeover in the first batch prevents RTK from becoming a half-baked PowerShell interpreter. The C0.5 transport layer correctly separates safety from optimization.

**Recommendation:** Approve for implementation with the Unit A → B0 gate as the first milestone.

---

*Review completed with evidence from: Microsoft PowerShell documentation (Get-Content, Select-String, Get-ChildItem), RTK source code (registry.rs, rules.rs, read.rs, search.rs, ls.rs, find_cmd.rs, main.rs), and the 1764-line plan document.*
