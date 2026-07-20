use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Deserialize;

use super::traits::{AgentContext, AutonomousAgent};
use crate::cache::{normalize_agent_name, ProjectCacheManager};
use crate::llm::{LlmClient, LlmRequest, LlmToolSet};

pub const EVALUATOR_SYSTEM_PROMPT: &str = r#"You are a Strict Quality Inspector (QA_AGENT). Your job is to evaluate other AI agents.
You will receive:
1. AGENT NAME
2. ORIGINAL TASK
3. THEIR OUTPUT

Return ONE valid JSON object (no markdown fence) with this shape:
{
  "score": [rating from 1 to 10],
  "critique": "[Concise summary: what went well, what was missing for 10/10? Watch for hallucinations, noise, or weak assertions]",
  "desired_output": "[Full exemplar rewrite of what the agent should have produced to earn 10/10 — same modality/format as THEIR OUTPUT (scout report, builder result, triage log, etc.). Not a checklist. If score is 10, set this to an empty string \"\".]"
}

Scoring guide:
- 8–10: Output contains verifiable evidence (commands run, exit status, file:line paths, log excerpts).
- 5–7: Correct FAIL or PASS conclusion with verifiable evidence (cmd/exit/log/snippet/path) even if incomplete.
- 1–4: Meta / status paraphrase with no supporting artifact.

If AGENT OUTPUT is a one-line status paraphrase with no paths, logs, or code, score ≤3 and say the orchestrator must paste the raw query_job_status.result.

When score < 10, desired_output MUST be a complete exemplar that would earn 10/10 for ORIGINAL TASK (file:line evidence, commands, logs — match the agent rubric). When score is 10, desired_output MUST be "".

Be ruthless. Give 10/10 only for perfect, surgical execution."#;

const PLANNER_RUBRIC: &str = r#"

PLANNER RUBRIC (override generic rubric):
- 9-10: Multi-step pipeline (create_file + patch_file SEARCH/REPLACE wiring + manifest/module entry when needed + generate_tests), every goal cites path:line, SEARCH anchors grounded in scouted files, paste-ready hunks with zero ellipses/placeholders
- 7-8: Correct pipeline structure with grounded SEARCH/REPLACE hunks and generate_tests step present; minor API/style issues only
- 5-6: Schema-valid but single-step feature, missing generate_tests when code changes exist, ungrounded SEARCH blocks, logic dumped into REPLACE (>15 lines), or comment sketches
- 1-4: Hallucinated modules, empty patches, ellipses/.../pseudo-code in patch_content, path-access failure with no recovery blueprint, or full-function rewrites instead of hunks
Hard caps: single-step feature blueprint max 6; no generate_tests on code changes max 6; any ellipsis or placeholder in patch_content max 4.
patch_file MUST use SEARCH/REPLACE hunks. generate_tests step MUST exist (final step) with non-empty goal citing the test file path:line.
Score down if blueprint violates stated coordinator plan_kind or expectations."#;

const BUILDER_RUBRIC: &str = r#"

BUILDER RUBRIC (override generic rubric):
- 9-10: Delivers full test source (or diff) at a repo-relative path, build command with exit code, and log excerpt proving pass/fail; covers every function named in the task
- 7-8: Correct test logic with file path but thin build evidence, or minor gaps in requested scope
- 5-6: Partial scaffolding; OR env/compile FAIL that includes error log plus attempted path/fix; OR correct diagnosis missing only full test body
- 1-4: Meta-commentary on failure without code/logs, skipped requested functions without file:line proof of existing coverage, or unverifiable success claim
Hard caps: no test source in output max 4; skipped primary task objective max 3; failure narrative without error logs max 3.
Evidenced FAIL (error log + attempted fix) scores 5-6, not 1-4."#;

const SCOUT_RUBRIC: &str = r#"

SCOUT RUBRIC (override generic rubric):
- 9-10: file:line citations for every claim plus 2–5 line code snippets or log excerpts; answers all sub-questions in the task; workspace-consistent paths
- 7-8: Correct file:line mapping but thin snippets or one missed sub-question
- 5-7: Partial answer with file:line plus at least one code snippet or log excerpt
- 1-4: Wrong repository/workspace, config/path error instead of trace, meta-commentary about a review/conversation, or summary with no file:line evidence
Hard caps: wrong repo or no file paths max 2; meta-commentary instead of technical trace max 3."#;

const TRIAGE_RUBRIC: &str = r#"

TRIAGE RUBRIC (override generic rubric):
- 9-10: PASS/FAIL backed by build command, exit code, workspace path, target files, and log excerpt for each module tested
- 7-8: Correct verdict with logs but missing exit code or incomplete target-file list
- 5-7: Correct FAIL (or incomplete PASS diagnosis) with command + exit code + log excerpt
- 1-4: PASS/FAIL without logs, wrong project/workspace, or generic assertion without command output
Hard caps: PASS without log excerpt max 3; wrong target project max 2.
Evidenced FAIL scores 5-7, not 1-4."#;

const BABYSITTER_RUBRIC: &str = r#"

