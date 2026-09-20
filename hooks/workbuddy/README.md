# WorkBuddy

Native command rewriting for WorkBuddy desktop:

```sh
rtk init -g --agent workbuddy --auto-patch
rtk init -g --agent workbuddy --show
rtk init -g --agent workbuddy --uninstall
```

Restart WorkBuddy after installation or removal. Without `--auto-patch`, init asks before changing settings. `--dry-run` makes no changes; `--no-patch` skips installation.

## Scope

The installer manages only the RTK command entry in `~/.workbuddy/settings.json`, or `$WORKBUDDY_CONFIG_DIR/settings.json` when that override is set. Existing hooks, permissions and other settings are preserved; writes back up the original file first.

This first version is **global-only**. WorkBuddy 5.6.0 uses project `.codebuddy/settings.json`, not project `.workbuddy/settings.json`. That project surface is shared with CodeBuddy, so installing both adapters there could cause duplicate rewriting. No CodeBuddy settings, plugins, awareness files or project files are installed or removed.

## Protocol and permissions

`rtk hook workbuddy` reads `PreToolUse` input for `Bash` or `execute_command`, preserves extra `tool_input` fields, and emits:

```json
{
  "continue": true,
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecisionReason": "RTK auto-rewrite",
    "updatedInput": {"command": "rtk git status"}
  }
}
```

RTK does **not** set `permissionDecision`, approve commands, or read Claude's permission rules. WorkBuddy retains approval and sandbox enforcement. Its native rules may see the rewritten `rtk ...` command rather than the original executable; automatic approval equivalence is not promised.

Malformed input, other tools/events, already-prefixed commands and commands the shared rewrite engine cannot safely attest pass through with no stdout. RTK does not implement a separate WorkBuddy permission-rule parser.

## Verification boundary

The protocol was tested with WorkBuddy desktop 5.6.0 (bundled engine 2.147.0), using a project-local test hook and its existing full-access mode. Both a sentinel command and a real `git status` call were rewritten through `updatedInput` without `permissionDecision`. The latter invoked this Rust adapter and returned RTK's compact Git output. This does not establish default/ask/deny behavior in every WorkBuddy edition or version.

The global installer is covered by isolated CLI tests. Project config discovery and global config discovery are distinct: do not infer the former from the latter.

Prior art: [#2066](https://github.com/rtk-ai/rtk/pull/2066). This implementation uses the current shared hooks infrastructure instead of reviving that PR's CodeBuddy stack.
