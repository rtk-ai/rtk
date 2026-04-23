# Go Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `go_cmd.rs` uses `GoCommands` sub-enum in main.rs (same pattern as git/cargo)
- `go test` outputs NDJSON (`-json` flag injected by RTK) -- parsed line-by-line as streaming events
- `golangci_cmd.rs` forces `--out-format=json` for structured parsing
