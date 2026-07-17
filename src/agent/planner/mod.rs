mod args;
mod constraints;
mod tools;

use async_trait::async_trait;
use serde_json::Value;

use super::orchestrator::{run_single_tool_turn, AgentLoopOrchestrator};
use super::traits::{AgentContext, AutonomousAgent};
use crate::cache::resolve_workspace_path;
use crate::llm::{LlmClient, LlmToolSet};

pub use args::{parse_plan_blueprint_args, PlanBlueprintArgs, PlanKind};
pub use constraints::CoordinatorConstraints;
pub use tools::{
    extract_json_object, planner_emit_tool_set, planner_scout_tool_set, validate_blueprint,
    validate_blueprint_coordinator, validate_blueprint_grounding,
};

pub const PLANNER_SCOUT_MAX_ITERATIONS: u32 = 12;
pub const PLANNER_EMIT_MAX_ITERATIONS: u32 = 3;
pub const PLANNER_JSON_FIX_ITERATIONS: u32 = 4;
pub const PLANNER_JSON_FIX_ROUNDS: u32 = 2;

pub const PLANNER_MAX_ITERATIONS: u32 = PLANNER_SCOUT_MAX_ITERATIONS + PLANNER_EMIT_MAX_ITERATIONS;

pub const PLANNER_SCOUT_SYSTEM_PROMPT: &str = r#"You are the Lead Software Architect (PLANNER scout phase). Gather repo evidence only — you do NOT emit the blueprint yet.

READ-ONLY tools:
- detect_language, ripgrep, ast_calls, read_file, extract_search_anchor

Strategy:
1. Use ripgrep/ast_calls to locate files and line numbers relevant to the feature or bug.
2. read_file every file you expect to patch or that defines wiring (package entry, manifest, target module).
3. Use extract_search_anchor(file, start, end) to copy verbatim SEARCH anchors for future patch_file hunks.
4. Scout 3–8 turns. One tool per turn. Do not output Blueprint JSON in chat."#;

pub const PLANNER_EMIT_SYSTEM_PROMPT: &str = r#"You are the Lead Software Architect (PLANNER emit phase). Synthesize a strict Blueprint JSON pipeline from scout evidence below.

Downstream sub-agents execute your pipeline. Triage runs automatically inside Builder/Transpiler — never put TriageAgent in pipeline steps.

EMIT tools:
- read_file (only if a target_file was missed during scout)
- emit_blueprint (terminal)

Agent routing (mandatory):
| action | agent |
| patch_file, create_file, generate_tests | BuilderAgent |
| sync_types | TranspilerAgent |

Blueprint JSON schema:
{
  "task_id": "kebab-case-id",
  "architecture_summary": "brief approach",
  "pipeline": [{
    "step": 1,
    "agent": "BuilderAgent" | "TranspilerAgent",
    "action": "patch_file" | "create_file" | "sync_types" | "generate_tests",
    "target_file": "repo-relative path",
    "goal": "directive citing path:line evidence from scout tools",
    "patch_content": "see rules 2 & 10"
  }]
}

patch_content format (critical):
- create_file → full file contents (new file, no anchors).
- patch_file on EXISTING files → one or more SEARCH/REPLACE hunks (never a full function rewrite):
<<<<<<< SEARCH
    let app = Router::new()
        .route("/api/config", get(get_config).put(put_config))
=======
    let limit = crate::config_rate_limit::RateLimitState::new(60);
    let api = Router::new()
        .route("/api/config", get(get_config).put(put_config))
>>>>>>> REPLACE
- sync_types / generate_tests → empty string "" (BuilderAgent writes tests; goal must cite tests/foo.rs:1).

Hard rules:
1. Emit only via emit_blueprint — no prose outside JSON.
2. patch_content for patch_file MUST be paste-ready SEARCH/REPLACE hunks (format above). create_file takes full contents. No comment sketches, no "...", no placeholders — emit_blueprint rejects ellipses.
3. You MUST read_file every target_file before emit_blueprint (scout evidence counts; re-read if unsure).
4. Every goal must cite path:line evidence (e.g. "Split router at config_server.rs:35"), including generate_tests goals (e.g. "tests/rate_limit_test.rs:1").
5. sync_types and generate_tests: patch_content must be "".
6. target_file must exist on disk (except create_file).
7. Any patch_file/create_file work MUST end with a generate_tests step (non-optional final step).
8. create_file for a new source module MUST include patch_file on the package entry (lib.rs, mod.rs, __init__.py, index.ts, …).
9. When adding dependencies, patch the project manifest (Cargo.toml, package.json, go.mod, pyproject.toml, …) with a SEARCH/REPLACE hunk anchored to a real manifest line.
10. SURGICAL: every SEARCH anchor MUST be copied verbatim from scout read_file/extract_search_anchor output. emit_blueprint rejects any SEARCH block not found on disk, any REPLACE identical to SEARCH, and any REPLACE that adds >15 lines over its SEARCH. New logic goes in a create_file step; patch_file is wiring/registration only (module declares, route registration, manifest line, struct field).

