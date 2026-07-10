mod codemod_report;
mod cpp_codemod;
mod field_migration;
mod java_codemod;
mod py_codemod;
mod struct_codemod;
mod tools;
mod ts_codemod;

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::traits::{AgentContext, AutonomousAgent};
use super::{
    AgentLoopOrchestrator, BuildCommandDiscoverer, BuildCommandRunner, ScoutAgent, TriageAgent,
};
use crate::domain::AdjutantConfig;
use crate::llm::{LlmClient, LlmRequest, LlmToolSet};
use crate::tools::{edit_file_line, edit_file_range, read_file_range};

use codemod_report::{format_change_report, summarize_snippet_diff, verification_passed, verify_field_migration};
use cpp_codemod::try_cpp_call_codemod;
use java_codemod::try_java_call_codemod;
use py_codemod::try_python_call_codemod;
use struct_codemod::try_rust_struct_literal_codemod;
use ts_codemod::try_ts_object_literal_codemod;

use field_migration::instruction_contains_field_migration;

pub use tools::{
    build_scout_refactor_query, extract_refactor_instruction, filter_targets_by_scope,
    find_refactor_targets, format_refactor_targets_block, parse_apply_structural_codemod_arguments,
    parse_method_name, path_under_scope, transformer_tool_set, RefactorTarget,
};

pub const TRANSFORMER_SYSTEM_PROMPT: &str = r#"You are an autonomous refactor agent (PHASE_3_5_TRANSFORMER). You apply structural code changes across multiple files.

Available tools (tool calls):
- gather_refactor_targets — runs Scout to locate call sites for a method/struct (ripgrep, ast_calls, ast_constructions)
- apply_structural_codemod — applies a transformation rule to Scout-provided file paths and line numbers

Workflow: gather_refactor_targets first, then apply_structural_codemod with refactor_targets_json copied from Scout's report. Triage verifies compilation after codemod.

Reply with a short rationale (Thought), then call tools."#;

const TRANSFORMER_CODEMOD_SYSTEM_PROMPT: &str =
    "You rewrite source code. Return ONLY the replacement content — one line or a multi-line block — with no markdown or explanation.";

const TRANSFORMER_SCOUT_MAX_ITERATIONS: u32 = 8;
const TRANSFORMER_TRIAGE_MAX_ITERATIONS: u32 = 4;
pub const TRANSFORMER_MAX_ITERATIONS: u32 = 4;
const CODEMOD_CONTEXT_MARGIN: usize = 2;

pub fn line_window(line: usize, margin: usize, file_len: usize) -> (usize, usize) {
    let file_len = file_len.max(1);
    let start = line.saturating_sub(margin).max(1);
    let end = (line + margin).min(file_len);
    (start, end)
}

pub struct TransformerAgent<C, SC, TC, CC, B, D>
where
    SC: LlmClient,
{
    llm_client: C,
    codemod_client: CC,
    scout_agent: ScoutAgent<SC>,
    triage_agent: TriageAgent<TC, B, D>,
    tools: LlmToolSet,
    scope_path: Option<PathBuf>,
}

