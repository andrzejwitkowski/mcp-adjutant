use std::io::{BufRead, BufReader, Read};

use serde::Deserialize;

use super::types::{LlmModelTurn, LlmToolCall, LlmUsage};

#[derive(Deserialize)]
struct ChatUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Deserialize)]
struct PromptTokensDetails {
    cached_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct StreamChunk {
    choices: Option<Vec<StreamChoice>>,
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: Option<StreamDelta>,
}

#[derive(Deserialize)]
struct StreamDelta {
    content: Option<String>,
    tool_calls: Option<Vec<StreamToolCallDelta>>,
}

#[derive(Deserialize)]
struct StreamToolCallDelta {
    index: Option<usize>,
    function: Option<StreamFunctionDelta>,
}

#[derive(Deserialize)]
struct StreamFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Default)]
struct StreamAccumulator {
    content: String,
    /// Indexed by OpenAI tool-call `index`.
    tool_calls: Vec<Option<PendingToolCall>>,
    usage: Option<ChatUsage>,
}

#[derive(Default)]
struct PendingToolCall {
    name: String,
    arguments: String,
}

fn map_chat_usage(usage: ChatUsage) -> LlmUsage {
    let prompt_tokens = usage.prompt_tokens.unwrap_or(0);
    let completion_tokens = usage.completion_tokens.unwrap_or(0);
    let total_tokens = usage
        .total_tokens
        .unwrap_or(prompt_tokens + completion_tokens);
    let cached_tokens = usage
        .prompt_tokens_details
        .and_then(|d| d.cached_tokens)
        .unwrap_or(0);
    LlmUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
        cached_tokens,
    }
}

/// Reads an OpenAI-compatible SSE body into a single [`LlmModelTurn`].
pub(crate) fn assemble_sse_stream(reader: impl Read) -> Result<LlmModelTurn, String> {
    let mut acc = StreamAccumulator::default();
    for line in BufReader::new(reader).lines() {
        let line = line.map_err(|err| format!("LLM stream read failed: {err}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(payload) = trimmed.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload == "[DONE]" {
            break;
        }
        apply_data(payload, &mut acc)?;
    }
    acc.into_turn()
}

fn apply_data(payload: &str, acc: &mut StreamAccumulator) -> Result<(), String> {
    let chunk: StreamChunk = serde_json::from_str(payload)
        .map_err(|err| format!("LLM stream chunk parse failed: {err}: {payload}"))?;
    if let Some(usage) = chunk.usage {
        acc.usage = Some(usage);
    }
    let Some(delta) = chunk
        .choices
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.delta)
    else {
        return Ok(());
    };
    if let Some(content) = delta.content {
        acc.content.push_str(&content);
    }
    for tool_delta in delta.tool_calls.unwrap_or_default() {
        let index = tool_delta.index.unwrap_or(0);
        if acc.tool_calls.len() <= index {
            acc.tool_calls.resize_with(index + 1, || None);
        }
        let slot = acc.tool_calls[index].get_or_insert_with(PendingToolCall::default);
        if let Some(f) = tool_delta.function {
            if let Some(name) = f.name.filter(|n| !n.is_empty()) {
                slot.name = name;
            }
            if let Some(args) = f.arguments {
                slot.arguments.push_str(&args);
            }
        }
    }
    Ok(())
}

impl StreamAccumulator {
    fn into_turn(self) -> Result<LlmModelTurn, String> {
        let tool_calls = self
            .tool_calls
            .into_iter()
            .flatten()
            .map(|call| {
                let arguments = serde_json::from_str(&call.arguments)
                    .map_err(|err| format!("invalid tool arguments JSON: {err}"))?;
                Ok(LlmToolCall {
                    name: call.name,
                    arguments,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(LlmModelTurn {
            content: (!self.content.is_empty()).then_some(self.content),
            tool_calls,
            usage: self.usage.map(map_chat_usage),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn content_only() {
        let sse = "\
data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
data: [DONE]\n\n";
        let turn = assemble_sse_stream(Cursor::new(sse)).unwrap();
        assert_eq!(turn.content.as_deref(), Some("Hello"));
        assert!(turn.tool_calls.is_empty());
    }

    #[test]
    fn merges_tool_call_argument_fragments() {
        let sse = "\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"ripgrep\",\"arguments\":\"{\\\"pattern\\\"\"}}]}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"token\\\"}\"}}]}}]}\n\n\
data: [DONE]\n\n";
        let turn = assemble_sse_stream(Cursor::new(sse)).unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "ripgrep");
        assert_eq!(turn.tool_calls[0].arguments, json!({"pattern": "token"}));
    }

    #[test]
    fn captures_usage_on_final_chunk() {
        let sse = "\
data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2,\"total_tokens\":12}}\n\n\
data: [DONE]\n\n";
        let turn = assemble_sse_stream(Cursor::new(sse)).unwrap();
        let usage = turn.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 2);
        assert_eq!(usage.total_tokens, 12);
    }

    #[test]
    fn stops_at_done() {
        let sse = "\
data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
data: [DONE]\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"ignored\"}}]}\n\n";
        let turn = assemble_sse_stream(Cursor::new(sse)).unwrap();
        assert_eq!(turn.content.as_deref(), Some("ok"));
    }

    #[test]
    fn malformed_payload_errors() {
        let err = assemble_sse_stream(Cursor::new("data: {not-json}\n\n")).unwrap_err();
        assert!(err.contains("parse failed"), "{err}");
    }

    #[test]
    fn empty_tool_arguments_error() {
        let sse = "\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"x\",\"arguments\":\"\"}}]}}]}\n\n\
data: [DONE]\n\n";
        let err = assemble_sse_stream(Cursor::new(sse)).unwrap_err();
        assert!(err.contains("invalid tool arguments"), "{err}");
    }
}