BABYSITTER RUBRIC (override generic rubric):
- 10: Valid JSON with action, pr_number, checks (every PR check name), reviews.paths_seen/handled/skipped_paths, gh_state, report_posted, iterations
- 9: Same fields; minor omission only
- 5-7: Blocked with refuse_reason plus named checks and review paths
- 1-4: Meta status, prose-only, or JSON missing check names / review paths / gh_state
Do not apply Triage build-log hard caps — babysitter evidence is PR/CI/review state, not cargo/npm logs."#;

#[derive(Debug, Clone)]
pub struct AgentEvalSummary {
    pub score: i32,
    pub critique: String,
    pub desired_output: String,
}

#[derive(Debug, Deserialize)]
struct EvaluationPayload {
    score: i32,
    critique: String,
    #[serde(default)]
    desired_output: String,
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
        let canonical = normalize_agent_name(&self.target_agent);
        let mut message = format!(
            "AGENT: {canonical}\n\nORIGINAL TASK:\n{}\n\nAGENT OUTPUT:\n{}",
            self.original_task, self.received_output
        );
        if let Some(rubric) = agent_evaluation_rubric(&canonical) {
            message.push_str(rubric);
        }
        message
    }

    pub async fn evaluate_once(&self) -> Result<AgentEvalSummary, String> {
        let user_message = self.build_user_message();
        let empty_tools = LlmToolSet::new();
        let request = LlmRequest::new(EVALUATOR_SYSTEM_PROMPT, &user_message, &empty_tools);
        let model_turn = self.client.complete(request)?;
        let raw_response = model_turn
            .content
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| "evaluator model response missing content".to_string())?;

        let mut evaluation = Self::parse_evaluation_response(&raw_response)?;
        if !(1..=10).contains(&evaluation.score) {
            return Err(format!(
                "evaluator score must be between 1 and 10, got {}",
                evaluation.score
            ));
        }
        normalize_desired_output(&mut evaluation)?;

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
            &evaluation.desired_output,
        )?;

        Ok(payload_to_summary(&evaluation))
    }

    fn parse_evaluation_response(raw: &str) -> Result<EvaluationPayload, String> {
        let trimmed = raw.trim();
        let fenced = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .and_then(|rest| rest.strip_suffix("```"))
            .map(str::trim)
            .unwrap_or(trimmed);

        let json_body = extract_json_object(fenced).unwrap_or(fenced);

        serde_json::from_str(json_body)
            .map_err(|err| format!("failed to parse evaluator JSON response: {err}"))
    }
}

/// Score 10 → force empty desired_output; score &lt; 10 → require non-blank exemplar.
fn normalize_desired_output(evaluation: &mut EvaluationPayload) -> Result<(), String> {
    if evaluation.score == 10 {
        evaluation.desired_output.clear();
        return Ok(());
    }
    let trimmed = evaluation.desired_output.trim();
    if trimmed.is_empty() {
        return Err(
            "evaluator desired_output must be a non-empty 10/10 exemplar when score < 10"
                .to_string(),
        );
    }
    if trimmed.len() != evaluation.desired_output.len() {
        evaluation.desired_output = trimmed.to_string();
    }
    Ok(())
}

#[allow(dead_code)]
fn format_evaluation_result(evaluation: &EvaluationPayload) -> String {
    let mut out = format!(
        "Evaluation saved. QA score: {}/10\nCritique: {}",
        evaluation.score, evaluation.critique
    );
    if !evaluation.desired_output.is_empty() {
        out.push_str("\nDesired output (10/10 exemplar):\n");
        out.push_str(&evaluation.desired_output);
    }
    out
}

pub fn format_eval_job_appendix(summary: &AgentEvalSummary) -> String {
    let mut out = format!(
        "\n\nEvaluation: QA score {}/10\nCritique: {}",
        summary.score, summary.critique
    );
    if !summary.desired_output.is_empty() {
        out.push_str("\nDesired output (10/10 exemplar):\n");
        out.push_str(&summary.desired_output);
    }
    out
}

fn payload_to_summary(evaluation: &EvaluationPayload) -> AgentEvalSummary {
    AgentEvalSummary {
        score: evaluation.score,
        critique: evaluation.critique.clone(),
        desired_output: evaluation.desired_output.clone(),
    }
}

