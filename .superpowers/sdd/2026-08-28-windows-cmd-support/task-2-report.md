# Task 2 Report: Public CLI and CMD Orchestration

## Status

Complete. The public `rtk cmd` route now executes CMD expressions through a
resolved `cmd.exe`; a hidden `__cmd-run` route executes encoded identity
segments without recursive orchestration.

## Commit

`feat(windows): add cmd orchestration` (the containing commit)

## Files Changed

- `src/main.rs`: adds public `cmd` and hidden `__cmd-run` clap routes, dispatch,
  parsing coverage, and meta-command validation handling.
- `src/core/constants.rs`: classifies both CMD routes as RTK-only commands.
- `src/cmds/windows/mod.rs`: exposes the orchestration module.
- `src/cmds/windows/orchestrator.rs`: normalizes invocations, reconstructs
  multi-argument expressions, applies catalog/parser-safe rewriting, and runs
  both public and hidden CMD routes.
- `src/cmds/windows/tests.rs`: covers invocation normalization, source-span
  preservation, stateful pass-through, opaque fail-open, percent-expansion
  fail-open, and hidden-runner rewrite output.
- `tests/windows_cmd_e2e.rs`: native-vs-RTK Windows parity tests for chains,
  state, variables, Unicode/spaces, redirection, batch invocation, and failures.

## TDD Evidence

1. RED: `rtk test cargo test test_cmd_accepts_a_raw_cmd_expression --bin rtk`
   initially failed because Clap had no `cmd` subcommand: `0 passed; 1 failed`.
2. GREEN: the same focused command passed after adding the public route:
   `1 passed; 0 failed`.
3. RED: `rtk test cargo test --test windows_cmd_e2e` initially reported native
   parity failures for the new path (`0 passed; 3 failed`).
4. GREEN: after preserving the normal current executable path, preserving
   segment trailing whitespace, and failing open for percent expansion, the
   parity command passed: `3 passed; 0 failed`.
5. A complete-suite red check caught the required meta-command-test update:
   `2726 passed; 1 failed; 8 ignored`; its focused regression command then
   passed: `1 passed; 0 failed`.

## Verification Commands and Exact Summaries

- `rtk cargo fmt --check` — passed (exit 0).
- `rtk test cargo test "cmds::windows::tests::" --bin rtk` — `16 passed; 0 failed`.
- `rtk test cargo test --test windows_cmd_e2e` — `3 passed; 0 failed`.
- `rtk test cargo test "test_cmd_" --bin rtk` — `2 passed; 0 failed`.
- `rtk test cargo test test_meta_commands_reject_bad_flags --bin rtk` —
  `1 passed; 0 failed`.
- `rtk test cargo test --all` — main unit suite `2727 passed; 0 failed; 8
  ignored`; all remaining test targets also passed, including the Windows CMD
  suite `3 passed; 0 failed`.
- `rtk git diff --check` — passed (exit 0).

## Self-Review

- The Task 1 parser is the only syntax classifier. Any `opaque_reason` returns
  the original expression without even partial rewriting.
- Query built-ins are the only rewritten catalog class. Mutation, stateful,
  control, interactive, and unknown commands stay in the parent CMD process.
- Percent-expansion expressions also fail open. This prevents child CMD from
  changing expansion timing after an earlier `set` command.
- Hidden runner payloads are hexadecimal UTF-8 source, so rewritten execution
  does not expose original metacharacters, spaces, quotes, or variables to the
  parent CMD parser a second time.
- The public compound route does not record savings. The hidden runner is the
  future sole accounting boundary, so Task 2 cannot double-count chains.
- Both routes use `resolve_binary("cmd.exe")`; `status()` inherits stdout,
  stderr, encoding, and console handles and returns the native exit code.

## Concerns

- Structured filters and per-segment savings accounting are intentionally not
  implemented until Task 3. Hidden runners are identity execution only.
- Multi-argument transport starts a nested CMD process to establish a safe
  delayed-expansion boundary. Raw one-expression input remains the route for
  exact parent-CMD control-flow semantics.

## Fix Round 1

### Changed Behavior

- `RedirectInput` (`<`) now has the same full-expression native fallback as
  output redirection. It is enforced from Task 1's parsed operator list even
  though the parser correctly leaves `opaque_reason` unset for input redirects.
- Multi-argument input no longer interpolates arguments into a CMD source
  string. It now passes each argument in a per-child environment variable and
  invokes CMD with `/V:ON` and `!RTK_CMD_ARG_n!` tokens. Delayed expansion is
  late enough that embedded quotes and `&|<>` remain argument data rather than
  changing command syntax. At this Fix Round 1 point, CR, LF, and `!` were
  rejected; Fix Round 2 removes the unnecessary bang restriction.

