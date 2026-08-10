# .NET Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `dotnet_cmd.rs` uses `DotnetCommands` sub-enum in main.rs
- Internal helper modules (`dotnet_trx.rs`, `dotnet_format_report.rs`, `binlog.rs`) are only used by `dotnet_cmd.rs` -- they parse specialized .NET output formats (TRX XML, binary logs, format reports)
- Test fixtures are in `tests/fixtures/dotnet/` (JSON and text formats)
