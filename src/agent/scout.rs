use std::path::Path;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::traits::{AgentContext, AutonomousAgent};
use crate::domain::PhaseProfile;
use crate::tools::{read_file_range, run_ripgrep, AstUsageFinder};

pub const SCOUT_SYSTEM_PROMPT: &str = r#"Jesteś autonomicznym robotem zwiadowczym (PHASE_1_SCOUT). Twoim celem jest zebranie i skondensowanie kontekstu kodu.

Masz do dyspozycji narzędzia (tool calls):
- ripgrep — szeroki zwiad tekstowy po repozytorium
- ast_calls — precyzyjny skalpel AST: miejsca wywołań metody w pliku
- read_file — wycinek pliku po numerach linii
- finalize — zakończenie zwiadu ze skondensowanym raportem markdown

Zasada wyboru: Jeśli nie znasz lokalizacji kodu, użyj najpierw ripgrep. Gdy znasz pliki, użyj ast_calls, aby precyzyjnie wyciągnąć miejsca wywołań i odrzucić komentarze. Gdy zbierzesz esencję, wywołaj finalize.

Odpowiadaj krótkim uzasadnieniem (Thought), a następnie wywołaj dokładnie jedno narzędzie."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoutToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoutModelTurn {
    pub content: Option<String>,
    pub tool_calls: Vec<ScoutToolCall>,
}

pub trait ChatClient: Send + Sync {
    fn complete(&self, system_prompt: &str, user_message: &str) -> Result<ScoutModelTurn, String>;
}

pub fn scout_tool_definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "ripgrep",
                "description": "Szeroki zwiad tekstowy: uruchamia ripgrep z kontekstem linii.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Wzorzec wyszukiwania przekazywany do ripgrep."
                        }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "ast_calls",
                "description": "Skalpel AST: zwraca numery linii fizycznych wywołań metody (bez komentarzy i stringów).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Ścieżka do pliku .rs / .ts / .tsx."
                        },
                        "method": {
                            "type": "string",
                            "description": "Nazwa wywoływanej metody/funkcji."
                        }
                    },
                    "required": ["file", "method"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Czyta wycinek pliku po numerach linii (1-based, włącznie).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Ścieżka do pliku."
                        },
                        "start": {
                            "type": "integer",
                            "description": "Pierwsza linia (>= 1)."
                        },
                        "end": {
                            "type": "integer",
                            "description": "Ostatnia linia (>= start)."
                        }
                    },
                    "required": ["file", "start", "end"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "finalize",
                "description": "Kończy zwiad i zwraca skondensowany raport markdown.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "report": {
                            "type": "string",
                            "description": "Finalny skondensowany raport markdown."
                        }
                    },
                    "required": ["report"]
                }
            }
        }
    ])
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

impl ChatClient for DeepSeekClient {
    fn complete(&self, system_prompt: &str, user_message: &str) -> Result<ScoutModelTurn, String> {
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
            tools: scout_tool_definitions(),
            tool_choice: "auto",
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

        let message = body
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message)
            .ok_or_else(|| "deepseek returned no choices".to_string())?;

        let tool_calls = message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|call| {
                let arguments = serde_json::from_str(&call.function.arguments)
                    .map_err(|err| format!("invalid tool arguments JSON: {err}"))?;
                Ok(ScoutToolCall {
                    name: call.function.name,
                    arguments,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(ScoutModelTurn {
            content: message.content,
            tool_calls,
        })
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

    fn parse_tool_call(call: &ScoutToolCall) -> Result<ScoutAction, String> {
        match call.name.as_str() {
            "ripgrep" => {
                let pattern = required_str(&call.arguments, "pattern")?;
                Ok(ScoutAction::Ripgrep { pattern })
            }
            "ast_calls" => {
                let file = required_str(&call.arguments, "file")?;
                let method = required_str(&call.arguments, "method")?;
                Ok(ScoutAction::AstCalls { file, method })
            }
            "read_file" => {
                let file = required_str(&call.arguments, "file")?;
                let start = required_usize(&call.arguments, "start")?;
                let end = required_usize(&call.arguments, "end")?;
                Ok(ScoutAction::ReadFile { file, start, end })
            }
            "finalize" => {
                let report = required_str(&call.arguments, "report")?;
                Ok(ScoutAction::Finalize { report })
            }
            other => Err(format!("unsupported tool: {other}")),
        }
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

fn required_str(arguments: &Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("tool argument '{key}' must be a string"))
}

fn required_usize(arguments: &Value, key: &str) -> Result<usize, String> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("tool argument '{key}' must be a positive integer"))
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
        let model_turn = self.client.complete(SCOUT_SYSTEM_PROMPT, &user_message)?;

        let tool_call = model_turn
            .tool_calls
            .first()
            .ok_or_else(|| "model response missing tool call".to_string())?;

        let action = Self::parse_tool_call(tool_call)?;
        let observation = Self::execute_action(&action)?;

        let thought = model_turn.content.unwrap_or_default();
        let step = format!(
            "Thought:\n{thought}\nTool: {}({})\nObservation:\n{observation}\n",
            tool_call.name, tool_call.arguments
        );
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
    fn scout_tool_definitions_include_all_tools() {
        let tools = scout_tool_definitions();
        let names: Vec<_> = tools
            .as_array()
            .expect("tools array")
            .iter()
            .map(|tool| {
                tool["function"]["name"]
                    .as_str()
                    .expect("tool name")
                    .to_string()
            })
            .collect();

        assert_eq!(
            names,
            vec![
                "ripgrep".to_string(),
                "ast_calls".to_string(),
                "read_file".to_string(),
                "finalize".to_string()
            ]
        );
    }

    #[test]
    fn parse_tool_call_reads_ripgrep() {
        let action = ScoutAgent::<ScriptMock>::parse_tool_call(&ScoutToolCall {
            name: "ripgrep".to_string(),
            arguments: json!({ "pattern": "foo bar" }),
        })
        .expect("parse");

        assert_eq!(
            action,
            ScoutAction::Ripgrep {
                pattern: "foo bar".to_string()
            }
        );
    }

    #[test]
    fn parse_tool_call_reads_finalize() {
        let action = ScoutAgent::<ScriptMock>::parse_tool_call(&ScoutToolCall {
            name: "finalize".to_string(),
            arguments: json!({ "report": "## Scout\n- done" }),
        })
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
        fn complete(&self, _: &str, _: &str) -> Result<ScoutModelTurn, String> {
            Ok(ScoutModelTurn {
                content: None,
                tool_calls: vec![],
            })
        }
    }
}
