mod tools;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;

use super::traits::{AgentContext, AutonomousAgent};
use super::{
    build_tool_loop_message, format_triage_success, triage_passed, AgentLoopOrchestrator,
    SystemBuildRunner, TriageAgent, TRIAGE_SYSTEM_PROMPT,
};
use crate::cache::{mcp_workspace_root, resolve_workspace_path};
use crate::llm::{LlmClient, LlmModelTurn, LlmRequest, LlmToolSet};
use crate::tools::{language_from_extension, run_build_command, BuildCommandDiscoverer};

pub use tools::{
    parse_report_reason, parse_triage_arguments, parse_write_arguments, transpiler_tool_set,
};

pub const TRANSPILER_SYSTEM_PROMPT: &str = r#"You are the TranspilerAgent — authoritative cross-language API type / DTO sync.

Every turn: reply with a short Thought, then call exactly ONE tool.

Rules:
- Read idiom mapping rules from architecture_layout (field naming, enums, Option/null, collections, validation libs).
- Adapt source idioms natively in the target language.
- Preserve wire-format field names when architecture_layout says so.
- Never edit preserve_paths or any file other than target_path (write_target_file is allowlisted to target_path only).
- Loop: write_target_file → invoke_child_triage → fix until observation contains [TRIAGE PASS] → finalize_sync.
- On iteration cap or unrecoverable mismatch → report_error.

Available tools: write_target_file, invoke_child_triage, finalize_sync, report_error."#;

pub const TRANSPILER_MAX_ITERATIONS: u32 = 15;
const CHILD_TRIAGE_MAX_ITERATIONS: u32 = 5;

#[derive(Debug)]
pub struct TranspileTypesArgs {
    pub source_paths: Vec<String>,
    pub target_path: String,
    pub architecture_layout: String,
    pub preserve_paths: Vec<String>,
    pub verify_workspace: Option<String>,
    pub verify_command: Option<String>,
}

