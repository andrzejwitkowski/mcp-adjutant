use std::path::Path;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::traits::{AgentContext, AutonomousAgent};
use crate::domain::PhaseProfile;
use crate::tools::{read_file_range, run_ripgrep, AstUsageFinder};

pub const SCOUT_SYSTEM_PROMPT: &str = r#"Jesteś autonomicznym robotem zwiadowczym (PHASE_1_SCOUT). Twoim celem jest zebranie i skondensowanie kontekstu kodu.
Masz do dyspozycji 3 akcje, które możesz wywołać, pisząc dokładnie:
- ACTION: ripgrep(pattern="fraza")
- ACTION: ast_calls(file="sciezka", method="nazwa")
- ACTION: read_file(file="sciezka", start=10, end=30)
- ACTION: finalize(report="TU_TWÓJ_FINALNY_SKONDENSOWANY_RAPORT_MARKDOWN")

Zasada wyboru: Jeśli nie znasz lokalizacji kodu, użyj najpierw 'ripgrep' (szeroka sieć). Gdy znasz pliki, użyj 'ast_calls' (skalpel AST), aby precyzyjnie wyciągnąć miejsca wywołań i odrzucić komentarze. Gdy zbierzesz esencję, wywołaj 'finalize'.

Odpowiadaj sekwencją Thought -> Action -> Observation."#;

pub trait ChatClient: Send + Sync {
    fn complete(&self, system_prompt: &str, user_message: &str) -> Result<String, String>;
}

pub struct DeepSeekClient {
    profile: PhaseProfile,
}

impl DeepSeekClient {
    pub fn new(profile: PhaseProfile) -> Self {
        Self { profile }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
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
    content: String,
}

impl ChatClient for DeepSeekClient {
    fn complete(&self, system_prompt: &str, user_message: &str) -> Result<String, String> {
        let url = format!(
            "{}/chat/completions",
            self.profile.base_url.trim_end_matches('/')
        );
        let body = ChatRequest {
            model: &self.profile.model_name,
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
            temperature: self.profile.temperature,
            max_tokens: self.profile.max_tokens,
        };

        let agent = ureq::AgentBuilder::new().build();
        let mut http = agent.post(&url).set("Content-Type", "application/json");

        if let Some(api_key) = &self.profile.api_key {
            http = http.set("Authorization", &format!("Bearer {api_key}"));
        }

        let response = http
            .send_json(body)
            .map_err(|err| format!("deepseek request failed: {err}"))?;

        let body: ChatResponse = response
            .into_json()
            .map_err(|err| format!("deepseek response parse failed: {err}"))?;

        body.choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content)
            .ok_or_else(|| "deepseek returned no choices".to_string())
    }
}

pub struct ScoutAgent<C: ChatClient> {
    client: C,
}

