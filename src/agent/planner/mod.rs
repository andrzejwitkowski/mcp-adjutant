mod args;
mod constraints;
mod tools;

use async_trait::async_trait;
use serde_json::Value;

use super::orchestrator::{run_single_tool_turn, AgentLoopOrchestrator};
use super::traits::{AgentContext, AutonomousAgent};
use crate::cache::resolve_workspace_path;
use crate::llm::{LlmClient, LlmToolSet};

pub use crate::cache::display_rel;
pub use args::{parse_plan_blueprint_args, PlanBlueprintArgs, PlanKind};
pub use constraints::CoordinatorConstraints;
pub use tools::{
    apply_blueprint_file_steps, apply_hunks_to_body, extract_json_object, path_line_from_goal,
    planner_emit_tool_set, planner_scout_tool_set, source_under_test_from_goal,
    test_type_for_target, validate_blueprint, validate_blueprint_coordinator,
    validate_blueprint_grounding,
};

pub const PLANNER_SCOUT_MAX_ITERATIONS: u32 = 12;
/// begin + ~6 patches + tests + finalize (+ retries)
pub const PLANNER_EMIT_MAX_ITERATIONS: u32 = 12;
/// Soft nudges to continue draft tools when finalize never ran.
pub const PLANNER_JSON_FIX_ITERATIONS: u32 = 4;
pub const PLANNER_JSON_FIX_ROUNDS: u32 = 1;

pub const PLANNER_MAX_ITERATIONS: u32 = PLANNER_SCOUT_MAX_ITERATIONS + PLANNER_EMIT_MAX_ITERATIONS;

pub const PLANNER_SCOUT_SYSTEM_PROMPT: &str = r#"You are the Lead Software Architect (PLANNER scout phase). Gather repo evidence only — you do NOT emit the blueprint yet.

READ-ONLY tools:
- detect_language, ripgrep, ast_calls, read_file, extract_search_anchor

Strategy:
1. Use ripgrep/ast_calls to locate files and line numbers relevant to the feature or bug.
2. read_file every file you expect to patch or that defines wiring (package entry, manifest, target module).
3. Use extract_search_anchor(file, start, end) to copy verbatim SEARCH anchors for future patch hunks (≥2 lines).
4. Scout 3–8 turns. One tool per turn. Do not output Blueprint JSON in chat."#;

pub const PLANNER_EMIT_SYSTEM_PROMPT: &str = r#"You are the Lead Software Architect (PLANNER emit phase). Build a Blueprint by calling draft tools — Rust assembles valid JSON. Never invent a Blueprint JSON string.

Downstream: BuilderAgent applies steps. Triage runs automatically — never put TriageAgent in the pipeline.

EMIT tools (exactly one per turn):
- read_file — only if a target was missed during scout
- blueprint_begin(task_id, architecture_summary) — once; task_id must be kebab-case with a hyphen
- blueprint_add_patch(target_file, goal, search, replace) — diffs only; pass RAW search/replace text (no <<<<<<< markers); search ≥2 non-empty lines copied from scout; replace ≤15 extra lines
- blueprint_add_create(target_file, goal, contents) — ONLY tiny new files ≤15 lines; prefer patch
- blueprint_add_tests(target_file, goal) — usually last; goal must cite path:line
- blueprint_finalize() — terminal; Rust validates and returns JSON

Hard rules:
1. Sequence: begin → one or more add_patch (and rare add_create) → add_tests → finalize.
2. Goals must cite path:line. architecture_summary should too.
3. Do NOT paste full files into create/patch. Plans are SEARCH/REPLACE diffs.
4. Do NOT write Blueprint JSON in chat or as a single string arg.
5. One tool per turn."#;

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
            "\nSurgical mode: use blueprint_add_patch with verbatim ≥2-line search from scout; replace ≤15 extra lines; prefer patch over create.\n",
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
    // ponytail: no soft-finalize between emit/fix — soft-finalize wiped rejected JSON (11-turn VALIDATION FAILED)
    emit_ctx = AgentLoopOrchestrator::resume_with_finalize(
        &emit_agent,
        emit_ctx,
        PLANNER_EMIT_MAX_ITERATIONS,
        false,
    )
    .await?;

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
            "\n\nBLUEPRINT DRAFT FIX REQUIRED: {reason}\n\
             Continue with blueprint_* tools (begin → add_patch/add_create/add_tests → finalize). \
             Do not paste Blueprint JSON in chat."
        ));
        emit_ctx = AgentLoopOrchestrator::resume_with_finalize(
            &emit_agent,
            emit_ctx,
            PLANNER_JSON_FIX_ITERATIONS,
            false,
        )
        .await?;
    }

    if !emit_ctx.agent_completed {
        // ponytail: skip soft-finalize when a valid JSON body already sits in accumulated_data
        if extract_json_object(&emit_ctx.accumulated_data)
            .and_then(|j| validate_blueprint(j).ok())
            .is_none()
        {
            AgentLoopOrchestrator::apply_iteration_cap(&emit_agent, &mut emit_ctx);
        }
    }

    Ok(emit_ctx)
}