pub fn parse_transpile_types_args(args: &Value) -> Result<TranspileTypesArgs, String> {
    let source_paths = tools::required_string_array(args, "source_paths")?;
    if source_paths.is_empty() {
        return Err("source_paths must not be empty".to_string());
    }
    Ok(TranspileTypesArgs {
        source_paths,
        target_path: args
            .get("target_path")
            .and_then(Value::as_str)
            .ok_or_else(|| "target_path is required".to_string())?
            .to_string(),
        architecture_layout: args
            .get("architecture_layout")
            .and_then(Value::as_str)
            .ok_or_else(|| "architecture_layout is required".to_string())?
            .to_string(),
        preserve_paths: tools::optional_string_array(args, "preserve_paths")?,
        verify_workspace: args
            .get("verify_workspace")
            .and_then(Value::as_str)
            .map(str::to_string),
        verify_command: args
            .get("verify_command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

pub struct TranspilerAgent<C, TC, D> {
    llm_client: C,
    triage_agent: TriageAgent<TC, SystemBuildRunner, D>,
    target_path: PathBuf,
    preserve_paths: Vec<PathBuf>,
    verify_workspace: PathBuf,
    verify_command: Option<String>,
    tools: LlmToolSet,
    triage_green: Mutex<bool>,
}

impl<C: LlmClient, TC: LlmClient, D: BuildCommandDiscoverer> TranspilerAgent<C, TC, D> {
    pub fn new(
        llm_client: C,
        triage_agent: TriageAgent<TC, SystemBuildRunner, D>,
        target_path: PathBuf,
        preserve_paths: Vec<PathBuf>,
        verify_workspace: PathBuf,
        verify_command: Option<String>,
    ) -> Self {
        Self {
            llm_client,
            triage_agent,
            target_path,
            preserve_paths,
            verify_workspace,
            verify_command,
            tools: transpiler_tool_set(),
            triage_green: Mutex::new(false),
        }
    }

    async fn run_child_triage(
        &self,
        target_paths: Vec<PathBuf>,
        error_context: &str,
    ) -> Result<String, String> {
        let mut paths = target_paths;
        if self.verify_workspace.is_dir() && !paths.iter().any(|p| p == &self.verify_workspace) {
            paths.push(self.verify_workspace.clone());
        }

        self.triage_agent.retarget(paths)?;
        let mut context_block = error_context.to_string();
        if let Some(command) = self.verify_command.as_deref() {
            context_block = format!(
                "Verify with: cd {} && {command}\n\n{context_block}",
                self.verify_workspace.display()
            );
            match run_build_command(&self.verify_workspace, command) {
                Ok(result) if !result.success => {
                    context_block.push_str("\n\nLatest verify output:\n");
                    context_block.push_str(&result.output);
                }
                Err(err) => {
                    context_block.push_str("\n\nLatest verify output:\n");
                    context_block.push_str(&err);
                }
                _ => {}
            }
        }

        let prompt = format!(
            "invoke_child_triage\n\n{TRIAGE_SYSTEM_PROMPT}\n\nError context:\n{context_block}"
        );
        let triage_ctx =
            AgentLoopOrchestrator::run(&self.triage_agent, prompt, CHILD_TRIAGE_MAX_ITERATIONS)
                .await?;

        let passed = triage_passed(&triage_ctx);
        let extra = if !passed {
            self.verify_command.as_deref().map_or(String::new(), |cmd| {
                match run_build_command(&self.verify_workspace, cmd) {
                    Ok(result) if result.success => {
                        format!("\n\nVerify output:\n{}", result.output)
                    }
                    Ok(result) => format!(
                        "\n\nVerify still failing (exit {}):\n{}",
                        result.exit_code, result.output
                    ),
                    Err(err) => format!("\n\nVerify spawn error:\n{err}"),
                }
            })
        } else {
            String::new()
        };

        if let Ok(mut guard) = self.triage_green.lock() {
            *guard = passed;
        }

        if passed {
            Ok(format_triage_success(&triage_ctx, &[]) + &extra)
        } else {
            Ok(format!(
                "[TRIAGE INCOMPLETE] finished={} iterations={}\n{}{}",
                triage_ctx.is_finished, triage_ctx.iterations, triage_ctx.accumulated_data, extra
            ))
        }
    }

    fn write_target_file(&self, path: &str, content: &str) -> Result<String, String> {
        let resolved = resolve_workspace_path(path);
        if resolved != self.target_path {
            return Err(format!(
                "refusing write_target_file: {} is not target_path {}",
                resolved.display(),
                self.target_path.display()
            ));
        }
        if self.preserve_paths.iter().any(|p| p == &resolved) {
            return Err(format!(
                "refusing write_target_file: {} is in preserve_paths",
                resolved.display()
            ));
        }
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("mkdir {}: {err}", parent.display()))?;
        }
        std::fs::write(&resolved, content)
            .map_err(|err| format!("write {}: {err}", resolved.display()))?;
        if let Ok(mut guard) = self.triage_green.lock() {
            *guard = false;
        }
        Ok(format!(
            "wrote {} ({} bytes)",
            resolved.display(),
            content.len()
        ))
    }

    async fn dispatch_tool(
        &self,
        tool_name: &str,
        arguments: &Value,
        context: &mut AgentContext,
    ) -> Result<String, String> {
        match tool_name {
            "write_target_file" => {
                let (path, content) = tools::parse_write_arguments(arguments)?;
                self.write_target_file(&path, &content)
            }
            "invoke_child_triage" => {
                let (paths, error_context) = tools::parse_triage_arguments(arguments)?;
                let mut resolved: Vec<PathBuf> = paths
                    .into_iter()
                    .map(|path| resolve_workspace_path(&path))
                    .collect();
                if !resolved.iter().any(|p| p == &self.target_path) {
                    resolved.push(self.target_path.clone());
                }
                self.run_child_triage(resolved, &error_context).await
            }
            "finalize_sync" => {
                let green = self
                    .triage_green
                    .lock()
                    .map(|guard| *guard)
                    .unwrap_or(false);
                if !green {
                    return Err(
                        "refusing finalize_sync: invoke_child_triage has not passed yet"
                            .to_string(),
                    );
                }
                let summary = arguments
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("type sync complete");
                context.is_finished = true;
                context.agent_completed = true;
                Ok(summary.to_string())
            }
            "report_error" => {
                let reason = tools::parse_report_reason(arguments)?;
                context.is_finished = true;
                context.agent_completed = false;
                context.accumulated_data = reason.clone();
                Ok(reason)
            }
            other => Err(format!("unsupported transpiler tool: {other}")),
        }
    }
}

#[async_trait]
impl<C: LlmClient, TC: LlmClient, D: BuildCommandDiscoverer> AutonomousAgent
    for TranspilerAgent<C, TC, D>
{
    fn name(&self) -> &'static str {
        "PHASE_TRANSPILER"
    }

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String> {
        let verify_hint = self
            .verify_command
            .as_deref()
            .map(|cmd| format!("cd {} && {cmd}", self.verify_workspace.display()))
            .unwrap_or_else(|| "(triage auto-discover)".to_string());
        context.input_prompt.push_str(&format!(
            "\n\nTarget: {}\nPreserve: {}\nVerify: {}\n",
            self.target_path.display(),
            self.preserve_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            verify_hint,
        ));
        Ok(())
    }

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String> {
        let user_message = build_tool_loop_message(context);
        let request = LlmRequest::new(TRANSPILER_SYSTEM_PROMPT, &user_message, &self.tools);
        let model_turn: LlmModelTurn = self.llm_client.complete(request)?;

        let tool_call = match model_turn.tool_calls.first() {
            Some(call) => call,
            None => {
                let thought = model_turn.content.unwrap_or_default();
                context.accumulated_data.push_str(&format!(
                    "Thought:\n{thought}\nObservation:\n(model did not call a tool — call exactly one tool)\n"
                ));
                return Ok(());
            }
        };

        let args_key = tool_call.arguments.to_string();
        let call_key = (tool_call.name.clone(), args_key);
        if context.last_tool_call.as_ref() == Some(&call_key) {
            context.accumulated_data.push_str(
                "Observation:\nduplicate tool call blocked — change approach or finalize.\n",
            );
            return Ok(());
        }
        context.last_tool_call = Some(call_key);

        let thought = model_turn.content.unwrap_or_default();
        let observation = self
            .dispatch_tool(&tool_call.name, &tool_call.arguments, context)
            .await?;

        if tool_call.name == "finalize_sync" || tool_call.name == "report_error" {
            context.accumulated_data = observation.clone();
            return Ok(());
        }

        context.accumulated_data.push_str(&format!(
            "Thought:\n{thought}\nTool: {}({})\nObservation:\n{observation}\n",
            tool_call.name, tool_call.arguments
        ));
        Ok(())
    }

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String> {
        if context.iterations >= context.max_iterations.saturating_sub(1) {
            context.input_prompt.push_str(
                "\nFinal turn: fix remaining verify errors or call report_error with reason.",
            );
        } else {
            context
                .input_prompt
                .push_str("\nContinue sync. Call exactly one harness tool.");
        }
        Ok(())
    }
}

fn language_tag(path: &Path) -> &'static str {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(language_from_extension)
        .map(|lang| lang.as_str())
        .unwrap_or("unknown")
}

