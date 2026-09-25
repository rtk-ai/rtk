# Go Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `go_cmd.rs` uses `GoCommands` sub-enum in main.rs (same pattern as git/cargo)
- `go test` outputs NDJSON (`-json` flag injected by RTK) -- parsed line-by-line as streaming events
- `golangci_cmd.rs` forces `--out-format=json` for structured parsing
- `buf` is standalone (`rtk buf`) and also reached from `go tool buf` through `go_cmd`'s tool interception
- buf `lint`/`build`/`breaking` get `--error-format=json` injected before `--` and are grouped by rule; COMPILE errors group on the message with identifiers masked, so one missing import's cascade collapses into two groups, root cause first. `build` reports on stderr, so it filters the combined stream
- buf `format -d` becomes a per-file `+/-` summary with the full diff in recall; `generate` failures keep a plugin panic's first frame only
- the hook rewrite needs the subcommand first: `buf --debug lint` is not auto-rewritten (typing `rtk buf --debug lint` still filters)
