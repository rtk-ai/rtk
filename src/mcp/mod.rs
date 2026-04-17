//! MCP (Model Context Protocol) server for Claude Desktop integration.
//!
//! Implements a synchronous stdio JSON-RPC 2.0 server that exposes RTK's
//! filter pipeline as a `bash` tool. Claude Desktop sends tool calls here;
//! RTK routes each command through its existing filters before returning
//! token-optimized output.
//!
//! Start the server: `rtk mcp-serve`
//! Install into Claude Desktop: `rtk mcp-install`

mod handler;
pub mod install;
pub mod protocol;
pub mod transport;

use anyhow::Result;
use serde_json::Value;

/// Run the MCP stdio server (blocking, single-threaded).
///
/// Reads newline-delimited JSON-RPC requests from stdin, dispatches to
/// the appropriate handler, and writes responses to stdout.
/// Exits cleanly on EOF or a `shutdown` request.
pub fn serve() -> Result<()> {
    let mut transport = transport::Transport::new();

    loop {
        let line = match transport.read_line()? {
            Some(l) => l,
            None => break, // EOF — client disconnected
        };

        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<protocol::Request>(&line) {
            Ok(req) => {
                // Notifications have no `id` — do not send a response.
                if req.id.is_none() {
                    continue;
                }

                let id = req.id.clone().unwrap_or(Value::Null);
                let is_shutdown = req.method == "shutdown";

                let response = handler::handle(&req.method, &req.params, id);
                let json = serde_json::to_string(&response)
                    .unwrap_or_else(|e| format!("{{\"jsonrpc\":\"2.0\",\"error\":{{\"code\":-32603,\"message\":\"{}\"}},\"id\":null}}", e));
                transport.write_response(&json)?;

                if is_shutdown {
                    break;
                }
            }
            Err(e) => {
                let error_response = protocol::Response::err(
                    Value::Null,
                    protocol::PARSE_ERROR,
                    format!("Parse error: {}", e),
                );
                let json = serde_json::to_string(&error_response).unwrap_or_else(|_| {
                    "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32700,\"message\":\"parse error\"},\"id\":null}".to_string()
                });
                transport.write_response(&json)?;
            }
        }
    }

    Ok(())
}
