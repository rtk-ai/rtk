![Ruby](https://img.shields.io/badge/Ruby-CC342D?logo=ruby&logoColor=white) ![Rails](https://img.shields.io/badge/Rails-D30001?logo=rubyonrails&logoColor=white)

# Summary

Unifies **5 competing PRs** (#198, #292, #379, #534, #643) into a single coherent Ruby on Rails implementation for RTK.

Adds **RSpec**, **RuboCop**, **Minitest** (via rake/rails test), and **Bundler** support with 3 new Rust command modules, 1 TOML filter, shared Ruby infrastructure, and automatic discover/rewrite rules.

Includes **56 unit tests** across the 3 modules and 4 inline TOML tests — all 1035 tests passing.

# New Commands

| Command | Supported Formats | Token Savings | Notes |
| :--- | :--- | :--- | :--- |
| **`rtk rspec`** | JSON, Text fallback | ![60%+ JSON](https://img.shields.io/badge/-60%25%2B_JSON-85E89D) ![30%+ text](https://img.shields.io/badge/-30%25%2B_text-FFD33D) | Injects `--format json` automatically. Falls back to text parsing when user specifies a custom format. |
| **`rtk rubocop`** | JSON, Autocorrect | ![60%+](https://img.shields.io/badge/-60%25%2B-85E89D) | Injects `--format json`, groups offenses by cop name and severity. Skips JSON in autocorrect mode (`-a`, `-A`). |
| **`rtk rake test`** | Text (state machine) | ![85-90%](https://img.shields.io/badge/-85--90%25-85E89D) | Parses Minitest output. Handles both standard and minitest-reporters formats. |
| **`rtk bundle install`** | TOML filter | ![90%+](https://img.shields.io/badge/-90%25%2B-85E89D) | Strips `Using` lines, short-circuits to `ok bundle: complete` on success. |

<details>
<summary><b>Key Design Decisions</b> (click to expand)</summary>

1. **JSON injection for rspec/rubocop** — Injects `--format json` unless user specified `-f`/`--format`/`-fj`/`--format=...`. Detects autocorrect mode (`-a`, `-A`) in rubocop to skip JSON.
2. **Noise stripping in rspec** — Strips Spring preloader, SimpleCov coverage reports, DEPRECATION warnings, `Finished in` timing, and Capybara screenshot details (keeps only path).
3. **3-tier JSON fallback in rspec** — Strip noise, parse JSON, try original, text parser, `fallback_tail()`. Logs serde error on final fallback for debugging.
4. **Safe JSON fallback in rubocop** — JSON parse failure uses `fallback_tail()` instead of feeding JSON through the text parser.
5. **State machine parsers** — Both rspec (text fallback) and minitest use state-machine text parsers for structured extraction.
6. **TOML for bundle** — `bundle install/update` has simple output with a `match_output` short-circuit (90%+ savings on success), making it a natural fit for the TOML DSL rather than a full Rust module.
7. **Defensive arithmetic** — `saturating_sub` throughout, graceful degradation on parse failure.
8. **Signal-aware exit codes** — `exit_code_from_output` returns `128 + signal` on Unix per convention.

</details>

<details>
<summary><b>Shared Infrastructure & Registry</b> (click to expand)</summary>

### Shared Infrastructure (`utils.rs`)
- **`ruby_exec(tool)`** — Auto-detects `bundle exec` when `Gemfile` exists in working directory. Transitive deps like `rake` (pulled in via `rails`) still go through bundler for version isolation.
- **`fallback_tail(output, label, n)`** — Last-resort filter fallback showing final N lines with diagnostic logging.
- **`exit_code_from_output(output, label)`** — Signal-aware exit code extraction: returns `128 + signal` on Unix per convention.
- **`count_tokens(text)`** — Shared test helper for token savings assertions.

### Discover Registry
- Detection patterns for `rspec`, `rubocop`, `rake test`, `rails test`, `bundle install/update` (with `bundle exec` and `bin/` variants)
- Rewrite prefixes cover all common invocation patterns including `bin/rspec`, `bin/rails test`, `bundle exec rake test`

</details>

# Hook Integration

The discover registry now correctly rewrites the following commands:

| Rewritten to | From input |
| :--- | :--- |
| `rtk rspec` | `rspec`, `bundle exec rspec`, `bin/rspec` |
| `rtk rubocop` | `rubocop`, `bundle exec rubocop` |
| `rtk rake test` | `rake test`, `rails test`, `bundle exec rake test`, `bundle exec rails test`, `bin/rails test` |
| `rtk bundle ...` | `bundle install`, `bundle update` |

# How to Test

```bash
# 1. Run unit tests (no Ruby required)
cargo test --all

# 2. Run Ruby-specific tests only
cargo test rspec_cmd    # 28 tests
cargo test rubocop_cmd  # 18 tests
cargo test rake_cmd     # 10 tests

# 3. Build and install locally
cargo install --path .

# 4. Run E2E smoke tests (~60-120s, requires Ruby/Bundler/Rails)
bash scripts/test-ruby.sh
# Creates a temp Rails app, exercises all 4 commands:
# rspec (JSON/text fallback, pending, empty suite, failure caps)
# rubocop (JSON, autocorrect, offenses)
# rake test (minitest pass/fail, exit codes)
# bundle install (TOML filter, idempotent)
# + exit code preservation, bundle exec variants, token savings checks
```

# Attribution

> [!NOTE]
> This PR unifies 5 competing implementations. Below is what was taken from each and why.

### PR #198 (by @deril) — RSpec only
- **Incorporated**: `#[serde(default)]` on `backtrace` field — reviewer-requested fix for RSpec versions that omit backtrace from JSON
- **Not taken**: Simpler rspec implementation — superseded by #292/#643's more robust version with noise stripping and state-machine text parser

### PR #292 (by @navidemad) — RSpec + RuboCop
- **Incorporated**: **Primary source** for `rspec_cmd.rs`, `rubocop_cmd.rs`, and shared utils (`ruby_exec`, `fallback_tail`, `exit_code_from_output`, `count_tokens`) — the most mature implementations with noise-stripping regex, 3-tier JSON fallback, state-machine text parser, and signal-aware exit codes
- **Not taken**: E2E smoke test script (`test-ruby.sh`) — requires Rails installed on CI

### PR #379 (by @navidemad) — Bundle + Rails (TOML DSL hybrid)
- **Incorporated**: The **TOML filter concept** for `bundle install/update` — simpler than a full Rust module for low-savings commands
- **Not taken**: The 7 Rails TOML filters (db:migrate, generate, etc.) — lower savings (10-40%) and tightly coupled to TOML DSL internals. Also `rails_cmd.rs` routes/other routing — too complex for initial merge

### PR #534 (by @cosgroveb) — RSpec with parser trait
- **Incorporated**: **Improved format flag detection** — handles `-fj`, `-fjson`, `-fdocumentation`, `--format=...` patterns that the other PRs missed, plus 4 dedicated tests
- **Not taken**: `parser` module trait-based architecture (adds indirection without benefit for standalone modules), tempfile `--out` approach (adds complexity and temp file cleanup), `Gemfile.lock` detection (the `Gemfile` check is simpler and covers the same cases)

### PR #643 (by @Maimer) — Most Complete
- **Incorporated**: `rake_cmd.rs` (**unique** to this PR — only implementation of Minitest parsing), discover rules structure (most comprehensive), `bundle-install.toml`. **Preferred as base** when approaches conflicted.

### Summary

| Module | Source PRs | Tests |
| :--- | :--- | :--- |
| `rspec_cmd.rs` | #292/#643 + #534 format detection + #198 serde fix | 28 |
| `rubocop_cmd.rs` | #292/#643 | 18 |
| `rake_cmd.rs` | #643 (unique) | 10 |
| `bundle-install.toml` | #643 + #379 (concept) | 4 |
| `utils.rs` additions | #292/#643 | — |
| `discover/rules.rs` | #643 (most complete) | — |

Closes #292, #379

Based on work by @deril (#198), @cosgroveb (#534), and @Maimer (#643) — thank you for your contributions.
