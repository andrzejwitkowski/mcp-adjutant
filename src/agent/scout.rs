mod tools;

use async_trait::async_trait;

use super::traits::{AgentContext, AutonomousAgent};
use crate::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall, LlmToolSet};

pub use tools::scout_tool_set;

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

pub struct ScoutAgent<C: LlmClient> {
    client: C,
    tools: LlmToolSet,
}

impl<C: LlmClient> ScoutAgent<C> {
    pub fn new(client: C) -> Self {
        Self {
            client,
            tools: scout_tool_set(),
        }
    }

    pub fn with_tools(client: C, tools: LlmToolSet) -> Self {
        Self { client, tools }
    }

    pub fn tools(&self) -> &LlmToolSet {
        &self.tools
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
        let request = LlmRequest::new(SCOUT_SYSTEM_PROMPT, &user_message, &self.tools);
        let model_turn = self.client.complete(request)?;

        let tool_call = model_turn
            .tool_calls
            .first()
            .ok_or_else(|| "model response missing tool call".to_string())?;

        let invocation = self.tools.invoke(&tool_call.name, &tool_call.arguments)?;

        let thought = model_turn.content.unwrap_or_default();
        let step = format!(
            "Thought:\n{thought}\nTool: {}({})\nObservation:\n{}\n",
            tool_call.name, tool_call.arguments, invocation.output
        );
        context.accumulated_data.push_str(&step);

        if invocation.is_terminal {
            context.accumulated_data = invocation.output;
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
