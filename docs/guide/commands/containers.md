---
title: Containers
description: RTK filters for Docker and Kubernetes
sidebar:
  order: 9
---

# Containers

RTK compresses Docker and Kubernetes command output into compact, token-efficient summaries.

## Docker

### docker ps — 80% savings

```bash
rtk docker ps [args...]
```

**Before:**
```
CONTAINER ID   IMAGE          COMMAND                  CREATED       STATUS                    PORTS                    NAMES
abc123def456   nginx:1.25     "/docker-entrypoint.…"   2 days ago    Up 2 days (healthy)       0.0.0.0:80->80/tcp       web
789012345678   postgres:16    "docker-entrypoint.s…"   2 days ago    Up 2 days (healthy)       0.0.0.0:5432->5432/tcp   db
```

**After:**
```
web   nginx:1.25    Up 2d (healthy)
db    postgres:16   Up 2d (healthy)
```

### docker images — 80% savings

```bash
rtk docker images [args...]
```

Compact list: name, tag, size, age.

### docker logs — 70% savings

```bash
rtk docker logs <container> [args...]
```

Deduplicates repeated log lines: `[ERROR] Connection refused (×42)`.

### Docker Compose

```bash
rtk docker compose ps                # compact service list — 80%
rtk docker compose logs [service]    # deduplicated logs — 70%
rtk docker compose build [service]   # build summary — 60%
```

Unrecognized `docker compose` subcommands pass through.

## Kubernetes

### kubectl pods — 75%+ savings

```bash
rtk kubectl pods [-n namespace] [-A]
```

Compact pod list with status.

### kubectl services

```bash
rtk kubectl services [-n namespace] [-A]
```

### kubectl logs — 70% savings

```bash
rtk kubectl logs <pod> [-c container]
```

Deduplicates repeated log lines.

### Passthrough

All other `kubectl` subcommands pass through to kubectl:

```bash
rtk kubectl apply -f deployment.yaml    # passes through
rtk kubectl describe pod <name>         # passes through
```
