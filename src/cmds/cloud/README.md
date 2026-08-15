# Cloud and Infrastructure

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `aws_cmd.rs` — 25 specialized filters covering STS, S3, EC2, ECS, RDS, CloudFormation, CloudWatch Logs, Lambda, IAM, DynamoDB, EKS, SQS, Secrets Manager. Forces `--output json` for structured parsing, uses `force_tee_hint()` for truncation recovery, strips Lambda secrets. Shared runner `run_aws_filtered()` handles boilerplate for JSON-based filters; text-based filters (S3 ls, S3 sync/cp) have dedicated runners
- `container.rs` handles Docker, Kubernetes, and OpenShift; `DockerCommands`, `KubectlCommands`, and `OcCommands` sub-enums in `main.rs` route to `container::run()` -- uses passthrough for unknown subcommands
- `curl_cmd.rs` truncates long responses, saves full output to file for recovery
- `web_cmd.rs` — explicit `rtk web <url>` (#1426): fetches via `curl`, and for `text/html` responses strips script/style/nav/etc., extracts the main/article content, and converts to Markdown with de-duplicated numbered link references. Non-HTML content (JSON, XML, binary) passes through unchanged, same passthrough philosophy as `curl_cmd.rs`
- `wget_cmd.rs` wraps wget with output filtering
- `psql_cmd.rs` filters PostgreSQL query output
