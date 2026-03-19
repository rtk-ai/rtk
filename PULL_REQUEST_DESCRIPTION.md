![Ruby](https://img.shields.io/badge/Ruby-CC342D?logo=ruby&logoColor=white) ![Rails](https://img.shields.io/badge/Rails-D30001?logo=rubyonrails&logoColor=white)

# Summary

Unifies **5 competing PRs** (#198, #292, #379, #534, #643) into a single coherent Ruby on Rails implementation for RTK.

Adds **RSpec**, **RuboCop**, **Minitest** (via rake/rails test), and **Bundler** support with 3 new Rust command modules, 1 TOML filter, shared Ruby infrastructure, and automatic discover/rewrite rules.

Includes **56 unit tests** across the 3 modules, 4 inline TOML tests, and **23 E2E assertions** in `scripts/test-ruby.sh` — all 1035 tests passing.

# New Commands

| Command | Savings | Notes |
| :--- | :--- | :--- |
| **`rtk rspec`** | 60%+ | JSON auto-inject (`--format json`), text fallback with state-machine parser |
| **`rtk rubocop`** | 60%+ | JSON auto-inject, groups by cop/severity, skips JSON in autocorrect mode |
| **`rtk rake test`** | 85%+ | Minitest text parser, handles standard and minitest-reporters formats |
| **`rtk bundle install`** | 90%+ | TOML filter, short-circuits to `ok bundle: complete` on success |

<details>
<summary><b>Key Design Decisions</b> (click to expand)</summary>

<br>

| Decision | Detail |
| :--- | :--- |
| JSON injection | Injects `--format json` for rspec/rubocop unless user specified `-f`/`--format`/`-fj`/`--format=...`. Skips in rubocop autocorrect mode (`-a`, `-A`). |
| Noise stripping (rspec) | Strips Spring preloader, SimpleCov, DEPRECATION warnings, `Finished in` timing, Capybara screenshots (keeps path only). |
| 3-tier JSON fallback (rspec) | Strip noise → parse JSON → try original → text parser → `fallback_tail()`. Logs serde error on final fallback. |
| Safe JSON fallback (rubocop) | JSON parse failure uses `fallback_tail()` instead of feeding JSON through the text parser. |
| State machine parsers | Both rspec (text fallback) and minitest use state-machine text parsers for structured extraction. |
| TOML for bundle | `bundle install/update` uses `match_output` short-circuit (90%+ savings on success) — natural fit for TOML DSL over a full Rust module. |
| Defensive arithmetic | `saturating_sub` throughout, graceful degradation on parse failure. |
| Signal-aware exit codes | `exit_code_from_output` returns `128 + signal` on Unix per convention. |

</details>

<details>
<summary><b>Shared Infrastructure & Registry</b> (click to expand)</summary>

<br>

| Function | Purpose |
| :--- | :--- |
| `ruby_exec(tool)` | Auto-detects `bundle exec` when `Gemfile` exists. Transitive deps like `rake` still go through bundler. |
| `fallback_tail(output, label, n)` | Last-resort filter fallback showing final N lines with diagnostic logging. |
| `exit_code_from_output(output, label)` | Signal-aware exit code extraction: returns `128 + signal` on Unix. |
| `count_tokens(text)` | Shared test helper for token savings assertions. |

**Discover Registry**: Detection patterns for `rspec`, `rubocop`, `rake test`, `rails test`, `bundle install/update` (with `bundle exec` and `bin/` variants).

</details>

# Hook Integration

The discover registry now correctly rewrites the following commands:

| Rewritten to | From input |
| :--- | :--- |
| `rtk rspec` | `rspec` <br> `bundle exec rspec` <br> `bin/rspec` |
| `rtk rubocop` | `rubocop` <br> `bundle exec rubocop` |
| `rtk rake test` | `rake test` <br> `rails test` <br> `bundle exec rake test` <br> `bundle exec rails test` <br> `bin/rails test` |
| `rtk bundle ...` | `bundle install` <br> `bundle update` |

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

# Documentation

- **README.md**: Ruby commands added to usage examples and hook rewrite table
- **CHANGELOG.md**: Unreleased section with all Ruby features (rspec, rubocop, rake, bundle)
- **ARCHITECTURE.md**: Ruby module table (rake, rspec, rubocop)

# Attribution

> [!NOTE]
> This PR unifies 5 competing implementations. Below is what was taken from each and why.

### PR #198 (by @deril) — RSpec only

**Incorporated**: `#[serde(default)]` on `backtrace` field — reviewer-requested fix for RSpec versions that omit backtrace from JSON.

**Not taken**: Simpler rspec implementation — superseded by #292/#643's more robust version.

---

### PR #292 (by @navidemad) — RSpec + RuboCop

**Incorporated**:
- **Primary source** for `rspec_cmd.rs`, `rubocop_cmd.rs`, and shared utils (`ruby_exec`, `fallback_tail`, `exit_code_from_output`, `count_tokens`)
- Noise-stripping regex, 3-tier JSON fallback, state-machine text parser, signal-aware exit codes
- E2E smoke test script (`test-ruby.sh`) — used as **base** for the unified `scripts/test-ruby.sh`

---

### PR #379 (by @navidemad) — Bundle + Rails (TOML DSL hybrid)

**Incorporated**:
- The **TOML filter concept** for `bundle install/update` — simpler than a full Rust module for low-savings commands
- E2E smoke test script (`test-bundle-rails.sh`) — contributed the rake test and bundle install sections to `scripts/test-ruby.sh`

**Not taken**: The 7 Rails TOML filters (db:migrate, generate, etc.) — lower savings (10-40%) and tightly coupled to TOML DSL internals. Also `rails_cmd.rs` routes/other routing — too complex for initial merge.

---

### PR #534 (by @cosgroveb) — RSpec with parser trait

**Incorporated**: **Improved format flag detection** — handles `-fj`, `-fjson`, `-fdocumentation`, `--format=...` patterns that the other PRs missed, plus 4 dedicated tests.

**Not taken**: `parser` module trait-based architecture (adds indirection without benefit for standalone modules), tempfile `--out` approach (adds complexity), `Gemfile.lock` detection (the `Gemfile` check is simpler).

---

### PR #643 (by @Maimer) — Most Complete

**Incorporated**: `rake_cmd.rs` (**unique** to this PR — only implementation of Minitest parsing), discover rules structure (most comprehensive), `bundle-install.toml`, ARCHITECTURE.md Ruby Module Architecture section and CLAUDE.md module table/fork-features updates. **Preferred as base** when approaches conflicted.

### Summary

| Module | Source PRs | Tests |
| :--- | :--- | :--- |
| **Filters** | | |
| `rspec_cmd.rs` | #292/#643 <br> + #534 format detection <br> + #198 serde fix | 28 |
| `rubocop_cmd.rs` | #292/#643 | 18 |
| `rake_cmd.rs` | #643 (unique) | 10 |
| `bundle-install.toml` | #643 <br> + #379 (concept) | 4 |
| **Infrastructure** | | |
| `utils.rs` additions | #292/#643 | — |
| `discover/rules.rs` | #643 (most complete) | — |
| **Testing** | | |
| `scripts/test-ruby.sh` | #292 (base) <br> + #379 (rake/bundle sections) | 23 E2E assertions |
| `scripts/test-all.sh` | #292/#643 <br> (Ruby conditional section) | 3 smoke checks |
| **Docs** | | |
| `README.md`, `CHANGELOG.md` | — | — |
| `ARCHITECTURE.md`, `CLAUDE.md` | #643 (Ruby architecture section, module table) | — |

Closes #292, #379

Based on work by @deril (#198), @cosgroveb (#534), and @Maimer (#643) — thank you for your contributions.