### Direct Coverage

- `rewrite_fails_open_for_input_redirection_even_when_the_parser_is_not_opaque`
  checks that Task 1 returns `RedirectInput` without an opaque reason and that
  orchestration nevertheless leaves `type < input.txt` unchanged.
- `redirection_and_batch_input_fail_open_to_native_cmd` now compares native CMD
  and `rtk cmd` stdout, stderr, and exit status for `type < input.txt`.
- `public_cmd_transports_embedded_quotes_and_cmd_metacharacters_as_data`
  asserts the environment transport shape for an argument containing an
  embedded quote, `&`, and `>`.
- `multi_argument_embedded_quote_and_metacharacters_do_not_execute_an_extra_command`
  supplies that adversarial argument to the public CLI and verifies that its
  redirected marker file is never created.

### TDD Red/Green Evidence

1. RED: `rtk test cargo test
   rewrite_fails_open_for_input_redirection_even_when_the_parser_is_not_opaque
   --bin rtk` reported `0 passed; 1 failed` before `RedirectInput` was added to
   the orchestration fallback gate.
2. GREEN: the same focused command reported `1 passed; 0 failed` after the
   fallback gate was added.
3. RED: `rtk test cargo test
   public_cmd_uses_caret_escapes_for_embedded_quotes_and_cmd_metacharacters
   --bin rtk` reported `0 passed; 1 failed`, and `rtk test cargo test --test
   windows_cmd_e2e
   multi_argument_embedded_quote_and_metacharacters_do_not_execute_an_extra_command`
   reported `0 passed; 1 failed` while the backslash-based reconstruction was
   still reachable.
4. GREEN: the final direct checks were `1 passed; 0 failed` for
   `public_cmd_transports_embedded_quotes_and_cmd_metacharacters_as_data`,
   `rewrite_fails_open_for_input_redirection_even_when_the_parser_is_not_opaque`,
   and the adversarial Windows E2E test.

### Fix Round 1 Verification

- `rtk cargo fmt --check` — passed (exit 0).
- `rtk test cargo test "cmds::windows::tests::" --bin rtk` — `18 passed; 0 failed`.
- `rtk test cargo test --test windows_cmd_e2e` — `4 passed; 0 failed`.
- `rtk test cargo test --all` — main unit suite `2729 passed; 0 failed; 8
  ignored`; all remaining targets passed, including `4 passed; 0 failed` for
  the Windows CMD E2E suite.

## Fix Round 2

### Changed Behavior

- Multi-argument execution again uses exactly the public default CMD switches:
  `/D /S /C`. The transport no longer supplies `/V:ON`.
- Transport tokens use `%RTK_CMD_ARG_n%` with per-child environment values.
  This retains embedded quotes and metacharacters as data without turning on
  delayed expansion, so `!` is a valid literal argument value.
- Empty positional arguments are emitted as the explicit CMD token `""` rather
  than expanding to an empty gap, preserving `rtk cmd echo ""` parity.
- The `RedirectInput` fallback from Fix Round 1 remains unchanged.

### Direct Coverage

- `public_cmd_transport_preserves_empty_and_bang_arguments_without_delayed_expansion`
  checks `%` transport tokens, the explicit empty token, and a literal bang.
- `multi_argument_embedded_quote_and_metacharacters_do_not_execute_an_extra_command`
  now asserts the exact echo payload in addition to verifying that no marker
  file was created.
- `multi_argument_empty_and_bang_values_match_default_cmd_semantics` compares
  status, stdout, and stderr against native `/D /S /C` for empty and bang
  values.

### TDD Red/Green Evidence

1. RED: `rtk test cargo test
   public_cmd_transport_preserves_empty_and_bang_arguments_without_delayed_expansion
   --bin rtk` reported `0 passed; 1 failed` while the `/V:ON` transport rejected
   `!` and produced delayed-expansion tokens.
2. RED: `rtk test cargo test --test windows_cmd_e2e
   multi_argument_empty_and_bang_values_match_default_cmd_semantics` reported
   `0 passed; 1 failed` before empty-token and delayed-expansion behavior was
   corrected.
3. GREEN: each focused command reported `1 passed; 0 failed` after switching
   to percent transport, removing `/V:ON`, and representing empty arguments as
   `""`.

### Fix Round 2 Verification

- `rtk cargo fmt` — passed (exit 0).
- `rtk test cargo test "cmds::windows::tests::" --bin rtk` — `19 passed; 0 failed`.
- `rtk test cargo test --test windows_cmd_e2e` — `5 passed; 0 failed`.
- `rtk test cargo test --all` — main unit suite `2730 passed; 0 failed; 8
  ignored`; all remaining targets passed, including `5 passed; 0 failed` for
  the Windows CMD E2E suite.

