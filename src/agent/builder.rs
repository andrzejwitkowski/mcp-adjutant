mod tools;

use std::path::{Component, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::traits::{AgentContext, AutonomousAgent};
use super::{
    AgentLoopOrchestrator, BuildCommandDiscoverer, BuildCommandRunner, ScoutAgent, TriageAgent,
};
use crate::cache::ProjectCacheManager;
use crate::domain::AdjutantConfig;
use crate::llm::{LlmClient, LlmRequest, LlmToolSet};

use super::builder_prompt::{source_file_from_builder_prompt, validate_test_path_for_source};

pub use tools::{
    build_scout_factory_query, build_scout_integration_query, builder_tool_set,
    extract_test_content, parse_components, parse_factory_arguments,
    parse_write_test_suite_arguments,
};

pub const BUILDER_SYSTEM_PROMPT: &str = r#"You are an autonomous TDD worker (PHASE_4_BUILDER). You generate unit tests, integration tests, and data factories.

Available tools (tool calls):
- gather_integration_context — runs a Scout sub-agent (ripgrep, AST, read_file) before integration tests
- generate_test_factory — runs Scout to produce an idiomatic factory/fixture for a type (language agnostic)
- write_test_suite — writes a test file with a TDD phase (red|green|refactor). Put the full test file contents in your message; the tool only takes path and tdd_phase.

Selection rule: unit tests -> write_test_suite directly (skip gather_integration_context). Write to a new test file matching the source language — never overwrite the source file. integration tests -> gather_integration_context then write_test_suite. factories -> generate_test_factory.

TDD workflow: write_test_suite(tdd_phase=red) then write_test_suite(tdd_phase=green). RED only proves compile + failing assertions. The job is NOT done until GREEN triage passes (all tests pass). Do not stop after RED.

Deliverable requirements (mandatory — MCP output is a structured report, not a tool transcript):
- Repo-relative test file path and the full test source you wrote (or a diff)
- Build command run, exit code, and a log excerpt (last ~40 lines) proving pass/fail
- Test file extension must match source language (tsx source -> .test.tsx, rust -> .rs, etc.)
- Cover every function/symbol named in the task — never skip scope without file:line proof that existing tests already cover it
- On env/compile errors: include the error output and attempt pathing/fix before giving up

Reply with a short rationale (Thought), then call tools."#;

const BUILDER_TRIAGE_MAX_ITERATIONS: u32 = 3;
const BUILDER_SCOUT_MAX_ITERATIONS: u32 = 8;
// ponytail: blank LLM turns — nudge then evidenced FAIL; never Err the MCP job
const EMPTY_TOOL_TURN_LIMIT: u32 = 3;

fn resolve_test_output_path(project_root: &std::path::Path, path: &str) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute()
        || candidate
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err("test output path must stay under project root".to_string());
    }

    if candidate
        .file_name()
        .is_some_and(|name| name == "cache_manager_tests.rs")
    {
        return Err(
            "refusing to overwrite tests/cache_manager_tests.rs — write a new file under tests/"
                .to_string(),
        );
    }

    Ok(project_root.join(candidate))
}

/// Returns true when blank-cap reached (caller should finish).
fn record_empty_tool_turn(
    accumulated: &mut String,
    thought: &str,
    consecutive_empty: &AtomicU32,
) -> bool {
    if thought.is_empty() {
        accumulated.push_str(
            "Observation:\n(model returned no tool call — call write_test_suite with path under tests/ and content)\n",
        );
    } else {
        accumulated.push_str(&format!(
            "Thought:\n{thought}\nObservation:\n(model did not call a tool — call write_test_suite with path under tests/ and content)\n"
        ));
    }
    let blanks = consecutive_empty.fetch_add(1, Ordering::Relaxed) + 1;
    if blanks < EMPTY_TOOL_TURN_LIMIT {
        return false;
    }
    accumulated.push_str(&format!(
        "\n[BUILDER FAIL EVIDENCE]\nno tool calls for {EMPTY_TOOL_TURN_LIMIT} consecutive turns\nno GREEN\n"
    ));
    true
}

