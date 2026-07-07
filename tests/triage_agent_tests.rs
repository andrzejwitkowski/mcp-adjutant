use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mcp_adjutant::agent::{
    AgentLoopOrchestrator, BuildCommandDiscoverer, BuildCommandRunner, TriageAgent,
    TRIAGE_SYSTEM_PROMPT,
};
use mcp_adjutant::domain::AdjutantConfig;
use mcp_adjutant::find_nearest_module_boundary;
use mcp_adjutant::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolCall};

struct MockBuildRunner {
    calls: AtomicUsize,
    broken_output: String,
    fixed_output: String,
}

impl MockBuildRunner {
    fn new(broken_output: impl Into<String>, fixed_output: impl Into<String>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            broken_output: broken_output.into(),
            fixed_output: fixed_output.into(),
        }
    }
}

impl BuildCommandRunner for MockBuildRunner {
    fn run_build_command(&self, _dir: &Path, _command: &str) -> Result<String, String> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Err(self.broken_output.clone())
        } else {
            Ok(self.fixed_output.clone())
        }
    }
}

struct MockTriageLlm {
    turn: Mutex<LlmModelTurn>,
}

impl MockTriageLlm {
    fn edit_file(path: &Path, line: usize, content: &str) -> Self {
        Self {
            turn: Mutex::new(LlmModelTurn {
                content: Some("Thought: fix compile error".to_string()),
                tool_calls: vec![LlmToolCall {
                    name: "edit_file".to_string(),
                    arguments: serde_json::json!({
                        "path": path.display().to_string(),
                        "line": line,
                        "content": content,
                    }),
                }],
            }),
        }
    }
}

impl LlmClient for MockTriageLlm {
    fn complete(&self, request: LlmRequest<'_>) -> Result<LlmModelTurn, String> {
        assert_eq!(request.system_prompt, TRIAGE_SYSTEM_PROMPT);
        assert!(
            request.user_message.contains("Logi kompilacji"),
            "expected compiler logs in user message"
        );
        assert!(
            !request.tools.is_empty(),
            "triage request should register tool definitions"
        );

        self.turn
            .lock()
            .map_err(|_| "mock llm lock poisoned".to_string())
            .map(|turn| turn.clone())
    }
}

fn temp_root(test_name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("mcp-adjutant-{test_name}-{nanos}"))
}

fn setup_monorepo(root: &Path) -> (PathBuf, PathBuf) {
    let backend = root.join("monorepo/backend");
    let frontend = root.join("monorepo/frontend");

    std::fs::create_dir_all(backend.join("src")).expect("backend dirs");
    std::fs::create_dir_all(frontend.join("src")).expect("frontend dirs");
    std::fs::write(
        backend.join("Cargo.toml"),
        "[package]\nname = \"backend\"\n",
    )
    .expect("cargo");
    std::fs::write(frontend.join("package.json"), "{}").expect("package.json");
    std::fs::write(backend.join("src/lib.rs"), "broken syntax here").expect("lib.rs");
    std::fs::write(frontend.join("src/App.tsx"), "export {}").expect("app");

    (backend, frontend)
}

