use std::path::{Path, PathBuf};
use std::sync::Arc;

use mcp_adjutant::agent::{
    AgentLoopOrchestrator, BuildCommandRunner, ScoutAgent, TransformerAgent, TriageAgent,
    TRANSFORMER_SYSTEM_PROMPT,
};
use mcp_adjutant::domain::AdjutantConfig;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};
use mcp_adjutant::BuildResult;

struct MockTransformerLlm {
    turn: LlmModelTurn,
}

impl MockTransformerLlm {
    fn apply_codemod(file_a: &Path, file_b: &Path) -> Self {
        let targets = serde_json::json!([
            {"file_path": file_a.display().to_string(), "lines": [1]},
            {"file_path": file_b.display().to_string(), "lines": [1]},
        ]);
        Self {
            turn: LlmModelTurn {
                content: Some("Thought: apply validate(true) at call sites".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "apply_structural_codemod".to_string(),
                    arguments: serde_json::json!({
                        "transformation_rule": "Add true as argument to validate",
                        "refactor_targets_json": targets.to_string(),
                    }),
                }],
                ..Default::default()
            },
        }
    }

    fn apply_range_codemod(file: &Path, start: usize, end: usize) -> Self {
        let targets = serde_json::json!([{
            "file_path": file.display().to_string(),
            "ranges": [{"start": start, "end": end}],
        }]);
        Self {
            turn: LlmModelTurn {
                content: Some("Thought: rename struct literal fields".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "apply_structural_codemod".to_string(),
                    arguments: serde_json::json!({
                        "transformation_rule": "headline -> subject",
                        "refactor_targets_json": targets.to_string(),
                    }),
                }],
                ..Default::default()
            },
        }
    }
}

impl LlmClient for MockTransformerLlm {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, TRANSFORMER_SYSTEM_PROMPT);
        assert!(!request.tools.is_empty());
        Ok(self.turn.clone())
    }
}

struct MockCodemodLlm;

impl LlmClient for MockCodemodLlm {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert!(
            request.user_message.contains("Transformation rule"),
            "expected codemod prompt"
        );
        let content = if request.user_message.contains("Target lines:") {
            "LogEvent {\n        subject: 1,\n    }".to_string()
        } else {
            "config.validate(true);".to_string()
        };
        Ok(LlmModelTurn {
            content: Some(content),
            tool_calls: vec![],
            ..Default::default()
        })
    }
}

struct PanicScoutLlm;

impl LlmClient for PanicScoutLlm {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        Err("Scout LLM should not run when refactor targets are supplied directly".to_string())
    }
}

struct MockTriageBuildRunner;

impl BuildCommandRunner for MockTriageBuildRunner {
    fn run_build_command(&self, _dir: &Path, _command: &str) -> Result<BuildResult, String> {
        Ok(BuildResult {
            exit_code: 0,
            output: "    Finished dev [unoptimized + debuginfo] target(s)".to_string(),
            success: true,
        })
    }
}

struct PanicTriageLlm;

impl LlmClient for PanicTriageLlm {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        Err("Triage LLM should not run when build already passes".to_string())
    }
}

fn temp_root(test_name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("mcp-adjutant-{test_name}-{nanos}"))
}

fn setup_refactor_project(root: &Path) -> (PathBuf, PathBuf) {
    std::fs::create_dir_all(root.join("src")).expect("src dir");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"refactor-demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("cargo.toml");
    std::fs::write(root.join("src/lib.rs"), "pub mod a;\npub mod b;\n").expect("lib.rs");

    let file_a = root.join("src/a.rs");
    let file_b = root.join("src/b.rs");
    std::fs::write(&file_a, "config.validate();\n").expect("a.rs");
    std::fs::write(&file_b, "config.validate();\n").expect("b.rs");

    (file_a, file_b)
}