pub struct BuilderAgent<C, SC, TC, B, D>
where
    SC: LlmClient,
{
    llm_client: C,
    cache_manager: Arc<Mutex<ProjectCacheManager>>,
    scout_agent: ScoutAgent<SC>,
    triage_agent: TriageAgent<TC, B, D>,
    tools: LlmToolSet,
    consecutive_empty_turns: AtomicU32,
    source_file: PathBuf,
}

impl<
        C: LlmClient,
        SC: LlmClient,
        TC: LlmClient,
        B: BuildCommandRunner,
        D: BuildCommandDiscoverer,
    > BuilderAgent<C, SC, TC, B, D>
{
    pub fn new(
        llm_client: C,
        cache_manager: Arc<Mutex<ProjectCacheManager>>,
        scout_agent: ScoutAgent<SC>,
        triage_agent: TriageAgent<TC, B, D>,
        source_file: PathBuf,
    ) -> Self {
        Self {
            llm_client,
            cache_manager,
            scout_agent,
            triage_agent,
            tools: builder_tool_set(),
            consecutive_empty_turns: AtomicU32::new(0),
            source_file,
        }
    }

    pub fn with_tools(
        llm_client: C,
        cache_manager: Arc<Mutex<ProjectCacheManager>>,
        scout_agent: ScoutAgent<SC>,
        triage_agent: TriageAgent<TC, B, D>,
        tools: LlmToolSet,
        source_file: PathBuf,
    ) -> Self {
        Self {
            llm_client,
            cache_manager,
            scout_agent,
            triage_agent,
            tools,
            consecutive_empty_turns: AtomicU32::new(0),
            source_file,
        }
    }

    fn build_user_message(context: &AgentContext) -> String {
        const MAX_OBSERVATION_CHARS: usize = 24_000;
        let observations = if context.accumulated_data.len() > MAX_OBSERVATION_CHARS {
            format!(
                "(observation history truncated)\n{}",
                &context.accumulated_data[context.accumulated_data.len() - MAX_OBSERVATION_CHARS..]
            )
        } else {
            context.accumulated_data.clone()
        };

        if observations.is_empty() {
            context.input_prompt.clone()
        } else {
            format!(
                "{}\n\n---\nObservation history:\n{}",
                context.input_prompt, observations
            )
        }
    }

    fn triage_directive(tdd_phase: &str) -> &'static str {
        match tdd_phase {
            "red" => "TDD RED PHASE: Code MUST compile cleanly (fix missing imports, type typos). Tests MUST fail assertions. DO NOT touch assertion logic.",
            "refactor" => "TDD REFACTOR PHASE: Code must compile and all tests MUST pass after refactoring.",
            _ => "TDD GREEN PHASE: Code must compile and all tests MUST pass. If there are errors, fix them.",
        }
    }

    fn triage_success(triage_ctx: &AgentContext, tdd_phase: &str) -> bool {
        if !triage_ctx.is_finished {
            return false;
        }

        match tdd_phase {
            "red" => triage_ctx
                .input_prompt
                .contains("compile succeeded, tests failing assertions"),
            _ => crate::agent::triage_passed(triage_ctx),
        }
    }

    async fn run_scout_subagent(
        &self,
        context: &mut AgentContext,
        label: &str,
        query: String,
    ) -> Result<String, String> {
        context
            .accumulated_data
            .push_str(&format!("\n[SYSTEM]: Launching Scout for {label}\n"));

        let scout_ctx =
            AgentLoopOrchestrator::run(&self.scout_agent, query, BUILDER_SCOUT_MAX_ITERATIONS)
                .await?;

        Ok(if scout_ctx.is_finished {
            scout_ctx.accumulated_data
        } else {
            format!(
                "Scout report (finished={}, iterations={}):\n{}",
                scout_ctx.is_finished, scout_ctx.iterations, scout_ctx.accumulated_data
            )
        })
    }
}

