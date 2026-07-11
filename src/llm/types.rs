use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LlmUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cached_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmModelTurn {
    pub content: Option<String>,
    pub tool_calls: Vec<LlmToolCall>,
    pub usage: Option<LlmUsage>,
}

impl Default for LlmModelTurn {
    fn default() -> Self {
        Self {
            content: None,
            tool_calls: vec![],
            usage: None,
        }
    }
}
