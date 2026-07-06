use super::tools::LlmToolSet;

pub struct LlmRequest<'a> {
    pub system_prompt: &'a str,
    pub user_message: &'a str,
    pub tools: &'a LlmToolSet,
}

impl<'a> LlmRequest<'a> {
    pub fn new(system_prompt: &'a str, user_message: &'a str, tools: &'a LlmToolSet) -> Self {
        Self {
            system_prompt,
            user_message,
            tools,
        }
    }
}