Feature pipeline template (follow this order when applicable):
1. create_file for new module (where new logic lives) OR patch_file on existing files (wiring hunks only)
2. patch_file package entry (1-line module declare hunk) when adding a module
3. patch_file dependency manifest (single-line hunk) when adding deps or features
4. generate_tests as the final step

Strategy: Call emit_blueprint with complete JSON. One tool per turn."#;

pub const PLANNER_SYSTEM_PROMPT: &str = PLANNER_EMIT_SYSTEM_PROMPT;

pub fn format_scout_prompt(args: &PlanBlueprintArgs) -> String {
    let mut prompt = format!(
        "plan_blueprint (scout phase)\n\nFeature request / bug report:\n{}\n",
        args.feature_request
    );
    if let Some(kind) = &args.plan_kind {
        prompt.push_str(&format!(
            "\n## Coordinator plan kind\n\n{} — {}\n",
            kind.as_str(),
            kind.playbook()
        ));
    }
    if let Some(expectation) = &args.expectation {
        prompt.push_str(&format!(
            "\n## Coordinator expectations\n\n{expectation}\nTreat these as hard constraints on the eventual pipeline shape.\n"
        ));
    }
    prompt.push_str("\n\n");
    prompt.push_str(PLANNER_SCOUT_SYSTEM_PROMPT);
    prompt
}

pub fn format_emit_prompt(args: &PlanBlueprintArgs, scout: &AgentContext) -> String {
    let mut prompt = format!(
        "plan_blueprint (emit phase)\n\nFeature request / bug report:\n{}\n",
        args.feature_request
    );
    if let Some(kind) = &args.plan_kind {
        prompt.push_str(&format!(
            "\n## Coordinator plan kind\n\n{} — {}\n",
            kind.as_str(),
            kind.playbook()
        ));
        prompt.push_str(kind.emit_few_shot());
    }
    if let Some(expectation) = &args.expectation {
        prompt.push_str(&format!(
            "\n## Coordinator expectations\n\n{expectation}\nTreat these as hard constraints on pipeline shape, agents, and patch style.\n"
        ));
    }
    let constraints = CoordinatorConstraints::from_args(args);
    if constraints.surgical_patches {
        prompt.push_str(
            "\nemit_blueprint enforces surgical SEARCH/REPLACE hunks: every SEARCH anchor must be copied verbatim from scout evidence (grounding), REPLACE must differ from SEARCH, and REPLACE may add at most 15 lines over SEARCH.\n",
        );
    }
    prompt.push_str("\n## Scout evidence (read_file / extract_search_anchor observations)\n\n");
    prompt.push_str(&condense_scout_evidence(&scout.accumulated_data));
    if !scout.touched_files.is_empty() {
        prompt.push_str("\n\n## Files read during scout\n\n");
        for path in &scout.touched_files {
            prompt.push_str(&format!("- {}\n", path.display()));
        }
    }
    prompt.push_str("\n\n");
    prompt.push_str(PLANNER_EMIT_SYSTEM_PROMPT);
    prompt
}

