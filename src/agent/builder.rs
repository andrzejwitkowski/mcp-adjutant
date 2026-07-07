mod tools;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::traits::{AgentContext, AutonomousAgent};
use super::{BuildCommandDiscoverer, BuildCommandRunner, TriageAgent};
use crate::cache::ProjectCacheManager;
use crate::domain::AdjutantConfig;
use crate::llm::{LlmClient, LlmRequest, LlmToolSet};

pub use tools::{
    builder_tool_set, gather_integration_context, generate_test_factory, parse_components,
    parse_factory_arguments, parse_write_test_suite_arguments,
};

pub const BUILDER_SYSTEM_PROMPT: &str = r#"Jesteś autonomicznym robotnikiem TDD (PHASE_4_BUILDER). Generujesz testy jednostkowe, integracyjne oraz fabryki danych.

Masz do dyspozycji narzędzia (tool calls):
- gather_integration_context — sygnatury z cache semantycznego i AST (przed testami integracyjnymi)
- generate_test_factory — szkielet Fluent Buildera dla struktury
- write_test_suite — zapis pliku testowego z fazą TDD (red|green|refactor)

Odpowiadaj krótkim uzasadnieniem (Thought), a następnie wywołaj narzędzia."#;

pub struct BuilderAgent<C, TC, B, D> {
    llm_client: C,
    cache_manager: Arc<Mutex<ProjectCacheManager>>,
    triage_agent: TriageAgent<TC, B, D>,
    tools: LlmToolSet,
}

impl<C: LlmClient, TC: LlmClient, B: BuildCommandRunner, D: BuildCommandDiscoverer>
    BuilderAgent<C, TC, B, D>
{
    pub fn new(
        llm_client: C,
        cache_manager: Arc<Mutex<ProjectCacheManager>>,
        triage_agent: TriageAgent<TC, B, D>,
    ) -> Self {
        Self {
            llm_client,
            cache_manager,
            triage_agent,
            tools: builder_tool_set(),
        }
    }

    pub fn with_tools(
        llm_client: C,
        cache_manager: Arc<Mutex<ProjectCacheManager>>,
        triage_agent: TriageAgent<TC, B, D>,
        tools: LlmToolSet,
    ) -> Self {
        Self {
            llm_client,
            cache_manager,
            triage_agent,
            tools,
        }
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

    fn triage_directive(tdd_phase: &str) -> &'static str {
        match tdd_phase {
            "red" => "TDD RED PHASE: Kod MUSI się bezbłędnie skompilować (napraw braki importów, literówki w typach). Testy MUSZĄ oblewać asercje. NIE DOTYKAJ logiki asercji.",
            "refactor" => "TDD REFACTOR PHASE: Kod musi się skompilować, a wszystkie testy MUSZĄ przejść na zielono po refaktoryzacji.",
            _ => "TDD GREEN PHASE: Kod musi się skompilować, a wszystkie testy MUSZĄ przejść na zielono. Jeśli są błędy, napraw je.",
        }
    }

    fn triage_success(triage_ctx: &AgentContext, tdd_phase: &str) -> bool {
        if !triage_ctx.is_finished {
            return false;
        }

        match tdd_phase {
            "red" => triage_ctx
                .input_prompt
                .contains("kompilacja udana, testy oblane"),
            _ => triage_ctx.input_prompt.contains("sukcesem"),
        }
    }
}

