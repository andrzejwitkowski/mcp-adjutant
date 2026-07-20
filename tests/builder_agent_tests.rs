mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{open_cache_manager, unique_temp_project, write_demo_cargo_manifest};
use mcp_adjutant::agent::{
    AgentLoopOrchestrator, BuildCommandRunner, BuilderAgent, ScoutAgent, TriageAgent,
    BUILDER_SYSTEM_PROMPT, SCOUT_SYSTEM_PROMPT, TRIAGE_SYSTEM_PROMPT,
};
use mcp_adjutant::domain::AdjutantConfig;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};
use mcp_adjutant::BuildResult;

struct MockBuilderLlm {
    turn: LlmModelTurn,
}

impl MockBuilderLlm {
    fn write_red_test(path: &str, content: &str) -> Self {
        Self {
            turn: LlmModelTurn {
                content: None,
                tool_calls: vec![LlmToolCall {
                    name: "write_test_suite".to_string(),
                    arguments: serde_json::json!({
                        "path": path,
                        "content": content,
                        "tdd_phase": "red",
                    }),
                }],
                ..Default::default()
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

struct PanicScoutLlm;

impl LlmClient for PanicScoutLlm {
    fn complete(&self, _request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        Err("Scout LLM should not run unless gather_integration_context is invoked".to_string())
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
    fn run_build_command(&self, _dir: &Path, command: &str) -> Result<BuildResult, String> {
        if command.contains("check") {
            Ok(BuildResult {
                exit_code: 0,
                output: "    Finished dev [unoptimized + debuginfo] target(s)".to_string(),
                success: true,
            })
        } else if command.contains("test") {
            Ok(BuildResult {
                exit_code: 101,
                output: "assertion `left == right` failed\n  left: 1\n right: 2\nfailures:\n    red_phase_case\n\ntest result: FAILED. 0 passed; 1 failed".to_string(),
                success: false,
            })
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
    let relative_test_path = "tests/red_phase.rs";

    let failing_test = r#"#[test]
fn red_phase_case() {
    assert_eq!(1, 2);
}
"#;

    let llm = MockBuilderLlm::write_red_test(relative_test_path, failing_test);
    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));

    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![test_path.clone()],
        Arc::clone(&config),
        TddRedBuildRunner,
    );
    let agent = BuilderAgent::new(llm, cache, ScoutAgent::new(PanicScoutLlm), triage_agent);

    let result = AgentLoopOrchestrator::run(
        &agent,
        "PHASE_4_BUILDER\nGenerate unit test for src/lib.rs".to_string(),
        1,
    )
    .await
    .expect("builder loop should complete");

    assert!(
        result.accumulated_data.contains("[RED OK]"),
        "expected RED milestone marker, got: {}",
        result.accumulated_data
    );
    assert!(
        !result.accumulated_data.contains("[BUILDER GREEN OK]"),
        "builder must not reach GREEN in RED-only test"
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

struct CountingRedBuildRunner {
    check_calls: AtomicUsize,
}

impl CountingRedBuildRunner {
    fn new() -> Self {
        Self {
            check_calls: AtomicUsize::new(0),
        }
    }
}

impl BuildCommandRunner for CountingRedBuildRunner {
    fn run_build_command(&self, _dir: &Path, command: &str) -> Result<BuildResult, String> {
        if command.contains("check") {
            let call = self.check_calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Ok(BuildResult {
                    exit_code: 101,
                    output: "error[E0425]: cannot find value `broken` in this scope".to_string(),
                    success: false,
                })
            } else {
                Ok(BuildResult {
                    exit_code: 0,
                    output: "    Finished dev [unoptimized + debuginfo] target(s)".to_string(),
                    success: true,
                })
            }
        } else if command.contains("test") {
            Ok(BuildResult {
                exit_code: 101,
                output:
                    "assertion `left == right` failed\n  left: 1\n right: 2\ntest result: FAILED"
                        .to_string(),
                success: false,
            })
        } else {
            Err(format!("unexpected build command: {command}"))
        }
    }
}

struct MockTriageLlmFixCompile {
    turn: Mutex<LlmModelTurn>,
}

impl MockTriageLlmFixCompile {
    fn new() -> Self {
        Self {
            turn: Mutex::new(LlmModelTurn {
                content: Some("Thought: fix missing import".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "edit_file".to_string(),
                    arguments: serde_json::json!({
                        "path": "tests/red_phase.rs",
                        "line": 1,
                        "content": "use demo::answer;",
                    }),
                }],
                ..Default::default()
            }),
        }
    }
}

impl LlmClient for MockTriageLlmFixCompile {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, TRIAGE_SYSTEM_PROMPT);
        assert!(
            request.user_message.contains("TDD RED"),
            "expected RED directive in triage prompt"
        );
        self.turn
            .lock()
            .map_err(|_| "mock triage llm lock poisoned".to_string())
            .map(|turn| turn.clone())
    }
}

#[tokio::test]
async fn builder_agent_red_phase_runs_full_triage_loop_for_compile_fixes() {
    let project_root = unique_temp_project("builder-red-loop");
    let test_path = setup_cargo_project(&project_root);
    let relative_path = "tests/red_phase.rs";

    let broken_test = "broken syntax\n#[test]\nfn red_phase_case() { assert_eq!(1, 2); }\n";

    let llm = MockBuilderLlm::write_red_test(relative_path, broken_test);
    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));

    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        MockTriageLlmFixCompile::new(),
        vec![test_path.clone()],
        Arc::clone(&config),
        CountingRedBuildRunner::new(),
    );
    let agent = BuilderAgent::new(llm, cache, ScoutAgent::new(PanicScoutLlm), triage_agent);

    let result =
        AgentLoopOrchestrator::run(&agent, "PHASE_4_BUILDER\nGenerate unit test".to_string(), 1)
            .await
            .expect("builder loop should complete");

    assert!(result.accumulated_data.contains("[RED OK]"));
    assert!(!result.accumulated_data.contains("[BUILDER GREEN OK]"));
    assert!(
        result.accumulated_data.contains("Applied edit_file"),
        "expected compile fix via triage loop"
    );
    assert!(
        result
            .accumulated_data
            .contains("TDD RED assertion failure (expected)"),
        "expected assertion verification after compile fix"
    );

    let written = std::fs::read_to_string(&test_path).expect("read updated test");
    assert!(
        written.contains("assert_eq!(1, 2)"),
        "assertions must remain failing after compile-only fix"
    );

    std::fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn builder_agent_writes_relative_test_path_under_project_root() {
    let project_root = unique_temp_project("builder-relative-path");
    setup_cargo_project(&project_root);

    let relative_path = "tests/relative_red.rs";
    let failing_test = r#"#[test]
fn relative_red_case() {
    assert_eq!(1, 2);
}
"#;

    let llm = MockBuilderLlm::write_red_test(relative_path, failing_test);
    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));

    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![project_root.join(relative_path)],
        Arc::clone(&config),
        TddRedBuildRunner,
    );
    let agent = BuilderAgent::new(llm, cache, ScoutAgent::new(PanicScoutLlm), triage_agent);

    let result =
        AgentLoopOrchestrator::run(&agent, "PHASE_4_BUILDER\nGenerate unit test".to_string(), 1)
            .await
            .expect("builder loop should complete");

    let expected_path = project_root.join(relative_path);
    assert!(
        expected_path.is_file(),
        "test should be written under project root"
    );
    assert!(result.accumulated_data.contains("[RED OK]"));
    assert!(!result.accumulated_data.contains("[BUILDER GREEN OK]"));

    std::fs::remove_dir_all(&project_root).ok();
}

