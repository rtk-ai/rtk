# Git and VCS

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- **git.rs** uses `trailing_var_arg = true` + `allow_hyphen_values = true` so native git flags (`--oneline`, `--cached`, etc.) pass through correctly
- Default `git status` uses `--porcelain -b` so the compact output never exceeds raw `git status` (an untracked directory collapses to a single line, matching git's default); branch/short-only flags reuse the compact path, other explicit args still pass through unchanged
- Global git options (`-C`, `--git-dir`, `--work-tree`, `--no-pager`) are prepended before the subcommand
- Exit code propagation is critical for CI/CD pipelines
- **glab_cmd.rs** declares `-R`/`--repo` and `-g`/`--group` at the clap level; they are **appended** to the glab args (not prepended) so subcommand dispatch stays intact
- `has_output_flag()` short-circuits to passthrough when the user explicitly requests `-F` / `--output` / `--json` (avoids double JSON injection)
- `should_passthrough_view()` redirects `mr/issue view` to passthrough when `--web` or `--comments` is set. `mr view --comments` stays passthrough on purpose (an explicitly requested detail — see *Correctness VS Token Savings*), even though glab documents it as the same output format as `mr note list`
- **`mr note` is a command group, not a write command** — glab exposes `create`/`delete`/`list`/`reopen`/`resolve`/`update` under it. `run_mr_note()` dispatches them; treating the group as a single write was ISSUE #3531 (`glab mr note list` answered `ok noted !list`, destroying the output). Two traps to keep in mind when touching this:
  - the MR is the **first** positional even on the sub-commands that also take a note or discussion id (`glab mr note update <id> <note-id>`), whatever some glab builds print in their `--help` synopsis — `note_mr_ref()` encodes that, and returns `None` on a single positional since glab then resolves the MR from the branch
  - an **unrecognized first token goes to passthrough**, never to a confirmation: a bare word is either a branch name or a sub-command glab added after this code was written, and guessing wrong is the #3531 bug class. Only a known sub-command, a leading flag, or a numeric/URL MR id (`looks_like_mr_ref()`) reaches a confirmation
- `mr note list` reads the discussions as JSON on purpose, **not** glab's text output: that output is presentation and it churns (headers, note/discussion ids, absolute timestamps and system-note visibility all changed across recent glab releases), while the fields consumed here come from the GitLab discussions API and are pinned by the server. On a glab too old to know `note list`, glab's own error surfaces with its exit code
- It filters out `system: true` activity events ("assigned to @user", "added 2 commits") and reports them as `[+N activity events]` so the omission is never silent; `-F json` passes the full set through. Note bodies are rendered **complete** — a note is a reviewer's instruction, so it is never line-capped. Savings come from `collapse_details_blocks()`, which folds author-collapsed `<details>` blocks back to their `<summary>` label (bot release notes, ticket linkbacks, CI summaries); it runs before `filter_markdown_body()` because folded blocks embed code fences that the fence-aware segmentation would split
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
