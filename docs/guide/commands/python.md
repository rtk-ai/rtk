---
title: Python
description: RTK filters for pytest, ruff, mypy, and pip
sidebar:
  order: 5
---

# Python

RTK covers the core Python toolchain: testing, linting/formatting, type checking, and package management.

## pytest — 90% savings

```bash
rtk pytest [args...]
```

Shows failures only. On success, shows a compact summary.

## ruff — 80% savings

```bash
rtk ruff check [args...]
rtk ruff format --check [args...]
```

Compressed JSON output grouped by file and rule.

## mypy — type errors grouped by file

```bash
rtk mypy [args...]
```

Groups type errors by file and error code, similar to the tsc filter.

## pip / uv

```bash
rtk pip list               # package list
rtk pip outdated           # outdated packages
rtk pip install <pkg>      # installation
```

Auto-detects `uv` if available and uses it instead of pip.

## deps — project overview

```bash
rtk deps [path]    # default: current directory
```

Compact summary of project dependencies. Auto-detects `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, `Gemfile`, and others.

**Savings:** ~70%
