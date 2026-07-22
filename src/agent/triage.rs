mod tools;

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::traits::{AgentContext, AutonomousAgent};
use crate::domain::AdjutantConfig;
use crate::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolSet};
use crate::tools::{
    edit_file_line, find_nearest_module_boundary, get_dirty_files_from_git, inference_anchor,
    run_build_command, snapshot_build_context, truncate_build_log, BuildCommandDiscoverer,
    BuildResult, NoopBuildDiscoverer,
};

pub use tools::{parse_edit_file_arguments, parse_report_error_arguments, triage_tool_set};

pub const TRIAGE_SYSTEM_PROMPT: &str = r#"You are a compiler repair agent (PHASE_5_TRIAGE). You will receive error logs.
Decide whether you can fix the code (e.g. missing import, typo).

Available tools (tool calls):
- edit_file — replace one file line (path, line, content)
- report_architectural_error — escalate when a fix needs an architect (msg)

Evidence requirements (mandatory in your final report):
- Workspace root path and the exact target files/modules triaged
- Each build/test command run with exit code and a log excerpt (last ~40 lines)
- Never claim PASS or FAIL without command output — "trust me" assertions score as failure
- Verify triage targets match the coordinator request before reporting success

Reply with a short rationale (Thought), then call exactly one tool."#;

pub trait BuildCommandRunner: Send + Sync {
    fn run_build_command(&self, dir: &Path, command: &str) -> Result<BuildResult, String>;
}

pub struct SystemBuildRunner;

impl BuildCommandRunner for SystemBuildRunner {
    fn run_build_command(&self, dir: &Path, command: &str) -> Result<BuildResult, String> {
        run_build_command(dir, command)
    }
}

#[derive(Default)]
struct TriageWorkspace {
    build_targets: Vec<(PathBuf, String)>,
    resolved_paths: Vec<PathBuf>,
    retarget_paths: Vec<PathBuf>,
    /// When non-empty (builder `retarget`), `edit_file` may only touch these paths.
    editable_paths: Vec<PathBuf>,
}

pub struct TriageAgent<C, B = SystemBuildRunner, D = NoopBuildDiscoverer> {
    llm_client: C,
    target_paths: Vec<PathBuf>,
    config: Arc<AdjutantConfig>,
    build_runner: B,
    discoverer: D,
    tools: LlmToolSet,
    workspace: Mutex<TriageWorkspace>,
}

impl<C: LlmClient> TriageAgent<C, SystemBuildRunner, NoopBuildDiscoverer> {
    pub fn new(llm_client: C, target_paths: Vec<PathBuf>, config: Arc<AdjutantConfig>) -> Self {
        Self::with_build_runner(llm_client, target_paths, config, SystemBuildRunner)
    }
}

impl<C: LlmClient, B: BuildCommandRunner> TriageAgent<C, B, NoopBuildDiscoverer> {
    pub fn with_build_runner(
        llm_client: C,
        target_paths: Vec<PathBuf>,
        config: Arc<AdjutantConfig>,
        build_runner: B,
    ) -> Self {
        Self::with_build_runner_and_discoverer(
            llm_client,
            target_paths,
            config,
            build_runner,
            NoopBuildDiscoverer,
        )
    }
}

impl<C: LlmClient, B: BuildCommandRunner, D: BuildCommandDiscoverer> TriageAgent<C, B, D> {
    pub fn with_build_runner_and_discoverer(
        llm_client: C,
        target_paths: Vec<PathBuf>,
        config: Arc<AdjutantConfig>,
        build_runner: B,
        discoverer: D,
    ) -> Self {
        Self {
            llm_client,
            target_paths,
            config,
            build_runner,
            discoverer,
            tools: triage_tool_set(),
            workspace: Mutex::new(TriageWorkspace::default()),
        }
    }