impl<
        C: LlmClient,
        SC: LlmClient,
        TC: LlmClient,
        CC: LlmClient,
        B: BuildCommandRunner,
        D: BuildCommandDiscoverer,
    > TransformerAgent<C, SC, TC, CC, B, D>
{
    pub fn new(
        llm_client: C,
        codemod_client: CC,
        scout_agent: ScoutAgent<SC>,
        triage_agent: TriageAgent<TC, B, D>,
    ) -> Self {
        Self {
            llm_client,
            codemod_client,
            scout_agent,
            triage_agent,
            tools: transformer_tool_set(),
            scope_path: None,
        }
    }

    pub fn with_scope(mut self, scope_path: PathBuf) -> Self {
        self.scope_path = Some(scope_path);
        self
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

    fn triage_success(triage_ctx: &AgentContext) -> bool {
        triage_ctx.is_finished
            && triage_ctx
                .input_prompt
                .contains("All builds/tests completed successfully.")
    }

    async fn run_scout_subagent(
        &self,
        context: &mut AgentContext,
        method_name: &str,
    ) -> Result<String, String> {
        context
            .accumulated_data
            .push_str("\n[SYSTEM]: Launching Scout for refactor targets\n");

        let scout_ctx = AgentLoopOrchestrator::run(
            &self.scout_agent,
            build_scout_refactor_query(method_name),
            TRANSFORMER_SCOUT_MAX_ITERATIONS,
        )
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

    fn file_line_count(path: &Path) -> Result<usize, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        Ok(content.lines().count().max(1))
    }

    fn codemod_line(
        &self,
        path: &Path,
        line: usize,
        transformation_rule: &str,
    ) -> Result<String, String> {
        let file_len = Self::file_line_count(path)?;
        let (start, end) = line_window(line, CODEMOD_CONTEXT_MARGIN, file_len);
        let snippet = read_file_range(path, start, end)?;
        let user_message = format!(
            "File: {path}\nTarget line: {line}\nTransformation rule: {transformation_rule}\n\nContext:\n{snippet}\n\nReturn ONLY the new content for line {line}.",
            path = path.display(),
        );
        self.codemod_content(&user_message, true)
    }

    fn codemod_range(
        &self,
        path: &Path,
        start: usize,
        end: usize,
        transformation_rule: &str,
    ) -> Result<String, String> {
        let snippet = read_file_range(path, start, end)?;
        let field_rules = if instruction_contains_field_migration(transformation_rule) {
            "\n- `source_module` / `sourceModule` is `String`, NOT `Option`. Only `correlation_id` / `correlationId` is optional.\n"
        } else {
            ""
        };
        let user_message = format!(
            "File: {path}\nTarget lines: {start}-{end}\nTransformation rule: {transformation_rule}{field_rules}\n- Preserve indentation of the first line of the replaced range.\n\nContext:\n{snippet}\n\nReturn ONLY the replacement content for lines {start} through {end} (may be multiple lines).",
            path = path.display(),
        );
        self.codemod_content(&user_message, false)
    }

    async fn apply_and_triage(
        &self,
        context: &mut AgentContext,
        targets: &[RefactorTarget],
        transformation_rule: &str,
        method_name: Option<&str>,
    ) -> Result<(), String> {
        let touched = self.apply_codemod_to_targets(
            context,
            targets,
            transformation_rule,
            method_name,
        )?;

        context.accumulated_data.push_str(
            "\n[TRANSFORMER]: Structural changes applied. Launching Triage to fix compiler side-effects...\n",
        );

        self.triage_agent.retarget(touched.clone())?;

        let triage_ctx = AgentLoopOrchestrator::run(
            &self.triage_agent,
            format!("Verify compilation after global refactor:\n{transformation_rule}"),
            TRANSFORMER_TRIAGE_MAX_ITERATIONS,
        )
        .await?;

        context.accumulated_data.push_str(&format!(
            "\n[TRIAGE RESULT]: {}\n",
            triage_ctx.accumulated_data
        ));

        if Self::triage_success(&triage_ctx) {
            let verification = verify_field_migration(&touched, transformation_rule);
            if !verification.is_empty() {
                context.accumulated_data.push_str(&verification);
            }
            if verification_passed(&verification) {
                context.is_finished = true;
                context.accumulated_data.push_str("\n[TRANSFORMER OK]\n");
            } else {
                context.accumulated_data.push_str(
                    "\n[TRANSFORMER INCOMPLETE]: verification checks failed for one or more touched files\n",
                );
            }
        } else {
            context.accumulated_data.push_str(&format!(
                "\n[TRIAGE FAILURE]: Refactoring broke compilation in a way that requires Architect intervention: {}\n",
                triage_ctx.accumulated_data
            ));
        }

        Ok(())
    }

    fn apply_codemod_to_targets(
        &self,
        context: &mut AgentContext,
        targets: &[RefactorTarget],
        transformation_rule: &str,
        method_name: Option<&str>,
    ) -> Result<Vec<PathBuf>, String> {
        let mut touched = Vec::new();
        let mut errors = Vec::new();

        for target in targets {
            if !target.file_path.exists() {
                context.accumulated_data.push_str(&format!(
                    "Skipped missing file: {}\n",
                    target.file_path.display()
                ));
                continue;
            }

            for range in &target.ranges {
                match self.apply_range_codemod(
                    context,
                    &target.file_path,
                    range.start,
                    range.end,
                    transformation_rule,
                    method_name,
                ) {
                    Ok(()) => {
                        if !touched.iter().any(|path| path == &target.file_path) {
                            touched.push(target.file_path.clone());
                            context.touched_files.push(target.file_path.clone());
                        }
                    }
                    Err(err) => {
                        errors.push(format!(
                            "{} lines {}-{}: {err}",
                            target.file_path.display(),
                            range.start,
                            range.end
                        ));
                        context
                            .accumulated_data
                            .push_str(&format!("Codemod error: {err}\n"));
                    }
                }
            }

            for &line in &target.lines {
                match self.codemod_line(&target.file_path, line, transformation_rule) {
                    Ok(new_line) => {
                        let before = read_file_range(&target.file_path, line, line)?;
                        edit_file_line(&target.file_path, line, &new_line)?;
                        let changes = summarize_snippet_diff(&before, &new_line, line);
                        context.accumulated_data.push_str(&format!(
                            "Applied codemod to {} line {line}\n",
                            target.file_path.display()
                        ));
                        context.accumulated_data.push_str(&format_change_report(
                            &target.file_path,
                            line,
                            line,
                            &changes,
                            &new_line,
                        ));
                        if !touched.iter().any(|path| path == &target.file_path) {
                            touched.push(target.file_path.clone());
                            context.touched_files.push(target.file_path.clone());
                        }
                    }
                    Err(err) => {
                        errors.push(format!("{} line {line}: {err}", target.file_path.display()));
                        context
                            .accumulated_data
                            .push_str(&format!("Codemod error: {err}\n"));
                    }
                }
            }
        }

        if touched.is_empty() {
            return Err(if errors.is_empty() {
                "no refactor targets were applied".to_string()
            } else {
                format!("all codemod attempts failed: {}", errors.join("; "))
            });
        }

        Ok(touched)
    }

    fn apply_range_codemod(
        &self,
        context: &mut AgentContext,
        path: &Path,
        start: usize,
        end: usize,
        transformation_rule: &str,
        _method_name: Option<&str>,
    ) -> Result<(), String> {
        let snippet = read_file_range(path, start, end)?;
        let ext = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let new_content = match ext {
            "rs" => try_rust_struct_literal_codemod(&snippet, transformation_rule, path),
            "ts" | "tsx" => try_ts_object_literal_codemod(&snippet, transformation_rule, path),
            "py" => try_python_call_codemod(&snippet, transformation_rule, path),
            "java" | "kt" => try_java_call_codemod(&snippet, transformation_rule, path),
            "cpp" | "cc" | "cxx" | "hpp" | "c" | "h" | "zig" => {
                try_cpp_call_codemod(&snippet, transformation_rule, path)
            }
            _ => None,
        }
        .or_else(|| self.codemod_range(path, start, end, transformation_rule).ok())
        .ok_or_else(|| "codemod produced no replacement".to_string())?;
        let changes = summarize_snippet_diff(&snippet, &new_content, start);
        edit_file_range(path, start, end, &new_content)?;
        context.accumulated_data.push_str(&format!(
            "Applied codemod to {} lines {}-{}\n",
            path.display(),
            start,
            end
        ));
        context.accumulated_data.push_str(&format_change_report(
            path, start, end, &changes, &new_content,
        ));
        Ok(())
    }

    fn codemod_content(&self, user_message: &str, single_line: bool) -> Result<String, String> {
        let empty_tools = LlmToolSet::new();
        let request = LlmRequest::new(TRANSFORMER_CODEMOD_SYSTEM_PROMPT, user_message, &empty_tools);
        let turn = self.codemod_client.complete(request)?;
        let content = turn
            .content
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            .ok_or_else(|| "codemod model returned empty content".to_string())?;
        if single_line {
            Ok(content.lines().next().unwrap_or(&content).to_string())
        } else {
            Ok(content)
        }
    }
}

#[async_trait]
impl<
        C: LlmClient,
        SC: LlmClient,
        TC: LlmClient,
        CC: LlmClient,
        B: BuildCommandRunner,
        D: BuildCommandDiscoverer,
    > AutonomousAgent for TransformerAgent<C, SC, TC, CC, B, D>
{
    fn name(&self) -> &'static str {
        "PHASE_3_5_TRANSFORMER"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        if !context.input_prompt.contains("PHASE_3_5_TRANSFORMER") {
            context.input_prompt.push_str("\n\n");
            context.input_prompt.push_str(TRANSFORMER_SYSTEM_PROMPT);
        }
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        let user_message = Self::build_user_message(context);
        let request = LlmRequest::new(TRANSFORMER_SYSTEM_PROMPT, &user_message, &self.tools);
        let model_turn = self.llm_client.complete(request)?;

        if model_turn.tool_calls.is_empty() {
            let thought = model_turn.content.as_deref().unwrap_or("").to_string();
            if thought.is_empty() {
                return Err("model response missing tool call".to_string());
            }
            context.accumulated_data.push_str(&format!(
                "Thought:\n{thought}\n(model did not call a tool — continue)\n"
            ));
            return Ok(());
        }

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
                "gather_refactor_targets" => {
                    let method_name = parse_method_name(&tool_call.arguments)?;
                    match find_refactor_targets(&method_name) {
                        Ok(mut targets) if !targets.is_empty() => {
                            if let Some(scope) = &self.scope_path {
                                let total = targets.len();
                                targets = filter_targets_by_scope(targets, scope);
                                if targets.is_empty() {
                                    return Err(format!(
                                        "no refactor targets under scope {}",
                                        scope.display()
                                    ));
                                }
                                context.accumulated_data.push_str(&format!(
                                    "\n[SYSTEM]: Scoped gather to {} ({}/{} targets)\n",
                                    scope.display(),
                                    targets.len(),
                                    total
                                ));
                            }
                            context.accumulated_data.push_str(
                                "\n[SYSTEM]: Deterministic refactor target scan succeeded\n",
                            );
                            let output = format_refactor_targets_block(&targets);
                            context
                                .accumulated_data
                                .push_str(&format!("Observation:\n{output}\n"));
                            let rule = extract_refactor_instruction(&context.input_prompt);
                            self.apply_and_triage(
                                context,
                                &targets,
                                &rule,
                                Some(&method_name),
                            )
                            .await?;
                        }
                        _ => {
                            let output =
                                self.run_scout_subagent(context, &method_name).await?;
                            context
                                .accumulated_data
                                .push_str(&format!("Observation:\n{output}\n"));
                        }
                    }
                }
                "apply_structural_codemod" => {
                    let (transformation_rule, targets) =
                        parse_apply_structural_codemod_arguments(&tool_call.arguments)?;
                    self.apply_and_triage(
                        context,
                        &targets,
                        &transformation_rule,
                        None,
                    )
                    .await?;
                }
                other => return Err(format!("unsupported transformer tool: {other}")),
            }
        }

        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        context
            .input_prompt
            .push_str("\nContinue refactor based on the latest observation.");
        Ok(())
    }
}

pub type DefaultTransformerAgent<C, SC, TC, CC> = TransformerAgent<
    C,
    SC,
    TC,
    CC,
    super::SystemBuildRunner,
    super::NoopBuildDiscoverer,
>;

pub fn default_transformer_agent<C: LlmClient, SC: LlmClient, TC: LlmClient, CC: LlmClient>(
    llm_client: C,
    codemod_client: CC,
    scout_llm_client: SC,
    triage_llm_client: TC,
    config: std::sync::Arc<AdjutantConfig>,
    target_paths: Vec<PathBuf>,
    scope_path: Option<PathBuf>,
) -> DefaultTransformerAgent<C, SC, TC, CC> {
    let scout_agent = ScoutAgent::new(scout_llm_client);
    let triage_agent = TriageAgent::new(triage_llm_client, target_paths, config);
    let agent = TransformerAgent::new(llm_client, codemod_client, scout_agent, triage_agent);
    if let Some(scope) = scope_path {
        agent.with_scope(scope)
    } else {
        agent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_window_clamps_to_file_bounds() {
        assert_eq!(line_window(1, 2, 10), (1, 3));
        assert_eq!(line_window(5, 2, 6), (3, 6));
        assert_eq!(line_window(1, 2, 0), (1, 1));
    }
}
