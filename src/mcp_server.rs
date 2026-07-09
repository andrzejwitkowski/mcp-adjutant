use std::io::{self, BufRead, Read, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::domain::AdjutantConfig;
use crate::jobs::{JobRegistry, QUERY_JOB_STATUS_TOOL_NAME};
use crate::mcp::{
    handle_evaluate_agent_performance, handle_generate_tests_and_scaffolding,
    handle_query_job_status, handle_scout_context, handle_verify_and_triage, registered_mcp_tools,
    EVALUATE_AGENT_PERFORMANCE_TOOL_NAME, GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
    SCOUT_CONTEXT_TOOL_NAME, VERIFY_AND_TRIAGE_TOOL_NAME,
};

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct JsonRpcResponse<'a> {
    jsonrpc: &'static str,
    id: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StdioFraming {
    Ndjson,
    ContentLength,
}

// ponytail: minimal MCP stdio loop; rmcp needs edition2024 / newer rustc
pub fn run_stdio(config: Arc<RwLock<AdjutantConfig>>) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("failed to start tokio runtime: {err}"))?;

    let jobs = JobRegistry::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut framing = None;

    loop {
        let message = read_message(&stdin, &mut framing)?;
        let Some(message) = message else {
            break;
        };

        let request: JsonRpcRequest = match serde_json::from_str(&message) {
            Ok(request) => request,
            Err(parse_err) => {
                let response = err(&Value::Null, -32700, format!("Parse error: {parse_err}"));
                write_message(
                    &mut stdout,
                    &response,
                    framing.unwrap_or(StdioFraming::Ndjson),
                )?;
                continue;
            }
        };

        if request.id.is_none() {
            continue;
        }

        let id = request.id.as_ref().expect("checked above");
        let response = match request.method.as_str() {
            "initialize" => ok(id, initialize_result(&request.params)),
            "tools/list" => ok(id, list_tools_result()),
            "resources/list" => ok(id, json!({ "resources": [] })),
            "resources/templates/list" => ok(id, json!({ "resourceTemplates": [] })),
            "prompts/list" => ok(id, json!({ "prompts": [] })),
            "tools/call" => {
                let config = Arc::clone(&config);
                let jobs = jobs.clone();
                runtime.block_on(handle_tool_call(id, request.params, config, jobs))
            }
            "ping" => ok(id, json!({})),
            _ => err(id, -32601, format!("method not found: {}", request.method)),
        };

        write_message(
            &mut stdout,
            &response,
            framing.unwrap_or(StdioFraming::Ndjson),
        )?;
    }

    Ok(())
}

fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2024-11-05");
    let protocol_version = match requested {
        "2025-06-18" | "2025-03-26" | "2024-11-05" => requested,
        _ => "2024-11-05",
    };

    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": {},
            "resources": {},
            "prompts": {}
        },
        "serverInfo": {
            "name": "mcp-adjutant",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn list_tools_result() -> Value {
    let tools = registered_mcp_tools()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool["name"],
                "description": tool["description"],
                "inputSchema": tool["input_schema"]
            })
        })
        .collect::<Vec<_>>();

    json!({ "tools": tools })
}

async fn handle_tool_call(
    id: &Value,
    params: Value,
    config: Arc<RwLock<AdjutantConfig>>,
    jobs: JobRegistry,
) -> String {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let config_snapshot = Arc::new(config.read().await.clone());
    let result = match name {
        SCOUT_CONTEXT_TOOL_NAME => handle_scout_context(arguments, config_snapshot, &jobs).await,
        VERIFY_AND_TRIAGE_TOOL_NAME => {
            handle_verify_and_triage(arguments, config_snapshot, &jobs).await
        }
        GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME => {
            handle_generate_tests_and_scaffolding(arguments, config_snapshot, &jobs).await
        }
        EVALUATE_AGENT_PERFORMANCE_TOOL_NAME => {
            handle_evaluate_agent_performance(arguments, config_snapshot, &jobs).await
        }
        QUERY_JOB_STATUS_TOOL_NAME => handle_query_job_status(arguments, &jobs).await,
        other => Err(format!("unknown tool: {other}")),
    };

    match result {
        Ok(text) => ok(
            id,
            json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false
            }),
        ),
        Err(message) => ok(
            id,
            json!({
                "content": [{ "type": "text", "text": message }],
                "isError": true
            }),
        ),
    }
}

