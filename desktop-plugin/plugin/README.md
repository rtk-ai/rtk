# RTK for Claude Desktop

RTK cuts what Claude reads and routes work to cheaper models. This plugin brings it to Cowork tasks in Claude Desktop, on macOS, Windows and Linux:

- **Compression.** Shell command output (`git`, `grep`, `cargo test`, `npm test`, `ls`, ...) is compressed before it reaches Claude, whether the command ran in the task's workspace or in a connected folder.
- **Pipelines.** RTK pipelines decide when a task should hand searching to a Haiku helper and large implementations to a Sonnet helper, so the main model reads and writes less. The two bundled defaults, `default-bash-compress` and `explore-first`, need nothing else. Any pipeline from the reviewed list at [rtk-ai.app/pipelines](https://www.rtk-ai.app/pipelines/) can be added by name; Laya-based ones run Laya, RTK's local classifier, on your computer.
- **Gains.** `/rtk-gain` shows what RTK saved in the current task and across all your tasks and Claude Code sessions on this computer. Delegation is counted as rtk itself counts it: the context a helper kept out of the main model (what it read and produced, minus its report), with the helper's cost shown next to it. A task is recorded on your computer when you run `/rtk-gain` in it.

## Install

Claude Desktop: open **Customize › Plugins › Add › Add marketplace**, enter `rtk-ai/claude-plugins`, install **rtk**, then quit and reopen Desktop; updates come by themselves. Prefer a file? [Download the plugin](https://github.com/rtk-ai/rtk/releases/latest/download/rtk-desktop-plugin.zip) and use **Add › Upload plugin**. Claude Code users do not need this plugin: `rtk init -g` sets up RTK there, and the plugin stays inert in Claude Code sessions. Nothing else to install: the plugin bundles rtk 1.1.0 for the task workspace and runs its small service with Node.js on your computer. In any task, `/rtk-setup` shows what is in place, what to expect, and asks which pipelines to use; your first task ends with a one-line reminder of it. If rtk is installed on your computer, its own savings history appears in `/rtk-gain` too.

## Commands

| Command | What it does |
|---|---|
| `/rtk-setup` | Guided setup: checklist for this computer, what to expect, pick a pipeline |
| `/rtk-gain` | Savings in this task, all Desktop tasks, and Claude Code on this computer |
| `/rtk-pipelines` | The pipelines your tasks use |
| `/rtk-pipelines-search [word]` | Browse the reviewed pipelines you can install |
| `/rtk-pipeline-add <name \| github:owner/repo \| URL \| path>` | Add a pipeline (applies from the next task) |
| `/rtk-pipeline-remove <name>`, `/rtk-pipelines-reset` | Remove one, or go back to the defaults |
| `/rtk-pipeline-off` | Run this one task without pipelines (compression stays on) |
| `/rtk-laya-status` | Whether Laya is up on your computer |
| `/rtk-report` | A detailed test report for the current task (models, cost, hooks, rules) |

## What to expect

- A task that uses a pipeline shows one or two steps marked **Failed** in red. They are RTK deliberately refusing a command: once per task to fetch your pipelines, and when a rule hands work to a helper. The task continues normally.
- Desktop asks you to approve each call of the plugin's tools on your computer, and offers no "always allow" for them. RTK keeps those calls to one per folder task (its setup, at the second look) and one per `/rtk-gain`; it never calls them on its own at the end of a task.
- Delegation pays on large writing tasks and costs a little extra on tiny ones; the defaults keep small changes with the main model.
- Laya pipelines need Node.js 20 or newer on your computer; the first use downloads about 1.7 GB of model files. Laya uses about 2 GB of memory while a task is labelled and stops after 30 minutes of inactivity.

## Files on your computer

The plugin keeps its data in one folder: `~/Library/Application Support/rtk-desktop` (macOS), `%APPDATA%\rtk-desktop` (Windows), `~/.local/share/rtk-desktop` (Linux): your pipelines, the task records, logs, and Laya's packages. Laya's model files go to `~/.cache/receptron-laya`. Deleting those two folders removes everything the plugin added. The plugin never writes to a connected folder; commands there run in Cowork's own VM.

Network use: pipeline files and the reviewed list from GitHub, Laya's packages from npm and its model from Hugging Face. No task content leaves your computer through the plugin.

## Licenses

The plugin is Apache-2.0. The bundled rtk binary is Apache-2.0 with its compression engine under BUSL-1.1 (`LICENSE-ENGINE`), free for production use.
