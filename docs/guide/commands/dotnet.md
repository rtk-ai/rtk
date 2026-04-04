---
title: .NET
description: RTK filters for dotnet build, test, and MSBuild logs
sidebar:
  order: 8
---

# .NET

RTK covers .NET build, test, and diagnostic outputs.

## dotnet build — 70-80% savings

```bash
rtk dotnet build [args...]
```

Removes per-project compilation lines, keeps errors and build summary.

**Before:**
```
Build started...
Microsoft (R) Build Engine version 17.x
  Restore complete (1.2s)
  MyLib -> bin/Debug/net8.0/MyLib.dll
  MyApp -> bin/Debug/net8.0/MyApp.dll

Build succeeded.
    0 Warning(s)
    0 Error(s)
Time Elapsed 00:00:04.23
```

**After:**
```
Build succeeded. 0 warnings, 0 errors (4.23s)
```

## dotnet test — 85% savings

```bash
rtk dotnet test [args...]
```

Shows failures only. On success, compact summary.

## MSBuild binary logs

```bash
rtk dotnet binlog [path/to/file.binlog]
```

Parses `.binlog` binary log files and displays a compact error/warning summary.

## dotnet format

```bash
rtk dotnet format [args...]
```

Shows only files that were reformatted or have formatting issues.

## Passthrough

Other `dotnet` subcommands pass through unchanged:

```bash
rtk dotnet run          # passes through
rtk dotnet publish      # passes through
rtk dotnet ef migrate   # passes through
```