impl<C: ChatClient> ScoutAgent<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }

    fn build_user_message(context: &AgentContext) -> String {
        if context.accumulated_data.is_empty() {
            context.input_prompt.clone()
        } else {
            format!(
                "{}\n\n---\nHistoria obserwacji:\n{}",
                context.input_prompt, context.accumulated_data
            )
        }
    }

    fn parse_action(response: &str) -> Result<ScoutAction, String> {
        let action_line = response
            .lines()
            .find(|line| line.trim_start().starts_with("ACTION:"))
            .ok_or_else(|| "model response missing ACTION line".to_string())?;

        let action_body = action_line
            .split_once("ACTION:")
            .map(|(_, body)| body.trim())
            .unwrap_or(action_line);

        if let Some(inner) = action_body
            .strip_prefix("ripgrep(pattern=")
            .and_then(|s| s.strip_suffix(')'))
        {
            let pattern = unquote(inner)?;
            return Ok(ScoutAction::Ripgrep { pattern });
        }

        if action_body.starts_with("ast_calls(") && action_body.ends_with(')') {
            let inner = &action_body["ast_calls(".len()..action_body.len() - 1];
            let file = parse_kv(inner, "file")?;
            let method = parse_kv(inner, "method")?;
            return Ok(ScoutAction::AstCalls { file, method });
        }

        if action_body.starts_with("read_file(") && action_body.ends_with(')') {
            let inner = &action_body["read_file(".len()..action_body.len() - 1];
            let file = parse_kv(inner, "file")?;
            let start = parse_usize_kv(inner, "start")?;
            let end = parse_usize_kv(inner, "end")?;
            return Ok(ScoutAction::ReadFile { file, start, end });
        }

        if action_body.starts_with("finalize(report=") && action_body.ends_with(')') {
            let inner = &action_body["finalize(report=".len()..action_body.len() - 1];
            let report = unquote(inner)?;
            return Ok(ScoutAction::Finalize { report });
        }

        Err(format!("unsupported ACTION: {action_body}"))
    }

    fn execute_action(action: &ScoutAction) -> Result<String, String> {
        match action {
            ScoutAction::Ripgrep { pattern } => run_ripgrep(pattern),
            ScoutAction::AstCalls { file, method } => {
                let lines = AstUsageFinder::find_calls_in_file(Path::new(file), method)?;
                if lines.is_empty() {
                    Ok("No call sites found.".to_string())
                } else {
                    Ok(format!("Call sites at lines: {lines:?}"))
                }
            }
            ScoutAction::ReadFile { file, start, end } => {
                read_file_range(Path::new(file), *start, *end)
            }
            ScoutAction::Finalize { report } => Ok(report.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ScoutAction {
    Ripgrep {
        pattern: String,
    },
    AstCalls {
        file: String,
        method: String,
    },
    ReadFile {
        file: String,
        start: usize,
        end: usize,
    },
    Finalize {
        report: String,
    },
}

fn parse_kv(payload: &str, key: &str) -> Result<String, String> {
    let needle = format!("{key}=");
    let segment = payload
        .split(',')
        .map(str::trim)
        .find(|part| part.starts_with(&needle))
        .ok_or_else(|| format!("missing {key} in ACTION"))?;

    let raw = segment[needle.len()..].trim();
    unquote(raw)
}

fn parse_usize_kv(payload: &str, key: &str) -> Result<usize, String> {
    let needle = format!("{key}=");
    let segment = payload
        .split(',')
        .map(str::trim)
        .find(|part| part.starts_with(&needle))
        .ok_or_else(|| format!("missing {key} in ACTION"))?;

    let raw = segment[needle.len()..].trim();
    raw.parse::<usize>()
        .map_err(|_| format!("{key} must be a positive integer"))
}

fn unquote(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        Ok(value[1..value.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\n", "\n"))
    } else {
        Err(format!("expected quoted value, got: {value}"))
    }
}

#[async_trait]
impl<C: ChatClient> AutonomousAgent for ScoutAgent<C> {
    fn name(&self) -> &'static str {
        "scout_agent"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        if !context.input_prompt.contains("PHASE_1_SCOUT") {
            context.input_prompt.push_str("\n\n");
            context.input_prompt.push_str(SCOUT_SYSTEM_PROMPT);
        }
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        let user_message = Self::build_user_message(context);
        let model_response = self.client.complete(SCOUT_SYSTEM_PROMPT, &user_message)?;

        let action = Self::parse_action(&model_response)?;
        let observation = Self::execute_action(&action)?;

        let step = format!("Thought/Action:\n{model_response}\nObservation:\n{observation}\n");
        context.accumulated_data.push_str(&step);

        if let ScoutAction::Finalize { report } = action {
            context.accumulated_data = report;
            context.is_finished = true;
        }

        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        context
            .input_prompt
            .push_str("\nKontynuuj zwiad na podstawie ostatniej obserwacji.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_action_reads_ripgrep() {
        let action = ScoutAgent::<ScriptMock>::parse_action(
            "Thought: szukam\nACTION: ripgrep(pattern=\"foo bar\")\n",
        )
        .expect("parse");

        assert_eq!(
            action,
            ScoutAction::Ripgrep {
                pattern: "foo bar".to_string()
            }
        );
    }

    #[test]
    fn parse_action_reads_finalize_with_escapes() {
        let action = ScoutAgent::<ScriptMock>::parse_action(
            "ACTION: finalize(report=\"## Scout\\n- done\")\n",
        )
        .expect("parse");

        assert_eq!(
            action,
            ScoutAction::Finalize {
                report: "## Scout\n- done".to_string()
            }
        );
    }

    struct ScriptMock;

    impl ChatClient for ScriptMock {
        fn complete(&self, _: &str, _: &str) -> Result<String, String> {
            Ok(String::new())
        }
    }
}
