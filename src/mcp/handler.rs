use serde_json::{json, Value};

use super::protocol::{
    ContentItem, InitializeResult, ServerCapabilities, ServerInfo, Tool, ToolCallResult,
    ToolsCapability, ToolsListResult, INVALID_PARAMS, METHOD_NOT_FOUND,
};
use super::protocol::{Response, PARSE_ERROR};

pub fn handle(method: &str, params: &Value, id: Value) -> Response {
    match method {
        "initialize" => handle_initialize(id),
        "tools/list" => handle_tools_list(id),
        "tools/call" => handle_tools_call(params, id),
        "ping" => Response::ok(id, json!({})),
        "shutdown" => Response::ok(id, json!(null)),
        _ => Response::err(
            id,
            METHOD_NOT_FOUND,
            format!("Method not found: {}", method),
        ),
    }
}

fn handle_initialize(id: Value) -> Response {
    let result = InitializeResult {
        protocol_version: "2024-11-05",
        capabilities: ServerCapabilities {
            tools: ToolsCapability {},
        },
        server_info: ServerInfo {
            name: "rtk",
            version: env!("CARGO_PKG_VERSION"),
        },
    };
    match serde_json::to_value(result) {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, PARSE_ERROR, format!("Serialization error: {}", e)),
    }
}

fn handle_tools_list(id: Value) -> Response {
    let tools = vec![Tool {
        name: "bash",
        description: "Execute a shell command with RTK token-optimized output filtering \
                       (60-90% token savings on git, cargo, npm, pnpm, vitest, playwright, \
                       docker, kubectl, and more).",
        input_schema: json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "description": {
                    "type": "string",
                    "description": "Human-readable description of the command (optional)"
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in milliseconds (optional, default: no timeout)"
                }
            },
            "required": ["command"]
        }),
    }];
    let result = ToolsListResult { tools };
    match serde_json::to_value(result) {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, PARSE_ERROR, format!("Serialization error: {}", e)),
    }
}

fn handle_tools_call(params: &Value, id: Value) -> Response {
    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return Response::err(id, INVALID_PARAMS, "Missing 'name' in tool call"),
    };

    if name != "bash" {
        return Response::err(id, INVALID_PARAMS, format!("Unknown tool: {}", name));
    }

    let command = match params
        .get("arguments")
        .and_then(|a| a.get("command"))
        .and_then(|c| c.as_str())
    {
        Some(c) => c.to_string(),
        None => return Response::err(id, INVALID_PARAMS, "Missing 'command' in bash arguments"),
    };

    let timer = crate::core::tracking::TimedExecution::start();
    let (output, exit_code) = run_with_rtk(&command);
    timer.track_with_source(
        &command,
        &format!("rtk mcp: {}", command),
        "",
        &output,
        "mcp",
    );

    let result = ToolCallResult {
        content: vec![ContentItem {
            content_type: "text",
            text: output,
        }],
        is_error: exit_code != 0,
    };
    match serde_json::to_value(result) {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, PARSE_ERROR, format!("Serialization error: {}", e)),
    }
}

/// Run a command string through the current RTK binary for filter-aware execution.
/// Falls back to raw `sh -c` if RTK re-invocation fails.
///
/// In test builds we skip the RTK re-invocation because `current_exe()` returns
/// the cargo test harness binary. Integration tests cover the RTK filter path.
fn run_with_rtk(command: &str) -> (String, i32) {
    #[cfg(test)]
    return run_raw_shell(command);

    #[cfg(not(test))]
    run_with_rtk_binary(command)
}

#[cfg(not(test))]
fn run_with_rtk_binary(command: &str) -> (String, i32) {
    let args = shell_split(command);
    if args.is_empty() {
        return (String::new(), 0);
    }

    // Use current RTK binary so filters are always applied
    let rtk_exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("rtk"));

    match std::process::Command::new(&rtk_exe)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
    {
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(1);
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let combined = merge_output(stdout, stderr);

            // Exit 127 = command not found by RTK's fallback (e.g., shell builtins on Windows).
            // Retry via system shell so `echo`, `dir`, compound expressions, etc. work correctly.
            if exit_code == 127 {
                return run_raw_shell(command);
            }

            (combined, exit_code)
        }
        Err(e) => {
            // RTK re-invocation failed — fall back to raw shell execution
            eprintln!(
                "rtk mcp: rtk re-invoke failed ({}), falling back to raw shell",
                e
            );
            run_raw_shell(command)
        }
    }
}

/// Last-resort fallback: execute command via system shell, no filtering.
fn run_raw_shell(command: &str) -> (String, i32) {
    let shell_result = {
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd")
                .args(["/C", command])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
        }
        #[cfg(not(target_os = "windows"))]
        {
            std::process::Command::new("sh")
                .args(["-c", command])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
        }
    };

    match shell_result {
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(1);
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            (merge_output(stdout, stderr), exit_code)
        }
        Err(e) => (format!("rtk mcp: failed to execute command: {}", e), 1),
    }
}

fn merge_output(mut stdout: String, stderr: String) -> String {
    if !stderr.is_empty() {
        if !stdout.is_empty() && !stdout.ends_with('\n') {
            stdout.push('\n');
        }
        stdout.push_str(&stderr);
    }
    stdout
}