fn take_char_boundary(s: &str, max_bytes: usize, from_end: bool) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    if from_end {
        let mut start = s.len().saturating_sub(max_bytes);
        while start < s.len() && !s.is_char_boundary(start) {
            start += 1;
        }
        &s[start..]
    } else {
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

fn condense_scout_evidence(accumulated: &str) -> String {
    const MAX_BYTES: usize = 24_000;
    const HALF: usize = 12_000;
    if accumulated.len() <= MAX_BYTES {
        return accumulated.to_string();
    }
    let head = take_char_boundary(accumulated, HALF, false);
    let tail = take_char_boundary(accumulated, HALF, true);
    format!("{head}\n\n...[scout evidence truncated]...\n\n{tail}")
}

pub async fn run_planner_hybrid<C: LlmClient>(
    scout_client: C,
    emit_client: C,
    args: PlanBlueprintArgs,
) -> Result<AgentContext, String> {
    let coordinator = CoordinatorConstraints::from_args(&args);
    let scout_agent = PlannerHybridAgent::scout(scout_client);
    let scout_ctx = AgentLoopOrchestrator::run(
        &scout_agent,
        format_scout_prompt(&args),
        PLANNER_SCOUT_MAX_ITERATIONS,
    )
    .await?;

    let emit_agent = PlannerHybridAgent::emit(emit_client, coordinator.clone());
    let mut emit_ctx = AgentContext {
        input_prompt: format_emit_prompt(&args, &scout_ctx),
        accumulated_data: scout_ctx.accumulated_data,
        iterations: 0,
        max_iterations: PLANNER_EMIT_MAX_ITERATIONS,
        is_finished: false,
        agent_completed: false,
        touched_files: scout_ctx.touched_files,
        last_tool_call: None,
    };
    emit_ctx =
        AgentLoopOrchestrator::resume(&emit_agent, emit_ctx, PLANNER_EMIT_MAX_ITERATIONS).await?;

    for _ in 0..PLANNER_JSON_FIX_ROUNDS {
        if emit_ctx.agent_completed {
            break;
        }
        let Some(reason) = planner_json_fixup_reason(&emit_ctx.accumulated_data, &coordinator)
        else {
            break;
        };
        emit_ctx.is_finished = false;
        emit_ctx.input_prompt.push_str(&format!(
            "\n\nBLUEPRINT JSON FIX REQUIRED: {reason}\nCall emit_blueprint with valid Blueprint JSON (schema in system prompt). Do not paste JSON in chat prose."
        ));
        emit_ctx =
            AgentLoopOrchestrator::resume(&emit_agent, emit_ctx, PLANNER_JSON_FIX_ITERATIONS)
                .await?;
    }

    Ok(emit_ctx)
}

pub fn planner_json_fixup_reason(
    accumulated_data: &str,
    coordinator: &CoordinatorConstraints,
) -> Option<String> {
    let Some(json) = extract_json_object(accumulated_data) else {
        return Some(
            "output contains no Blueprint JSON object — call emit_blueprint with the full schema"
                .to_string(),
        );
    };
    match validate_blueprint(json) {
        Ok(bp) => match validate_blueprint_coordinator(&bp, coordinator) {
            Ok(()) => Some(
                "valid Blueprint JSON found in observations but emit_blueprint was not called — call emit_blueprint with it".to_string(),
            ),
            Err(err) => Some(err),
        },
        Err(err) => Some(err),
    }
}

#[derive(Clone, Copy)]
enum PlannerLoopPhase {
    Scout,
    Emit,
}

pub struct PlannerHybridAgent<C: LlmClient> {
    client: C,
    phase: PlannerLoopPhase,
    tools: LlmToolSet,
}

pub type PlannerAgent<C> = PlannerHybridAgent<C>;

impl<C: LlmClient> PlannerHybridAgent<C> {
    pub fn scout(client: C) -> Self {
        Self {
            client,
            phase: PlannerLoopPhase::Scout,
            tools: planner_scout_tool_set(),
        }
    }

    pub fn emit(client: C, coordinator: CoordinatorConstraints) -> Self {
        Self {
            client,
            phase: PlannerLoopPhase::Emit,
            tools: planner_emit_tool_set(coordinator),
        }
    }

    pub fn new(client: C, args: PlanBlueprintArgs) -> Self {
        Self::emit(client, CoordinatorConstraints::from_args(&args))
    }

    fn system_prompt(&self) -> &'static str {
        match self.phase {
            PlannerLoopPhase::Scout => PLANNER_SCOUT_SYSTEM_PROMPT,
            PlannerLoopPhase::Emit => PLANNER_EMIT_SYSTEM_PROMPT,
        }
    }
}

fn record_planner_touched_file(context: &mut AgentContext, tool_name: &str, args: &Value) {
    let Some(path) = (match tool_name {
        "read_file" | "extract_search_anchor" => args.get("file").and_then(Value::as_str),
        _ => None,
    }) else {
        return;
    };
    let resolved = resolve_workspace_path(path);
    if !context.touched_files.iter().any(|p| p == &resolved) {
        context.touched_files.push(resolved);
    }
}

