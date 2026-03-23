# Git and VCS

## Specifics

- **git.rs** uses `trailing_var_arg = true` + `allow_hyphen_values = true` so native git flags (`--oneline`, `--cached`, etc.) pass through correctly
- Auto-detects `--merges` flag to avoid conflicting with `--no-merges` injection
- Global git options (`-C`, `--git-dir`, `--work-tree`, `--no-pager`) are prepended before the subcommand
- Exit code propagation is critical for CI/CD pipelines

## Cross-command

- `gh_cmd.rs` imports markdown filtering helpers from `git.rs` for PR body rendering
- `diff_cmd.rs` is a standalone ultra-condensed diff (separate from `git diff`)