/// Minimal POSIX shell-word splitter.
/// Handles double quotes, single quotes, and backslash escapes.
pub fn shell_split(s: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            '"' => {
                while let Some(c2) = chars.next() {
                    match c2 {
                        '"' => break,
                        '\\' => {
                            if let Some(c3) = chars.next() {
                                current.push(c3);
                            }
                        }
                        _ => current.push(c2),
                    }
                }
            }
            '\'' => {
                for c2 in chars.by_ref() {
                    if c2 == '\'' {
                        break;
                    }
                    current.push(c2);
                }
            }
            '\\' => {
                if let Some(c2) = chars.next() {
                    current.push(c2);
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── shell_split tests ────────────────────────────────────────────────────

    #[test]
    fn test_shell_split_simple() {
        assert_eq!(shell_split("git status"), vec!["git", "status"]);
    }

    #[test]
    fn test_shell_split_double_quotes() {
        assert_eq!(
            shell_split(r#"git commit -m "fix bug""#),
            vec!["git", "commit", "-m", "fix bug"]
        );
    }

    #[test]
    fn test_shell_split_single_quotes() {
        assert_eq!(
            shell_split("echo 'hello world'"),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn test_shell_split_backslash_escape() {
        assert_eq!(
            shell_split(r"echo hello\ world"),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn test_shell_split_empty() {
        assert!(shell_split("").is_empty());
    }

    #[test]
    fn test_shell_split_whitespace_only() {
        assert!(shell_split("   ").is_empty());
    }

    #[test]
    fn test_shell_split_multiple_spaces() {
        assert_eq!(shell_split("a  b   c"), vec!["a", "b", "c"]);
    }

    // ── handler tests ────────────────────────────────────────────────────────

    #[test]
    fn test_handle_ping() {
        let r = handle("ping", &json!({}), json!(5));
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"result\""));
        assert!(!s.contains("\"error\""));
    }

    #[test]
    fn test_handle_unknown_method() {
        let r = handle("unknown/method", &json!({}), json!(9));
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"error\""));
        assert!(s.contains("-32601"));
    }

    #[test]
    fn test_handle_initialize_returns_protocol_version() {
        let r = handle("initialize", &json!({}), json!(1));
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        let version = v["result"]["protocolVersion"].as_str().unwrap_or("");
        assert_eq!(version, "2024-11-05");
    }

    #[test]
    fn test_handle_initialize_returns_server_info() {
        let r = handle("initialize", &json!({}), json!(1));
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(v["result"]["serverInfo"]["name"].as_str().unwrap(), "rtk");
    }

    #[test]
    fn test_handle_tools_list_returns_bash() {
        let r = handle("tools/list", &json!({}), json!(2));
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        let tools = v["result"]["tools"].as_array().unwrap();
        assert!(!tools.is_empty());
        assert_eq!(tools[0]["name"].as_str().unwrap(), "bash");
    }

    #[test]
    fn test_handle_tools_list_bash_has_input_schema() {
        let r = handle("tools/list", &json!({}), json!(2));
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        let schema = &v["result"]["tools"][0]["inputSchema"];
        assert_eq!(schema["type"].as_str().unwrap(), "object");
        assert!(schema["properties"]["command"].is_object());
    }

    #[test]
    fn test_handle_tools_call_missing_name() {
        let params = json!({"arguments": {"command": "ls"}});
        let r = handle("tools/call", &params, json!(3));
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"error\""));
        assert!(s.contains("-32602"));
    }

    #[test]
    fn test_handle_tools_call_unknown_tool() {
        let params = json!({"name": "unknown_tool", "arguments": {}});
        let r = handle("tools/call", &params, json!(3));
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"error\""));
        assert!(s.contains("-32602"));
    }

    #[test]
    fn test_handle_tools_call_missing_command() {
        let params = json!({"name": "bash", "arguments": {}});
        let r = handle("tools/call", &params, json!(3));
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"error\""));
        assert!(s.contains("-32602"));
    }

    #[test]
    fn test_handle_tools_call_bash_executes() {
        let params = json!({
            "name": "bash",
            "arguments": {"command": "echo hello_rtk_mcp"}
        });
        let r = handle("tools/call", &params, json!(4));
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        // Should be a result (no error key at top level)
        assert!(v.get("result").is_some(), "expected result, got: {}", v);
        let text = v["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            text.contains("hello_rtk_mcp"),
            "expected echo output, got: {:?}",
            text
        );
    }

    #[test]
    fn test_handle_tools_call_failing_command_sets_is_error() {
        // Use `sh -c "exit 1"` style via the raw shell fallback (cfg(test) path)
        let params = json!({
            "name": "bash",
            "arguments": {"command": "sh -c 'exit 1'"}
        });
        let r = handle("tools/call", &params, json!(5));
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert!(v.get("result").is_some());
        assert_eq!(v["result"]["isError"].as_bool(), Some(true));
    }

    #[test]
    fn test_handle_shutdown() {
        let r = handle("shutdown", &json!({}), json!(99));
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"result\""));
    }

    #[test]
    fn test_merge_output_appends_stderr() {
        let out = merge_output("stdout line".to_string(), "stderr line".to_string());
        assert!(out.contains("stdout line"));
        assert!(out.contains("stderr line"));
    }

    #[test]
    fn test_merge_output_empty_stderr() {
        let out = merge_output("only stdout".to_string(), String::new());
        assert_eq!(out, "only stdout");
    }
}