fn agent_evaluation_rubric(target_agent: &str) -> Option<&'static str> {
    match target_agent {
        "PlannerAgent" => Some(PLANNER_RUBRIC),
        "Phase_1_Scout" => Some(SCOUT_RUBRIC),
        "Phase_5_Triage" => Some(TRIAGE_RUBRIC),
        "BabysitterAgent" => Some(BABYSITTER_RUBRIC),
        name if name.starts_with("Phase_4_Builder") => Some(BUILDER_RUBRIC),
        _ => None,
    }
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
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
        let summary = self.evaluate_once().await?;
        context.accumulated_data = format!(
            "Evaluation saved. QA score: {}/10\nCritique: {}{}",
            summary.score,
            summary.critique,
            if summary.desired_output.is_empty() {
                String::new()
            } else {
                format!(
                    "\nDesired output (10/10 exemplar):\n{}",
                    summary.desired_output
                )
            }
        );
        context.is_finished = true;
        Ok(())
    }

    async fn mutate_next_iteration(&self, _context: &mut AgentContext) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        agent_evaluation_rubric, extract_json_object, format_evaluation_result,
        normalize_desired_output, EvaluationPayload, EvaluatorAgent, EVALUATOR_SYSTEM_PROMPT,
    };

    #[test]
    fn planner_rubric_appended_for_planner_agent() {
        let rubric = agent_evaluation_rubric("PlannerAgent").expect("rubric");
        assert!(rubric.contains("PLANNER RUBRIC"));
        assert!(rubric.contains("ellipsis"));
        assert!(rubric.contains("generate_tests"));
    }

    #[test]
    fn agent_rubrics_route_by_canonical_name_only() {
        assert!(agent_evaluation_rubric("Phase_4_Builder").is_some());
        assert!(agent_evaluation_rubric("Phase_4_Builder_GREEN").is_some());
        assert!(agent_evaluation_rubric("StringBuilder").is_none());
        let builder = agent_evaluation_rubric("Phase_4_Builder").expect("builder");
        assert!(builder.contains("Evidenced FAIL"));
        let scout = agent_evaluation_rubric("Phase_1_Scout").expect("scout");
        assert!(scout.contains("5-7: Partial answer"));
        let triage = agent_evaluation_rubric("Phase_5_Triage").expect("triage");
        assert!(triage.contains("Evidenced FAIL"));
        let baby = agent_evaluation_rubric("BabysitterAgent").expect("babysitter");
        assert!(baby.contains("BABYSITTER RUBRIC"));
        assert!(baby.contains("Valid JSON"));
    }

    #[test]
    fn evaluator_prompt_flags_orchestrator_paraphrase() {
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("query_job_status.result"));
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("score ≤3"));
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("verifiable evidence"));
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("desired_output"));
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("When score is 10, desired_output MUST be \"\""));
    }

    #[test]
    fn extract_json_object_finds_object_inside_prose() {
        let raw = "Here is my verdict: {\"score\": 7, \"critique\": \"ok\"} thanks.";
        assert_eq!(
            extract_json_object(raw),
            Some(r#"{"score": 7, "critique": "ok"}"#)
        );
    }

    #[test]
    fn parse_evaluation_response_accepts_desired_output() {
        let mut payload =
            EvaluatorAgent::<crate::llm::ConfiguredLlmClient>::parse_evaluation_response(
                r#"{"score": 6, "critique": "thin", "desired_output": "full exemplar with file:line"}"#,
            )
            .expect("parse with desired_output");
        normalize_desired_output(&mut payload).expect("normalize");

        assert_eq!(payload.score, 6);
        assert_eq!(payload.critique, "thin");
        assert_eq!(payload.desired_output, "full exemplar with file:line");
    }

    #[test]
    fn parse_evaluation_response_score_10_clears_desired_output() {
        let mut payload =
            EvaluatorAgent::<crate::llm::ConfiguredLlmClient>::parse_evaluation_response(
                r#"{"score": 10, "critique": "perfect", "desired_output": "should be cleared"}"#,
            )
            .expect("parse score 10");
        normalize_desired_output(&mut payload).expect("normalize");

        assert_eq!(payload.score, 10);
        assert!(payload.desired_output.is_empty());
    }

    #[test]
    fn parse_evaluation_response_rejects_empty_desired_when_score_below_10() {
        let mut payload =
            EvaluatorAgent::<crate::llm::ConfiguredLlmClient>::parse_evaluation_response(
                r#"{"score": 5, "critique": "weak", "desired_output": "   "}"#,
            )
            .expect("parse");
        let err =
            normalize_desired_output(&mut payload).expect_err("empty desired_output must fail");

        assert!(err.contains("desired_output"));
    }

    #[test]
    fn parse_evaluation_response_accepts_wrapped_prose() {
        let mut payload =
            EvaluatorAgent::<crate::llm::ConfiguredLlmClient>::parse_evaluation_response(
                "Thought: done.\n{\"score\": 8, \"critique\": \"solid\", \"desired_output\": \"exemplar\"}\n",
            )
            .expect("parse wrapped json");
        normalize_desired_output(&mut payload).expect("normalize");

        assert_eq!(payload.score, 8);
        assert_eq!(payload.critique, "solid");
        assert_eq!(payload.desired_output, "exemplar");
    }

    #[test]
    fn normalize_desired_output_trims_whitespace() {
        let mut payload = EvaluationPayload {
            score: 4,
            critique: "x".into(),
            desired_output: "  exemplar  ".into(),
        };
        normalize_desired_output(&mut payload).expect("ok");
        assert_eq!(payload.desired_output, "exemplar");
    }

    #[test]
    fn format_evaluation_result_omits_desired_when_empty() {
        let payload = EvaluationPayload {
            score: 10,
            critique: "perfect".into(),
            desired_output: String::new(),
        };
        let text = format_evaluation_result(&payload);
        assert!(text.contains("QA score: 10/10"));
        assert!(text.contains("Critique: perfect"));
        assert!(!text.contains("Desired output"));
    }
}