fn reject_incomplete_blueprint(context: &mut AgentContext, reason: &str) {
    context.agent_completed = false;
    context.is_finished = false;
    context.input_prompt.push_str(&format!(
        "\nBlueprint rejected after emit_blueprint: {reason}\nRead every target_file with read_file, fix the JSON, call emit_blueprint again."
    ));
}

fn nudge_prose_json(context: &mut AgentContext) {
    let Some(thought) = last_thought_block(&context.accumulated_data) else {
        return;
    };
    if !thought.contains('{') {
        return;
    }
    let nudge = match extract_json_object(thought)
        .map(validate_blueprint)
        .transpose()
    {
        Ok(Some(_)) => {
            "You pasted valid Blueprint JSON in chat — call emit_blueprint with that JSON instead."
        }
        Ok(None) => return,
        Err(err) => {
            return context.input_prompt.push_str(&format!(
                "\nBlueprint JSON in chat is invalid ({err}) — fix and call emit_blueprint."
            ));
        }
    };
    if !context.input_prompt.contains(nudge) {
        context.input_prompt.push_str(&format!("\n{nudge}\n"));
    }
}

fn last_thought_block(accumulated: &str) -> Option<&str> {
    let marker = "Thought:\n";
    let idx = accumulated.rfind(marker)?;
    let start = idx + marker.len();
    let rest = &accumulated[start..];
    let end = rest.find("\nObservation:").unwrap_or(rest.len());
    Some(rest[..end].trim())
}

