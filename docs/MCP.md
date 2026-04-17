# RTK as an MCP Server (Claude Desktop)

RTK works with Claude Desktop via the [Model Context Protocol (MCP)](https://modelcontextprotocol.io). It exposes a single `bash` tool that routes commands through RTK's filter pipeline — the same 60-90% token savings you get in Claude Code, now available in Claude Desktop.

## Quick start

**1. Build RTK**

```bash
cargo install --path .
```

**2. Register with Claude Desktop**

```bash
rtk mcp-install
```

This writes the MCP server entry into Claude Desktop's config file and prints the path that was updated.

**3. Restart Claude Desktop**

The server is loaded at startup. After restarting, open the tool picker — you should see a `bash` tool provided by `rtk`.

---

## Manual installation

If `rtk mcp-install` doesn't work for your setup, add the server entry manually.

**Config file locations:**

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/Claude/claude_desktop_config.json` |
| Windows | `%APPDATA%\Claude\claude_desktop_config.json` |
| Linux | `~/.config/Claude/claude_desktop_config.json` |

**Entry to add:**

```json
{
  "mcpServers": {
    "rtk": {
      "command": "/full/path/to/rtk",
      "args": ["mcp-serve"]
    }
  }
}
```

Replace `/full/path/to/rtk` with the output of `which rtk`.

---

## How it works

```
Claude Desktop → tools/call bash {command: "git log -10"}
                      ↓
              rtk mcp-serve (this process)
                      ↓
              spawns: rtk git log -10
                      ↓
              RTK git filter (compact output)
                      ↓
              returns filtered text to Claude Desktop
```

- The MCP server is a **synchronous stdio JSON-RPC 2.0** process (no tokio, <10ms startup).
- Every command is routed through the same RTK filter pipeline used by Claude Code hooks.
- Token savings are tracked in `rtk gain` under the `mcp` source.

---

## Verifying it works

Start the server manually and send a test request:

```bash
echo '{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}' | rtk mcp-serve
```

Expected output (one JSON line):

```json
{"jsonrpc":"2.0","result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"rtk","version":"0.37.0"}},"id":1}
```

Run `tools/list` to confirm the `bash` tool is registered:

```bash
printf '{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}\n{"jsonrpc":"2.0","method":"tools/list","params":{},"id":2}\n' | rtk mcp-serve
```

---

## Token savings via `rtk gain`

MCP-sourced commands are tracked separately:

```bash
rtk gain           # total savings (all sources)
rtk gain --history # shows source column: hook | mcp | direct
```

---

## Troubleshooting

**Claude Desktop doesn't show the bash tool**
- Confirm the config file is valid JSON: `cat <config_path> | python3 -m json.tool`
- Confirm the `command` path is absolute and the binary is executable: `chmod +x $(which rtk)`
- Restart Claude Desktop after editing the config

**`rtk mcp-install` says "Claude Desktop not installed"**
- The installer checks for the config directory, not the app itself
- Create the directory manually: `mkdir -p ~/Library/Application\ Support/Claude` (macOS)
- Then re-run `rtk mcp-install`

**Commands run without RTK filtering**
- This happens if `rtk mcp-serve` can't locate itself via `current_exe()`
- Use an absolute path in the config: `"command": "/usr/local/bin/rtk"`

---

## Integration tests

Integration tests spawn the real binary and exercise the full handshake:

```bash
cargo build                                            # build first
cargo test --test mcp_integration -- --ignored         # run all E2E tests
cargo test --test mcp_integration test_initialize -- --ignored  # single test
```
