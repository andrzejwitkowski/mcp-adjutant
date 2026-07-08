use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Deserialize;

use super::traits::{AgentContext, AutonomousAgent};
use crate::cache::ProjectCacheManager;
use crate::llm::{LlmClient, LlmRequest, LlmToolSet};

pub const EVALUATOR_SYSTEM_PROMPT: &str = r#"Jesteś Surowym Inspektorem Jakości (QA_AGENT). Twoim zadaniem jest ocena pracy innych agentów AI.
Otrzymasz:
1. NAZWĘ AGENTA
2. ORYGINALNE ZADANIE
3. WYNIK JEGO PRACY

Twoim celem jest zwrócenie JEDNEGO poprawnego obiektu JSON (bez bloku markdown) o strukturze:
{
  "score": [ocena od 1 do 10],
  "critique": "[Zwięzły opis: co zrobił dobrze, a czego zabrakło, by dostać 10/10? Zwróć uwagę na halucynacje, szum w danych lub braki w asercjach]"
}
Bądź bezlitosny. Ocenę 10/10 przyznawaj tylko za idealne, chirurgiczne wykonanie."#;

#[derive(Debug, Deserialize)]
struct EvaluationPayload {
    score: i32,
    critique: String,
}

pub struct EvaluatorAgent<C: LlmClient> {
    client: C,
    cache_manager: Arc<Mutex<ProjectCacheManager>>,
    target_agent: String,
    original_task: String,
    received_output: String,
}

impl<C: LlmClient> EvaluatorAgent<C> {
    pub fn new(
        client: C,
        cache_manager: Arc<Mutex<ProjectCacheManager>>,
        target_agent: impl Into<String>,
        original_task: impl Into<String>,
        received_output: impl Into<String>,
    ) -> Self {
        Self {
            client,
            cache_manager,
            target_agent: target_agent.into(),
            original_task: original_task.into(),
            received_output: received_output.into(),
        }
    }

    fn build_user_message(&self) -> String {
        format!(
            "AGENT: {}\n\nORYGINALNE ZADANIE:\n{}\n\nWYNIK PRACY AGENTA:\n{}",
            self.target_agent, self.original_task, self.received_output
        )
    }

    fn parse_evaluation_response(raw: &str) -> Result<EvaluationPayload, String> {
        let trimmed = raw.trim();
        let json_body = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .and_then(|rest| rest.strip_suffix("```"))
            .map(str::trim)
            .unwrap_or(trimmed);

        serde_json::from_str(json_body)
            .map_err(|err| format!("failed to parse evaluator JSON response: {err}"))
    }
}

#[async_trait]
impl<C: LlmClient> AutonomousAgent for EvaluatorAgent<C> {
    fn name(&self) -> &'static str {
        "evaluator_agent"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        if !context.input_prompt.contains("QA_AGENT") {
            context.input_prompt.push_str("\n\n");
            context.input_prompt.push_str(EVALUATOR_SYSTEM_PROMPT);
        }
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        // ponytail: one-shot judge — no tool loop, just ask and parse JSON
        let user_message = self.build_user_message();
        let empty_tools = LlmToolSet::new();
        let request = LlmRequest::new(EVALUATOR_SYSTEM_PROMPT, &user_message, &empty_tools);
        let model_turn = self.client.complete(request)?;

        let raw_response = model_turn
            .content
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| "evaluator model response missing content".to_string())?;

        let evaluation = Self::parse_evaluation_response(&raw_response)?;

        if !(1..=10).contains(&evaluation.score) {
            return Err(format!(
                "evaluator score must be between 1 and 10, got {}",
                evaluation.score
            ));
        }

        let mut cache = self
            .cache_manager
            .lock()
            .map_err(|_| "cache manager lock poisoned".to_string())?;

        cache.store_evaluation(
            &self.target_agent,
            &self.original_task,
            &self.received_output,
            evaluation.score,
            &evaluation.critique,
        )?;

        let summary = format!("Ewaluacja zapisana. Ocena QA: {}/10", evaluation.score);
        context.accumulated_data = summary.clone();
        context.is_finished = true;

        Ok(())
    }

    async fn mutate_next_iteration(&self, _context: &mut AgentContext) -> Result<(), String> {
        Ok(())
    }
}
