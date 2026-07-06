use super::request::LlmRequest;
use super::types::LlmModelTurn;

pub trait LlmClient: Send + Sync {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String>;
}
