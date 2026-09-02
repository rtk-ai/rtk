use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Incoming JSON-RPC 2.0 message (request or notification).
#[derive(Debug, Deserialize)]
pub struct Request {
    #[allow(dead_code)] // validated implicitly by serde; not read in handler
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
    /// Absent for notifications; present for requests.
    pub id: Option<Value>,
}

/// Outgoing JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
    pub id: Value,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
            id,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

// ── MCP result types ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: &'static str,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

#[derive(Debug, Serialize)]
pub struct ServerInfo {
    pub name: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ServerCapabilities {
    pub tools: ToolsCapability,
}

/// Signals to the client that `tools/list` and `tools/call` are supported.
#[derive(Debug, Serialize)]
pub struct ToolsCapability {}

#[derive(Debug, Serialize)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Serialize)]
pub struct ToolsListResult {
    pub tools: Vec<Tool>,
}

#[derive(Debug, Serialize)]
pub struct ContentItem {
    #[serde(rename = "type")]
    pub content_type: &'static str,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct ToolCallResult {
    pub content: Vec<ContentItem>,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

// ── JSON-RPC error codes ──────────────────────────────────────────────────────

pub const PARSE_ERROR: i32 = -32700;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_request_deserialize_with_id() {
        let raw = r#"{"jsonrpc":"2.0","method":"tools/list","params":{},"id":1}"#;
        let req: Request = serde_json::from_str(raw).expect("parse failed");
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(json!(1)));
    }

    #[test]
    fn test_request_deserialize_notification() {
        let raw = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
        let req: Request = serde_json::from_str(raw).expect("parse failed");
        assert_eq!(req.method, "initialized");
        assert!(req.id.is_none());
    }

    #[test]
    fn test_request_no_params_defaults_to_null() {
        let raw = r#"{"jsonrpc":"2.0","method":"ping","id":2}"#;
        let req: Request = serde_json::from_str(raw).expect("parse failed");
        assert!(req.params.is_null());
    }

    #[test]
    fn test_response_ok_serializes() {
        let r = Response::ok(json!(1), json!({"answer": 42}));
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"result\""));
        assert!(!s.contains("\"error\""));
        assert!(s.contains("\"id\":1"));
    }

    #[test]
    fn test_response_err_serializes() {
        let r = Response::err(json!(3), METHOD_NOT_FOUND, "Method not found");
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"error\""));
        assert!(!s.contains("\"result\""));
        assert!(s.contains("-32601"));
    }

    #[test]
    fn test_response_err_omits_result_field() {
        let r = Response::err(json!(null), PARSE_ERROR, "bad json");
        let serialized = serde_json::to_string(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert!(v.get("result").is_none(), "result should be omitted");
        assert!(v.get("error").is_some());
    }

    #[test]
    fn test_response_ok_omits_error_field() {
        let r = Response::ok(json!(1), json!({}));
        let serialized = serde_json::to_string(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert!(v.get("error").is_none(), "error should be omitted");
        assert!(v.get("result").is_some());
    }

    #[test]
    fn test_initialize_result_serializes() {
        let r = InitializeResult {
            protocol_version: "2024-11-05",
            capabilities: ServerCapabilities {
                tools: ToolsCapability {},
            },
            server_info: ServerInfo {
                name: "rtk",
                version: "0.37.0",
            },
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("protocolVersion"));
        assert!(s.contains("2024-11-05"));
        assert!(s.contains("serverInfo"));
    }

    #[test]
    fn test_tool_call_result_is_error_true() {
        let r = ToolCallResult {
            content: vec![ContentItem {
                content_type: "text",
                text: "error output".to_string(),
            }],
            is_error: true,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"isError\":true"));
    }

    #[test]
    fn test_tool_call_result_is_error_false() {
        let r = ToolCallResult {
            content: vec![ContentItem {
                content_type: "text",
                text: "ok output".to_string(),
            }],
            is_error: false,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"isError\":false"));
    }
}