    pub fn retarget(&self, paths: Vec<PathBuf>) -> Result<(), String> {
        let mut workspace = self
            .workspace
            .lock()
            .map_err(|_| "triage workspace lock poisoned".to_string())?;
        workspace.retarget_paths = paths.clone();
        workspace.editable_paths = paths;
        workspace.build_targets.clear();
        Ok(())
    }

    fn editable_paths(&self) -> Result<Vec<PathBuf>, String> {
        let workspace = self
            .workspace
            .lock()
            .map_err(|_| "triage workspace lock poisoned".to_string())?;
        Ok(workspace.editable_paths.clone())
    }

    fn resolve_target_paths(&self) -> Result<Vec<PathBuf>, String> {
        let workspace = self
            .workspace
            .lock()
            .map_err(|_| "triage workspace lock poisoned".to_string())?;
        if !workspace.retarget_paths.is_empty() {
            return Ok(workspace.retarget_paths.clone());
        }
        drop(workspace);

        if self.target_paths.is_empty() {
            Ok(get_dirty_files_from_git()?
                .into_iter()
                .map(|path| crate::cache::resolve_workspace_path(&path))
                .collect())
        } else {
            Ok(self
                .target_paths
                .iter()
                .map(crate::cache::resolve_workspace_path)
                .collect())
        }
    }

    fn red_test_paths(&self) -> Result<Vec<PathBuf>, String> {
        let workspace = self
            .workspace
            .lock()
            .map_err(|_| "triage workspace lock poisoned".to_string())?;
        if !workspace.retarget_paths.is_empty() {
            Ok(workspace.retarget_paths.clone())
        } else {
            Ok(workspace.resolved_paths.clone())
        }
    }

    fn resolve_build_targets(&self, paths: &[PathBuf]) -> Result<Vec<(PathBuf, String)>, String> {
        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        let mut needs_discovery = HashSet::new();

        for path in paths {
            if let Some((dir, command)) = find_nearest_module_boundary(path, &self.config) {
                let key = (dir.clone(), command.clone());
                if seen.insert(key.clone()) {
                    targets.push(key);
                }
            } else {
                needs_discovery.insert(inference_anchor(path));
            }
        }

        for anchor in needs_discovery {
            let anchor = if anchor.is_dir() {
                anchor
            } else {
                anchor.parent().map(Path::to_path_buf).unwrap_or(anchor)
            };
            if targets.iter().any(|(dir, _)| dir == &anchor) {
                continue;
            }
            let snapshot = snapshot_build_context(&anchor, 3)?;
            if let Some(command) = self.discoverer.discover(&anchor, &snapshot)? {
                let key = (anchor.clone(), command);
                if seen.insert(key.clone()) {
                    targets.push(key);
                }
            }
        }

        Ok(targets)
    }

    const BUILD_LOG_MAX_LINES: usize = 120;
    const BUILD_LOG_MAX_BYTES: usize = 24_000;

    fn module_roots(targets: &[(PathBuf, String)]) -> Vec<PathBuf> {
        targets.iter().map(|(dir, _)| dir.clone()).collect()
    }

    fn run_build_targets(
        &self,
        targets: &[(PathBuf, String)],
        context: &mut AgentContext,
    ) -> Result<bool, String> {
        let mut combined_errors = Vec::new();
        let mut all_ok = true;

        for (dir, command) in targets {
            match self.build_runner.run_build_command(dir, command) {
                Ok(result) => {
                    let (body, truncated) = truncate_build_log(
                        &result.output,
                        Self::BUILD_LOG_MAX_LINES,
                        Self::BUILD_LOG_MAX_BYTES,
                    );
                    let log = if truncated {
                        format!("(log truncated)\n{body}")
                    } else {
                        body
                    };
                    if result.success {
                        let log_body = if log.trim().is_empty() {
                            "(no further stdout/stderr)"
                        } else {
                            log.trim_end()
                        };
                        let step = format!(
                            "Workspace: {}\nCommand: `{command}`\nExit code: {}\nBuild output:\n{log_body}\n",
                            dir.display(),
                            result.exit_code,
                        );
                        context.accumulated_data.push_str(&step);
                    } else {
                        all_ok = false;
                        combined_errors.push(format!(
                            "Workspace: {}\nCommand: `{command}`\nExit code: {}\nBuild FAILED:\n{log}\n",
                            dir.display(),
                            result.exit_code,
                        ));
                    }
                }
                Err(spawn_err) => {
                    all_ok = false;
                    combined_errors.push(format!(
                        "Workspace: {}\nCommand: `{command}`\nSpawn error: {spawn_err}\n",
                        dir.display(),
                    ));
                }
            }
        }

        if all_ok {
            context.is_finished = true;
            context.input_prompt = crate::agent::TRIAGE_PASS_MARKER.to_string();
        } else if !combined_errors.is_empty() {
            context
                .accumulated_data
                .push_str(&combined_errors.join("\n---\n"));
        }

        Ok(all_ok)
    }

