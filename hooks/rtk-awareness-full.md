# RTK

Prefix external programs with `rtk`: `rtk git status`, `rtk cargo test`,
`rtk npm run build`, `rtk ls src/`; keep prefixes inside chains such as `rtk git add . &&
rtk git commit -m "msg"`. Programs without an RTK filter run as-is, so prefixing from `PATH`
is safe. Do not prefix shell builtins or keywords: `test`, `[`, `read`, `cd`, `echo`, `export`,
`eval`, `source`, `printf`, or shadowed `env`; run the native command directly when needed.

# Command output

Command output here is condensed to save tokens, keeping every signal and
dropping costly noise. Treat it as the complete result: run commands
normally, and batch related commands into one call to avoid extra turns.
Truncated results state their recovery path in their own output. Re-run a
command as `rtk proxy <cmd>` only when its result is unusable: empty when
output was clearly expected, contradicting its exit code, or garbled.

## About RTK

RTK (Rust Token Killer) is a CLI proxy that filters command output to save tokens; behavior and exit code are unchanged.

- `rtk gain` / `rtk gain --history` — token savings, overall and per command.
- `rtk proxy <cmd>` — run a command unfiltered, still tracked.
- `RTK_DISABLED=1 <cmd>` — skip RTK for one command.
- `rtk discover` — find past commands RTK could have condensed.
