# JVM ecosystem filters

Filters for JVM-based build tools.

| Module           | Tool(s)                              | Modes                                                                                  |
|------------------|--------------------------------------|----------------------------------------------------------------------------------------|
| `gradlew_cmd.rs` | `./gradlew`, `gradlew.bat`, `gradle` | Build / Test / ConnectedTest / Lint / Dependencies — streaming line filter + passthrough |
| `mvn_cmd.rs`     | `mvn`, `./mvnw`, `mvnw.cmd`, `mvnd`  | Test / Compile / Package / Passthrough — buffered single-pass filter per phase           |

## Maven (`mvn_cmd.rs`)

Phase routing (`detect_phase`):

| Phase       | Goals                                                  | Filter                  |
|-------------|--------------------------------------------------------|-------------------------|
| `Test`      | `test`, `integration-test` (Failsafe = Surefire shape) | `filter_surefire`       |
| `Compile`   | `compile`, `test-compile`                              | `filter_compile`        |
| `Package`   | `package`, `install`, `verify`, `deploy`               | `filter_package`        |
| `Passthrough` | `clean`, `site`, `dependency:*`, `--version`, `--help`, empty, any unrecognised goal | none |

Key behaviours:

- **ANSI strip first** in every filter — real Maven output contains colour escapes.
- **English-footer guard** — if neither `BUILD SUCCESS` nor `BUILD FAILURE` appears as a trimmed line suffix, return the ANSI-stripped raw input unchanged. Protects non-English locales.
- **Verbose bypass** — `-X`, `--debug`, `-e`, `--errors` skip filtering (`run_passthrough`). User asked for detail; respect it.
- **Surefire block collapse** — Surefire emits `[INFO] Running <FQN>` … `[INFO] Tests run: N, Failures: F, Errors: E, …, Time elapsed: T s - in <FQN>`. The filter buffers each block and emits it only when `F > 0` or `E > 0`. Passing blocks (the bulk of healthy-project output) are dropped silently. Failing blocks are emitted with framework stack frames stripped via a deny-list (`at org.junit.`, `at java.util.`, `at sun.reflect.`, etc.).
- **Multi-failure classes (trail re-arm)** — when a single class has several failing tests, Surefire 3.x emits one blank-separated detail block per failing test under a single close line. When a failure trail ends at a blank line, the state machine arms a re-entry: the next per-test subline (`[ERROR] FQN.method -- Time elapsed: … <<< FAILURE!` or `<<< ERROR!`) re-enters the trail with the same keep/drop decision, so every failure message survives (and a capped class drops *all* its blocks). Any other non-blank line disarms the re-entry.
- **`<<< ERROR!` markers** — per-test sublines use `<<< ERROR!` for thrown (non-assertion) exceptions; the close-line regex also tolerates an `ERROR!` marker defensively (Surefire 3.5.5 emits `FAILURE!` even for errors-only classes — failure detection keys off the `Failures`/`Errors` counts, not the marker).
- **Help-boilerplate stripping (all modes)** — the post-failure block Maven emits after `[ERROR] Failed to execute goal` (`See …`, `-> [Help 1]`, `Re-run Maven`, `To see the full stack trace`, `For more information`, help URLs, bare `[ERROR]` dividers) is dropped in quiet *and* non-quiet filters alike (shared `BOILER_PREFIXES`). Deliberately kept as signal: `Failed to execute goal` itself and the multi-module resume hint (`[ERROR] After correcting the problems…` + `[ERROR]   mvn <args> -rf :module` — tells the user/agent how to resume the build). Real durations (`Time elapsed: … s`, `Total time: …`) ship untouched — the numbers are diagnostic signal.
- **Wrapper detection** — `./mvnw` (POSIX) and `mvnw.cmd` (Windows) detected via string-literal `Command::new` (semgrep-safe); falls back to `resolved_command("mvn")`.
- **Maven Daemon (`rtk mvnd`)** — `mvnd` is a separate entry point (`run_daemon`), not a `mvn` alias: it shares phase detection and every filter, but always executes `mvnd` and is never substituted by a `./mvnw` wrapper found in the working directory. The daemon's rolling console UI only engages on a TTY, so what rtk captures is ordinary Maven log output and the existing filters apply unchanged. Two daemon-specific quirks are handled explicitly:
  - **`[INFO]`-prefixed blanks** — the daemon logger prefixes even empty lines with `[INFO] `, so the Surefire failure trail's blank-line terminator uses `is_blank_separator` (a bare `[INFO]` counts as blank). Without it the trail never closes and everything after the last failing class passes through verbatim.
  - **Parallel-reactor lanes** — `mvnd` builds multi-module reactors in parallel by default and prefixes per-module lines with `[module] `, interleaved line-by-line. `split_lane` classifies on the prefix-stripped view while emitting the original (module identity survives), and each module gets its own `SurefireLane` (block machine + compile-continuation flag + summary-open flag) so a passing close from one module can't be attributed to another's open block. Three consequences of the interleaving are handled explicitly: unprefixed raw lines (stack traces, javac `symbol:` / `location:` continuations) go to the lane in an active **failure trail** before any lane merely holding an open block (`raw_owner`) — otherwise a module opening a block between a failing close and its exception swallows the diagnostics and drops them when it closes green — and when ownership is genuinely ambiguous (several plain blocks open, no trail) the line is preserved verbatim rather than routed to a guess. The `[ERROR] Failures:` budget stays reactor-wide (`FailuresSummaryCap`, reset only when a header opens the first summary), so interleaved per-module summaries share one cap instead of each getting `MAX_MVN_FAILING_CLASSES`.

  One known limit remains, not a regression — it already applies to `mvn -T`: daemon status chatter (`Connecting to the daemon`, …) carries no `[INFO]`/`[ERROR]` prefix, so the keep-list filters (`filter_surefire`, `filter_compile`, `filter_package`) drop it — but `filter_quiet` has no keep-list, only a deliberate "keep anything else" safety net, so `rtk mvnd -q` passes that chatter through where `rtk mvn -q` emits nothing.
