# RTK Codex Windows Native Next Plan Review (Codex)

> Review date: 2026-07-10
>
> Reviewed document: `docs/rtk-codex-windows-native-next-plan.md`
>
> Review scope: plan correctness, Windows argv and PowerShell transport safety, preservation of the existing local Windows-native commands, upstream absorption feasibility, and whether the proposed verification can prove the stated guarantees.

## 1. Review Conclusion

The plan has the right top-level priorities: preserve the existing Windows-native implementations, keep transport separate from semantic optimization, use direct argv for resolved executables, and selectively reconcile upstream rather than replacing local files wholesale.

It is not ready for execution yet. The main blockers are:

1. Raw PowerShell rewrite still depends on the existing Bash-oriented lexer and has no shell-correct rewrite renderer.
2. The Codex SQLite provider is modeled as file sessions even though the observed database stores row-level timestamps and thread IDs.
3. Upstream absorption is listed as a matrix but has no executable U0/U1/U2 tasks.
4. Several proposed compatibility mappings are not semantically equivalent (`Get-Command`, recursive `Get-ChildItem`, `head`, and `Get-Content -`).
5. The plan overstates what can be guaranteed for `.cmd` / `.bat` argv transport and introduces an unjustified `ExecutionPolicy Bypass` option.

These issues should be corrected in the plan before implementation begins.

## 2. Findings

### [P0] F1: PowerShell rewrite incorrectly reuses the Bash-oriented lexer

**Plan evidence:**

- Section 3.2 says to use quote-aware tokens from `src/discover/lexer.rs`.
- Compound PowerShell pipelines are intended to be recognized before generic rewrite handling.

**Code evidence:**

- `src/discover/lexer.rs::tokenize_inner` treats backslash as an escape introducer outside single quotes.
- `src/discover/lexer.rs::shell_split` implements Bash-like backslash and quote behavior.
- PowerShell instead uses backtick escaping, doubled single quotes, different double-quote interpolation, here-strings, and other grammar that the existing lexer does not model.

The plan also does not define how rewritten paths and patterns containing spaces, quotes, empty strings, trailing backslashes, or metacharacters are rendered back into a PowerShell command string. Parsing safely is not enough if the rewrite output is re-quoted incorrectly.

**Required correction:**

- Direct `rtk Get-*` invocation should parse existing argv without a shell lexer.
- Raw hook/rewrite input needs a dedicated conservative PowerShell tokenizer and renderer.
- Unsupported, ambiguous, or non-round-trippable input must return no rewrite.
- Add parse-render-parse tests for spaces, empty arguments, quotes, backticks, Unicode, UNC paths, and trailing backslashes.

### [P0] F2: CodexProvider uses the wrong session and time-filter model

**Plan evidence:**

- The plan proposes `<table>:<rowid>` as `ExtractedCommand.session_id`.
- `since_days` is applied to candidate SQLite file modification time.
- It proposes keeping the current file-oriented `SessionProvider` trait unless a display name is needed.

**Observed database evidence:**

The local read-only `~/.codex/logs_2.sqlite` schema contains a `logs` table with, among other fields:

```text
id INTEGER PRIMARY KEY AUTOINCREMENT
ts INTEGER NOT NULL
ts_nanos INTEGER NOT NULL
feedback_log_body TEXT
thread_id TEXT
process_uuid TEXT
```

Recent `ToolCall: shell_command` rows carry the actual Codex `thread_id` and row timestamp.

Using database mtime for `since_days` is incorrect because an actively written database remains new while containing old history. Assigning every row a separate session ID destroys per-thread grouping and sequence analytics.

The current `SessionProvider` contract returns `Vec<PathBuf>` and then extracts a whole file. A single SQLite file containing many row-level sessions does not fit that model cleanly.

**Required correction:**

- Filter rows with `WHERE ts >= ?`.
- Group and identify sessions with `thread_id`.
- Order commands with `(ts, ts_nanos, id)`.
- Change or extend the provider abstraction so a provider can return logical sessions rather than only files.
- Define project filtering for Codex explicitly instead of silently inheriting Claude's path-based behavior.
- Do not automatically scan every `*.sqlite` under Codex home; unknown databases should be diagnostic candidates or explicit overrides.

### [P0] F3: Upstream absorption has no executable implementation tasks

The selective upstream matrix correctly classifies TOML, trust, grep, Cargo, analytics, signal propagation, checkout, hooks, Droid, and PHP. However, the detailed implementation section begins with C0.5 and contains no detailed B0, U0, U1, or U2 tasks.

This leaves the implementer without:

- exact local files and final upstream source state to reconcile;
- failing regression tests to restore first;
- focused verification commands;
- protected-file diff checks for each port;
- commit boundaries and dependencies between TOML restoration and C0.5 edits.