## Fix Round 3

### Changed Behavior

- Removed the shell-injectable `%RTK_CMD_ARG_n%` transport. Untrusted values
  are no longer percent-expanded into the outer CMD command line.
- The public multi-argument path still invokes its resolved outer CMD with the
  required exact switches `/D /S /C`. Its expression launches a nested
  `cmd.exe /D /S /V:ON /C` only after the outer parser, whose delayed expansion
  is off by default, has received literal `!RTK_CMD_ARG_n!` tokens.
- The nested command expands each environment transport value after parsing,
  preserving literal `%`, `!`, embedded quotes, `&|<>^`, empty values, and
  CR/LF as data. Input-redirection fallback remains intact.

### Safe Probe and Direct Coverage

- `multi_argument_percent_and_crlf_payloads_remain_data` is a real Windows
  probe: a percent-bearing `& ... > marker` value and a CR/LF-bearing value
  with the same redirected marker syntax both produce their exact echo payload
  and leave marker files absent.
- `multi_argument_embedded_quote_and_metacharacters_do_not_execute_an_extra_command`
  continues to require exact stdout and no marker file for quote plus `&` and
  `>` input.
- `multi_argument_empty_and_bang_values_match_default_cmd_semantics` compares
  status, stdout, and stderr with native `/D /S /C` for empty and literal bang
  values.
- `public_cmd_transport_preserves_percent_and_crlf_values` verifies the
  generated nested delayed-expansion command and unmodified environment values.

### TDD Red/Green Evidence

1. RED: `rtk test cargo test
   public_cmd_transport_preserves_percent_and_crlf_values --bin rtk` reported
   `0 passed; 1 failed` while percent transport was still generated and CR/LF
   was rejected.
2. RED: `rtk test cargo test --test windows_cmd_e2e
   multi_argument_percent_and_crlf_payloads_remain_data` reported `0 passed; 1
   failed` before the nested delayed-expansion boundary was introduced.
3. GREEN: the unit probe reported `1 passed; 0 failed`; the Windows percent and
   CR/LF probe reported `1 passed; 0 failed`; focused quote/metacharacter and
   empty/bang parity probes also each reported `1 passed; 0 failed`.

### Fix Round 3 Verification

- `rtk cargo fmt` — passed (exit 0).
- `rtk test cargo test "cmds::windows::tests::" --bin rtk` — `20 passed; 0 failed`.
- `rtk test cargo test --test windows_cmd_e2e` — `6 passed; 0 failed`.
- `rtk test cargo test --all` — main unit suite `2731 passed; 0 failed; 8
  ignored`; all remaining targets passed, including `6 passed; 0 failed` for
  the Windows CMD E2E suite.

## Fix Round 4

### Changed Behavior

- Public `rtk cmd` resolves `cmd.exe` once, then uses that absolute path both
  to launch the required outer `/D /S /C` process and to construct the nested
  delayed-expansion command. Nested execution no longer depends on a separate
  PATH lookup for bare `cmd.exe`.
- The path is rendered as a CMD token using the existing path-quoting helper;
  a path with spaces remains one executable token across the outer parser.
- Input-redirection fail-open behavior remains unchanged.

### Direct Coverage

- `public_cmd_transport_does_not_emit_a_bare_nested_cmd_executable` supplies a
  fake resolved path containing spaces and asserts the exact quoted nested CMD
  expression.
- `multi_argument_pipe_input_redirection_and_caret_payloads_remain_data` runs
  real Windows E2E probes for `|`, `<`, and `^` payloads. Each asserts exact
  stdout and verifies pipe, input-redirection, and caret marker files are not
  created.

### TDD Red/Green Evidence

1. RED: `rtk test cargo test
   public_cmd_transport_does_not_emit_a_bare_nested_cmd_executable --bin rtk`
   reported `0 passed; 1 failed` while the transport emitted bare `cmd.exe`.
2. GREEN: the same focused test reported `1 passed; 0 failed` after resolution
   was performed once in `run` and the absolute path was passed to the
   testable invocation preparation function.
3. GREEN: `rtk test cargo test --test windows_cmd_e2e
   multi_argument_pipe_input_redirection_and_caret_payloads_remain_data`
   reported `1 passed; 0 failed`.

### Fix Round 4 Verification

- `rtk cargo fmt` — passed (exit 0).
- `rtk test cargo test "cmds::windows::tests::" --bin rtk` — `21 passed; 0 failed`.
- `rtk test cargo test --test windows_cmd_e2e` — `7 passed; 0 failed`.
- `rtk test cargo test --all` — main unit suite `2732 passed; 0 failed; 8
  ignored`; all remaining targets passed, including `7 passed; 0 failed` for
  the Windows CMD E2E suite.
