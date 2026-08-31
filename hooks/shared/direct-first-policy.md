## Command Selection Priority

Use the narrowest RTK route that can perform the task:

1. **Direct RTK first**: use supported commands such as `rtk read`, `rtk rg`, `rtk grep`, `rtk find`, `rtk ls`, `rtk git ...`, `rtk cargo ...`, and `rtk gh ...`. Use `rtk --help` when unsure.
2. **CMD expressions on Windows**: use `rtk cmd "<CMD expression>"` (or the MCP `run_cmd` tool) when the task needs CMD operators, expansion, state, or control flow and safe filtering is useful.
3. **Executable fallback**: use `rtk proxy <program> <args>` only when RTK has no matching route or exact unfiltered output is required.
4. **Native shell fallback last**: use raw `cmd.exe /D /S /C ...`, `rtk proxy cmd.exe ...`, or `rtk proxy pwsh -NoProfile -Command ...` only for interactive, exact-output, redirected, machine-consumed, batch, or opaque shell behavior.

On Windows, prefer `rtk cmd` for optimizable CMD expressions and never hide an RTK-supported command inside a native shell. Use `rtk git status`, not `rtk proxy pwsh -Command "git status"`; use raw `cmd.exe` only when the native semantics must remain exact.