impl MockBuilderLlm {
    fn gather_integration(components: &[&str]) -> Self {
        Self {
            turn: LlmModelTurn {
                content: Some("Thought: scout integration context".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "gather_integration_context".to_string(),
                    arguments: serde_json::json!({ "components": components }),
                }],
                ..Default::default()
            },
        }
    }

    fn generate_factory(target_struct: &str, target_file: &str) -> Self {
        Self {
            turn: LlmModelTurn {
                content: Some("Thought: scout test factory".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "generate_test_factory".to_string(),
                    arguments: serde_json::json!({
                        "target_struct": target_struct,
                        "target_file": target_file,
                    }),
                }],
                ..Default::default()
            },
        }
    }
}

struct MockScoutLlmIntegration {
    report: String,
}

impl MockScoutLlmIntegration {
    fn new(report: impl Into<String>) -> Self {
        Self {
            report: report.into(),
        }
    }
}

impl LlmClient for MockScoutLlmIntegration {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, SCOUT_SYSTEM_PROMPT);
        assert!(
            request.user_message.contains("auth::middleware"),
            "scout should receive integration component query"
        );
        Ok(LlmModelTurn {
            content: Some("Scout report ready.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "finalize".to_string(),
                arguments: serde_json::json!({ "report": self.report }),
            }],
            ..Default::default()
        })
    }
}