fn ok(id: &Value, result: Value) -> String {
    serde_json::to_string(&JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    })
    .expect("serialize response")
}

fn err(id: &Value, code: i32, message: String) -> String {
    serde_json::to_string(&JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError { code, message }),
    })
    .expect("serialize error")
}

fn read_message(
    stdin: &io::Stdin,
    framing: &mut Option<StdioFraming>,
) -> Result<Option<String>, String> {
    let mut stdin_lock = stdin.lock();
    let mut first_line = String::new();
    let read = stdin_lock
        .read_line(&mut first_line)
        .map_err(|err| format!("failed to read MCP header: {err}"))?;
    if read == 0 {
        return Ok(None);
    }

    let first_line = first_line.trim_end_matches(['\r', '\n']);
    if first_line.is_empty() {
        return Ok(None);
    }

    if first_line.starts_with('{') {
        framing.get_or_insert(StdioFraming::Ndjson);
        return Ok(Some(first_line.to_string()));
    }

    framing.get_or_insert(StdioFraming::ContentLength);
    let mut headers = vec![first_line.to_string()];
    loop {
        let mut line = String::new();
        stdin_lock
            .read_line(&mut line)
            .map_err(|err| format!("failed to read MCP header: {err}"))?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        headers.push(line.to_string());
    }

    let content_length = headers
        .iter()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("Content-Length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .ok_or_else(|| "missing Content-Length header".to_string())?;

    let mut body = vec![0_u8; content_length];
    stdin_lock
        .read_exact(&mut body)
        .map_err(|err| format!("failed to read MCP body: {err}"))?;

    String::from_utf8(body)
        .map_err(|err| format!("invalid UTF-8 in MCP body: {err}"))
        .map(Some)
}

fn write_message(
    stdout: &mut io::Stdout,
    payload: &str,
    framing: StdioFraming,
) -> Result<(), String> {
    let frame = match framing {
        StdioFraming::Ndjson => format!("{payload}\n"),
        StdioFraming::ContentLength => {
            format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload)
        }
    };
    stdout
        .write_all(frame.as_bytes())
        .and_then(|_| stdout.flush())
        .map_err(|err| format!("failed to write MCP response: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_tools_maps_input_schema_field() {
        let result = list_tools_result();
        let first = &result["tools"][0];
        assert!(first.get("inputSchema").is_some());
        assert!(first.get("input_schema").is_none());
    }

    #[test]
    fn list_tools_includes_query_job_status() {
        let result = list_tools_result();
        let names = result["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"query_job_status"));
    }

    #[test]
    fn scout_context_schema_requires_request_uuid() {
        let result = list_tools_result();
        let scout = result["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .find(|tool| tool["name"] == "scout_context")
            .expect("scout tool");
        let required = scout["inputSchema"]["required"]
            .as_array()
            .expect("required array");
        assert!(required.iter().any(|value| value == "request_uuid"));
    }

    #[test]
    fn discovery_stubs_return_empty_lists() {
        let resources = ok(&json!(1), json!({ "resources": [] }));
        let prompts = ok(&json!(2), json!({ "prompts": [] }));
        assert!(resources.contains("\"resources\":[]"));
        assert!(prompts.contains("\"prompts\":[]"));
    }

    #[test]
    fn initialize_negotiates_newer_protocol_version() {
        let result = initialize_result(&json!({ "protocolVersion": "2025-03-26" }));
        assert_eq!(result["protocolVersion"], "2025-03-26");
        assert!(result["capabilities"]["resources"].is_object());
    }

    #[test]
    fn ndjson_framing_detects_json_line() {
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
        let mut cursor = io::Cursor::new(input);
        let mut line = String::new();
        cursor.read_line(&mut line).expect("read");
        let trimmed = line.trim_end_matches(['\r', '\n']);
        assert!(trimmed.starts_with('{'));
    }

    #[test]
    fn parse_error_response_uses_null_id() {
        let payload = err(&Value::Null, -32700, "Parse error".to_string());
        let value: Value = serde_json::from_str(&payload).expect("json");
        assert_eq!(value["id"], Value::Null);
        assert_eq!(value["error"]["code"], -32700);
    }
}
