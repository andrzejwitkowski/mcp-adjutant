use std::path::PathBuf;

use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct AgentContext {
    pub input_prompt: String,
    pub accumulated_data: String,
    pub iterations: u32,
    pub max_iterations: u32,
    pub is_finished: bool,
    pub agent_completed: bool,
    pub touched_files: Vec<PathBuf>,
    /// Last tool invocation `(name, serialized args)` — used to block repeat loops.
    pub last_tool_call: Option<(String, String)>,
}

#[async_trait]
pub trait AutonomousAgent {
    fn name(&self) -> &'static str;

    async fn enrich_context(&self, context: &mut AgentContext) -> Result<(), String>;

    async fn process_and_evaluate(&self, context: &mut AgentContext) -> Result<(), String>;

    async fn mutate_next_iteration(&self, context: &mut AgentContext) -> Result<(), String>;
}
