use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::traits::{AgentContext, AutonomousAgent};
use crate::domain::AdjutantConfig;
use crate::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolSet};
use crate::tools::{
    edit_file_line, find_nearest_module_boundary, get_dirty_files_from_git, run_build_command,
};

pub const TRIAGE_SYSTEM_PROMPT: &str = r#"Jesteś agentem naprawczym kompilatora (PHASE_5_TRIAGE). Dostaniesz logi z błędami.
Oceń, czy potrafisz naprawić kod (np. brakujący import, literówka).
Dozwolone akcje:
- ACTION: edit_file(path="src/main.rs", line=42, content="pub struct NewName;")
- ACTION: report_architectural_error(msg="Złożony błąd lifetime'ów, wymagam pomocy architekta.")"#;

pub trait BuildCommandRunner: Send + Sync {
    fn run_build_command(&self, dir: &Path, command: &str) -> Result<String, String>;
}

pub struct SystemBuildRunner;

impl BuildCommandRunner for SystemBuildRunner {
    fn run_build_command(&self, dir: &Path, command: &str) -> Result<String, String> {
        run_build_command(dir, command)
    }
}

#[derive(Debug, Clone)]
enum TriageAction {
    EditFile {
        path: PathBuf,
        line: usize,
        content: String,
    },
    ReportArchitecturalError {
        msg: String,
    },
}

#[derive(Default)]
struct TriageWorkspace {
    build_targets: Vec<(PathBuf, String)>,
}

pub struct TriageAgent<C, B = SystemBuildRunner> {
    llm_client: C,
    target_paths: Vec<PathBuf>,
    config: Arc<AdjutantConfig>,
    build_runner: B,
    workspace: Mutex<TriageWorkspace>,
}

impl<C: LlmClient> TriageAgent<C, SystemBuildRunner> {
    pub fn new(llm_client: C, target_paths: Vec<PathBuf>, config: Arc<AdjutantConfig>) -> Self {
        Self::with_build_runner(llm_client, target_paths, config, SystemBuildRunner)
    }
}