    fn try_finish_tdd_red(
        &self,
        targets: &[(PathBuf, String)],
        context: &mut AgentContext,
    ) -> Result<bool, String> {
        let mut check_failures = Vec::new();

        for (dir, command) in targets {
            let (check_cmd, _) = split_check_and_test_commands(command);
            match self.build_runner.run_build_command(dir, &check_cmd) {
                Ok(result) if result.success => {
                    context.accumulated_data.push_str(&format!(
                        "TDD RED check OK in {} (`{check_cmd}`):\nExit code: {}\n{}\n",
                        dir.display(),
                        result.exit_code,
                        result.output,
                    ));
                }
                Ok(result) => {
                    let (body, truncated) = truncate_build_log(
                        &result.output,
                        Self::BUILD_LOG_MAX_LINES,
                        Self::BUILD_LOG_MAX_BYTES,
                    );
                    let log = if truncated {
                        format!("(log truncated — showing tail)\n{body}")
                    } else {
                        body
                    };
                    check_failures.push(format!(
                        "TDD RED check FAILED in {} (`{check_cmd}`):\nExit code: {}\n{log}\n",
                        dir.display(),
                        result.exit_code,
                    ));
                }
                Err(spawn_err) => {
                    check_failures.push(format!(
                        "TDD RED check spawn error in {} (`{check_cmd}`): {spawn_err}\n",
                        dir.display(),
                    ));
                }
            }
        }

        if !check_failures.is_empty() {
            context
                .accumulated_data
                .push_str(&check_failures.join("\n---\n"));
            return Ok(false);
        }

        let test_paths = self.red_test_paths()?;
        let mut assertion_failures = 0usize;
        let mut unexpected = Vec::new();

        for (dir, command) in targets {
            let (_, base_test_cmd) = split_check_and_test_commands(command);
            let test_cmd = scope_test_command_for_paths(&base_test_cmd, &test_paths);
            let command_scoped = test_cmd != base_test_cmd;
            match self.build_runner.run_build_command(dir, &test_cmd) {
                Ok(result) if result.success => {
                    unexpected.push(format!(
                        "TDD RED unexpected pass in {} (`{test_cmd}`):\nExit code: {}\n{}\n",
                        dir.display(),
                        result.exit_code,
                        result.output,
                    ));
                }
                Ok(result) => {
                    if is_assertion_test_failure(&result.output, &test_paths, command_scoped) {
                        assertion_failures += 1;
                        context.accumulated_data.push_str(&format!(
                            "TDD RED assertion failure (expected) in {} (`{test_cmd}`):\nExit code: {}\n{}\n",
                            dir.display(),
                            result.exit_code,
                            result.output,
                        ));
                    } else {
                        let (body, truncated) = truncate_build_log(
                            &result.output,
                            Self::BUILD_LOG_MAX_LINES,
                            Self::BUILD_LOG_MAX_BYTES,
                        );
                        let log = if truncated {
                            format!("(log truncated — showing tail)\n{body}")
                        } else {
                            body
                        };
                        unexpected.push(format!(
                            "TDD RED non-assertion failure in {} (`{test_cmd}`):\nExit code: {}\n{log}\n",
                            dir.display(),
                            result.exit_code,
                        ));
                    }
                }
                Err(spawn_err) => {
                    unexpected.push(format!(
                        "TDD RED spawn error in {} (`{test_cmd}`): {spawn_err}\n",
                        dir.display(),
                    ));
                }
            }
        }

        if !unexpected.is_empty() {
            context
                .accumulated_data
                .push_str(&unexpected.join("\n---\n"));
            return Ok(false);
        }

        if assertion_failures > 0 {
            context.is_finished = true;
            context.input_prompt = "compile succeeded, tests failing assertions".to_string();
            return Ok(true);
        }

        Ok(false)
    }
}

