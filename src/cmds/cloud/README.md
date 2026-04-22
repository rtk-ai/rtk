# Cloud and Infrastructure

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `aws_cmd.rs` — 25 specialized filters covering STS, S3, EC2, ECS, RDS, CloudFormation, CloudWatch Logs, Lambda, IAM, DynamoDB, EKS, SQS, Secrets Manager. Forces `--output json` for structured parsing, uses `force_tee_hint()` for truncation recovery, strips Lambda secrets. Shared runner `run_aws_filtered()` handles boilerplate for JSON-based filters; text-based filters (S3 ls, S3 sync/cp) have dedicated runners
- `container.rs` handles both Docker and Kubernetes; `DockerCommands` and `KubectlCommands` sub-enums in `main.rs` route to `container::run()` -- uses passthrough for unknown subcommands
- `curl_cmd.rs` truncates long responses, saves full output to file for recovery
- `wget_cmd.rs` wraps wget with output filtering
- `psql_cmd.rs` filters PostgreSQL query output
