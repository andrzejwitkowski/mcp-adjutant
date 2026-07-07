use serde_json::Value;

use crate::cache::ProjectCacheManager;
use crate::llm::{LlmTool, LlmToolSet, ToolDefinition};
use crate::tools::AstUsageFinder;

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
                "Odpytuje bazę semantyczną i parser AST o sygnatury metod z zewnętrznych plików. Wywołaj to ZAWSZE przed napisaniem testu integracyjnego.",
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
                "Generuje wzorzec Fluent Buildera dla danej struktury danych (na potrzeby mockowania).",
            )
            .string_param("target_struct", "Nazwa struktury docelowej.", true)
            .string_param("target_file", "Ścieżka pliku źródłowego.", true),
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
                "Zapisuje wygenerowany zestaw testów na dysk.",
            )
            .string_param("path", "Ścieżka docelowa pliku testowego.", true)
            .string_param("content", "Treść pliku testowego.", true)
            .enum_param(
                "tdd_phase",
                "red: test ma się skompilować, ale celowo oblewać asercje. green: test ma przejść na zielono.",
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
    Ok((path, content, tdd_phase))
}

pub fn gather_integration_context(
    cache: &ProjectCacheManager,
    components: &[String],
) -> Result<String, String> {
    let project_root = cache.project_root();
    let mut sections = Vec::new();

    for component in components {
        let query = format!("integration signatures for {component}");
        let mut section = format!("### {component}\n");

        if let Some(insight) = cache.try_get_valid_insight(&query)? {
            section.push_str(&insight);
            section.push('\n');
        } else {
            section.push_str("(no semantic cache hit)\n");
            if let Some((module, symbol)) = component.split_once("::") {
                let candidate = project_root.join(format!("src/{module}.rs"));
                if candidate.is_file() {
                    match AstUsageFinder::find_calls_in_file(&candidate, symbol) {
                        Ok(lines) if !lines.is_empty() => {
                            section.push_str(&format!(
                                "AST call sites for `{symbol}` in {}: {lines:?}\n",
                                candidate.display()
                            ));
                        }
                        Ok(_) => section.push_str("AST: no call sites found.\n"),
                        Err(err) => section.push_str(&format!("AST scan failed: {err}\n")),
                    }
                }
            }
        }

        sections.push(section);
    }

    Ok(sections.join("\n"))
}

pub fn generate_test_factory(target_struct: &str, target_file: &str) -> String {
    format!(
        r#"// Fluent builder for {target_struct} (source: {target_file})
pub struct {target_struct}Builder {{
    inner: {target_struct},
}}

impl {target_struct}Builder {{
    pub fn new() -> Self {{
        Self {{
            inner: {target_struct}::default(),
        }}
    }}

    pub fn build(self) -> {target_struct} {{
        self.inner
    }}
}}
"#
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
    fn generate_test_factory_emits_fluent_builder_skeleton() {
        let code = generate_test_factory("User", "src/user.rs");
        assert!(code.contains("UserBuilder"));
        assert!(code.contains("src/user.rs"));
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
}