This is especially blocking because TOML lossiness and trust hardening are declared prerequisites for C0.5.

**Required correction:** add detailed tasks for B0, U0-TOML, U0-trust, U1-grep, U1-Cargo, U1-analytics/ccusage, and U2-checkout. Each task must list files, tests, expected failure, minimal port, verification, protected native regression gate, and commit boundary.

The upstream matrix should also record conflict risk and a fallback implementation strategy. Based on the current local diff, the initial risk assessment is:

| Absorption item | Conflict risk | Reason | Plan B |
|------|------|------|------|
| TOML lossiness / fallback guard | High | both `src/main.rs` fallback and `src/core/toml_filter.rs` diverge substantially | reimplement the final `Lossiness` contract against local filter types instead of copying upstream blocks |
| custom-filter trust hardening | High | local hook and trust files have broad edits and deleted upstream behavior | port behavior and tests one invariant at a time; do not restore whole hook files |
| grep separator fidelity | High | local `search.rs` contains the Windows Rust grep fallback absent upstream | transplant only separator logic and regression tests into the local parser/output pipeline |
| Cargo JSON fixes | High | local `cargo_cmd.rs` removed a large upstream JSON-diagnostic path | reconstruct the final typed-diagnostic behavior in a separate task rather than applying a large historical diff |
| analytics UTF-8 / ccusage | Medium | focused files but local analytics also diverges | port the final char-safe helpers and aliases manually with regression fixtures |
| Git checkout | Medium | touches git dispatch, registry, rules, and guards but not native system modules | implement checkout as a new local handler using upstream tests as the contract |

High conflict risk must not block C0.5 indefinitely. Each U0 prerequisite needs a bounded behavior-level Plan B and a checkpoint that can stop source-level porting once the required invariant is restored locally.

### [P0] F4: Unsupported cmdlet fallback contradicts the parameter-table design

The plan says unsupported semantic shapes can continue through safe PowerShell transport, while the renderer keeps parameter tokens bare only if they are present in the C1 per-cmdlet table.

Legitimate but non-optimized forms such as these are therefore undefined:

```powershell
rtk Get-Content -Raw file.txt
rtk Get-Content -Tail 20 file.txt
rtk Select-String -Context 2 pattern file.txt
```

If the table contains only optimized parameters, these forms become `RejectAmbiguous` and lose current fallback behavior. If every dash-prefixed token is accepted as a parameter, a literal filename beginning with `-` can be reinterpreted.

**Required correction:** separate the semantic rewrite allowlist from a broader, explicit transport parameter schema. If broad transport is intentionally not supported, document that these bare forms require explicit `powershell -Command` and remove the promise that all static unsupported cmdlets transparently fall back.

### [P1] F5: Windows `rtk run` is redefined without a compatibility decision

The current Windows implementation joins positional `rtk run` arguments and treats them as a PowerShell script. The plan changes positional form into literal invocation argv and rejects operators, pipelines, variables, and scriptblocks.

This is safer, but it is a behavior change. It also creates different positional semantics on Windows and Unix while the plan claims Unix behavior remains frozen. In addition, a resolvable positional program should use the same `DirectExternal` path as fallback instead of being unnecessarily rendered through PowerShell.

**Required correction:** decide and document the public `rtk run` contract before C0.5:

- `run -c <script>` can be the explicit script surface using encoded PowerShell.
- positional `run <program> <args...>` can be direct argv, but must be documented as a breaking or migration change if current script behavior is retained by users.
- resolved positional executables should use direct process execution.
- add compatibility tests for existing positional forms and explicit migration diagnostics.

### [P1] F6: `.cmd` and `.bat` cannot receive the same exact-argv guarantee as `.exe`

The plan groups `.cmd` / `.bat` wrappers with normal executables and relies on `Command::new(resolved_path).args(original_args)`. Rust can start simple batch wrappers, but Windows ultimately invokes batch files through `cmd.exe`; batch grammar does not provide native executable argv semantics for every combination of empty arguments, quotes, percent expansion, delayed expansion, and metacharacters.

The existing upstream tests prove basic wrapper discovery and execution, not adversarial argv round trips.

**Required correction:** introduce a distinct `BatchTransport` classification. Define and test the safe representable subset. Reject arguments outside that subset with explicit `cmd` guidance instead of claiming exact preservation.

### [P1] F7: Automatic `-ExecutionPolicy Bypass` is unjustified

The plan allows implicit PowerShell transport to add `-ExecutionPolicy Bypass` to Windows PowerShell "where needed". It does not define the condition and gives the transport layer authority to override a user or machine security policy.