impl<C: LlmClient, B: BuildCommandRunner> TriageAgent<C, B> {
    pub fn with_build_runner(
        llm_client: C,
        target_paths: Vec<PathBuf>,
        config: Arc<AdjutantConfig>,
        build_runner: B,
    ) -> Self {
        Self {
            llm_client,
            target_paths,
            config,
            build_runner,
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

    fn collect_build_targets(&self, paths: &[PathBuf]) -> Vec<(PathBuf, String)> {
        let mut seen = HashSet::new();
        let mut targets = Vec::new();

        for path in paths {
            if let Some((dir, command)) = find_nearest_module_boundary(path, &self.config) {
                let key = (dir.clone(), command.clone());
                if seen.insert(key.clone()) {
                    targets.push(key);
                }
            }
        }

        targets
    }

    fn condense_build_errors(output: &str) -> String {
        output
            .lines()
            .filter(|line| {
                let lower = line.to_ascii_lowercase();
                lower.contains("error")
                    || lower.contains("warning[")
                    || line.contains("-->")
                    || line.contains(".rs:")
                    || line.contains(".ts:")
                    || line.contains(".tsx:")
            })
            .take(80)
            .collect::<Vec<_>>()
            .join("\n")
    }

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
                    let condensed = Self::condense_build_errors(&output);
                    combined_errors.push(format!(
                        "Build FAILED in {} (`{command}`):\n{condensed}\n",
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
impl<C: LlmClient, B: BuildCommandRunner> AutonomousAgent for TriageAgent<C, B> {
    fn name(&self) -> &'static str {
        "triage_agent"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        if !context.input_prompt.contains("PHASE_5_TRIAGE") {
            context.input_prompt.push_str("\n\n");
            context.input_prompt.push_str(TRIAGE_SYSTEM_PROMPT);
        }

        let paths = self.resolve_target_paths()?;
        let targets = self.collect_build_targets(&paths);

        let mut workspace = self
            .workspace
            .lock()
            .map_err(|_| "triage workspace lock poisoned".to_string())?;
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
        let targets = {
            let workspace = self
                .workspace
                .lock()
                .map_err(|_| "triage workspace lock poisoned".to_string())?;
            workspace.build_targets.clone()
        };

        if targets.is_empty() {
            context.is_finished = true;
            context.input_prompt =
                "Brak modułów do sprawdzenia (brak zmian w git lub nieznane ścieżki).".to_string();
            return Ok(());
        }

        if self.run_build_targets(&targets, context)? {
            return Ok(());
        }

        let user_message = format!(
            "Logi kompilacji do naprawy:\n\n{}\n\nWybierz jedną dozwoloną akcję ACTION.",
            context.accumulated_data
        );
        let tools = LlmToolSet::new();
        let request = LlmRequest::new(TRIAGE_SYSTEM_PROMPT, &user_message, &tools);
        let model_turn: LlmModelTurn = self.llm_client.complete(request)?;

        let response = model_turn.content.unwrap_or_default();

        context.accumulated_data.push_str(&format!(
            "LLM triage response (iter {}):\n{response}\n",
            context.iterations
        ));

        match parse_triage_action(&response) {
            Some(TriageAction::EditFile {
                path,
                line,
                content,
            }) => {
                let module_roots = Self::module_roots(&targets);
                let resolved = resolve_edit_path(&path, &module_roots)?;
                edit_file_line(&resolved, line, &content)?;
                context.accumulated_data.push_str(&format!(
                    "Applied edit_file({}, line={line})\n",
                    resolved.display()
                ));
                self.run_build_targets(&targets, context)?;
            }
            Some(TriageAction::ReportArchitecturalError { msg }) => {
                context.accumulated_data = msg;
                context.is_finished = true;
            }
            None => {
                context.accumulated_data.push_str(
                    "LLM response missing recognizable ACTION — retrying next iteration.\n",
                );
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
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!("edit path must not contain ..: {}", path.display()));
    }

    for root in module_roots {
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        if candidate.starts_with(root) {
            return Ok(candidate);
        }
    }

    Err(format!(
        "edit path must be inside a triage module root: {}",
        path.display()
    ))
}

fn parse_triage_action(text: &str) -> Option<TriageAction> {
    let action_line = text
        .lines()
        .find(|line| line.trim().starts_with("ACTION:"))?;
    let action_line = action_line.trim();

    if let Some(args) = action_line
        .strip_prefix("ACTION: edit_file(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let path = parse_action_value(args, "path")?;
        let line = parse_action_value(args, "line")?.parse().ok()?;
        let content = parse_action_value(args, "content")?;
        return Some(TriageAction::EditFile {
            path: PathBuf::from(path),
            line,
            content,
        });
    }

    if let Some(msg) = action_line
        .strip_prefix("ACTION: report_architectural_error(msg=")
        .and_then(|s| s.strip_suffix(')'))
    {
        return Some(TriageAction::ReportArchitecturalError {
            msg: unquote_action_value(msg.trim()),
        });
    }

    None
}

fn parse_action_value(args: &str, key: &str) -> Option<String> {
    let pattern = format!("{key}=");
    let start = args.find(&pattern)? + pattern.len();
    let rest = &args[start..];
    Some(unquote_action_value(
        rest.split_at(unquote_end(rest))
            .0
            .trim_end_matches(',')
            .trim(),
    ))
}

fn unquote_end(input: &str) -> usize {
    if input.starts_with('"') {
        let mut escaped = false;
        for (idx, ch) in input.char_indices().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                return idx + 1;
            }
        }
        input.len()
    } else {
        input.find(',').unwrap_or(input.len())
    }
}

fn unquote_action_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].replace("\\\"", "\"")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_edit_file_action() {
        let action = parse_triage_action(
            r#"Thought: fix typo
ACTION: edit_file(path="src/main.rs", line=42, content="pub struct NewName;")"#,
        )
        .expect("action");

        match action {
            TriageAction::EditFile {
                path,
                line,
                content,
            } => {
                assert_eq!(path, PathBuf::from("src/main.rs"));
                assert_eq!(line, 42);
                assert_eq!(content, "pub struct NewName;");
            }
            other => panic!("unexpected action: {other:?}"),
        }
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
    fn parse_architectural_error_action() {
        let action = parse_triage_action(
            r#"ACTION: report_architectural_error(msg="Złożony błąd lifetime'ów, wymagam pomocy architekta.")"#,
        )
        .expect("action");

        match action {
            TriageAction::ReportArchitecturalError { msg } => {
                assert!(msg.contains("lifetime"));
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }
}
