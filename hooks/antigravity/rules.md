# RTK - Rust Token Killer (Antigravity)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk` to minimize token consumption.

Examples:

```bash
rtk git status
rtk git log -10
rtk git diff --cached
rtk cargo test
rtk cargo clippy
rtk ls src/
rtk grep "pattern" src/
rtk find "*.rs" .
rtk docker ps
rtk kubectl get pods
```

## GCP / Google Cloud

```bash
rtk gcloud compute instances list
rtk gcloud run services list
rtk gcloud functions list
rtk gcloud iam roles list
rtk gcloud container clusters list
rtk gcloud sql instances list
rtk gcloud logging read "severity>=ERROR" --limit=50
rtk gsutil ls gs://my-bucket/
rtk gsutil du -s gs://my-bucket/
rtk bq ls
rtk bq show dataset.table
```

## Meta Commands

```bash
rtk gain              # Show token savings
rtk gain --history    # Command history with savings
rtk discover          # Find missed RTK opportunities
rtk proxy <cmd>       # Run raw (no filtering, for debugging)
```

## Environment Variables

RTK respects these GCP environment variables when present:

- `CLOUDSDK_CORE_PROJECT` — Default GCP project
- `GOOGLE_CLOUD_PROJECT` — Alternative project env var
- `CLOUDSDK_COMPUTE_REGION` — Default compute region

## Multi-Agent Output

When producing artifacts or structured output:

- Prefer compact, single-file output over multi-file sprawl
- Use `rtk json <file>` to inspect JSON structures (schema-only mode)
- Use `rtk summary <cmd>` for quick overviews of verbose commands
- Pipe long outputs through `rtk log` for deduplication

## Why

RTK filters and compresses command output before it reaches the LLM context, saving 60-90% tokens on common operations. Always use `rtk <cmd>` instead of raw commands.
