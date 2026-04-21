# Git and VCS

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- **git.rs** uses `trailing_var_arg = true` + `allow_hyphen_values = true` so native git flags (`--oneline`, `--cached`, etc.) pass through correctly
- Auto-detects `--merges` flag to avoid conflicting with `--no-merges` injection
- Global git options (`-C`, `--git-dir`, `--work-tree`, `--no-pager`) are prepended before the subcommand
- Exit code propagation is critical for CI/CD pipelines
- **glab_cmd.rs** declares `-R`/`--repo` and `-g`/`--group` at the clap level; they are **appended** to the glab args (not prepended) so subcommand dispatch stays intact
- `has_output_flag()` short-circuits to passthrough when the user explicitly requests `-F` / `--output` / `--json` (avoids double JSON injection)
- `should_passthrough_view()` redirects `mr/issue view` to passthrough when `--web` or `--comments` is set
- JSON handlers use the local `run_glab_json<F>()` helper wrapping `runner::run_filtered` + `RunOptions::stdout_only().early_exit_on_failure().no_trailing_newline()`; on JSON parse error, falls back to the raw stdout (glab sometimes emits plain text for empty results)
- `ci status` uses text-keyword parsing (glab doesn't support `-F json` for this subcommand); when no English status keyword is recognized (non-English locale), returns raw verbatim
- `ci trace` uses ANSI-stripping + GitLab section-marker filtering + runner/git/artifact boilerplate removal; kept as text-only filter, not JSON
- `release list` falls back to raw output when the glab 1.82+ format doesn't match the legacy tab-delimited parser
- Pipeline / merge-status indicators use text tags (`[ok]`, `[fail]`, `[cancel]`, `[run]`, `[pend]`, `[skip]`, `[conflict]`) to match `gh_cmd.rs` and avoid multi-byte rendering quirks

## Cross-command

- `gh_cmd.rs` imports `compact_diff()` from `git.rs` for diff formatting; markdown helpers (`filter_markdown_body`, `filter_markdown_segment`) are defined in `gh_cmd.rs` itself
- `glab_cmd.rs` also uses `compact_diff()` from `git.rs` for `mr diff`; its `filter_markdown_body` is currently **duplicated** from `gh_cmd.rs` (shared-module refactor deferred)
- `diff_cmd.rs` is a standalone ultra-condensed diff (separate from `git diff`)

## glab vs gh JSON schema quick-ref

| Aspect | gh | glab |
|--------|----|------|
| Notation | `#42` | `!42` |
| States | `OPEN`/`MERGED`/`CLOSED` | `opened`/`merged`/`closed` |
| Author | `author.login` | `author.username` |
| URL field | `url` | `web_url` |
| Body field | `body` | `description` |
| Merge check | `mergeable` | `merge_status` (`can_be_merged` / `cannot_be_merged`) |
| CI status | `statusCheckRollup` | `head_pipeline.status` |
| Labels | `labels` (array of objects) | `labels` (array of strings) |
| Reviewers | `reviewRequests`/`reviews` | `reviewers` (array of objects with `username`) |
