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
    let agent = BuilderAgent::new(llm, cache, ScoutAgent::new(PanicScoutLlm), triage_agent);

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
    fn run_build_command(&self, _dir: &Path, command: &str) -> Result<String, String> {
        if command.contains("check") {
            let call = self.check_calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Err("error[E0425]: cannot find value `broken` in this scope".to_string())
            } else {
                Ok("    Finished dev [unoptimized + debuginfo] target(s)".to_string())
            }
        } else if command.contains("test") {
            Err(
                "assertion `left == right` failed\n  left: 1\n right: 2\ntest result: FAILED"
                    .to_string(),
            )
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
        AgentLoopOrchestrator::run(&agent, "PHASE_4_BUILDER\nGenerate unit test".to_string(), 3)
            .await
            .expect("builder loop should complete");

    assert!(
        result.is_finished,
        "multi-iteration RED triage should finish successfully: {}",
        result.accumulated_data
    );
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
        AgentLoopOrchestrator::run(&agent, "PHASE_4_BUILDER\nGenerate unit test".to_string(), 3)
            .await
            .expect("builder loop should complete");

    let expected_path = project_root.join(relative_path);
    assert!(
        expected_path.is_file(),
        "test should be written under project root"
    );
    assert!(
        result.is_finished,
        "expected triage success for relative path"
    );

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

impl MockBuilderLlm {
    fn unsupported_tool(name: &str) -> Self {
        Self {
            turn: LlmModelTurn {
                content: Some("Thought: try an unknown tool".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: name.to_string(),
                    arguments: serde_json::json!({}),
                }],
            },
        }
    }

    fn no_tool_call_and_no_thought() -> Self {
        Self {
            turn: LlmModelTurn {
                content: None,
                tool_calls: vec![],
            },
        }
    }

    fn thought_only(thought: &str) -> Self {
        Self {
            turn: LlmModelTurn {
                content: Some(thought.to_string()),
                tool_calls: vec![],
            },
        }
    }
}

#[tokio::test]
async fn builder_agent_errors_on_unsupported_tool_call() {
    let project_root = unique_temp_project("builder-unsupported-tool");
    setup_cargo_project(&project_root);

    let llm = MockBuilderLlm::unsupported_tool("delete_repository");
    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));

    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![],
        Arc::clone(&config),
        TddRedBuildRunner,
    );
    let agent = BuilderAgent::new(llm, cache, ScoutAgent::new(PanicScoutLlm), triage_agent);

    let result = AgentLoopOrchestrator::run(&agent, "PHASE_4_BUILDER\nDo something".to_string(), 1)
        .await;

    let err = result.expect_err("unsupported tool call should error");
    assert_eq!(err, "unsupported builder tool: delete_repository");

    std::fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn builder_agent_errors_when_model_returns_neither_tool_call_nor_thought() {
    let project_root = unique_temp_project("builder-empty-response");
    setup_cargo_project(&project_root);

    let llm = MockBuilderLlm::no_tool_call_and_no_thought();
    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));

    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![],
        Arc::clone(&config),
        TddRedBuildRunner,
    );
    let agent = BuilderAgent::new(llm, cache, ScoutAgent::new(PanicScoutLlm), triage_agent);

    let result = AgentLoopOrchestrator::run(&agent, "PHASE_4_BUILDER\nDo something".to_string(), 1)
        .await;

    let err = result.expect_err("missing tool call and empty thought should error");
    assert_eq!(err, "model response missing tool call");

    std::fs::remove_dir_all(&project_root).ok();
}

#[tokio::test]
async fn builder_agent_continues_without_finishing_when_model_only_emits_thought() {
    let project_root = unique_temp_project("builder-thought-only");
    setup_cargo_project(&project_root);

    let llm = MockBuilderLlm::thought_only("Still deciding which tool to call.");
    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));

    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        PanicTriageLlm,
        vec![],
        Arc::clone(&config),
        TddRedBuildRunner,
    );
    let agent = BuilderAgent::new(llm, cache, ScoutAgent::new(PanicScoutLlm), triage_agent);

    let result = AgentLoopOrchestrator::run(&agent, "PHASE_4_BUILDER\nDo something".to_string(), 1)
        .await
        .expect("builder loop should complete without error");

    assert!(
        !result.is_finished,
        "thought-only response must not finish the builder loop"
    );
    assert!(
        result
            .accumulated_data
            .contains("model nie wywołał narzędzia — kontynuuj"),
        "expected continuation marker, got: {}",
        result.accumulated_data
    );
    assert!(result.accumulated_data.contains("Still deciding"));

    std::fs::remove_dir_all(&project_root).ok();
}

struct AlwaysFailBuildRunner;

impl BuildCommandRunner for AlwaysFailBuildRunner {
    fn run_build_command(&self, _dir: &Path, _command: &str) -> Result<String, String> {
        Err("error[E0425]: cannot find value `still_broken` in this scope".to_string())
    }
}

struct MockTriageLlmNoOpEditLoop;

impl LlmClient for MockTriageLlmNoOpEditLoop {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, TRIAGE_SYSTEM_PROMPT);
        Ok(LlmModelTurn {
            content: Some("Thought: attempt a no-op edit".to_string()),
            tool_calls: vec![LlmToolCall {
                name: "edit_file".to_string(),
                arguments: serde_json::json!({
                    "path": "tests/green_phase.rs",
                    "line": 1,
                    "content": "// still broken",
                }),
            }],
        })
    }
}

#[tokio::test]
async fn builder_agent_green_phase_reports_triage_failure_without_finishing() {
    let project_root = unique_temp_project("builder-green-failure");
    setup_cargo_project(&project_root);

    let relative_path = "tests/green_phase.rs";
    let test_content = "#[test]\nfn green_phase_case() { assert_eq!(1, 1); }\n";

    let llm = LlmModelTurn {
        content: Some("Thought: emit GREEN test".to_string()),
        tool_calls: vec![LlmToolCall {
            name: "write_test_suite".to_string(),
            arguments: serde_json::json!({
                "path": relative_path,
                "content": test_content,
                "tdd_phase": "green",
            }),
        }],
    };
    let builder_llm = MockBuilderLlm { turn: llm };
    let cache = Arc::new(Mutex::new(open_cache_manager(&project_root)));

    let config = Arc::new(AdjutantConfig::default());
    let triage_agent = TriageAgent::with_build_runner(
        MockTriageLlmNoOpEditLoop,
        vec![project_root.join(relative_path)],
        Arc::clone(&config),
        AlwaysFailBuildRunner,
    );
    let agent = BuilderAgent::new(
        builder_llm,
        cache,
        ScoutAgent::new(PanicScoutLlm),
        triage_agent,
    );

    let result = AgentLoopOrchestrator::run(
        &agent,
        "PHASE_4_BUILDER\nGenerate unit test".to_string(),
        1,
    )
    .await
    .expect("builder loop should complete without error");

    assert!(
        !result.is_finished,
        "builder must not finish when triage cannot fix the build"
    );
    assert!(
        result.accumulated_data.contains("[TRIAGE FAILURE]"),
        "expected triage failure marker, got: {}",
        result.accumulated_data
    );

    std::fs::remove_dir_all(&project_root).ok();
}