#[async_trait]
impl<C: LlmClient, B: BuildCommandRunner, D: BuildCommandDiscoverer> AutonomousAgent
    for TriageAgent<C, B, D>
{
    fn name(&self) -> &'static str {
        "triage_agent"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        if !context.input_prompt.contains("Workspace root:") {
            let root = crate::cache::mcp_workspace_root();
            context
                .input_prompt
                .push_str(&format!("\n\nWorkspace root: {}\n", root.display()));
        }
        if !context.input_prompt.contains("PHASE_5_TRIAGE") {
            context.input_prompt.push_str("\n\n");
            context.input_prompt.push_str(TRIAGE_SYSTEM_PROMPT);
        }

        let paths = self.resolve_target_paths()?;
        let targets = self.resolve_build_targets(&paths)?;

        let mut workspace = self
            .workspace
            .lock()
            .map_err(|_| "triage workspace lock poisoned".to_string())?;
        workspace.resolved_paths = paths.clone();
        workspace.build_targets = targets.clone();

        let summary: Vec<String> = targets
            .iter()
            .map(|(dir, cmd)| format!("{} => {cmd}", dir.display()))
            .collect();
        context.accumulated_data = format!(
            "Triage targets ({} modules):\n{}\n",
            summary.len(),
            summary.join("\n")
        );

        let root = crate::cache::mcp_workspace_root();
        let file_list: Vec<String> = paths
            .iter()
            .map(|p| p.strip_prefix(&root).unwrap_or(p).display().to_string())
            .collect();
        if !file_list.is_empty() {
            context.accumulated_data.push_str("\nTarget files:\n");
            for f in &file_list {
                context.accumulated_data.push_str(&format!("- {f}\n"));
            }
        }

        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        let (targets, paths) = {
            let workspace = self
                .workspace
                .lock()
                .map_err(|_| "triage workspace lock poisoned".to_string())?;
            (
                workspace.build_targets.clone(),
                workspace.resolved_paths.clone(),
            )
        };

        if targets.is_empty() {
            context.is_finished = true;
            let root = crate::cache::mcp_workspace_root();
            let join = |paths: &[PathBuf]| {
                if paths.is_empty() {
                    "(none)".to_string()
                } else {
                    paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            };
            let requested = join(&self.target_paths);
            let resolved = join(&paths);
            context.input_prompt = if paths.is_empty() {
                format!(
                    "No modules to check (no git changes or unknown paths).\n\
                     Workspace root: {}\nRequested paths: {requested}\nResolved paths: (none)\n\
                     Discovery: no build targets.",
                    root.display()
                )
            } else {
                format!(
                    "Could not resolve a build command (no manifest and discovery returned no command).\n\
                     Workspace root: {}\nRequested paths: {requested}\nResolved paths: {resolved}\n\
                     Discovery: attempted build-command discovery — no command.",
                    root.display()
                )
            };
            context.accumulated_data.push_str(&format!(
                "\n[TRIAGE EARLY EXIT]\n{}\n",
                context.input_prompt
            ));
            return Ok(());
        }

        if context.input_prompt.contains("TDD RED PHASE") {
            if self.try_finish_tdd_red(&targets, context)? {
                return Ok(());
            }
        } else if self.run_build_targets(&targets, context)? {
            return Ok(());
        }

        let user_message = if context.input_prompt.contains("TDD RED PHASE") {
            format!(
                "Build logs to fix (TDD RED — fix ONLY compile errors, do NOT change assertions):\n\n{}\n\nCall one tool.",
                context.accumulated_data
            )
        } else {
            format!(
                "Build logs to fix:\n\n{}\n\nCall one tool.",
                context.accumulated_data
            )
        };
        let request = LlmRequest::new(TRIAGE_SYSTEM_PROMPT, &user_message, &self.tools);
        let model_turn: LlmModelTurn = self.llm_client.complete(request)?;

        let tool_call = match model_turn.tool_calls.first() {
            Some(call) => call,
            None => {
                let thought = model_turn.content.unwrap_or_default();
                context.accumulated_data.push_str(&format!(
                    "LLM triage response (iter {}):\n{thought}\n(model did not call a tool — retrying)\n",
                    context.iterations
                ));
                return Ok(());
            }
        };

        let thought = model_turn.content.unwrap_or_default();
        context.accumulated_data.push_str(&format!(
            "LLM triage response (iter {}):\nThought: {thought}\nTool: {}({})\n",
            context.iterations, tool_call.name, tool_call.arguments
        ));

        match tool_call.name.as_str() {
            "edit_file" => {
                let (path, line, content) = parse_edit_file_arguments(&tool_call.arguments)?;
                let trimmed = content.trim_start();
                let edits_assertion_statement =
                    trimmed.starts_with("assert") || trimmed.starts_with("panic!");
                if context.input_prompt.contains("TDD RED PHASE") && edits_assertion_statement {
                    return Err(
                        "TDD RED PHASE: modifying assertions or panic! is forbidden".to_string()
                    );
                }
                let module_roots = Self::module_roots(&targets);
                let resolved = resolve_edit_path(&path, &module_roots)?;
                let editable_paths = self.editable_paths()?;
                if let Err(msg) = assert_edit_allowed(&resolved, &editable_paths) {
                    context
                        .accumulated_data
                        .push_str(&format!("Edit rejected: {msg}\n"));
                    return Ok(());
                }
                edit_file_line(&resolved, line, &content)?;
                context.accumulated_data.push_str(&format!(
                    "Applied edit_file({}, line={line})\n",
                    resolved.display()
                ));
                if context.input_prompt.contains("TDD RED PHASE") {
                    self.try_finish_tdd_red(&targets, context)?;
                } else {
                    self.run_build_targets(&targets, context)?;
                }
            }
            "report_architectural_error" => {
                let msg = parse_report_error_arguments(&tool_call.arguments)?;
                context.accumulated_data = msg;
                context.is_finished = true;
            }
            other => {
                return Err(format!("unsupported triage tool: {other}"));
            }
        }

        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        context
            .input_prompt
            .push_str("\nRe-run the build after the latest fix attempt.");
        Ok(())
    }
}

