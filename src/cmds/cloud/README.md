# Cloud and Infrastructure

## Specifics

- `aws_cmd.rs` forces `--output json` for structured parsing
- `container.rs` handles both Docker and Kubernetes; `DockerCommands` and `KubectlCommands` sub-enums in `main.rs` route to `container::run()` -- uses passthrough for unknown subcommands
- `curl_cmd.rs` auto-detects JSON responses and shows schema (structure without values)
- `wget_cmd.rs` wraps wget with output filtering
- `psql_cmd.rs` filters PostgreSQL query output
