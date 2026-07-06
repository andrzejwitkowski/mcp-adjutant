use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmModelTurn {
    pub content: Option<String>,
    pub tool_calls: Vec<LlmToolCall>,
}