fn split_check_and_test_commands(command: &str) -> (String, String) {
    if command.starts_with("cargo check") {
        ("cargo check".to_string(), "cargo test".to_string())
    } else if command.contains("typecheck") {
        (command.to_string(), command.replace("typecheck", "test"))
    } else {
        (command.to_string(), command.to_string())
    }
}

fn scope_test_command_for_paths(test_cmd: &str, test_paths: &[PathBuf]) -> String {
    if !test_cmd.starts_with("cargo test") || test_paths.len() != 1 {
        return test_cmd.to_string();
    }

    let path = &test_paths[0];
    let in_tests_dir = path
        .components()
        .any(|component| component.as_os_str() == "tests");
    let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
        return test_cmd.to_string();
    };

    if in_tests_dir && path.extension().is_some_and(|ext| ext == "rs") {
        insert_cargo_test_filter(test_cmd, stem)
    } else {
        test_cmd.to_string()
    }
}

fn insert_cargo_test_filter(test_cmd: &str, stem: &str) -> String {
    let (head, tail) = match test_cmd.split_once(" -- ") {
        Some(parts) => parts,
        None => (test_cmd, ""),
    };

    if head.contains("--test ") {
        return test_cmd.to_string();
    }

    let scoped_head = format!("{head} --test {stem}");
    if tail.is_empty() {
        scoped_head
    } else {
        format!("{scoped_head} -- {tail}")
    }
}

