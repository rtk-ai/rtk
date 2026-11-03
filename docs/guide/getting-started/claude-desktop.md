---
title: Claude Desktop
description: Get RTK in Claude Desktop (Cowork) in three steps, from the rtk-ai marketplace
sidebar:
  order: 4
---

# Claude Desktop

Using Claude Desktop (Cowork)? RTK comes as a plugin. It makes each task cheaper: Claude reads shorter command results, and routine work goes to cheaper models.

## Install

1. In Claude, open **Customize › Plugins**, choose **Add › Add marketplace**, and enter `rtk-ai/claude-plugins`.
2. Install **rtk**.
3. Quit and reopen Claude Desktop.

Updates come by themselves (turn on **Sync automatically** for the marketplace). Prefer a file? [Download the plugin](https://github.com/rtk-ai/rtk/releases/latest/download/rtk-desktop-plugin.zip) and use **Add › Upload plugin** instead; a new version then needs a new upload.

## First task

In any task, type `/rtk-setup`. It checks that everything is in place and lets you choose how RTK works. The standard setting needs nothing else. The other settings label each task on your computer first and need Node.js installed.

## Commands

| Command | What it does |
|---|---|
| `/rtk-setup` | Check everything and choose how RTK works |
| `/rtk-gain` | What RTK saved in this task and overall |
| `/rtk-pipelines-search` | Browse the other ways RTK can work |
| `/rtk-pipeline-add <name>` | Switch to one of them, from your next task |
| `/rtk-pipelines-reset` | Back to the standard setting |
| `/rtk-pipeline-off` | Run this one task without hand-offs to cheaper models |

## What you will see

- One or two steps marked **Failed** in red in a task. That is RTK stepping in on purpose; the task goes on.
- Claude Desktop asks you to approve the plugin once at the start of a task that uses a folder, and when you run `/rtk-gain`. RTK never asks on its own.
- Hand-offs to cheaper models pay off on big tasks. On tiny ones they can cost a little more, so the standard setting keeps small changes with the main model.

## Your data

The plugin keeps its files in one folder on your computer: `~/Library/Application Support/rtk-desktop` on macOS, `%APPDATA%\rtk-desktop` on Windows, `~/.local/share/rtk-desktop` on Linux. It never changes your project folders and keeps no text of your tasks. Nothing you work on leaves your computer through the plugin.

## If something is off

- A command says RTK is not reachable: quit and reopen Claude Desktop.
- `/rtk-setup` says Node.js is missing: install it from [nodejs.org](https://nodejs.org), then reopen Claude Desktop. Only the settings that label tasks need it.

macOS is tested. Windows and Linux should work the same; tell us what you see.

## Uninstall

Remove the plugin in **Customize › Plugins**, then delete the folder above.

The plugin is free to use: Apache-2.0, with RTK's engine under BUSL-1.1.