- **Reactor Summary preservation** — for multi-module builds, the trailing `Reactor Summary for <root>` block with per-module SUCCESS/FAILURE rows is kept (toggled by a `[INFO] Reactor Summary for ` header and cleared on `BUILD SUCCESS` / `BUILD FAILURE`).
- **Failure cap** — both the count of emitted failing test classes and the size of the `[ERROR] Failures:` summary block are bounded by `MAX_MVN_FAILING_CLASSES = CAP_WARNINGS` (the shared test-failure cap class from `src/core/truncate.rs`, same binding as pytest/rspec/rake/runner). Excess emissions are replaced by a single `… +N more failing test classes` / `… +N more failures` tail (canonical `join_with_overflow` shape) to keep large failure sets compact; the raw output stays recoverable via the tee `[full output: …]` hint. Per the core cap policy, a cap of `0` means summary-only: no blocks emitted, the tail still counts every dropped class.

Token-savings tests run inline as part of `cargo test --all`. The `mvn` fixtures (gzipped, ~1100 lines each; `flate2`, already in `Cargo.toml`, decompresses the ~3 KB gzipped fixtures in milliseconds) verify ≥90% savings for `mvn test` and ≥85% for `mvn install`. The `mvnd` fixtures are smaller synthetic captures and are held to the repo-wide ≥60% savings floor instead (`mvnd_reactor_pass_savings`, `mvnd_test_fail_savings`, `mvnd_parallel_reactor_fail_savings`, `mvnd_compile_error_savings`; actuals run 64.9–76.5%).

### Integrity-check whitelist

`Commands::Mvn` and `Commands::Mvnd` are intentionally omitted from `is_operational_command` in `src/main.rs`, matching the gradle precedent (`Commands::Gradlew` also omitted). The whitelist guards SHA-256 hook-integrity verification; filter modules invoked through an already-verified hook do not need a second check on their own dispatch path. Per the comment above the function, the whitelist is opt-in by design and a forgotten command fails open rather than creating false confidence about what's protected.

## Gradle (`gradlew_cmd.rs`)

See module docs and the gradle PR (`feat/gradlew-android-support`) for rationale. Streaming filter chosen because Gradle output is task-line-based, not block-based — unlike Maven Surefire.
