---
title: Ruby
description: RTK filters for rake, rspec, and rubocop
sidebar:
  order: 7
---

# Ruby

RTK covers Ruby's core development tools: build tasks, test output, and linting.

## rspec — 60-90% savings

```bash
rtk rspec [args...]
```

Shows failures only. On success, compact summary.

## rubocop — 80%+ savings

```bash
rtk rubocop [args...]
```

Groups violations by cop and file.

## rake — 60-80% savings

```bash
rtk rake [args...]
rtk rake test
rtk rake spec
```

Filters task execution noise, keeps errors and final result.

## Passthrough

Unrecognized rake tasks pass through to rake directly:

```bash
rtk rake db:migrate    # passes through unchanged
rtk rake assets:precompile
```
