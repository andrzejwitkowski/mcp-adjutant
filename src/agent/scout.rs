use std::path::Path;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::traits::{AgentContext, AutonomousAgent};
use crate::llm::{LlmClient, LlmModelTurn, LlmToolCall};
use crate::tools::{
    detect_file_language, detect_project_languages, read_file_range, run_ripgrep, AstUsageFinder,
};

pub const SCOUT_SYSTEM_PROMPT: &str = r#"Jesteś autonomicznym robotem zwiadowczym (PHASE_1_SCOUT). Twoim celem jest zebranie i skondensowanie kontekstu kodu.

Masz do dyspozycji narzędzia (tool calls):
- detect_language — wykrywa język pliku lub projektu (rozszerzenie, manifesty, heurystyki treści)
- ripgrep — szeroki zwiad tekstowy po repozytorium
- ast_calls — precyzyjny skalpel AST: miejsca wywołań metody w pliku (Rust, TS, Python, Java, Kotlin, SQL, C, C++)
- read_file — wycinek pliku po numerach linii
- finalize — zakończenie zwiadu ze skondensowanym raportem markdown

Zasada wyboru: Gdy nie znasz języka ani struktury repo, użyj detect_language. Jeśli nie znasz lokalizacji kodu, użyj ripgrep. Gdy znasz pliki, użyj ast_calls. Gdy zbierzesz esencję, wywołaj finalize.

Odpowiadaj krótkim uzasadnieniem (Thought), a następnie wywołaj dokładnie jedno narzędzie."#;

pub type ScoutToolCall = LlmToolCall;
pub type ScoutModelTurn = LlmModelTurn;

pub fn scout_tool_definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "detect_language",
                "description": "Wykrywa język pliku lub projektu na podstawie rozszerzenia, markerów (Cargo.toml, package.json, ...) i heurystyk treści.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Ścieżka do pliku lub katalogu projektu."
                        },
                        "scope": {
                            "type": "string",
                            "enum": ["file", "project"],
                            "description": "file = pojedynczy plik, project = skan katalogu repo."
                        }
                    },
                    "required": ["path", "scope"]
                }
            }
        },
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
                            "description": "Ścieżka do pliku źródłowego (np. .rs, .py, .java, .kt, .sql, .c, .cpp)."
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

pub struct ScoutAgent<C: LlmClient> {
    client: C,
}

impl<C: LlmClient> ScoutAgent<C> {
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

    fn parse_tool_call(call: &LlmToolCall) -> Result<ScoutAction, String> {
        match call.name.as_str() {
            "detect_language" => {
                let path = required_str(&call.arguments, "path")?;
                let scope = required_str(&call.arguments, "scope")?;
                Ok(ScoutAction::DetectLanguage { path, scope })
            }
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
            ScoutAction::DetectLanguage { path, scope } => {
                let report = match scope.as_str() {
                    "file" => serde_json::to_string(&detect_file_language(Path::new(path))?),
                    "project" => serde_json::to_string(&detect_project_languages(Path::new(path))?),
                    other => {
                        return Err(format!(
                            "detect_language scope must be file|project, got: {other}"
                        ))
                    }
                }
                .map_err(|err| format!("failed to serialize language report: {err}"))?;
                Ok(report)
            }
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
    DetectLanguage {
        path: String,
        scope: String,
    },
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
impl<C: LlmClient> AutonomousAgent for ScoutAgent<C> {
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
        let model_turn = self.client.complete_with_tools(
            SCOUT_SYSTEM_PROMPT,
            &user_message,
            scout_tool_definitions(),
        )?;

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
    use crate::llm::LlmClient;

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
                "detect_language".to_string(),
                "ripgrep".to_string(),
                "ast_calls".to_string(),
                "read_file".to_string(),
                "finalize".to_string()
            ]
        );
    }

    #[test]
    fn parse_tool_call_reads_ripgrep() {
        let action = ScoutAgent::<ScriptMock>::parse_tool_call(&LlmToolCall {
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
        let action = ScoutAgent::<ScriptMock>::parse_tool_call(&LlmToolCall {
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

    impl LlmClient for ScriptMock {
        fn complete_with_tools(&self, _: &str, _: &str, _: Value) -> Result<LlmModelTurn, String> {
            Ok(LlmModelTurn {
                content: None,
                tool_calls: vec![],
            })
        }
    }
}
