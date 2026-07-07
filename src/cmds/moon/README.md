# MoonBit (moon) ecosystem

> Part of [`src/cmds/`](../README.md) — MoonBit language build tool

## Tools

| Module | Tool | Subcommands |
|--------|------|-------------|
| `moon_cmd.rs` | `moon` (MoonBit CLI) | build, test, check, run |

## Strategy

MoonBit's CLI (`moon`) supports `--output-json` for machine-parseable output (NDJSON).
The filter injects this flag automatically for `build`, `test`, and `check`, then:

1. **NDJSON parsing**: each diagnostic JSON line is parsed and reformatted as compact
   one-liners in Rust-compiler style: `path:line:col: {level} [{code}]: {message}`
2. **Passthrough**: non-JSON lines (warning headers, summary) pass through unchanged
3. **No filtering for**: `run` (app-specific output), other subcommands like `new`/`clean`/`doc`/`fmt`

## Compact output (post-filter)

```
Warning: Package `user/example` does not declare `supported_targets`...
external/examples/example/lib/moon.pkg:10:3: warning [29]: Unused package 'moonbitlang/async'
external/extension/myapp-ext-bash/src/myapp_ext_bash.mbt:52:44: warning [2]: Unused variable 'e'
Finished. moon: ran 5 tasks, now up to date (9 warnings, 0 errors)
```
