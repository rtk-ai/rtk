# RTK for Claude Desktop

This is the plugin marketplace for [RTK](https://www.rtk-ai.app) in Claude Desktop (Cowork). It is filled by rtk's release workflow; the source is in the [rtk repository](https://github.com/rtk-ai/rtk/tree/master/desktop-plugin).

## Install

1. In Claude, open **Customize › Plugins › Add › Add marketplace** and enter `rtk-ai/claude-plugins`.
2. Install **rtk**, then quit and reopen Claude Desktop.
3. In a task, type `/rtk-setup` once.

Updates come by themselves. Guide: [Claude Desktop](https://www.rtk-ai.app/guide/getting-started/claude-desktop).

## What is inside

Only the listing: `marketplace.json` points at the plugin zip of the latest rtk release, with its sha256, so installing downloads that one file from the [releases](https://github.com/rtk-ai/rtk/releases). The plugin is Apache-2.0; RTK's engine inside it is BUSL-1.1 (`LICENSE-ENGINE`), free to use.

