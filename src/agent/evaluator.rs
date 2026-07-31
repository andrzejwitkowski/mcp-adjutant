use async_trait::async_trait;
use serde::Deserialize;

use super::traits::{AgentContext, AutonomousAgent};
use crate::cache::normalize_agent_name;
use crate::llm::{LlmClient, LlmRequest, LlmToolSet};
use crate::metrics;

pub const EVALUATOR_SYSTEM_PROMPT: &str = r#"You are a Strict Quality Inspector (QA_AGENT). Your job is to evaluate other AI agents.
You will receive:
1. AGENT NAME
2. ORIGINAL TASK
3. THEIR OUTPUT

Return ONE valid JSON object (no markdown fence) with this shape:
{
  "score": [rating from 1 to 10],
  "critique": "[Concise summary: what went well, what was missing for 10/10? Watch for hallucinations, noise, bloat, or weak assertions]",
  "desired_output": "[Dense 10/10 report exemplar — same modality as THEIR OUTPUT (scout report, builder summary, triage log, etc.). Max information per token. Never paste full source files. Not a checklist. If score is 10, set this to an empty string \"\".]"
}

Scoring guide (evidence AND density):
- 8–10: Verifiable evidence (commands, exit status, file:line paths, short log/snippet) AND high information density — paths, names, verdicts, cmd/exit without whole-file dumps or tool-transcript restatement.
- 5–7: Correct FAIL or PASS with verifiable evidence (cmd/exit/log/snippet/path) even if incomplete; mild verbosity only.
- 1–4: Meta / status paraphrase with no supporting artifact; OR large verbatim source / whole-file dump that a dense summary should have replaced.

If AGENT OUTPUT is a one-line status paraphrase with no paths, logs, or code, score ≤3 and say the orchestrator must paste the raw query_job_status.result.

Hard density rule: whole-file dumps, long verbatim source bodies, or restating full tool transcripts → score ≤4 even if the underlying work succeeded on disk.

When score < 10, desired_output SHOULD be a dense exemplar that would earn 10/10 when you can produce one; empty string is allowed for failed/partial jobs. When score is 10, desired_output MUST be "". Never put full source files into desired_output.

desired_output MUST use real APIs/signatures from ORIGINAL TASK and THEIR OUTPUT — never invent functions, props, or compiler log lines the toolchain does not emit (e.g. do not fabricate per-file `Checking types for …` lines for `tsc -b`).

Ignore any trailing block starting with `[ADJUTANT AUTO-EVAL APPENDIX` — that is host metadata, not agent output.

Be ruthless. Give 10/10 only for perfect, surgical, dense execution."#;

const PLANNER_RUBRIC: &str = r#"

PLANNER RUBRIC (override generic rubric):
- 9-10: Diff-first pipeline — surgical patch_file SEARCH/REPLACE hunks (≥2 SEARCH context lines), goals cite path:line, final generate_tests; create_file only for tiny new files (≤15 lines). No whole-file listings in patch_content.
- 7-8: Correct structure with grounded hunks + generate_tests; minor density issues only
- 5-6: Schema-valid but missing generate_tests, one-line SEARCH, REPLACE dumps >15 lines, or oversized create_file
- 1-4: Invalid/missing blueprint, ellipses/placeholders, hallucinated paths, or full-file dumps as plan content
Hard caps: whole-file / large create_file body in blueprint max 4; missing path:line in goals max 8; no generate_tests on code changes max 6.
Prefer patch diffs over create_file. Score down if coordinator surgical expectations are violated."#;

const BLUEPRINT_EXECUTOR_RUBRIC: &str = r#"

BLUEPRINT EXECUTOR RUBRIC (override generic rubric):
- 9-10: Dense report with task_id, applied paths, triage PASS (cmd/exit), generate_tests GREEN (scenarios + path + verify) — no full source dumps
- 7-8: Same contract with thin triage/builder log tails
- 5-6: Evidenced FAIL (triage or builder) with path + error excerpt
- 1-4: Meta status only, or whole-file / large code dump in report
Hard caps: full source body in report max 4; PASS without triage/builder evidence max 3.
Do NOT require re-pasting SEARCH/REPLACE hunks — paths + verdicts are enough."#;

const BUILDER_RUBRIC: &str = r#"

