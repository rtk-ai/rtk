# WorkBuddy

Native command rewriting for WorkBuddy desktop:

```sh
rtk init -g --agent workbuddy --auto-patch
rtk init -g --agent workbuddy --show
rtk init -g --agent workbuddy --uninstall
```

Restart WorkBuddy after installation or removal. Without `--auto-patch`, init asks before changing settings. `--dry-run` makes no changes; `--no-patch` skips installation.

## Scope

The installer manages only the RTK command entry in `~/.workbuddy/settings.json`, or `$WORKBUDDY_CONFIG_DIR/settings.json` when that override is nonempty. The override selects the file RTK manages; it does not configure WorkBuddy to discover that directory. Existing hooks, permissions and other settings are preserved; writes back up the original file first.

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

Malformed input (including invalid UTF-8 or input over 1 MiB), other tools/events, already-prefixed commands and commands the shared rewrite engine cannot safely attest pass through with no stdout and exit status 0. RTK does not implement a separate WorkBuddy permission-rule parser.

## Verification boundary

The protocol was tested with WorkBuddy desktop 5.6.0 (bundled engine 2.147.0), using a project-local test hook and its existing full-access mode. Both a sentinel command and a real `git status` call were rewritten through `updatedInput` without `permissionDecision`. The latter invoked this Rust adapter and returned RTK's compact Git output. This does not establish default/ask/deny behavior in every WorkBuddy edition or version.

Isolated CLI tests cover the default global path (Unix), empty and explicit overrides, install/status/uninstall, preservation of project settings, and the absence of approval decisions under Claude default/allow/ask/deny rules. These test RTK's file selection and protocol only. `--show` reports registration in the selected file; it does not prove that the desktop loaded it. Global discovery and WorkBuddy's native default/ask/deny behavior remain unverified.

### Remaining desktop checks

Use a disposable project and record the WorkBuddy/engine versions, selected global settings path, and native approval mode for each check:

1. Install globally, restart WorkBuddy, and request `git status` in a project with no project-local rewrite hook. Confirm from the app's tool trace that the original command was rewritten and RTK produced the result. A project hook or a direct `rtk hook workbuddy` invocation is not evidence of global discovery.
2. Compare the same harmless command before and after installation under native default, ask, and deny policies. For ask, confirm no execution before approval; for deny, confirm no execution. Check policies for both `git status` and its rewritten `rtk git status` form, since matching may occur after the rewrite. Do not infer enforcement from the adapter's JSON alone.
3. Uninstall, restart, and repeat the command to confirm rewriting stops and unrelated hooks still work.

Do not enable full-access mode to count default/ask/deny checks as passed. Keep failed or unavailable checks explicitly unverified.

Prior art: [#2066](https://github.com/rtk-ai/rtk/pull/2066). This implementation uses the current shared hooks infrastructure instead of reviving that PR's CodeBuddy stack.
