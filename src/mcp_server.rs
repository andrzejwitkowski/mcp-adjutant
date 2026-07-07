use std::io::{self, BufRead, Read, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::domain::AdjutantConfig;
use crate::mcp::{
    handle_generate_tests_and_scaffolding, handle_scout_context, handle_verify_and_triage,
    registered_mcp_tools, GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME, SCOUT_CONTEXT_TOOL_NAME,
    VERIFY_AND_TRIAGE_TOOL_NAME,
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

// ponytail: minimal MCP stdio loop; rmcp needs edition2024 / newer rustc
pub fn run_stdio(config: Arc<RwLock<AdjutantConfig>>) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("failed to start tokio runtime: {err}"))?;

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        let message = read_message(&stdin)?;
        let Some(message) = message else {
            break;
        };

        let request: JsonRpcRequest = serde_json::from_str(&message)
            .map_err(|err| format!("invalid JSON-RPC request: {err}"))?;

        if request.id.is_none() {
            continue;
        }

        let id = request.id.as_ref().expect("checked above");
        let response = match request.method.as_str() {
            "initialize" => ok(id, initialize_result()),
            "tools/list" => ok(id, list_tools_result()),
            "tools/call" => {
                let config = Arc::clone(&config);
                runtime.block_on(handle_tool_call(id, request.params, config))
            }
            "ping" => ok(id, json!({})),
            _ => err(id, -32601, format!("method not found: {}", request.method)),
        };

        write_message(&mut stdout, &response)?;
    }

    Ok(())
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
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
        SCOUT_CONTEXT_TOOL_NAME => handle_scout_context(arguments, config_snapshot).await,
        VERIFY_AND_TRIAGE_TOOL_NAME => handle_verify_and_triage(arguments, config_snapshot).await,
        GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME => {
            handle_generate_tests_and_scaffolding(arguments, config_snapshot).await
        }
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

fn read_message(stdin: &io::Stdin) -> Result<Option<String>, String> {
    let mut stdin_lock = stdin.lock();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        let read = stdin_lock
            .read_line(&mut line)
            .map_err(|err| format!("failed to read MCP header: {err}"))?;
        if read == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            break;
        }
        headers.push(line);
    }

    if headers.is_empty() {
        return Ok(None);
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

    String::from_utf8(body).map_err(|err| format!("invalid UTF-8 in MCP body: {err}"))
        .map(Some)
}

fn write_message(stdout: &mut io::Stdout, payload: &str) -> Result<(), String> {
    let frame = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);
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
}
