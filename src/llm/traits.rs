use serde_json::Value;

use super::types::LlmModelTurn;

pub trait LlmClient: Send + Sync {
    fn complete_with_tools(
        &self,
        system_prompt: &str,
        user_message: &str,
        tools: Value,
    ) -> Result<LlmModelTurn, String>;
}
