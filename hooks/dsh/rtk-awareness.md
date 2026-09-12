<!-- rtk-dsh-instructions -->
## RTK command output

RTK compresses command output. When `rtk` is available, prefer its supported
commands through the existing shell tool, for example:

```sh
rtk git status
rtk git diff
rtk cargo test
rtk pytest -q
```

Use `rtk --help` to check supported commands. Leave unsupported commands unchanged;
do not blindly prefix every shell command or add a second `rtk` prefix.
When exact, complete output is needed, use `rtk proxy <command> [args...]`.
If RTK is unavailable, use the original command. Keep the requested working
directory, timeout, background execution, and sandbox/approval settings.
RTK does not grant permission or bypass a denial. Use `rtk gain` to inspect savings.

This is instruction-based guidance; commands are not automatically rewritten.
<!-- /rtk-dsh-instructions -->
