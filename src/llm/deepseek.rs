use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::PhaseProfile;

use super::request::LlmRequest;
use super::traits::LlmClient;
use super::types::{LlmModelTurn, LlmToolCall};

pub struct DeepSeekClient {
    profile: PhaseProfile,
}

impl DeepSeekClient {
    pub fn new(profile: PhaseProfile) -> Self {
        Self { profile }
    }

    fn request_label(&self) -> String {
        format!("{} @ {}", self.profile.model_name, self.profile.base_url)
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    tools: Value,
    tool_choice: &'static str,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ApiToolCall>>,
}

#[derive(Deserialize)]
struct ApiToolCall {
    function: ApiToolFunction,
}

#[derive(Deserialize)]
struct ApiToolFunction {
    name: String,
    arguments: String,
}

impl LlmClient for DeepSeekClient {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        let url = format!(
            "{}/chat/completions",
            self.profile.base_url.trim_end_matches('/')
        );
        let body = ChatRequest {
            model: &self.profile.model_name,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: request.system_prompt,
                },
                ChatMessage {
                    role: "user",
                    content: request.user_message,
                },
            ],
            tools: request.tools.to_openai_json(),
            tool_choice: if request.tools.is_empty() {
                "auto"
            } else {
                "required"
            },
            temperature: self.profile.temperature,
            max_tokens: self.profile.max_tokens,
        };

        let agent = ureq::AgentBuilder::new().build();
        let mut http = agent.post(&url).set("Content-Type", "application/json");

        if let Some(api_key) = &self.profile.api_key {
            http = http.set("Authorization", &format!("Bearer {api_key}"));
        }

        let label = self.request_label();
        let response = match http.send_json(body) {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) => {
                let detail = response.into_string().unwrap_or_default();
                return Err(format!(
                    "LLM request failed ({label}): status {code}: {detail}"
                ));
            }
            Err(err) => return Err(format!("LLM request failed ({label}): {err}")),
        };

        let body: ChatResponse = response
            .into_json()
            .map_err(|err| format!("LLM response parse failed ({label}): {err}"))?;

        let message = body
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message)
            .ok_or_else(|| format!("LLM returned no choices ({label})"))?;

        let tool_calls = message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|call| {
                let arguments = serde_json::from_str(&call.function.arguments)
                    .map_err(|err| format!("invalid tool arguments JSON: {err}"))?;
                Ok(LlmToolCall {
                    name: call.function.name,
                    arguments,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(LlmModelTurn {
            content: message.content,
            tool_calls,
        })
    }
}