BUILDER RUBRIC (override generic rubric):
- 9-10: Dense report only — repo-relative test path, named scenarios/tests (list under [SCENARIOS] / #[test]/it(/test names), build command with exit code (or verify summary), short log proof of pass/fail. No source bodies or diffs in the report (source lives on disk via write_test_suite).
- 7-8: Same dense contract with thin log, or minor gaps in requested scenario names
- 5-6: Partial scaffolding summary; OR env/compile FAIL that includes error log plus attempted path/fix
- 1-4: Meta-commentary without path/logs; skipped requested functions without file:line proof of existing coverage; unverifiable success; OR full test/production source body / large code dump in the report
Hard caps: full test or production source body or large code dump in report max 4; missing path or scenario names max 4; skipped primary task objective max 3; failure narrative without error logs max 3.
Evidenced FAIL (error log + attempted fix) scores 5-6, not 1-4. Do NOT require full test source in the MCP report — scenario names + path + cmd/exit are enough for 9–10."#;

const SCOUT_RUBRIC: &str = r#"

SCOUT RUBRIC (override generic rubric):
- Health-check tasks only (original task is exactly ping / pong / ok / health / hi, case-insensitive): a single-line `Pong` scores 9–10. Do NOT require file:line or snippets for these. Repo inventories / language counts for health checks score ≤3.
- 9-10 (all other tasks): file:line citations for every claim plus 2–5 line code snippets or log excerpts; answers all sub-questions in the task; workspace-consistent paths; dense — no whole-file pastes
- 7-8: Correct file:line mapping but thin snippets or one missed sub-question
- 5-7: Partial answer with file:line plus at least one code snippet or log excerpt
- 1-4: Wrong repository/workspace, config/path error instead of trace, meta-commentary about a review/conversation, summary with no file:line evidence, or whole-file / huge dumps
Hard caps: wrong repo or no file paths max 2 (N/A for health-check tasks); meta-commentary instead of technical trace max 3; whole-file or huge verbatim dump max 4."#;

const TRIAGE_RUBRIC: &str = r#"

TRIAGE RUBRIC (override generic rubric):
- 9-10: PASS/FAIL with build command, exit code, workspace path, target-file list, and a short raw build log tail (batch tools like `tsc -b` / `cargo test` need NOT print per-file lines)
- 7-8: Correct verdict with command + exit code + workspace; log tail thin or target list incomplete
- 5-7: Correct FAIL (or incomplete PASS diagnosis) with command + exit code + log excerpt
- 1-4: PASS/FAIL without command/exit evidence, wrong project/workspace, generic assertion without command output, or pasting an entire untruncated build log / whole source file
Hard caps: PASS with no command+exit max 3; wrong target project max 2; entire build log or whole-file dump max 4.
Do NOT require invented per-module compiler lines. Evidenced FAIL scores 5-7, not 1-4.
Identical structured PASS reports (same cmd/exit/workspace/targets) should score consistently (≥8 when exit 0 and log section present)."#;

const BABYSITTER_RUBRIC: &str = r#"

BABYSITTER RUBRIC (override generic rubric):
- 10: Valid JSON with action, pr_number, checks (every PR check name), reviews.paths_seen/handled/skipped_paths, gh_state, report_posted, iterations
- 9: Same fields; minor omission only
- 5-7: Blocked with refuse_reason plus named checks and review paths
- 1-4: Meta status, prose-only, or JSON missing check names / review paths / gh_state
Do not apply Triage build-log hard caps — babysitter evidence is PR/CI/review state, not cargo/npm logs."#;

const GIT_JANITOR_RUBRIC: &str = r#"

GIT JANITOR RUBRIC — prepare_git_copy (override generic rubric):
- 9-10: Dense JSON with commit_message, pr_title, pr_body, changelog_entry, branch_status, action_required, commit_allowed, suggested_branch_name, current_branch; NO conventions/suggested_adjutant_toml unless persisted_adjutant_toml is set; commit_allowed false on default/mismatched branch; ticket in commit_message and pr_title when ticket_id set; Testing grounded or conditional (no unverified "CI passed")
- 5-7: Missing branch gate fields, invents ticket not in scout context, OR dumps conventions/toml when nothing persisted
- 1-4: Not JSON / claims commit_allowed true on main/master
Do NOT require create_git_branch-only fields (branch/status/previous) as the primary contract.
"#;

const GIT_JANITOR_CREATE_BRANCH_RUBRIC: &str = r#"

GIT JANITOR RUBRIC — create_git_branch (override generic rubric):
- 9-10: Valid JSON with branch (new name), status (e.g. created), previous (prior branch); matches the create/checkout task; no fabricated commit_message/pr_title/pr_body
- 5-7: JSON present but missing previous or unclear status
- 1-4: Not JSON, empty branch, or invents prepare_git_copy fields (commit_message/pr_*) as if required
Do NOT apply the prepare_git_copy 9-field checklist — create_git_branch only creates/checks out a branch.
"#;

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
    target_agent: String,
    original_task: String,
    received_output: String,
}