fn is_setup_test_failure(output: &str) -> bool {
    let lower = output.to_lowercase();
    lower.contains("no such file or directory")
        || lower.contains("failed to canonicalize")
        || lower.contains("e0433:")
        || lower.contains("e0061:")
        || lower.contains("e0599:")
        || lower.contains("unresolved import")
        || lower.contains("tempfile::")
}

fn is_assertion_test_failure(output: &str, test_paths: &[PathBuf], command_scoped: bool) -> bool {
    if is_setup_test_failure(output) {
        return false;
    }

    let lower = output.to_lowercase();
    let has_assertion_signal = lower.contains("assertion `")
        || lower.contains("assert_eq!")
        || lower.contains("assertion failed")
        || (lower.contains("panicked at") && lower.contains("assert"));

    if !has_assertion_signal {
        return false;
    }

    if command_scoped || test_paths.is_empty() {
        return true;
    }

    test_paths.iter().any(|path| {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| lower.contains(&stem.to_lowercase()))
    })
}

fn is_protected_test_infra(path: &Path) -> bool {
    path.ends_with("tests/common/mod.rs") || path.ends_with("tests/cache_manager_tests.rs")
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn assert_edit_allowed(resolved: &Path, editable_paths: &[PathBuf]) -> Result<(), String> {
    if is_protected_test_infra(resolved) {
        return Err(format!(
            "{} is shared test infrastructure — edit the generated test file instead",
            resolved.display()
        ));
    }
    if editable_paths.is_empty() {
        return Ok(());
    }
    if editable_paths
        .iter()
        .any(|allowed| paths_equivalent(resolved, allowed))
    {
        return Ok(());
    }
    Err(format!(
        "{} is outside builder triage scope (editable: {})",
        resolved.display(),
        editable_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn resolve_edit_path(path: &Path, module_roots: &[PathBuf]) -> Result<PathBuf, String> {
    if path.is_absolute() {
        for root in module_roots {
            if root.as_os_str().is_empty() {
                continue;
            }
            if path.starts_with(root) {
                return Ok(path.to_path_buf());
            }
        }
        return Err(format!(
            "edit path must be relative to a triage module root: {}",
            path.display()
        ));
    }

    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!("edit path must not contain ..: {}", path.display()));
    }

    for root in module_roots {
        if root.as_os_str().is_empty() {
            continue;
        }
        let candidate = root.join(path);
        if candidate.starts_with(root) {
            return Ok(candidate);
        }
    }

    Err(format!(
        "edit path must be inside a triage module root: {}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_edit_allowed_blocks_shared_test_infra() {
        let err = assert_edit_allowed(
            Path::new("/repo/tests/common/mod.rs"),
            &[PathBuf::from("/repo/tests/new_integration_test.rs")],
        )
        .expect_err("shared infra must be protected");
        assert!(err.contains("shared test infrastructure"));
    }

    #[test]
    fn assert_edit_allowed_scopes_builder_edits_to_target_test() {
        let target = PathBuf::from("/repo/tests/new_integration_test.rs");
        assert!(assert_edit_allowed(&target, std::slice::from_ref(&target)).is_ok());

        let err = assert_edit_allowed(
            Path::new("/repo/tests/other.rs"),
            &[PathBuf::from("/repo/tests/new_integration_test.rs")],
        )
        .expect_err("out-of-scope edit");
        assert!(err.contains("outside builder triage scope"));
    }

    #[test]
    fn assert_edit_allowed_allows_standalone_triage_outside_denylist() {
        assert!(
            assert_edit_allowed(Path::new("/repo/src/lib.rs"), &[]).is_ok(),
            "empty editable_paths keeps standalone triage behavior"
        );
    }

    #[test]
    fn resolve_edit_path_accepts_absolute_paths_under_module_root() {
        let root = PathBuf::from("/repo/backend");
        let allowed = vec![root.clone()];
        let resolved =
            resolve_edit_path(Path::new("/repo/backend/src/main.rs"), &allowed).expect("absolute");
        assert_eq!(resolved, PathBuf::from("/repo/backend/src/main.rs"));
    }

    #[test]
    fn resolve_edit_path_rejects_traversal_and_outside_roots() {
        let root = PathBuf::from("/repo/backend");
        let allowed = vec![root.clone()];

        let resolved = resolve_edit_path(Path::new("src/main.rs"), &allowed).expect("relative");
        assert_eq!(resolved, PathBuf::from("/repo/backend/src/main.rs"));

        assert!(resolve_edit_path(Path::new("../etc/passwd"), &allowed).is_err());
        assert!(resolve_edit_path(Path::new("/etc/passwd"), &allowed).is_err());
    }

    #[test]
    fn resolve_edit_path_rejects_escape_via_empty_module_root() {
        let allowed = vec![PathBuf::from("")];
        assert!(resolve_edit_path(Path::new("/etc/passwd"), &allowed).is_err());
    }

    #[test]
    fn scope_test_command_for_paths_targets_cargo_integration_test() {
        let scoped =
            scope_test_command_for_paths("cargo test", &[PathBuf::from("tests/red_phase.rs")]);
        assert_eq!(scoped, "cargo test --test red_phase");
    }

    #[test]
    fn scope_test_command_for_paths_preserves_existing_cargo_flags() {
        let scoped = scope_test_command_for_paths(
            "cargo test -p demo --features json",
            &[PathBuf::from("tests/red_phase.rs")],
        );
        assert_eq!(
            scoped,
            "cargo test -p demo --features json --test red_phase"
        );
    }

    #[test]
    fn scope_test_command_for_paths_preserves_double_dash_separator() {
        let scoped = scope_test_command_for_paths(
            "cargo test -p demo -- --nocapture",
            &[PathBuf::from("tests/red_phase.rs")],
        );
        assert_eq!(scoped, "cargo test -p demo --test red_phase -- --nocapture");
    }

    #[test]
    fn is_assertion_test_failure_requires_generated_test_name_when_unscoped() {
        let output = "assertion `left == right` failed\nfailures:\n    red_phase_case\n";
        assert!(is_assertion_test_failure(
            output,
            &[PathBuf::from("tests/red_phase.rs")],
            false
        ));
        assert!(!is_assertion_test_failure(
            output,
            &[PathBuf::from("tests/other.rs")],
            false
        ));
    }

    #[test]
    fn is_assertion_test_failure_accepts_scoped_command_without_name_match() {
        let output = "assertion `left == right` failed\ntest result: FAILED";
        assert!(is_assertion_test_failure(
            output,
            &[PathBuf::from("tests/relative_red.rs")],
            true
        ));
    }

    #[test]
    fn is_assertion_test_failure_rejects_setup_panic() {
        let output = "panicked at tests/cache_manager_integration_tests.rs:15:10:\n\
            Failed to create ProjectCacheManager: \"failed to canonicalize /tmp/x: \
            No such file or directory (os error 2)\"\n\
            test result: FAILED. 0 passed; 1 failed";
        assert!(!is_assertion_test_failure(
            output,
            &[PathBuf::from("tests/cache_manager_integration_tests.rs")],
            true,
        ));
    }

    #[test]
    fn is_assertion_test_failure_rejects_panic_without_assertion_signal() {
        assert!(!is_assertion_test_failure(
            "thread 'worker' panicked at 'boom'",
            &[PathBuf::from("tests/red_phase.rs")],
            false
        ));
    }
}