#[async_trait]
impl<
        C: LlmClient,
        SC: LlmClient,
        TC: LlmClient,
        B: BuildCommandRunner,
        D: BuildCommandDiscoverer,
    > AutonomousAgent for BuilderAgent<C, SC, TC, B, D>
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
            let thought = model_turn.content.as_deref().unwrap_or("");
            if record_empty_tool_turn(
                &mut context.accumulated_data,
                thought,
                &self.consecutive_empty_turns,
            ) {
                context.is_finished = true;
            }
            return Ok(());
        }

        self.consecutive_empty_turns.store(0, Ordering::Relaxed);

        let thought = model_turn.content.as_deref().unwrap_or("").to_string();
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
                    if context.input_prompt.contains("`unit` test") {
                        context.accumulated_data.push_str(
                            "Observation:\nFor unit tests, call write_test_suite directly with path under tests/ and content. Do not use gather_integration_context.\n",
                        );
                        return Ok(());
                    }
                    let components = parse_components(&tool_call.arguments)?;
                    let scout_query = build_scout_integration_query(&components);
                    let output = self
                        .run_scout_subagent(context, "integration context", scout_query)
                        .await?;
                    context
                        .accumulated_data
                        .push_str(&format!("Observation:\n{output}\n"));
                }
                "generate_test_factory" => {
                    let (target_struct, target_file) =
                        parse_factory_arguments(&tool_call.arguments)?;
                    let scout_query = build_scout_factory_query(&target_struct, &target_file);
                    let output = self
                        .run_scout_subagent(context, "test factory", scout_query)
                        .await?;
                    context
                        .accumulated_data
                        .push_str(&format!("Observation:\n{output}\n"));
                }
                "write_test_suite" => {
                    let (path, tdd_phase) = parse_write_test_suite_arguments(&tool_call.arguments)?;
                    let project_root = {
                        let cache = self
                            .cache_manager
                            .lock()
                            .map_err(|_| "cache manager lock poisoned".to_string())?;
                        cache.project_root().to_path_buf()
                    };
                    if self.source_file.is_file() {
                        if let Err(err) = validate_test_path_for_source(&path, &self.source_file) {
                            context
                                .accumulated_data
                                .push_str(&format!("Observation:\n{err}\n"));
                            return Ok(());
                        }
                    } else if let Some(source_rel) =
                        source_file_from_builder_prompt(&context.input_prompt)
                    {
                        let source_path = project_root.join(&source_rel);
                        if let Err(err) = validate_test_path_for_source(&path, &source_path) {
                            context
                                .accumulated_data
                                .push_str(&format!("Observation:\n{err}\n"));
                            return Ok(());
                        }
                    }
                    let content = match extract_test_content(
                        model_turn.content.as_deref(),
                        &tool_call.arguments,
                    ) {
                        Ok(content) => content,
                        Err(err) => {
                            context
                                .accumulated_data
                                .push_str(&format!("Observation:\n{err}\n"));
                            return Ok(());
                        }
                    };

                    let path_buf = resolve_test_output_path(&project_root, &path)?;

                    if let Some(parent) = path_buf.parent() {
                        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
                    }
                    std::fs::write(&path_buf, &content).map_err(|err| err.to_string())?;

                    let triage_directive = Self::triage_directive(&tdd_phase);
                    context.accumulated_data.push_str(&format!(
                        "\n[SYSTEM]: Launching Triage ({tdd_phase}) for {}\n",
                        path_buf.display()
                    ));

                    self.triage_agent.retarget(vec![path_buf.clone()])?;

                    let triage_ctx = AgentLoopOrchestrator::run(
                        &self.triage_agent,
                        format!("Verify {}:\n{triage_directive}", path_buf.display()),
                        BUILDER_TRIAGE_MAX_ITERATIONS,
                    )
                    .await?;

                    context.accumulated_data.push_str(&format!(
                        "\n[TRIAGE RESULT]: {}\n",
                        triage_ctx.accumulated_data
                    ));

                    if Self::triage_success(&triage_ctx, &tdd_phase) {
                        if tdd_phase == "red" {
                            context.accumulated_data.push_str(
                                "\n[RED OK]: tests compile and fail assertions. \
                                 Call write_test_suite again with tdd_phase=green — job is not done until GREEN.\n",
                            );
                        } else {
                            context.is_finished = true;
                            context.accumulated_data.push_str("\n[BUILDER GREEN OK]\n");
                        }
                    } else {
                        context.accumulated_data.push_str(&format!(
                            "\n[TRIAGE FAILURE]: triage did not reach expected TDD outcome\n\
                             test_path: {}\n",
                            path_buf.display()
                        ));
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
        if context.iterations >= context.max_iterations.saturating_sub(1) {
            context.input_prompt.push_str(
                "\nFinal turn: if not GREEN, end with test path, last triage log excerpt, and explicit no GREEN — never a status-only sentence.",
            );
        }
        context
            .input_prompt
            .push_str("\nContinue generating tests based on the latest observation.");
        Ok(())
    }
}