#[tokio::test]
async fn transformer_agent_applies_multi_file_codemod_and_chains_triage() {
    let project_root = temp_root("transformer-codemod");
    let (file_a, file_b) = setup_refactor_project(&project_root);

    let llm = MockTransformerLlm::apply_codemod(&file_a, &file_b);
    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![file_a.clone(), file_b.clone()],
        Arc::clone(&config),
        MockTriageBuildRunner,
    );
    let agent = TransformerAgent::new(
        llm,
        MockCodemodLlm,
        ScoutAgent::new(PanicScoutLlm),
        triage_agent,
    );

    let result = AgentLoopOrchestrator::run(
        &agent,
        "PHASE_3_5_TRANSFORMER\nAdd true as argument to validate call sites".to_string(),
        1,
    )
    .await
    .expect("transformer loop should complete");

    let updated_a = std::fs::read_to_string(&file_a).expect("read a.rs");
    let updated_b = std::fs::read_to_string(&file_b).expect("read b.rs");

    assert!(
        updated_a.contains("config.validate(true)"),
        "a.rs should be codemodded, got: {updated_a}"
    );
    assert!(
        updated_b.contains("config.validate(true)"),
        "b.rs should be codemodded, got: {updated_b}"
    );
    assert!(result.is_finished, "refactor should finish successfully");
    assert!(
        result.accumulated_data.contains("[TRANSFORMER OK]"),
        "expected success marker, got: {}",
        result.accumulated_data
    );
    assert!(
        result.accumulated_data.contains("Launching Triage"),
        "expected triage chaining log"
    );
    assert!(
        !result.accumulated_data.contains("[TRIAGE FAILURE]"),
        "triage should succeed"
    );

    std::fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn transformer_agent_applies_range_codemod_for_struct_literals() {
    let project_root = temp_root("transformer-range-codemod");
    std::fs::create_dir_all(project_root.join("src")).expect("src dir");
    std::fs::write(
        project_root.join("Cargo.toml"),
        "[package]\nname = \"range-demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("cargo.toml");
    let file = project_root.join("src/use.rs");
    std::fs::write(
        &file,
        "fn demo() {\n    let _ = LogEvent {\n        headline: 1,\n    };\n}\n",
    )
    .expect("write");

    let llm = MockTransformerLlm::apply_range_codemod(&file, 2, 4);
    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![file.clone()],
        Arc::clone(&config),
        MockTriageBuildRunner,
    );
    let agent = TransformerAgent::new(
        llm,
        MockCodemodLlm,
        ScoutAgent::new(PanicScoutLlm),
        triage_agent,
    );

    let result = AgentLoopOrchestrator::run(
        &agent,
        "PHASE_3_5_TRANSFORMER\nRename headline to subject in LogEvent literals".to_string(),
        1,
    )
    .await
    .expect("transformer loop should complete");

    let updated = std::fs::read_to_string(&file).expect("read");
    assert!(
        updated.contains("subject: 1"),
        "range codemod should rewrite struct literal block, got: {updated}"
    );
    assert!(
        result.accumulated_data.contains("lines 2-4"),
        "expected range codemod log, got: {}",
        result.accumulated_data
    );
    assert!(result.accumulated_data.contains("[TRANSFORMER OK]"));

    std::fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn transformer_skips_missing_target_files_without_failing_whole_job() {
    let project_root = temp_root("transformer-missing-file");
    let (file_a, _file_b) = setup_refactor_project(&project_root);
    let missing = project_root.join("src/missing.rs");

    let targets = serde_json::json!([
        {"file_path": missing.display().to_string(), "lines": [1]},
        {"file_path": file_a.display().to_string(), "lines": [1]},
    ]);
    let llm = MockTransformerLlm {
        turn: LlmModelTurn {
            content: Some("Thought: apply codemod".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "apply_structural_codemod".to_string(),
                arguments: serde_json::json!({
                    "transformation_rule": "Add true as argument to validate",
                    "refactor_targets_json": targets.to_string(),
                }),
            }],
            ..Default::default()
        },
    };

    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![file_a.clone()],
        Arc::clone(&config),
        MockTriageBuildRunner,
    );
    let agent = TransformerAgent::new(
        llm,
        MockCodemodLlm,
        ScoutAgent::new(PanicScoutLlm),
        triage_agent,
    );

    let result = AgentLoopOrchestrator::run(
        &agent,
        "PHASE_3_5_TRANSFORMER\nAdd true as argument to validate call sites".to_string(),
        1,
    )
    .await
    .expect("transformer should continue after missing file");

    assert!(
        result.accumulated_data.contains("Skipped missing file"),
        "expected skip log, got: {}",
        result.accumulated_data
    );
    assert!(
        std::fs::read_to_string(&file_a)
            .expect("read a.rs")
            .contains("config.validate(true)"),
        "existing file should still be codemodded"
    );

    std::fs::remove_dir_all(&project_root).ok();
}
