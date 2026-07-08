use serde_json::Value;

use crate::llm::{LlmTool, LlmToolSet, ToolDefinition};

pub fn builder_tool_set() -> LlmToolSet {
    LlmToolSet::new()
        .register(GatherIntegrationContextTool::new())
        .register(GenerateTestFactoryTool::new())
        .register(WriteTestSuiteTool::new())
}

struct GatherIntegrationContextTool {
    definition: ToolDefinition,
}

impl GatherIntegrationContextTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "gather_integration_context",
                "Runs a Scout sub-agent (ripgrep, AST, read_file) to collect signatures and files needed for an integration test. ALWAYS call this before writing an integration test.",
            )
            .string_array_param(
                "components",
                "Np. ['auth::middleware', 'db::UserRepository']",
                true,
            ),
        }
    }
}

impl LlmTool for GatherIntegrationContextTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, _arguments: &Value) -> Result<String, String> {
        Err("gather_integration_context is executed by BuilderAgent".to_string())
    }
}

struct GenerateTestFactoryTool {
    definition: ToolDefinition,
}

impl GenerateTestFactoryTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "generate_test_factory",
                "Runs a Scout sub-agent to produce an idiomatic factory/fixture pattern for a type from a source file (language agnostic).",
            )
            .string_param("target_struct", "Target struct name.", true)
            .string_param("target_file", "Source file path.", true),
        }
    }
}

impl LlmTool for GenerateTestFactoryTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, _arguments: &Value) -> Result<String, String> {
        Err("generate_test_factory is executed by BuilderAgent".to_string())
    }
}

struct WriteTestSuiteTool {
    definition: ToolDefinition,
}

impl WriteTestSuiteTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "write_test_suite",
                "Writes the generated test suite to disk.",
            )
            .string_param("path", "Destination test file path.", true)
            .string_param("content", "Test file contents.", true)
            .enum_param(
                "tdd_phase",
                "red: test must compile but intentionally fail assertions. green: test must pass.",
                &["red", "green", "refactor"],
                true,
            ),
        }
    }
}

impl LlmTool for WriteTestSuiteTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, _arguments: &Value) -> Result<String, String> {
        Err("write_test_suite is executed by BuilderAgent".to_string())
    }
}

pub fn parse_components(arguments: &Value) -> Result<Vec<String>, String> {
    let items = arguments
        .get("components")
        .and_then(Value::as_array)
        .ok_or_else(|| "tool argument 'components' must be an array of strings".to_string())?;

    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "tool argument 'components' must contain only strings".to_string())
        })
        .collect()
}

pub fn parse_factory_arguments(arguments: &Value) -> Result<(String, String), String> {
    let target_struct = required_str(arguments, "target_struct")?;
    let target_file = required_str(arguments, "target_file")?;
    Ok((target_struct, target_file))
}

pub fn parse_write_test_suite_arguments(
    arguments: &Value,
) -> Result<(String, String, String), String> {
    let path = required_str(arguments, "path")?;
    let content = required_str(arguments, "content")?;
    let tdd_phase = required_str(arguments, "tdd_phase")?;
    if !matches!(tdd_phase.as_str(), "red" | "green" | "refactor") {
        return Err("tool argument 'tdd_phase' must be one of: red, green, refactor".to_string());
    }
    Ok((path, content, tdd_phase))
}

pub fn build_scout_integration_query(components: &[String]) -> String {
    format!(
        "PHASE_1_SCOUT\n\nExplore the repository for integration tests covering components:\n{}\n\n\
         Use ripgrep, ast_calls, and read_file to collect method signatures, file paths, dependencies, \
         and call examples. Finish with a finalize report.",
        components.join(", ")
    )
}

pub fn build_scout_factory_query(target_struct: &str, target_file: &str) -> String {
    format!(
        "PHASE_1_SCOUT\n\nPrepare a factory/fixture/builder template for type `{target_struct}` \
         based on file `{target_file}`.\n\n\
         Detect the language (detect_language), read the type definition (read_file, ast_calls), and return \
         an idiomatic test fixture pattern for that stack (fluent builder, object mother, factory \
         method — depending on language conventions). Do not assume a specific language up front. \
         Finish with a finalize report."
    )
}

fn required_str(arguments: &Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("tool argument '{key}' must be a string"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builder_tool_set_registers_all_tools() {
        let tools = builder_tool_set();
        let names: Vec<_> = tools
            .definitions()
            .into_iter()
            .map(|tool| tool.name.clone())
            .collect();

        assert_eq!(
            names,
            vec![
                "gather_integration_context".to_string(),
                "generate_test_factory".to_string(),
                "write_test_suite".to_string(),
            ]
        );
    }

    #[test]
    fn build_scout_integration_query_lists_components() {
        let query = build_scout_integration_query(&[
            "auth::middleware".to_string(),
            "db::UserRepository".to_string(),
        ]);
        assert!(query.contains("PHASE_1_SCOUT"));
        assert!(query.contains("auth::middleware"));
        assert!(query.contains("db::UserRepository"));
        assert!(query.contains("finalize"));
    }

    #[test]
    fn build_scout_factory_query_is_language_agnostic() {
        let query = build_scout_factory_query("User", "src/models/User.java");
        assert!(query.contains("PHASE_1_SCOUT"));
        assert!(query.contains("User"));
        assert!(query.contains("src/models/User.java"));
        assert!(query.contains("detect_language"));
        assert!(query.contains("finalize"));
        assert!(!query.contains("pub struct"));
    }

    #[test]
    fn parse_write_test_suite_arguments_extracts_fields() {
        let (path, content, phase) = parse_write_test_suite_arguments(&json!({
            "path": "tests/example.rs",
            "content": "#[test] fn t() {}",
            "tdd_phase": "red",
        }))
        .expect("args");

        assert_eq!(path, "tests/example.rs");
        assert_eq!(content, "#[test] fn t() {}");
        assert_eq!(phase, "red");
    }

    #[test]
    fn parse_write_test_suite_arguments_rejects_unknown_tdd_phase() {
        let err = parse_write_test_suite_arguments(&json!({
            "path": "tests/example.rs",
            "content": "#[test] fn t() {}",
            "tdd_phase": "blue",
        }))
        .expect_err("invalid phase");

        assert!(err.contains("tdd_phase"));
    }
}
