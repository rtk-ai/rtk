# Ruby on Rails

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `rake_cmd.rs` filters Minitest output via `rake test` / `rails test`; state machine text parser, failures only (85-90% reduction)
- `rspec_cmd.rs` uses JSON injection (`--format json`) with text fallback; failures only (60%+ reduction)
- `rubocop_cmd.rs` uses JSON injection, groups by cop/severity (60%+ reduction)
- All three modules use `ruby_exec()` from `utils.rs` to auto-detect `bundle exec` when a Gemfile exists
- TOML filter `bundle-install.toml` strips `Using` lines from `bundle install`/`update` (90%+ reduction)

RSpec's injected JSON can share stdout with suite-hook warnings or `at_exit` coverage messages. The filter extracts a complete RSpec report before stripping noise lines, including text attached directly to the closing brace, and skips unrelated JSON log records. This prevents a coverage message from removing the report or a warning from making the entire JSON appear as the summary. Explicit formatters and unrecognized reports use the text fallback; text summaries must begin with the example and failure counts, so JSON fields cannot be mistaken for summary lines.

Regression tests cover passing, failing, and pending examples, colored warnings, CRLF prefixes, adjacent suffixes, and captured RSpec 3.13.6 output. Extraction requires a complete JSON object; it does not repair truncated reports.
