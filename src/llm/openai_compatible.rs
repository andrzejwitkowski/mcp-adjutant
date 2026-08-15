use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde_json::Value;

use crate::domain::PhaseProfile;

use super::request::LlmRequest;
use super::sse::assemble_sse_stream;
use super::traits::LlmClient;
use super::types::LlmModelTurn;

/// OpenAI-compatible `/v1/chat/completions` transport for all configured providers.
pub struct OpenAiCompatibleClient {
    profile: PhaseProfile,
    omit_temperature: AtomicBool,
}

impl OpenAiCompatibleClient {
    pub fn new(profile: PhaseProfile) -> Self {
        Self {
            profile,
            omit_temperature: AtomicBool::new(false),
        }
    }

    pub fn with_omit_temperature(profile: PhaseProfile, omit: bool) -> Self {
        let client = Self::new(profile);
        client.omit_temperature.store(omit, Ordering::Relaxed);
        client
    }

    pub fn omits_temperature(&self) -> bool {
        self.omit_temperature.load(Ordering::Relaxed)
    }

    fn request_label(&self) -> String {
        format!("{} @ {}", self.profile.model_name, self.profile.base_url)
    }

    fn chat_completions_url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.profile.base_url.trim_end_matches('/')
        )
    }

    fn models_url(&self) -> String {
        format!("{}/models", self.profile.base_url.trim_end_matches('/'))
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    max_tokens: u32,
    stream: bool,
}

#[derive(Clone, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// Builds the JSON body for chat/completions (testable without HTTP).
pub(crate) fn build_chat_request_body(
    model: &str,
    system_prompt: &str,
    user_message: &str,
    tools_json: Option<Value>,
    tool_choice: Option<&'static str>,
    temperature: Option<f32>,
    max_tokens: u32,
) -> Value {
    let body = ChatRequest {
        model,
        messages: vec![
            ChatMessage {
                role: "system",
                content: system_prompt,
            },
            ChatMessage {
                role: "user",
                content: user_message,
            },
        ],
        tools: tools_json,
        tool_choice,
        temperature,
        max_tokens,
        stream: true,
    };
    serde_json::to_value(body).expect("ChatRequest serializes")
}

fn temperature_rejected(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("temperature")
        && (lower.contains("400")
            || lower.contains("unsupported")
            || lower.contains("invalid")
            || lower.contains("not support")
            || lower.contains("does not support"))
}

impl LlmClient for OpenAiCompatibleClient {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        let url = self.chat_completions_url();
        let has_tools = !request.tools.is_empty();
        let tools_json = if has_tools {
            Some(request.tools.to_openai_json())
        } else {
            None
        };
        let tool_choice = if has_tools { Some("required") } else { None };
        let temperature = if self.omit_temperature.load(Ordering::Relaxed) {
            None
        } else {
            Some(self.profile.temperature)
        };
        let label = self.request_label();

        let mut choice = tool_choice;
        let mut temp = temperature;
        let mut response = self.send_request(
            &url,
            request.system_prompt,
            request.user_message,
            tools_json.as_ref(),
            choice,
            temp,
            &label,
        );
        if let Err(err) = &response {
            if has_tools && err.contains("tool_choice") {
                choice = Some("auto");
                response = self.send_request(
                    &url,
                    request.system_prompt,
                    request.user_message,
                    tools_json.as_ref(),
                    choice,
                    temp,
                    &label,
                );
            }
        }
        if let Err(err) = &response {
            if temp.is_some() && temperature_rejected(err) {
                self.omit_temperature.store(true, Ordering::Relaxed);
                temp = None;
                response = self.send_request(
                    &url,
                    request.system_prompt,
                    request.user_message,
                    tools_json.as_ref(),
                    choice,
                    temp,
                    &label,
                );
            }
        }
        if let Err(err) = &response {
            if has_tools && choice != Some("auto") && err.contains("tool_choice") {
                choice = Some("auto");
                response = self.send_request(
                    &url,
                    request.system_prompt,
                    request.user_message,
                    tools_json.as_ref(),
                    choice,
                    temp,
                    &label,
                );
            }
        }
        response
    }
}

impl OpenAiCompatibleClient {
    #[allow(clippy::too_many_arguments)]
    fn send_request(
        &self,
        url: &str,
        system_prompt: &str,
        user_message: &str,
        tools: Option<&Value>,
        tool_choice: Option<&'static str>,
        temperature: Option<f32>,
        label: &str,
    ) -> Result<LlmModelTurn, String> {
        let body = build_chat_request_body(
            &self.profile.model_name,
            system_prompt,
            user_message,
            tools.cloned(),
            tool_choice,
            temperature,
            self.profile.max_tokens,
        );

        let agent = ureq::AgentBuilder::new().build();
        let mut http = agent.post(url).set("Content-Type", "application/json");

        if let Some(api_key) = &self.profile.api_key {
            http = http.set("Authorization", &format!("Bearer {api_key}"));
        }

        match http.send_json(body) {
            Ok(response) => {
                assemble_sse_stream(response.into_reader()).map_err(|err| format!("{err} ({label})"))
            }
            Err(ureq::Error::Status(code, response)) => {
                let detail = response.into_string().unwrap_or_default();
                Err(format!(
                    "LLM request failed ({label}): status {code}: {detail}"
                ))
            }
            Err(err) => Err(format!("LLM request failed ({label}): {err}")),
        }
    }

    /// GET `{base}/models` and check the configured model id is listed.
    pub fn fetch_models_list(&self) -> Result<Value, String> {
        let url = self.models_url();
        let label = self.request_label();
        let agent = ureq::AgentBuilder::new().build();
        let mut http = agent.get(&url);
        if let Some(api_key) = &self.profile.api_key {
            http = http.set("Authorization", &format!("Bearer {api_key}"));
        }
        match http.call() {
            Ok(response) => response
                .into_json()
                .map_err(|err| format!("models list parse failed ({label}): {err}")),
            Err(ureq::Error::Status(code, response)) => {
                let detail = response.into_string().unwrap_or_default();
                Err(format!(
                    "models list failed ({label}): status {code}: {detail}"
                ))
            }
            Err(err) => Err(format!("models list failed ({label}): {err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_tools_omits_tools_and_tool_choice() {
        let body =
            build_chat_request_body("gpt-5-mini", "sys", "user", None, None, Some(0.2), 1024);
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert!((body["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(body["model"], json!("gpt-5-mini"));
    }

    #[test]
    fn request_body_always_streams() {
        let body = build_chat_request_body("m", "s", "u", None, None, None, 100);
        assert_eq!(body["stream"], json!(true));
    }

    #[test]
    fn with_tools_includes_tools_and_tool_choice() {
        let tools = json!([{
            "type": "function",
            "function": { "name": "echo", "parameters": { "type": "object" } }
        }]);
        let body = build_chat_request_body(
            "gpt-5-mini",
            "sys",
            "user",
            Some(tools.clone()),
            Some("required"),
            Some(0.0),
            512,
        );
        assert_eq!(body["tools"], tools);
        assert_eq!(body["tool_choice"], json!("required"));
    }

    #[test]
    fn omitted_temperature_absent_from_body() {
        let body = build_chat_request_body("m", "s", "u", None, None, None, 100);
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn temperature_rejected_detects_common_errors() {
        assert!(temperature_rejected(
            "LLM request failed: status 400: temperature is not supported"
        ));
        assert!(!temperature_rejected("status 500: internal error"));
    }
}
