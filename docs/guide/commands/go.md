---
title: Go
description: RTK filters for go test and golangci-lint
sidebar:
  order: 6
---

# Go

RTK covers Go's two most verbose outputs: test runs and the linter.

## go test — 90% savings

```bash
rtk go test [args...]
```

Parses Go's NDJSON streaming output for precise failure filtering. Shows failures only on failure, compact summary on success.

Uses `go test -json` internally for reliable event-by-event parsing.

## golangci-lint — 85% savings

```bash
rtk golangci-lint run [args...]
```

Compressed JSON output grouped by linter and file.

**Before (verbose lint output):**
```
src/main.go:42:5: Error return value of `fmt.Fprintf` is not checked (errcheck)
src/main.go:67:1: exported function `ProcessData` should have comment (godot)
src/handler.go:15:9: G104: Errors unhandled. (gosec)
...
```

**After:**
```
src/main.go (2 issues)
  errcheck: Error return not checked (x1)
  godot: Missing comment on exported func (x1)
src/handler.go (1 issue)
  gosec/G104: Errors unhandled (x1)
```

## go build

Unrecognized `go` subcommands pass through directly:

```bash
rtk go build ./...    # passes through to go build
rtk go vet ./...      # passes through to go vet
```
