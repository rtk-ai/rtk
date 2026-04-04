---
title: Data & Network
description: RTK filters for JSON, environment variables, logs, curl, and wget
sidebar:
  order: 11
---

# Data & Network

RTK compresses structured data, network output, and log streams.

## json — 60% savings

```bash
rtk json <file> [--depth N]    # default: depth 5
rtk json -                      # from stdin
```

Shows JSON structure (keys and types) without values. Useful for exploring large API responses or config files.

**Before — `cat package.json` (50 lines):**
```json
{
  "name": "my-app",
  "version": "1.0.0",
  "dependencies": {
    "react": "^18.2.0",
    "next": "^14.0.0",
    ...15 dependencies...
  }
}
```

**After — `rtk json package.json` (10 lines):**
```
{
  name: string
  version: string
  dependencies: { 15 keys }
  devDependencies: { 8 keys }
  scripts: { 6 keys }
}
```

## env — sensitive values masked

```bash
rtk env                    # all variables (sensitive values masked)
rtk env -f AWS             # filter by name
rtk env --show-all         # include sensitive values
```

Sensitive variables (tokens, secrets, passwords) are masked by default: `AWS_SECRET_ACCESS_KEY=***`.

## log — 60-80% savings

```bash
rtk log <file>    # from a file
rtk log           # from stdin (pipe)
```

Deduplicates repeated log lines: `[ERROR] Connection refused (×42)`. Savings depend on how repetitive the log is.

## curl — HTTP with JSON detection

```bash
rtk curl [args...]
```

Auto-detects JSON responses and shows schema instead of full content. Falls back to raw output for non-JSON.

## wget

```bash
rtk wget <url> [args...]
rtk wget -O - <url>    # output to stdout
```

Removes progress bars and download noise.

## aws

```bash
rtk aws <service> [args...]
```

Forces JSON output mode and compresses the result. Supports all AWS services (sts, s3, ec2, ecs, rds, cloudformation, etc.).

## psql

```bash
rtk psql [args...]
```

Removes table borders and compresses query output.

## summary

```bash
rtk summary <command...>
```

Runs any command and generates a heuristic summary of the output. Useful for commands that don't have a dedicated RTK filter.