#[test]
fn find_nearest_module_boundary_detects_rust_and_frontend_modules() {
    let root = temp_root("env-detect");
    let (backend, frontend) = setup_monorepo(&root);
    let config = AdjutantConfig::default();

    let (rust_dir, rust_cmd) =
        find_nearest_module_boundary(&backend.join("src/lib.rs"), &config).expect("rust boundary");
    assert_eq!(rust_dir, backend);
    assert_eq!(rust_cmd, "cargo check --message-format=json");

    let (fe_dir, fe_cmd) =
        find_nearest_module_boundary(&frontend.join("src/App.tsx"), &config).expect("fe boundary");
    assert_eq!(fe_dir, frontend);
    assert_eq!(fe_cmd, "npm run typecheck");

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn triage_agent_fix_loop_edits_file_and_finishes_successfully() {
    let root = temp_root("fix-loop");
    let (backend, _) = setup_monorepo(&root);
    let source = backend.join("src/lib.rs");
    let broken = std::fs::read_to_string(&source).expect("read source");

    let llm = MockTriageLlm::edit_file(&source, 1, "pub fn fixed() {}");
    let runner = MockBuildRunner::new(
        "error[E0425]: cannot find value `syntax` in this scope",
        "    Finished dev",
    );

    let config = Arc::new(AdjutantConfig::default());
    let agent =
        TriageAgent::with_build_runner(llm, vec![source.clone()], Arc::clone(&config), runner);

    let result = AgentLoopOrchestrator::run(&agent, "verify triage".to_string(), 3)
        .await
        .expect("triage loop should complete");

    let updated = std::fs::read_to_string(&source).expect("read updated source");
    assert_ne!(updated, broken, "file should be modified on disk");
    assert!(updated.contains("pub fn fixed()"));
    assert!(
        result.is_finished,
        "orchestrator should finish successfully"
    );
    assert!(
        result
            .input_prompt
            .contains("Wszystkie testy/kompilacje zakończone sukcesem."),
        "success message expected, got: {}",
        result.input_prompt
    );
    assert!(
        result.iterations >= 1,
        "expected at least one iteration, got {}",
        result.iterations
    );

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn triage_agent_verifies_fix_within_single_iteration() {
    let root = temp_root("single-iter");
    let (backend, _) = setup_monorepo(&root);
    let source = backend.join("src/lib.rs");

    let llm = MockTriageLlm::edit_file(&source, 1, "pub fn fixed() {}");
    let runner = MockBuildRunner::new(
        "error[E0425]: cannot find value `syntax` in this scope",
        "    Finished dev",
    );

    let config = Arc::new(AdjutantConfig::default());
    let agent =
        TriageAgent::with_build_runner(llm, vec![source.clone()], Arc::clone(&config), runner);

    let result = AgentLoopOrchestrator::run(&agent, "verify triage".to_string(), 1)
        .await
        .expect("triage loop should complete");

    assert!(
        result.is_finished,
        "fix should be verified on the same iteration"
    );
    assert!(result
        .input_prompt
        .contains("Wszystkie testy/kompilacje zakończone sukcesem."));

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn triage_agent_rejects_edit_outside_module_roots() {
    let root = temp_root("reject-edit");
    let (backend, _) = setup_monorepo(&root);
    let source = backend.join("src/lib.rs");

    let llm = MockTriageLlm::edit_file(Path::new("/etc/passwd"), 1, "pwned");
    let runner = MockBuildRunner::new("error[E0425]: broken", "ok");

    let config = Arc::new(AdjutantConfig::default());
    let agent =
        TriageAgent::with_build_runner(llm, vec![source.clone()], Arc::clone(&config), runner);

    let result = AgentLoopOrchestrator::run(&agent, "verify triage".to_string(), 1).await;

    let err = result.expect_err("edit outside module roots should be rejected");
    assert!(
        err.contains("edit path must be inside a triage module root"),
        "unexpected error: {err}"
    );

    let contents = std::fs::read_to_string(&source).expect("read source");
    assert!(
        contents.contains("broken syntax"),
        "file must remain unchanged"
    );

    std::fs::remove_dir_all(&root).ok();
}

struct MockDiscoverer {
    command: String,
}

impl BuildCommandDiscoverer for MockDiscoverer {
    fn discover(&self, _anchor: &Path, snapshot: &str) -> Result<Option<String>, String> {
        assert!(
            snapshot.contains("kernel.cu"),
            "discovery should receive directory snapshot, got:\n{snapshot}"
        );
        Ok(Some(self.command.clone()))
    }
}

#[tokio::test]
async fn triage_agent_discovers_build_command_for_unknown_stack() {
    let root = temp_root("discovery");
    let cuda = root.join("kernels");
    std::fs::create_dir_all(&cuda).expect("dirs");
    std::fs::write(cuda.join("kernel.cu"), "__global__ void k() {}").expect("cu");

    let source = cuda.join("kernel.cu");

    let llm = MockTriageLlm::edit_file(&source, 1, "__global__ void k() {}");
    let runner = MockBuildRunner::new("error: expected ';'", "nvcc ok");
    let discoverer = MockDiscoverer {
        command: "nvcc -std=c++17 -c kernel.cu".to_string(),
    };

    let config = Arc::new(AdjutantConfig::default());
    let agent = TriageAgent::with_build_runner_and_discoverer(
        llm,
        vec![source.clone()],
        Arc::clone(&config),
        runner,
        discoverer,
    );

    let result = AgentLoopOrchestrator::run(&agent, "verify triage".to_string(), 2)
        .await
        .expect("triage with discovery should complete");

    assert!(
        result.is_finished,
        "discovery + fix loop should finish successfully"
    );
    assert!(
        result
            .accumulated_data
            .contains("nvcc -std=c++17 -c kernel.cu"),
        "expected discovered build command in report"
    );

    std::fs::remove_dir_all(&root).ok();
}