struct MockScoutLlmFactory {
    report: String,
}

impl MockScoutLlmFactory {
    fn new(report: impl Into<String>) -> Self {
        Self {
            report: report.into(),
        }
    }
}

impl LlmClient for MockScoutLlmFactory {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, SCOUT_SYSTEM_PROMPT);
        assert!(
            request.user_message.contains("User"),
            "scout should receive target struct name"
        );
        assert!(
            request.user_message.contains("src/models/User.java"),
            "scout should receive target file path"
        );
        assert!(
            request.user_message.contains("detect_language"),
            "factory query should be language-agnostic"
        );
        Ok(LlmModelTurn {
            content: Some("Scout factory ready.".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "finalize".to_string(),
                arguments: serde_json::json!({ "report": self.report }),
            }],
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn builder_agent_gather_integration_context_delegates_to_scout() {
    let project_root = unique_temp_project("builder-scout");
    setup_cargo_project(&project_root);

    let llm = MockBuilderLlm::gather_integration(&["auth::middleware", "db::UserRepository"]);
    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));
    let scout_report = "## Scout\n- auth middleware signatures\n- repository call sites";

    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![],
        Arc::clone(&config),
        TddRedBuildRunner,
    );
    let agent = BuilderAgent::new(
        llm,
        cache,
        ScoutAgent::new(MockScoutLlmIntegration::new(scout_report)),
        triage_agent,
    );

    let result = AgentLoopOrchestrator::run(
        &agent,
        "PHASE_4_BUILDER\nPrepare integration test scaffolding".to_string(),
        2,
    )
    .await
    .expect("builder loop should complete");

    assert!(
        result
            .accumulated_data
            .contains("Launching Scout for integration context"),
        "expected scout chaining log"
    );
    assert!(
        result.accumulated_data.contains(scout_report),
        "builder should surface scout finalize report, got: {}",
        result.accumulated_data
    );

    std::fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn builder_agent_generate_test_factory_delegates_to_scout() {
    let project_root = unique_temp_project("builder-factory");
    setup_cargo_project(&project_root);

    let llm = MockBuilderLlm::generate_factory("User", "src/models/User.java");
    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));
    let scout_report = "## Factory\n```java\nclass UserMother { static User valid() { ... } }\n```";

    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![],
        Arc::clone(&config),
        TddRedBuildRunner,
    );
    let agent = BuilderAgent::new(
        llm,
        cache,
        ScoutAgent::new(MockScoutLlmFactory::new(scout_report)),
        triage_agent,
    );

    let result = AgentLoopOrchestrator::run(
        &agent,
        "PHASE_4_BUILDER\nGenerate object mother for User entity".to_string(),
        2,
    )
    .await
    .expect("builder loop should complete");

    assert!(
        result
            .accumulated_data
            .contains("Launching Scout for test factory"),
        "expected scout chaining log"
    );
    assert!(
        result.accumulated_data.contains(scout_report),
        "builder should surface scout factory report, got: {}",
        result.accumulated_data
    );

    std::fs::remove_dir_all(&project_root).ok();
}

struct EmptyTurnBuilderLlm;

impl LlmClient for EmptyTurnBuilderLlm {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, BUILDER_SYSTEM_PROMPT);
        Ok(LlmModelTurn {
            content: None,
            tool_calls: vec![],
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn builder_empty_turns_finish_with_fail_evidence_not_err() {
    let project_root = unique_temp_project("builder-empty");
    setup_cargo_project(&project_root);
    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));
    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![],
        Arc::clone(&config),
        TddRedBuildRunner,
    );
    let agent = BuilderAgent::new(
        EmptyTurnBuilderLlm,
        cache,
        ScoutAgent::new(PanicScoutLlm),
        triage_agent,
    );

    let result = AgentLoopOrchestrator::run(
        &agent,
        "PHASE_4_BUILDER\nGenerate unit test for src/lib.rs".to_string(),
        5,
    )
    .await
    .expect("empty turns must not Err the job");

    assert!(result.is_finished);
    assert!(
        result.accumulated_data.contains("[BUILDER FAIL EVIDENCE]"),
        "expected fail evidence, got: {}",
        result.accumulated_data
    );
    assert!(result.accumulated_data.contains("write_test_suite"));
    assert!(!result.accumulated_data.contains("[BUILDER GREEN OK]"));

    std::fs::remove_dir_all(&project_root).ok();
}
