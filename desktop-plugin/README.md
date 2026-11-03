# Claude Desktop plugin (developer notes)

The **rtk** plugin brings RTK to Claude Desktop's Cowork tasks. Users get it as `rtk-desktop-plugin-<version>.zip` on each release and upload it in Desktop; its user-facing README is [`plugin/README.md`](plugin/README.md).

## Layout

| Path | What |
|---|---|
| `plugin/` | The plugin as Claude loads it: `.claude-plugin/plugin.json`, `hooks/` (run in Cowork's cloud workspace: compression, pipeline gates, the savings record), `commands/` (the `/rtk-*` commands), `pipeline/` (the two bundled defaults), `icon.png`. |
| `desktop-src/` | The service that runs on the user's computer, an MCP server over stdio with no dependencies (Node 18+): pipelines, Laya, the savings record, the `/rtk-setup` checklist. `build.sh` zips it into `plugin/desktop/rtk-desktop.mcpb`. |
| `test_hooks.py` | Tests of the hooks against a real `rtk` binary (`RTK_BIN`). |
| `build.sh` | Builds the zip: stages `plugin/`, sets the version, adds the bundle, the licenses and `runtime/rtk`. |

Two pieces are never committed: the Linux x86_64 musl `rtk` the task workspace runs (Cowork's workspace is Linux whatever the user's computer is) and the built bundle; `build.sh` stages both under `dist/`. The release workflow adds the `rtk` it just built, so the plugin always bundles the rtk it was released with and carries its version.


## Build and test

```sh
# tests of the service (no network, no Laya)
(cd desktop-plugin/desktop-src && node --test 'test/*.test.mjs')

# tests of the hooks against an rtk binary (any build of this version line)
RTK_BIN=target/release/rtk python3 desktop-plugin/test_hooks.py

# the zip, with a Linux musl rtk (for example from a release's rtk-x86_64-unknown-linux-musl.tar.gz)
sh desktop-plugin/build.sh --version 1.2.0 --rtk path/to/rtk --out desktop-plugin/dist
```

`claude plugin validate desktop-plugin/dist/rtk` checks the staged plugin. `node desktop-src/test/live.mjs <scratch dir>` exercises the network paths by hand (the reviewed list, an install by name).

## Release

`release.yml` has a `desktop-plugin` job that runs after the release is created: it tests the plugin against the musl `rtk` of that release, builds the zip, uploads it to the release twice (`rtk-desktop-plugin-<version>.zip` and `rtk-desktop-plugin.zip`, the stable link) with their lines in `checksums.txt`, and on a stable release pushes the plugin into the marketplace repository `rtk-ai/claude-plugins` (`marketplace/` holds that repository's listing and README; the GitHub App of the release workflow mints the token, so the App must be installed on that repository). It never blocks the release: if it fails, the run carries an error annotation, an issue titled "Claude Desktop plugin build failed" is opened (or commented, when one is open) and the release Discord channel is notified. `ci.yml` runs the same tests on pull requests that touch `desktop-plugin/`.

## How it works, in short

Cowork runs each task in a fresh cloud workspace; connected folders are reachable only through a Linux VM on the user's computer that sees nothing else. The hooks therefore run in the workspace with the bundled `rtk` (compression of command output, including the output of folder commands after it returns; pipeline gates and rewrites as in Claude Code), and everything that lives on the user's computer (their pipelines, Laya, the savings record) is reached through the service's tools: `rtk_task_setup` once per folder task at the second look, `rtk_gain` when the user asks, `rtk_status`/`rtk_pipelines`/`rtk_laya` for the commands. Desktop asks the user to approve each of those calls; nothing calls them on its own.
