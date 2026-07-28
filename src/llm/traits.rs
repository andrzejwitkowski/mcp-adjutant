use std::sync::Arc;

use super::request::LlmRequest;
use super::types::LlmModelTurn;

pub trait LlmClient: Send + Sync {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String>;
}

impl<C: LlmClient + ?Sized> LlmClient for Arc<C> {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        (**self).complete(request)
    }
}
