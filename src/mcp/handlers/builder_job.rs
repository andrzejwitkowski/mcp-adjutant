//! Shared Builder GREEN job body used by generate_tests and execute_blueprint.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use crate::agent::{
    builder_task_parts, default_builder_agent, format_builder_report, AgentLoopOrchestrator,
    BuilderReportInput, BUILDER_GREEN_MARKER,
};
use crate::cache::mcp_workspace_root;
use crate::domain::AdjutantConfig;
use crate::llm::{create_builder_llm_client, create_scout_llm_client, create_triage_llm_client};
use crate::mcp::schemas::GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME;
use crate::metrics::{load_best_builder_dense_exemplar, metrics_store};

use super::{open_cache_manager_near, BUILDER_MAX_ITERATIONS};

pub struct BuilderGreenResult {
    pub green_ok: bool,
    pub output: String,
    pub original_task: String,
}

pub async fn run_builder_green(
    config: &AdjutantConfig,
    source_path: &Path,
    source_file_path: &str,
    test_type: &str,
) -> Result<BuilderGreenResult, String> {
    let project_root = mcp_workspace_root();
    let cache_manager = Arc::new(Mutex::new(open_cache_manager_near(source_path)?));
    let agent = default_builder_agent(
        create_builder_llm_client(config)?,
        cache_manager,
        create_scout_llm_client(config)?,
        create_triage_llm_client(config)?,
        Arc::new(config.clone()),
        vec![source_path.to_path_buf()],
    );

    let source_excerpt = std::fs::read_to_string(source_path)
        .map(|contents| {
            const MAX: usize = 8_000;
            if contents.len() > MAX {
                format!("{}...\n(truncated)", &contents[..MAX])
            } else {
                contents
            }
        })
        .unwrap_or_else(|err| format!("(could not read source file: {err})"));

    let parts = builder_task_parts(source_path, test_type, source_file_path, &project_root);
    let mut prompt = format!(
        "{GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME}\nPHASE_4_BUILDER\n\n{}",
        parts.workflow
    );
    let exemplar = metrics_store().and_then(|store| {
        let guard = store.lock().ok()?;
        load_best_builder_dense_exemplar(guard.connection())
            .ok()
            .flatten()
    });
    if let Some(exemplar) = exemplar {
        prompt.push_str(
            "\n\n## 10/10 dense report exemplar (path + diffstat + scenarios + pass/fail — no source bodies)\n",
        );
        prompt.push_str(&exemplar);
    }
    if !parts.exemplar.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&parts.exemplar);
    }
    prompt.push_str(&format!("\n\nSource excerpt:\n```\n{source_excerpt}\n```"));

    let original_task = prompt.clone();
    let result = AgentLoopOrchestrator::run(&agent, prompt, BUILDER_MAX_ITERATIONS).await?;
    let green_marker = result.accumulated_data.contains(BUILDER_GREEN_MARKER);
    let hard_stopped = result.iterations >= BUILDER_MAX_ITERATIONS
        && result.accumulated_data.contains("iteration limit after");

    let (green_ok, verify_summary) = if result.is_finished && green_marker && !hard_stopped {
        match extract_green_test_path(&result.accumulated_data) {
            None => (
                false,
                "builder GREEN but no test path found in log".to_string(),
            ),
            Some(test_path) => match verify_test_passes(&test_path, &project_root) {
                Ok(summary) => (true, summary),
                Err(err) => (false, format!("post-GREEN verify failed: {err}")),
            },
        }
    } else {
        (false, String::new())
    };

    let output = format_builder_report(&BuilderReportInput {
        accumulated_data: &result.accumulated_data,
        project_root: &project_root,
        source_file_path,
        test_type,
        green_ok,
        verify_summary: (!verify_summary.is_empty()).then_some(verify_summary.as_str()),
        config,
    });
    Ok(BuilderGreenResult {
        green_ok,
        output,
        original_task,
    })
}

fn verify_test_passes(test_path: &Path, project_root: &Path) -> Result<String, String> {
    match test_path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => verify_cargo_test_passes(test_path),
        Some("ts" | "tsx") => verify_npm_test_passes(test_path, project_root),
        _ => Ok(String::new()),
    }
}

fn extract_green_test_path(log: &str) -> Option<PathBuf> {
    log.lines()
        .filter(|line| line.contains("[SYSTEM]: Launching Triage (green)"))
        .filter_map(|line| line.split(" for ").nth(1))
        .map(str::trim)
        .map(PathBuf::from)
        .next_back()
}

fn verify_cargo_test_passes(test_path: &Path) -> Result<String, String> {
    let project_root = mcp_workspace_root();
    let stem = test_path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid test path: {}", test_path.display()))?;

    let in_tests_dir = test_path
        .components()
        .any(|component| component.as_os_str() == "tests");

    let mut command = Command::new("cargo");
    let label = if in_tests_dir {
        command.args(["test", "--test", stem]);
        format!("cargo test --test {stem}")
    } else {
        command.args(["test", "--lib"]);
        "cargo test --lib".to_string()
    };

    let output = command
        .current_dir(&project_root)
        .output()
        .map_err(|err| format!("failed to run {label}: {err}"))?;

    if output.status.success() {
        return Ok(format!("{label}: all tests passed"));
    }

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Err(format!("{label} failed:\n{combined}"))
}

fn verify_npm_test_passes(test_path: &Path, project_root: &Path) -> Result<String, String> {
    let frontend = project_root.join("frontend");
    let rel = test_path
        .strip_prefix(&frontend)
        .or_else(|_| test_path.strip_prefix(project_root))
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| test_path.to_string_lossy().into_owned());

    let output = Command::new("npm")
        .args(["test", "--", &rel])
        .current_dir(&frontend)
        .output()
        .map_err(|err| format!("failed to run npm test in frontend: {err}"))?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(format!("npm test -- {rel}: passed\n{combined}"))
    } else {
        Err(format!("npm test -- {rel} failed:\n{combined}"))
    }
}
