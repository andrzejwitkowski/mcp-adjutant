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
    NoopBuildDiscoverer,
};

pub use tools::{parse_edit_file_arguments, parse_report_error_arguments, triage_tool_set};

pub const TRIAGE_SYSTEM_PROMPT: &str = r#"Jesteś agentem naprawczym kompilatora (PHASE_5_TRIAGE). Dostaniesz logi z błędami.
Oceń, czy potrafisz naprawić kod (np. brakujący import, literówka).

Masz do dyspozycji narzędzia (tool calls):
- edit_file — zamienia jedną linię pliku (path, line, content)
- report_architectural_error — eskalacja, gdy naprawa wymaga architekta (msg)

Odpowiadaj krótkim uzasadnieniem (Thought), a następnie wywołaj dokładnie jedno narzędzie."#;

pub trait BuildCommandRunner: Send + Sync {
    fn run_build_command(&self, dir: &Path, command: &str) -> Result<String, String>;
}

pub struct SystemBuildRunner;

impl BuildCommandRunner for SystemBuildRunner {
    fn run_build_command(&self, dir: &Path, command: &str) -> Result<String, String> {
        run_build_command(dir, command)
    }
}

#[derive(Default)]
struct TriageWorkspace {
    build_targets: Vec<(PathBuf, String)>,
    input_paths: Vec<PathBuf>,
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

    fn resolve_target_paths(&self) -> Result<Vec<PathBuf>, String> {
        if self.target_paths.is_empty() {
            get_dirty_files_from_git()
        } else {
            Ok(self.target_paths.clone())
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
                Ok(output) => {
                    let step = format!("Build OK in {} (`{command}`):\n{output}\n", dir.display());
                    context.accumulated_data.push_str(&step);
                }
                Err(output) => {
                    all_ok = false;
                    let (body, truncated) = truncate_build_log(
                        &output,
                        Self::BUILD_LOG_MAX_LINES,
                        Self::BUILD_LOG_MAX_BYTES,
                    );
                    let log = if truncated {
                        format!("(log truncated — showing tail)\n{body}")
                    } else {
                        body
                    };
                    combined_errors.push(format!(
                        "Build FAILED in {} (`{command}`):\n{log}\n",
                        dir.display()
                    ));
                }
            }
        }

        if all_ok {
            context.is_finished = true;
            context.input_prompt = "Wszystkie testy/kompilacje zakończone sukcesem.".to_string();
        } else if !combined_errors.is_empty() {
            context
                .accumulated_data
                .push_str(&combined_errors.join("\n---\n"));
        }

        Ok(all_ok)
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
        workspace.input_paths = paths;
        workspace.build_targets = targets.clone();

        let summary: Vec<String> = targets
            .iter()
            .map(|(dir, cmd)| format!("{} => {cmd}", dir.display()))
            .collect();
        context.accumulated_data = format!(
            "Triage targets ({} modules):\n{}",
            summary.len(),
            summary.join("\n")
        );

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
                workspace.input_paths.clone(),
            )
        };

        if targets.is_empty() {
            context.is_finished = true;
            context.input_prompt = if paths.is_empty() {
                "Brak modułów do sprawdzenia (brak zmian w git lub nieznane ścieżki).".to_string()
            } else {
                "Nie udało się rozpoznać polecenia kompilacji (brak manifestu i discovery nie zwróciło komendy)."
                    .to_string()
            };
            return Ok(());
        }

        if self.run_build_targets(&targets, context)? {
            return Ok(());
        }

        let user_message = format!(
            "Logi kompilacji do naprawy:\n\n{}\n\nWywołaj jedno narzędzie.",
            context.accumulated_data
        );
        let request = LlmRequest::new(TRIAGE_SYSTEM_PROMPT, &user_message, &self.tools);
        let model_turn: LlmModelTurn = self.llm_client.complete(request)?;

        let tool_call = match model_turn.tool_calls.first() {
            Some(call) => call,
            None => {
                let thought = model_turn.content.unwrap_or_default();
                context.accumulated_data.push_str(&format!(
                    "LLM triage response (iter {}):\n{thought}\n(model nie wywołał narzędzia — ponawiam)\n",
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
                let module_roots = Self::module_roots(&targets);
                let resolved = resolve_edit_path(&path, &module_roots)?;
                edit_file_line(&resolved, line, &content)?;
                context.accumulated_data.push_str(&format!(
                    "Applied edit_file({}, line={line})\n",
                    resolved.display()
                ));
                self.run_build_targets(&targets, context)?;
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
            .push_str("\nPonów kompilację po ostatniej próbie naprawy.");
        Ok(())
    }
}

fn resolve_edit_path(path: &Path, module_roots: &[PathBuf]) -> Result<PathBuf, String> {
    if path.is_absolute() {
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
}