Encoded commands normally do not need this flag. Script-file policy failures should not be silently bypassed by RTK.

**Required correction:** remove automatic bypass. Preserve it only when the user explicitly supplied it to an explicit PowerShell host invocation.

### [P1] F8: `head` cannot reuse the current `read --max-lines` implementation

The plan maps head to:

```rust
read::run(file, FilterLevel::None, Some(lines), None, false, verbose)
```

Current `read::apply_line_window` sends `max_lines` through `filter::smart_truncate`, which inserts an omission marker. That is not exact `head` output.

For stdin, `read::run_stdin` calls `read_to_string` and waits for EOF. A true head operation should emit N lines and stop; the proposed implementation can hang on a long-running or infinite producer.

**Required correction:** extract an exact line-window primitive. Implement head stdin as a streaming `BufRead::read_line` loop that stops at N lines. Tail may buffer to EOF, but should use an exact tail primitive rather than semantic truncation.

### [P1] F9: Generic `Get-Command` to `rtk which` rewrite is not semantically safe

`Get-Command` resolves applications, cmdlets, functions, aliases, and scripts. `rtk which` resolves PATH executables. Syntax alone cannot prove that `Get-Command <name>` refers to an application.

Examples such as `Get-Command ls` and `Get-Command Get-Content` would be changed incorrectly even though they have the same syntax as `Get-Command cargo`.

**Required correction:** do not rewrite bare `Get-Command`. Initially support an explicit application-only shape such as `Get-Command -CommandType Application <name>`, or leave all `Get-Command` forms on PowerShell transport and provide `rtk which` only as an explicit portable command.

### [P1] F10: Recursive `Get-ChildItem` is not equivalent to the current `rtk find`

The plan maps `Get-ChildItem -Recurse -Filter` to `rtk find <path> -name <glob>`. Current `rtk find`:

- returns files by default;
- respects `.gitignore` and global ignore files;
- skips hidden entries by default;
- compresses and caps output.

PowerShell recursive enumeration does not respect `.gitignore` and can return matching directories as well as files. This is a semantic change, not only an output compaction.

**Required correction:** keep recursive `Get-ChildItem` passthrough in this batch. Reconsider it only after adding a deliberately PowerShell-compatible enumeration mode with explicit file/directory, hidden, ignore, and result-cap semantics.

### [P2] F11: `Get-Content -` is not PowerShell stdin syntax

The plan accepts `-` and intends to call `read::run_stdin`. Native PowerShell treats `-` as a path. A local explicit PowerShell check returned:

```text
Cannot find path '-' because it does not exist.
```

**Required correction:** remove `-` from `Get-Content` compatibility. Keep stdin support for explicit `rtk read -`, not for the PowerShell cmdlet alias.

### [P2] F12: Baseline verification is not self-contained or portable

The B0 commands assume a usable MSVC/Windows SDK environment. A direct attempt to list the selected tests failed before test discovery because the current shell lacked headers and libraries including `vcruntime.h`, `stdarg.h`, and `msvcrt.lib`.

The later verification command hard-codes both a Visual Studio 18 Professional installation and Visual Studio 2022 BuildTools 14.44 paths. This is machine-specific, mixes toolsets, and is unsuitable for CI or another developer machine.

**Required correction:** make environment discovery and validation the first B0 step. Use `vswhere` or the selected installation's `VsDevCmd` / `Launch-VsDevShell`, validate `INCLUDE`, `LIB`, compiler, linker, and Windows SDK paths, and then run exact test selectors. Do not hard-code toolset versions in the general verification plan.

### [P2] F13: Section 0.4 and phase B0 duplicate baseline ownership

Section 0.4 defines the protected baseline commands and recording rule, while the phase table describes B0 as enumerating and locking the same baseline. It is unclear whether B0 is an analysis task that changes the command list or simply execution of section 0.4.

**Required correction:** rename the phase to `B0: Baseline gate` and define it as executing the section 0.4 checks, recording exact non-zero test selectors and smoke outputs, and establishing the protected-file diff snapshot. Keep the actual source of truth in one section.

## 3. Structural Recommendation

The document now covers several independent subsystems. Keep it as a master roadmap, but split executable detail into separate plans:

1. B0 and upstream correctness reconciliation.
2. C0.5 Windows fallback transport.
3. C1 PowerShell classification and rewrite.
4. Codex SQLite provider.
5. C2 native small tools.

B0/U0 and C0.5 are the immediate blockers. C1 should not begin until a dedicated PowerShell lexer/renderer contract exists. CodexProvider should not proceed with the current file-session abstraction.

## 4. Verification Performed