pub fn planner_json_fixup_reason(
    accumulated_data: &str,
    coordinator: &CoordinatorConstraints,
) -> Option<String> {
    // Terminal finalize replaces accumulated_data with pretty JSON.
    if let Some(json) = extract_json_object(accumulated_data) {
        if let Ok(bp) = validate_blueprint(json) {
            return match validate_blueprint_coordinator(&bp, coordinator) {
                Ok(()) => None, // already valid complete blueprint
                Err(err) => Some(err),
            };
        }
    }
    if accumulated_data.contains("Tool: blueprint_finalize")
        || accumulated_data.contains("draft begun")
        || accumulated_data.contains("queued step")
    {
        return Some(
            "draft incomplete — finish with blueprint_add_* as needed then blueprint_finalize"
                .into(),
        );
    }
    Some(
        "no Blueprint draft yet — call blueprint_begin, then add_patch/add_tests, then blueprint_finalize"
            .into(),
    )
}

/// Prefer finalized JSON body (terminal output) over legacy emit_blueprint string arg.
pub fn last_emit_blueprint_arg(accumulated: &str) -> Option<String> {
    if let Ok(bp) = validate_blueprint(accumulated.trim()) {
        return serde_json::to_string(&bp).ok();
    }
    if let Some(json) = extract_json_object(accumulated) {
        if validate_blueprint(json).is_ok() {
            return Some(json.to_string());
        }
    }
    // legacy string-blob emit (tests / old transcripts)
    let marker = "Tool: emit_blueprint(";
    let start = accumulated.rfind(marker)?;
    let after = &accumulated[start + marker.len()..];
    let end = after.find(")\nObservation:")?;
    let args_raw = after[..end].trim();
    let value: Value = serde_json::from_str(args_raw).ok()?;
    value
        .get("blueprint")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
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
        "\nBlueprint rejected after finalize: {reason}\nFix with blueprint_add_* then blueprint_finalize again."
    ));
}

fn nudge_prose_json(context: &mut AgentContext) {
    let Some(thought) = last_thought_block(&context.accumulated_data) else {
        return;
    };
    if !thought.contains('{') {
        return;
    }
    let nudge =
        "Do not paste Blueprint JSON in chat — call blueprint_begin / add_patch / add_tests / blueprint_finalize.";
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
                "\nFinal emit turn: blueprint_finalize if draft is ready (begin + patches + tests)."
            }
            PlannerLoopPhase::Emit => {
                "\nContinue. Call exactly one tool: read_file, blueprint_begin, blueprint_add_patch, blueprint_add_create, blueprint_add_tests, or blueprint_finalize."
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
    fn last_emit_blueprint_arg_prefers_finalized_json() {
        let golden = include_str!("../../../tests/fixtures/golden-rate-limit-blueprint.json");
        let raw = last_emit_blueprint_arg(golden).expect("payload");
        assert!(raw.contains("task_id"));
        assert!(planner_json_fixup_reason(golden, &CoordinatorConstraints::none()).is_none());
    }

    #[test]
    fn last_emit_blueprint_arg_legacy_emit_string() {
        let hist = r#"Tool: emit_blueprint({"blueprint":"{ \"task_id\": \"bad\" }"})
Observation:
Blueprint rejected
"#;
        let raw = last_emit_blueprint_arg(hist).expect("payload");
        assert!(raw.contains("task_id"));
    }

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
        assert!(prompt.contains("blueprint_finalize"));
        assert!(prompt.contains("blueprint_add_patch"));
        assert!(!prompt.contains("emit_blueprint"));
    }

    #[test]
    fn emit_system_prompt_documents_emit_tools() {
        for tool in [
            "read_file",
            "blueprint_begin",
            "blueprint_add_patch",
            "blueprint_add_create",
            "blueprint_add_tests",
            "blueprint_finalize",
        ] {
            assert!(
                PLANNER_EMIT_SYSTEM_PROMPT.contains(tool),
                "emit prompt missing {tool}"
            );
        }
        assert!(!PLANNER_EMIT_SYSTEM_PROMPT.contains("emit_blueprint"));
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
    fn planner_json_fixup_reason_flags_missing_draft() {
        let none = CoordinatorConstraints::none();
        let reason = planner_json_fixup_reason("Thought:\njust prose\n", &none).expect("reason");
        assert!(reason.contains("blueprint_begin"), "{reason}");
    }

    #[test]
    fn planner_json_fixup_reason_flags_invalid_json() {
        let none = CoordinatorConstraints::none();
        let reason = planner_json_fixup_reason(
            r#"{"task_id":"my-task","architecture_summary":"ok"}"#,
            &none,
        )
        .expect("reason");
        assert!(
            reason.contains("pipeline") || reason.contains("blueprint_begin"),
            "{reason}"
        );
    }

    #[test]
    fn planner_json_fixup_reason_accepts_finalized_valid_json() {
        let none = CoordinatorConstraints::none();
        let golden = include_str!("../../../tests/fixtures/golden-rate-limit-blueprint.json");
        assert!(planner_json_fixup_reason(golden, &none).is_none());
    }

    #[test]
    fn planner_json_fixup_reason_flags_incomplete_draft() {
        let none = CoordinatorConstraints::none();
        let hist = "Observation:\ndraft begun task_id=fix-x\nqueued step 1: patch_file\n";
        let reason = planner_json_fixup_reason(hist, &none).expect("reason");
        assert!(reason.contains("blueprint_finalize"), "{reason}");
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