pub type DefaultBuilderAgent<C, SC, TC> =
    BuilderAgent<C, SC, TC, super::SystemBuildRunner, super::NoopBuildDiscoverer>;

pub fn default_builder_agent<C: LlmClient, SC: LlmClient, TC: LlmClient>(
    llm_client: C,
    cache_manager: Arc<Mutex<ProjectCacheManager>>,
    scout_llm_client: SC,
    triage_llm_client: TC,
    config: Arc<AdjutantConfig>,
    target_paths: Vec<PathBuf>,
) -> DefaultBuilderAgent<C, SC, TC> {
    let scout_agent = ScoutAgent::new(scout_llm_client);
    let triage_agent = TriageAgent::new(triage_llm_client, target_paths.clone(), config);
    let source_file = target_paths
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("src/lib.rs"));
    BuilderAgent::new(
        llm_client,
        cache_manager,
        scout_agent,
        triage_agent,
        source_file,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::validate_test_path_for_source;

    #[test]
    fn validate_test_path_rejects_rust_for_tsx_source() {
        let dir = std::env::temp_dir().join(format!("builder-lang-{}", std::process::id()));
        let source = dir.join("frontend/src/Foo.tsx");
        std::fs::create_dir_all(source.parent().unwrap()).expect("mkdir");
        std::fs::write(&source, "export const x = 1").expect("write");
        let err = validate_test_path_for_source("tests/foo_integration_test.rs", &source)
            .expect_err("cross-language");
        assert!(err.contains("tsx") || err.contains("extension"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_test_output_path_joins_relative_paths_to_project_root() {
        let root = PathBuf::from("/repo/demo");
        let resolved = resolve_test_output_path(&root, "tests/unit.rs").expect("relative path");
        assert_eq!(resolved, PathBuf::from("/repo/demo/tests/unit.rs"));
    }

    #[test]
    fn resolve_test_output_path_rejects_absolute_paths() {
        let root = PathBuf::from("/repo/demo");
        assert!(resolve_test_output_path(&root, "/tmp/abs_test.rs").is_err());
    }

    #[test]
    fn resolve_test_output_path_rejects_parent_traversal() {
        let root = PathBuf::from("/repo/demo");
        assert!(resolve_test_output_path(&root, "../escape.rs").is_err());
    }

    #[test]
    fn empty_tool_turn_nudges_without_finishing() {
        let counter = AtomicU32::new(0);
        let mut data = String::new();
        assert!(!record_empty_tool_turn(&mut data, "", &counter));
        assert!(data.contains("no tool call"));
        assert!(data.contains("write_test_suite"));
        assert!(!data.contains("[BUILDER FAIL EVIDENCE]"));
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn empty_tool_turn_with_thought_still_nudges() {
        let counter = AtomicU32::new(0);
        let mut data = String::new();
        assert!(!record_empty_tool_turn(&mut data, "planning", &counter));
        assert!(data.contains("Thought:\nplanning"));
        assert!(data.contains("did not call a tool"));
    }

    #[test]
    fn third_empty_tool_turn_finishes_with_fail_evidence() {
        let counter = AtomicU32::new(0);
        let mut data = String::new();
        assert!(!record_empty_tool_turn(&mut data, "", &counter));
        assert!(!record_empty_tool_turn(&mut data, "", &counter));
        assert!(record_empty_tool_turn(&mut data, "", &counter));
        assert!(data.contains("[BUILDER FAIL EVIDENCE]"));
        assert!(data.contains("no GREEN"));
        assert_eq!(counter.load(Ordering::Relaxed), 3);
    }
}