#[async_trait]
impl<C: LlmClient> AutonomousAgent for PlannerHybridAgent<C> {
    fn name(&self) -> &'static str {
        match self.phase {
            PlannerLoopPhase::Scout => "planner_scout_agent",
            PlannerLoopPhase::Emit => "planner_emit_agent",
        }
    }

    async fn enrich_context(&self, _context: &mut AgentContext) -> Result<(), String> {
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        let called =
            run_single_tool_turn(&self.client, &self.tools, self.system_prompt(), context)?;
        if let Some((tool_name, args)) = called {
            record_planner_touched_file(context, &tool_name, &args);
            if matches!(self.phase, PlannerLoopPhase::Emit) && context.agent_completed {
                if let Err(reason) = validate_blueprint(&context.accumulated_data)
                    .and_then(|bp| validate_blueprint_grounding(&bp, &context.touched_files))
                {
                    reject_incomplete_blueprint(context, &reason);
                }
            }
        } else if matches!(self.phase, PlannerLoopPhase::Emit) {
            nudge_prose_json(context);
        }
        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        let nudge = match self.phase {
            PlannerLoopPhase::Scout if context.iterations >= context.max_iterations.saturating_sub(1) => {
                "\nFinal scout turn: read_file any unread target, then scouting ends."
            }
            PlannerLoopPhase::Scout => {
                "\nContinue scouting. Call exactly one tool: detect_language, ripgrep, ast_calls, read_file, or extract_search_anchor."
            }
            PlannerLoopPhase::Emit if context.iterations >= context.max_iterations.saturating_sub(1) => {
                "\nFinal emit turn: read_file any unread target_file, then emit_blueprint with complete JSON."
            }
            PlannerLoopPhase::Emit => {
                "\nContinue. Call exactly one tool: read_file or emit_blueprint."
            }
        };
        context.input_prompt.push_str(nudge);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_scout_prompt_includes_kind_and_expectation() {
        let args = PlanBlueprintArgs {
            feature_request: "add rate limit".to_string(),
            plan_kind: Some(PlanKind::Feature),
            expectation: Some("surgical patches only".to_string()),
        };
        let prompt = format_scout_prompt(&args);
        assert!(prompt.contains("scout phase"));
        assert!(prompt.contains("Coordinator plan kind"));
        assert!(prompt.contains("feature —"));
        assert!(prompt.contains("Coordinator expectations"));
        assert!(prompt.contains("extract_search_anchor"));
        assert!(!prompt.contains("emit_blueprint"));
    }

    #[test]
    fn format_emit_prompt_includes_few_shot_and_scout_evidence() {
        let args = PlanBlueprintArgs {
            feature_request: "add rate limit".to_string(),
            plan_kind: Some(PlanKind::Feature),
            expectation: Some("surgical".to_string()),
        };
        let scout = AgentContext {
            input_prompt: String::new(),
            accumulated_data: "Tool: read_file\nObservation:\nline 1\n".to_string(),
            iterations: 3,
            max_iterations: PLANNER_SCOUT_MAX_ITERATIONS,
            is_finished: true,
            agent_completed: false,
            touched_files: vec![resolve_workspace_path("src/lib.rs")],
            last_tool_call: None,
        };
        let prompt = format_emit_prompt(&args, &scout);
        assert!(prompt.contains("emit phase"));
        assert!(prompt.contains("Example feature pipeline shape"));
        assert!(prompt.contains("Scout evidence"));
        assert!(prompt.contains("emit_blueprint"));
        assert!(prompt.contains("SEARCH/REPLACE"));
    }

    #[test]
    fn emit_system_prompt_documents_emit_tools() {
        for tool in ["read_file", "emit_blueprint"] {
            assert!(
                PLANNER_EMIT_SYSTEM_PROMPT.contains(tool),
                "emit prompt missing {tool}"
            );
        }
    }

    #[test]
    fn scout_system_prompt_documents_scout_tools() {
        for tool in [
            "detect_language",
            "ripgrep",
            "ast_calls",
            "read_file",
            "extract_search_anchor",
        ] {
            assert!(
                PLANNER_SCOUT_SYSTEM_PROMPT.contains(tool),
                "scout prompt missing {tool}"
            );
        }
        assert!(!PLANNER_SCOUT_SYSTEM_PROMPT.contains("emit_blueprint"));
    }

    #[test]
    fn record_touched_file_tracks_read_file_only() {
        let mut ctx = AgentContext {
            input_prompt: String::new(),
            accumulated_data: String::new(),
            iterations: 0,
            max_iterations: PLANNER_MAX_ITERATIONS,
            is_finished: false,
            agent_completed: false,
            touched_files: Vec::new(),
            last_tool_call: None,
        };
        let args = serde_json::json!({ "file": "src/main.rs" });
        record_planner_touched_file(&mut ctx, "read_file", &args);
        assert_eq!(ctx.touched_files.len(), 1);
        assert!(ctx.touched_files[0].ends_with("src/main.rs"));

        record_planner_touched_file(&mut ctx, "ast_calls", &args);
        assert_eq!(
            ctx.touched_files.len(),
            1,
            "ast_calls must not count as read_file grounding"
        );

        record_planner_touched_file(&mut ctx, "ripgrep", &args);
        assert_eq!(
            ctx.touched_files.len(),
            1,
            "ripgrep should not record paths"
        );
    }

    #[test]
    fn planner_json_fixup_reason_flags_missing_json() {
        let none = CoordinatorConstraints::none();
        let reason = planner_json_fixup_reason("Thought:\njust prose\n", &none).expect("reason");
        assert!(reason.contains("no Blueprint JSON"));
    }

    #[test]
    fn planner_json_fixup_reason_flags_invalid_json() {
        let none = CoordinatorConstraints::none();
        let reason = planner_json_fixup_reason(
            r#"{"task_id":"my-task","architecture_summary":"ok"}"#,
            &none,
        )
        .expect("reason");
        assert!(reason.contains("pipeline"));
    }

    #[test]
    fn planner_json_fixup_reason_flags_unemitted_valid_json() {
        let none = CoordinatorConstraints::none();
        let golden = include_str!("../../../tests/fixtures/golden-rate-limit-blueprint.json");
        let reason = planner_json_fixup_reason(golden, &none).expect("reason");
        assert!(reason.contains("emit_blueprint"));
    }

    #[test]
    fn reject_incomplete_blueprint_clears_completion() {
        let mut ctx = AgentContext {
            input_prompt: String::new(),
            accumulated_data: "{}".to_string(),
            iterations: 1,
            max_iterations: PLANNER_EMIT_MAX_ITERATIONS,
            is_finished: true,
            agent_completed: true,
            touched_files: Vec::new(),
            last_tool_call: None,
        };
        reject_incomplete_blueprint(&mut ctx, "not grounded");
        assert!(!ctx.agent_completed);
        assert!(!ctx.is_finished);
        assert!(ctx.input_prompt.contains("not grounded"));
    }

    #[test]
    fn condense_scout_evidence_passes_short_input_unchanged() {
        let short = "scout observations";
        assert_eq!(condense_scout_evidence(short), short);
    }

    #[test]
    fn condense_scout_evidence_truncates_on_char_boundary() {
        let mut s = "x".repeat(11_997);
        s.push('🦀');
        s.push_str(&"y".repeat(12_000));
        let out = condense_scout_evidence(&s);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        assert!(out.contains("truncated"));
    }
}