#[async_trait]
impl<C: LlmClient, TC: LlmClient, B: BuildCommandRunner, D: BuildCommandDiscoverer> AutonomousAgent
    for BuilderAgent<C, TC, B, D>
{
    fn name(&self) -> &'static str {
        "PHASE_4_BUILDER"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        if !context.input_prompt.contains("PHASE_4_BUILDER") {
            context.input_prompt.push_str("\n\n");
            context.input_prompt.push_str(BUILDER_SYSTEM_PROMPT);
        }
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        let user_message = Self::build_user_message(context);
        let request = LlmRequest::new(BUILDER_SYSTEM_PROMPT, &user_message, &self.tools);
        let model_turn = self.llm_client.complete(request)?;

        if model_turn.tool_calls.is_empty() {
            let thought = model_turn.content.unwrap_or_default();
            if thought.is_empty() {
                return Err("model response missing tool call".to_string());
            }
            context.accumulated_data.push_str(&format!(
                "Thought:\n{thought}\n(model nie wywołał narzędzia — kontynuuj)\n"
            ));
            return Ok(());
        }

        let thought = model_turn.content.unwrap_or_default();
        if !thought.is_empty() {
            context
                .accumulated_data
                .push_str(&format!("Thought:\n{thought}\n"));
        }

        for tool_call in &model_turn.tool_calls {
            context.accumulated_data.push_str(&format!(
                "Tool: {}({})\n",
                tool_call.name, tool_call.arguments
            ));

            match tool_call.name.as_str() {
                "gather_integration_context" => {
                    let components = parse_components(&tool_call.arguments)?;
                    let cache = self
                        .cache_manager
                        .lock()
                        .map_err(|_| "cache manager lock poisoned".to_string())?;
                    let output = gather_integration_context(&cache, &components)?;
                    context
                        .accumulated_data
                        .push_str(&format!("Observation:\n{output}\n"));
                }
                "generate_test_factory" => {
                    let (target_struct, target_file) =
                        parse_factory_arguments(&tool_call.arguments)?;
                    let factory = generate_test_factory(&target_struct, &target_file);
                    context
                        .accumulated_data
                        .push_str(&format!("Observation:\n{factory}\n"));
                }
                "write_test_suite" => {
                    let (path, content, tdd_phase) =
                        parse_write_test_suite_arguments(&tool_call.arguments)?;
                    let path_buf = PathBuf::from(&path);

                    if let Some(parent) = path_buf.parent() {
                        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
                    }
                    std::fs::write(&path_buf, &content).map_err(|err| err.to_string())?;

                    let triage_directive = Self::triage_directive(&tdd_phase);
                    context.accumulated_data.push_str(&format!(
                        "\n[SYSTEM]: Launching Triage ({tdd_phase}) for {path}\n"
                    ));

                    self.triage_agent.retarget(vec![path_buf.clone()])?;

                    let mut triage_ctx = AgentContext {
                        input_prompt: format!("Verify {path}:\n{triage_directive}"),
                        accumulated_data: String::new(),
                        iterations: 0,
                        max_iterations: 3,
                        is_finished: false,
                    };

                    self.triage_agent.enrich_context(&mut triage_ctx).await?;
                    self.triage_agent
                        .process_and_evaluate(&mut triage_ctx)
                        .await?;

                    context.accumulated_data.push_str(&format!(
                        "\n[TRIAGE RESULT]: {}\n",
                        triage_ctx.accumulated_data
                    ));

                    if Self::triage_success(&triage_ctx, &tdd_phase) {
                        context.is_finished = true;
                    } else {
                        context.accumulated_data.push_str(
                            "\n[TRIAGE FAILURE]: triage did not reach expected TDD outcome\n",
                        );
                    }
                }
                other => {
                    return Err(format!("unsupported builder tool: {other}"));
                }
            }
        }

        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        context
            .input_prompt
            .push_str("\nKontynuuj generowanie testów na podstawie ostatniej obserwacji.");
        Ok(())
    }
}

pub type DefaultBuilderAgent<C, TC> =
    BuilderAgent<C, TC, super::SystemBuildRunner, super::NoopBuildDiscoverer>;

pub fn default_builder_agent<C: LlmClient, TC: LlmClient>(
    llm_client: C,
    cache_manager: Arc<Mutex<ProjectCacheManager>>,
    triage_llm_client: TC,
    config: Arc<AdjutantConfig>,
    target_paths: Vec<PathBuf>,
) -> DefaultBuilderAgent<C, TC> {
    let triage_agent = TriageAgent::new(triage_llm_client, target_paths, config);
    BuilderAgent::new(llm_client, cache_manager, triage_agent)
}