fn truncate_utf8_prefix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

pub fn embed_source_files(paths: &[PathBuf], max_bytes_per_file: usize) -> Result<String, String> {
    let mut out = String::from("## Source files\n\n");
    for path in paths {
        let contents = std::fs::read_to_string(path)
            .map_err(|err| format!("read {}: {err}", path.display()))?;
        let lang = language_tag(path);
        out.push_str(&format!("### `{}` ({lang})\n\n", path.display()));
        let body = if contents.len() > max_bytes_per_file {
            format!(
                "{}\n... [truncated]",
                truncate_utf8_prefix(&contents, max_bytes_per_file)
            )
        } else {
            contents
        };
        out.push_str(&format!("```{lang}\n{body}\n```\n\n"));
    }
    Ok(out)
}

pub fn default_verify_workspace(target_path: &Path) -> PathBuf {
    target_path
        .parent()
        .map(|p| mcp_workspace_root().join(p))
        .filter(|p| p.is_dir())
        .unwrap_or_else(mcp_workspace_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn embed_source_files_truncates_large_files() {
        let dir = std::env::temp_dir().join(format!("transpiler-embed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let path = dir.join("big.rs");
        let mut file = std::fs::File::create(&path).expect("create");
        write!(file, "{}", "x".repeat(200)).expect("write");

        let embedded = embed_source_files(std::slice::from_ref(&path), 50).expect("embed");
        assert!(embedded.contains("[truncated]"));
        assert!(embedded.contains("rust"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_transpile_types_args_requires_sources() {
        assert!(parse_transpile_types_args(&serde_json::json!({
            "target_path": "out.ts",
            "architecture_layout": "sync"
        }))
        .is_err());
    }

    #[test]
    fn parse_transpile_types_args_rejects_non_string_source() {
        let err = parse_transpile_types_args(&serde_json::json!({
            "source_paths": ["ok.rs", 1],
            "target_path": "out.ts",
            "architecture_layout": "sync"
        }))
        .unwrap_err();
        assert!(err.contains("source_paths[1]"));
    }

    #[test]
    fn truncate_utf8_prefix_respects_byte_limit() {
        let s = "é".repeat(100); // 2 bytes per char
        assert!(truncate_utf8_prefix(&s, 50).len() <= 50);
    }
}
