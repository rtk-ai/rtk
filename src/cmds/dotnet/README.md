# .NET Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `dotnet_cmd.rs` uses the `DotnetCommands` sub-enum in `src/lib.rs`.
- Internal helper modules (`dotnet_trx.rs`, `dotnet_format_report.rs`, `binlog.rs`) are only used by `dotnet_cmd.rs` -- they parse specialized .NET output formats (TRX XML, binary logs, format reports)
- `DotnetCommands::Tool` routes to `dotnet_cmd::run_tool`, which filters `list` through `dotnet_tool_list.rs` and passes other tool operations through unchanged. Native `OsString` arguments and the shared separator-restoration helper preserve `--` and non-UTF-8 paths.
- JSON and help use inherited streams. Text filtering uses verbatim capture with cancellation relay, preserving raw stdout/stderr on fallback and failure.
- Test fixtures are in `tests/fixtures/dotnet/` (JSON and text formats)

## Tool lists

```bash
rtk dotnet tool list
rtk dotnet tool list --global
rtk dotnet tool list --tool-path "/path/to/tool store"
rtk dotnet tool list --format json  # SDKs supporting JSON: output stays unchanged
```

Tool lists retain every package ID, version, command name, and local manifest path.
Table padding and separator lines are removed; columns are separated by tabs.
The hook rewrites `dotnet tool list` automatically. Other tool operations pass
through unchanged. Empty lists, help, failed commands, and unrecognized output
are preserved without filtering.

Legacy and current text tables use the same three global columns or four local
columns. Explicit `--format json` / `--format=json` output is preserved byte for
byte; RTK does not inject JSON options, so SDKs without JSON support still work.
Tables with Unicode in a column before the final column are passed through:
Microsoft pads by Unicode text elements, which cannot safely be sliced by byte
offsets. Unicode in the final command or manifest column is retained.

JSON is streamed without buffering or a capture-size limit. Raw text fallbacks,
failure output, and stderr retain CRLF and a missing final newline. The .NET
rewrite rule is shared with build/restore, with a separate savings estimate for
tool lists.

The base tables, JSON, and Unicode fixtures were captured with .NET SDK 10.0.400
on macOS. Temporary local manifest roots were normalized to `/workspace`,
including a directory containing spaces. Trailing table padding was stripped.

The `tool_list_sdk8_*.txt` fixtures were rendered by the unmodified
[SDK 8.0.100 PrintableTable implementation](https://github.com/dotnet/sdk/blob/v8.0.100/src/Cli/dotnet/PrintableTable.cs)
in a temporary .NET 10 harness, using the columns and command delimiter from
the SDK 8 tool list commands. They cover multiple global commands and Windows
manifest paths; they are not captures from an installed SDK 8 runtime.
JSON compatibility is also checked against the
[SDK 9.0.100 contract](https://github.com/dotnet/sdk/blob/v9.0.100/src/Cli/dotnet/commands/dotnet-tool/list/ToolListJsonHelper.cs).

Native CLI comparisons were also run with SDK 8.0.426 and 10.0.400 on macOS for
local lists, custom paths, help, missing packages, unsupported flags, and JSON
requests. SDK 8's unsupported JSON option is forwarded unchanged. Replay tests
are enabled on Unix and Windows and cover CRLF, missing final newlines, and
output above 10 MiB; Unix additionally checks the exact native argument bytes.