- Re-fetched and compared the tracked upstream reference earlier in the same review session; `HEAD` and `origin/develop` were both `5d32d0736f686b69d1e8b9dc45c007d4eb77a0a2` at that point.
- Inspected the local `discover` lexer, provider trait, `read` line-window implementation, `find` behavior, fallback paths, and proxy implementation.
- Opened the local Codex database read-only and verified the observed `logs` schema and `thread_id` values.
- Verified with explicit PowerShell that `Get-Content -` treats `-` as a path.
- Attempted `rtk cargo test cmds::system::ls -- --list`; compilation failed before test listing because the current terminal did not have a complete MSVC/Windows SDK environment.

No implementation code or plan document was changed as part of this review report.

## 5. DeepSeek Review Verification

Reviewed report: `docs/rtk-codex-windows-native-next-plan-review-deepseek.md` (Revision 3).

The report was treated as review input, not as authoritative status. Its claims were checked against the current plan, source, local Codex database schema, and observed Windows behavior.

### 5.1 New Findings

| DeepSeek item | Decision | Verification result |
|------|------|------|
| Finding 15: U0/U1 prerequisites add critical-path conflict risk | Accepted and strengthened | The risk is real. The Codex report now adds per-item conflict ratings and behavior-level Plan B guidance to F3. The issue is more serious than a scheduling note because U0 currently has no executable task definition. |
| Finding 16: adding native commands to `RTK_META_COMMANDS` is deliberate and correct | Accepted as a non-issue | A native Clap command should fail closed on invalid RTK syntax instead of falling through to an external command with different semantics. No plan correction is required for this point. |
| Finding 17: section 0.4 and B0 overlap | Accepted | Added F13. B0 should be defined as execution and recording of one canonical baseline section, not a second baseline-design activity. |
| Finding 18: the new `Commands::Run` split is unconditionally a strength | Not accepted | The split is a reasonable target contract, but it changes current Windows positional behavior and may diverge from Unix. It requires an explicit compatibility decision, migration diagnostics, and direct-external routing. F5 remains valid. |

### 5.2 Previous-Issue Resolution Claims

Several claims that all previous issues are resolved are not supported by the current plan:

| DeepSeek claim | Decision | Reason |
|------|------|------|
| `Get-Command` scope is resolved by unifying on `rtk which` | Not accepted | Bare `Get-Command` may resolve aliases, functions, cmdlets, or scripts. A generic syntactic rewrite to PATH-only `which` remains incorrect; see F9. |
| `-Raw` mismatch is resolved by documenting the trade-off | Partially accepted | The semantic rewrite exclusion is documented, but safe fallback remains internally inconsistent with the restricted parameter table; see F4. |
| Codex SQLite support is robust through path detection and schema probing | Not accepted | Path/schema diagnostics are useful, but row timestamps and `thread_id` are ignored by the proposed session model; see F2. |
| Direct head/tail entrypoints resolve the head/tail concern | Not accepted | The proposed head implementation emits smart-truncation markers and reads stdin to EOF; the entrypoint exists only on paper and its primitive is wrong; see F8. |
| C2 tools are thin wrappers around `read::run` | Not accepted for head | Reuse is appropriate only if the shared primitive preserves command semantics. Current `read --max-lines` does not preserve exact head behavior. |

### 5.3 Positive Observations

The following DeepSeek observations are reasonable and require no additional finding:

- Deferring `where.exe` is correct because its current-directory and all-match behavior differs from `which`.
- The full-name-only alias policy is conservative and appropriate for the first C1 batch.
- Explicit `touch` and `mkdir` commands should preserve normal CLI path semantics rather than pretending RTK is a workspace sandbox.
- Adding direct `base64 = "0.22"` is preferable to relying on a transitive dependency.
- Documenting UTF-8 BOM behavior is useful and avoids claiming byte-preserving reads.

### 5.4 Recommendations Not Absorbed

- A telemetry threshold such as "five `where.exe` calls per week" is arbitrary without a stable sampling policy; future prioritization should use the planned CodexProvider after its data model is corrected.
- `--dry-run` for `touch` / `mkdir` is unnecessary in this batch. Raw automatic rewrites are already disabled, and adding mutation-preview semantics would expand C2 without addressing a demonstrated need.
- The overall verdict "ready for implementation" is not accepted. F1 through F4 are design blockers, and F5 through F10 contain behavior or semantic risks that must be resolved before their respective phases execute.

### 5.5 Revised Verdict After Cross-Review

DeepSeek's conflict-risk and B0-clarity observations improve the review, but they do not change the overall result: the document is a useful master roadmap, not yet an executable implementation plan. B0/U0 details, C0.5 transport contracts, the PowerShell lexer/renderer, and the Codex row/session model need revision before implementation starts.
