mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::{open_cache_manager, unique_temp_project, write_demo_cargo_manifest};
use mcp_adjutant::agent::{
    AgentLoopOrchestrator, BuildCommandRunner, BuilderAgent, TriageAgent, BUILDER_SYSTEM_PROMPT,
};
use mcp_adjutant::domain::AdjutantConfig;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};

struct MockBuilderLlm {
    turn: LlmModelTurn,
}

impl MockBuilderLlm {
    fn write_red_test(path: &str, content: &str) -> Self {
        Self {
            turn: LlmModelTurn {
                content: Some("Thought: emit failing RED test".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "write_test_suite".to_string(),
                    arguments: serde_json::json!({
                        "path": path,
                        "content": content,
                        "tdd_phase": "red",
                    }),
                }],
            },
        }
    }
}

impl LlmClient for MockBuilderLlm {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, BUILDER_SYSTEM_PROMPT);
        assert!(
            !request.tools.is_empty(),
            "builder request should register tool definitions"
        );
        Ok(self.turn.clone())
    }
}

struct PanicTriageLlm;

impl LlmClient for PanicTriageLlm {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        Err("Triage LLM should not run during successful TDD RED flow".to_string())
    }
}

struct TddRedBuildRunner;

impl BuildCommandRunner for TddRedBuildRunner {
    fn run_build_command(&self, _dir: &Path, command: &str) -> Result<String, String> {
        if command.contains("check") {
            Ok("    Finished dev [unoptimized + debuginfo] target(s)".to_string())
        } else if command.contains("test") {
            Err(
                "assertion `left == right` failed\n  left: 1\n right: 2\nfailures:\n    red_phase_case\n\ntest result: FAILED. 0 passed; 1 failed".to_string(),
            )
        } else {
            Err(format!("unexpected build command: {command}"))
        }
    }
}

fn setup_cargo_project(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root).expect("project root");
    write_demo_cargo_manifest(root);
    std::fs::create_dir_all(root.join("src")).expect("src dir");
    std::fs::write(root.join("src/lib.rs"), "pub fn answer() -> i32 { 1 }\n").expect("lib.rs");
    root.join("tests/red_phase.rs")
}

#[tokio::test]
async fn builder_agent_red_phase_accepts_failing_assertions_via_triage() {
    let project_root = unique_temp_project("builder-red");
    let test_path = setup_cargo_project(&project_root);
    let test_path_str = test_path.to_string_lossy().into_owned();

    let failing_test = r#"#[test]
fn red_phase_case() {
    assert_eq!(1, 2);
}
"#;

    let llm = MockBuilderLlm::write_red_test(&test_path_str, failing_test);
    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));

    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![test_path.clone()],
        Arc::clone(&config),
        TddRedBuildRunner,
    );
    let agent = BuilderAgent::new(llm, cache, triage_agent);

    let result = AgentLoopOrchestrator::run(
        &agent,
        "PHASE_4_BUILDER\nGenerate unit test for src/lib.rs".to_string(),
        3,
    )
    .await
    .expect("builder loop should complete");

    assert!(
        result.is_finished,
        "builder should finish after TDD RED triage success, got: {}",
        result.accumulated_data
    );
    assert!(
        result.accumulated_data.contains("Launching Triage (red)"),
        "expected triage chaining log"
    );
    assert!(
        result
            .accumulated_data
            .contains("TDD RED assertion failure (expected)"),
        "expected triage to observe assertion failure"
    );

    let written = std::fs::read_to_string(&test_path).expect("read written test");
    assert!(
        written.contains("assert_eq!(1, 2)"),
        "RED test must remain intentionally failing on disk"
    );

    std::fs::remove_dir_all(&project_root).ok();
}