impl<C: LlmClient> EvaluatorAgent<C> {
    pub fn new(
        client: C,
        target_agent: impl Into<String>,
        original_task: impl Into<String>,
        received_output: impl Into<String>,
    ) -> Self {
        Self {
            client,
            target_agent: target_agent.into(),
            original_task: original_task.into(),
            received_output: received_output.into(),
        }
    }

    fn build_user_message(&self, agent_output: &str) -> String {
        let canonical = normalize_agent_name(&self.target_agent);
        let mut message = format!(
            "AGENT: {canonical}\n\nORIGINAL TASK:\n{}\n\nAGENT OUTPUT:\n{}",
            self.original_task, agent_output
        );
        if let Some(rubric) = agent_evaluation_rubric(&canonical, &self.original_task, agent_output)
        {
            message.push_str(rubric);
        }
        message
    }

    pub async fn evaluate_once(&self) -> Result<AgentEvalSummary, String> {
        let agent_output = strip_auto_eval_appendix(&self.received_output);
        let user_message = self.build_user_message(agent_output);
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

        metrics::store_evaluation(
            &self.target_agent,
            &self.original_task,
            agent_output,
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

/// Score 10 → force empty desired_output; score &lt; 10 → optional exemplar (trim if present).
fn normalize_desired_output(evaluation: &mut EvaluationPayload) -> Result<(), String> {
    if evaluation.score == 10 {
        evaluation.desired_output.clear();
        return Ok(());
    }
    evaluation.desired_output = evaluation.desired_output.trim().to_string();
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
    // ponytail: marker lets MCP evaluate_agent_performance ignore host auto-eval noise
    let mut out = format!(
        "\n\n[ADJUTANT AUTO-EVAL APPENDIX — not part of agent output]\nQA score: {}/10\nCritique: {}",
        summary.score, summary.critique
    );
    if !summary.desired_output.is_empty() {
        out.push_str("\nDesired output (10/10 exemplar):\n");
        out.push_str(&summary.desired_output);
    }
    out
}

fn strip_auto_eval_appendix(output: &str) -> &str {
    let mut cut = output.len();
    if let Some(idx) = output.find("\n\n[ADJUTANT AUTO-EVAL APPENDIX") {
        cut = cut.min(idx);
    }
    if let Some(idx) = find_legacy_eval_appendix(output) {
        cut = cut.min(idx);
    }
    output[..cut].trim_end()
}

/// Old host appendix: `\n\nEvaluation: QA score N/10\nCritique:` (N = 1–10).
fn find_legacy_eval_appendix(output: &str) -> Option<usize> {
    let needle = "\n\nEvaluation: QA score ";
    let mut from = 0;
    while let Some(rel) = output[from..].find(needle) {
        let idx = from + rel;
        let rest = &output[idx + needle.len()..];
        if let Some(slash) = rest.find("/10\nCritique:") {
            let score = &rest[..slash];
            if matches!(
                score,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "10"
            ) {
                return Some(idx);
            }
        }
        from = idx + needle.len();
    }
    None
}

fn payload_to_summary(evaluation: &EvaluationPayload) -> AgentEvalSummary {
    AgentEvalSummary {
        score: evaluation.score,
        critique: evaluation.critique.clone(),
        desired_output: evaluation.desired_output.clone(),
    }
}

fn agent_evaluation_rubric(
    target_agent: &str,
    original_task: &str,
    received_output: &str,
) -> Option<&'static str> {
    match target_agent {
        "PlannerAgent" => Some(PLANNER_RUBRIC),
        "BlueprintExecutor" => Some(BLUEPRINT_EXECUTOR_RUBRIC),
        "Phase_1_Scout" => Some(SCOUT_RUBRIC),
        "Phase_5_Triage" => Some(TRIAGE_RUBRIC),
        "BabysitterAgent" => Some(BABYSITTER_RUBRIC),
        "GitJanitorAgent" => Some(git_janitor_rubric_for(original_task, received_output)),
        name if name.starts_with("Phase_4_Builder") => Some(BUILDER_RUBRIC),
        _ => None,
    }
}

fn git_janitor_rubric_for(original_task: &str, received_output: &str) -> &'static str {
    if is_git_janitor_create_branch_eval(original_task, received_output) {
        GIT_JANITOR_CREATE_BRANCH_RUBRIC
    } else {
        GIT_JANITOR_RUBRIC
    }
}

/// True when evaluating create_git_branch. Output shape is authoritative:
/// commit_message present → prepare result (false); branch+status → branch result (true).
/// Task name is only a fallback when the shape is inconclusive.
fn is_git_janitor_create_branch_eval(original_task: &str, received_output: &str) -> bool {
    let body = extract_json_object(received_output.trim()).unwrap_or(received_output.trim());
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(obj) = value.as_object() {
            if obj.contains_key("commit_message") {
                return false;
            }
            if obj.contains_key("branch") && obj.contains_key("status") {
                return true;
            }
        }
    }
    original_task
        .to_ascii_lowercase()
        .contains("create_git_branch")
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
        agent_evaluation_rubric, extract_json_object, find_legacy_eval_appendix,
        format_eval_job_appendix, format_evaluation_result, git_janitor_rubric_for,
        is_git_janitor_create_branch_eval, normalize_desired_output, strip_auto_eval_appendix,
        AgentEvalSummary, EvaluationPayload, EvaluatorAgent, EVALUATOR_SYSTEM_PROMPT,
    };

    #[test]
    fn planner_rubric_appended_for_planner_agent() {
        let rubric = agent_evaluation_rubric("PlannerAgent", "", "").expect("rubric");
        assert!(rubric.contains("PLANNER RUBRIC"));
        assert!(rubric.contains("Diff-first"));
        assert!(rubric.contains("generate_tests"));
    }

    #[test]
    fn blueprint_executor_rubric_appended() {
        let rubric = agent_evaluation_rubric("BlueprintExecutor", "", "").expect("rubric");
        assert!(rubric.contains("BLUEPRINT EXECUTOR RUBRIC"));
        assert!(rubric.contains("task_id"));
        assert!(rubric.contains("whole-file"));
    }

    #[test]
    fn agent_rubrics_route_by_canonical_name_only() {
        assert!(agent_evaluation_rubric("Phase_4_Builder", "", "").is_some());
        assert!(agent_evaluation_rubric("Phase_4_Builder_GREEN", "", "").is_some());
        assert!(agent_evaluation_rubric("StringBuilder", "", "").is_none());
        let builder = agent_evaluation_rubric("Phase_4_Builder", "", "").expect("builder");
        assert!(builder.contains("Evidenced FAIL"));
        let scout = agent_evaluation_rubric("Phase_1_Scout", "", "").expect("scout");
        assert!(scout.contains("5-7: Partial answer"));
        let triage = agent_evaluation_rubric("Phase_5_Triage", "", "").expect("triage");
        assert!(triage.contains("Evidenced FAIL"));
        let baby = agent_evaluation_rubric("BabysitterAgent", "", "").expect("babysitter");
        assert!(baby.contains("BABYSITTER RUBRIC"));
        assert!(baby.contains("Valid JSON"));
        // default GitJanitor (no task/output) → prepare rubric
        let janitor = agent_evaluation_rubric("GitJanitorAgent", "", "").expect("janitor");
        assert!(janitor.contains("prepare_git_copy"));
        assert!(janitor.contains("commit_message"));
    }

    #[test]
    fn git_janitor_create_branch_rubric_from_task_name() {
        assert!(is_git_janitor_create_branch_eval(
            "create_git_branch feat/GIT-1-git-janitor",
            r#"{"branch":"feat/GIT-1-git-janitor","status":"created","previous":"main"}"#
        ));
        let rubric =
            git_janitor_rubric_for("Create and checkout feat/X via create_git_branch", "{}");
        assert!(rubric.contains("create_git_branch"));
        assert!(rubric.contains("previous"));
        assert!(rubric.contains("Do NOT apply the prepare_git_copy"));
    }

    #[test]
    fn git_janitor_create_branch_rubric_from_output_shape() {
        let out = r#"{"branch":"feat/GIT-1-git-janitor","status":"created","previous":"cursor/evaluator-desired-output-exemplar"}"#;
        assert!(is_git_janitor_create_branch_eval(
            "Create and checkout feat/GIT-1-git-janitor from current HEAD",
            out
        ));
        // even without create_git_branch in task, branch/status JSON routes correctly
        assert!(is_git_janitor_create_branch_eval(
            "checkout new feature branch",
            out
        ));
        let rubric = agent_evaluation_rubric("GitJanitorAgent", "checkout new feature branch", out)
            .expect("rubric");
        assert!(rubric.contains("create_git_branch"));
        assert!(rubric.contains("Do NOT apply the prepare_git_copy"));
    }

    #[test]
    fn git_janitor_prepare_rubric_when_emit_json() {
        let out = r#"{"commit_message":"feat: x","pr_title":"feat: x","pr_body":"b","changelog_entry":"c","branch_status":"ok","action_required":"none","commit_allowed":true,"suggested_branch_name":"feat/x","current_branch":"feat/x"}"#;
        assert!(!is_git_janitor_create_branch_eval(
            "prepare_git_copy for feature",
            out
        ));
        let rubric = git_janitor_rubric_for("prepare_git_copy for feature", out);
        assert!(rubric.contains("prepare_git_copy"));
        assert!(rubric.contains("commit_allowed"));
    }

    #[test]
    fn git_janitor_prepare_shape_wins_over_create_branch_task_name() {
        let out = r#"{"commit_message":"feat: x","pr_title":"feat: x","pr_body":"b","changelog_entry":"c","branch_status":"on_default","action_required":"create_branch","commit_allowed":false,"suggested_branch_name":"feat/x","current_branch":"main"}"#;
        // Task mentions create_git_branch, but the output is a prepare result —
        // shape must win so the prepare rubric applies, not the branch rubric.
        assert!(!is_git_janitor_create_branch_eval(
            "create_git_branch then re-run prepare_git_copy",
            out
        ));
        let rubric = git_janitor_rubric_for("create_git_branch then re-run prepare_git_copy", out);
        assert!(rubric.contains("prepare_git_copy"));
        assert!(!rubric.contains("create_git_branch (override"));
    }

    #[test]
    fn evaluator_prompt_flags_orchestrator_paraphrase() {
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("query_job_status.result"));
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("score ≤3"));
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("verifiable evidence"));
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("desired_output"));
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("When score is 10, desired_output MUST be \"\""));
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("information density"));
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("Never put full source files into desired_output"));
        assert!(EVALUATOR_SYSTEM_PROMPT.contains("whole-file dumps"));
    }

    #[test]
    fn builder_rubric_rewards_dense_report_not_full_source() {
        let builder = agent_evaluation_rubric("Phase_4_Builder", "", "").expect("builder");
        assert!(builder.contains("[SCENARIOS]"));
        assert!(builder.contains("named scenarios"));
        assert!(builder
            .contains("full test or production source body or large code dump in report max 4"));
        assert!(builder.contains("Do NOT require full test source"));
        assert!(!builder.contains("Delivers full test source"));
    }

    #[test]
    fn scout_rubric_caps_whole_file_dumps() {
        let scout = agent_evaluation_rubric("Phase_1_Scout", "", "").expect("scout");
        assert!(scout.contains("whole-file or huge verbatim dump max 4"));
        assert!(scout.contains("2–5 line"));
        assert!(scout.contains("Health-check"));
        assert!(scout.contains("Pong"));
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
    fn parse_evaluation_response_allows_empty_desired_when_score_below_10() {
        let mut payload =
            EvaluatorAgent::<crate::llm::ConfiguredLlmClient>::parse_evaluation_response(
                r#"{"score": 5, "critique": "weak", "desired_output": "   "}"#,
            )
            .expect("parse");
        normalize_desired_output(&mut payload)
            .expect("empty desired_output allowed when score < 10");
        assert!(payload.desired_output.is_empty());
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

    #[test]
    fn strip_auto_eval_appendix_cuts_new_and_legacy_markers() {
        let with_new = format!(
            "agent body{}",
            format_eval_job_appendix(&AgentEvalSummary {
                score: 7,
                critique: "ok".into(),
                desired_output: "better".into(),
            })
        );
        assert_eq!(strip_auto_eval_appendix(&with_new), "agent body");

        let with_legacy = "agent body\n\nEvaluation: QA score 3/10\nCritique: stale";
        assert_eq!(strip_auto_eval_appendix(with_legacy), "agent body");
    }

    #[test]
    fn strip_auto_eval_appendix_ignores_incidental_qa_score_mention() {
        let body = "Discussed prior run:\n\nEvaluation: QA score was weak overall.\nKeep going.";
        assert_eq!(strip_auto_eval_appendix(body), body);
    }

    #[test]
    fn find_legacy_eval_appendix_rejects_empty_and_zero_scores() {
        assert!(
            find_legacy_eval_appendix("body\n\nEvaluation: QA score /10\nCritique: x").is_none()
        );
        assert!(
            find_legacy_eval_appendix("body\n\nEvaluation: QA score 0/10\nCritique: x").is_none()
        );
        assert!(
            find_legacy_eval_appendix("body\n\nEvaluation: QA score 00/10\nCritique: x").is_none()
        );
        assert_eq!(
            find_legacy_eval_appendix("body\n\nEvaluation: QA score 10/10\nCritique: x"),
            Some(4)
        );
    }
}
