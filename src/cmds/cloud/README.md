# Cloud and Infrastructure

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `aws_cmd.rs` — 25 specialized filters covering STS, S3, EC2, ECS, RDS, CloudFormation, CloudWatch Logs, Lambda, IAM, DynamoDB, EKS, SQS, Secrets Manager. Forces `--output json` for structured parsing, uses `force_tee_hint()` for truncation recovery, strips Lambda secrets. Shared runner `run_aws_filtered()` handles boilerplate for JSON-based filters; text-based filters (S3 ls, S3 sync/cp) have dedicated runners
- `gcloud_cmd.rs` — command-aware support for Compute instance, GKE cluster, Cloud Run service, Cloud Logging, and Cloud Storage list/transfer output. Explicit `--format` and unsupported commands pass through untouched. Filtered inventories use the shared semantic caps and a recall hint when items are hidden.

`rtk gcloud` filters only `compute instances list`, `container clusters list`, `run services list`,
`logging read`, and `storage ls`, plus progress-only output from `storage cp`, `storage rsync`, and
`storage mv`. Resource lists use a compact JSON projection; `storage ls` keeps its native
one-path-per-line output because that command only accepts gcloud's `gsutil` format. Capped
results include an `rtk recall` hint. Any caller-supplied `--format` (including `--format value`
and `--format=value`) is passed to gcloud unchanged, as are unsupported commands.
- `container.rs` handles Docker, Kubernetes, and OpenShift; `DockerCommands`, `KubectlCommands`, and `OcCommands` sub-enums in `main.rs` route to `container::run()` -- uses passthrough for unknown subcommands
- `curl_cmd.rs` truncates long responses, saves full output to file for recovery
- `wget_cmd.rs` wraps wget with output filtering
- `psql_cmd.rs` filters PostgreSQL query output
