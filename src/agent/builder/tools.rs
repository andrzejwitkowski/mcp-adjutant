use serde_json::Value;

use crate::llm::{required_str, LlmTool, LlmToolSet, ToolDefinition};

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
                "Writes the generated test suite to disk. Pass ONLY valid source code in `content` (no markdown/prose). Prefer co-located `*.test.ts(x)` or a new file under `tests/`.",
            )
            .string_param("path", "Destination test file path.", true)
            .string_param(
                "content",
                "Full test file contents to write.",
                true,
            )
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

pub fn parse_write_test_suite_arguments(arguments: &Value) -> Result<(String, String), String> {
    let path = required_str(arguments, "path")?;
    let tdd_phase = required_str(arguments, "tdd_phase")?;
    if !matches!(tdd_phase.as_str(), "red" | "green" | "refactor") {
        return Err("tool argument 'tdd_phase' must be one of: red, green, refactor".to_string());
    }
    Ok((path, tdd_phase))
}

pub fn build_scout_integration_query(components: &[String]) -> String {
    format!(
        "PHASE_1_SCOUT\n\nRead `tests/cache_manager_tests.rs` and `tests/common/mod.rs` first for integration-test setup patterns.\n\
         Then explore integration tests covering components:\n{}\n\n\
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

fn normalize_test_source(text: &str) -> String {
    // ponytail: LLM tool JSON often emits \' instead of " inside Rust string literals
    text.replace("\\'", "\"")
}

fn strip_code_fences(text: &str) -> String {
    let trimmed = text.trim();
    let body = if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.split_once('\n').map(|(_, body)| body).unwrap_or(rest);
        rest.trim_end_matches("```").trim()
    } else {
        trimmed
    };
    normalize_test_source(body)
}

// ponytail: provider tool-arg EOF ~30k; bump if larger suites become common
pub const MAX_TEST_CONTENT_CHARS: usize = 24_000;

pub fn validate_test_content(content: &str, path: &str) -> Result<(), String> {
    if content.len() > MAX_TEST_CONTENT_CHARS {
        return Err(format!(
            "write_test_suite content is {} chars (max {MAX_TEST_CONTENT_CHARS}); shrink the file",
            content.len()
        ));
    }
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("write_test_suite content is empty".to_string());
    }
    let lower = trimmed.to_ascii_lowercase();
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    // `# ` is a normal Python comment; only treat it as markdown for non-py.
    let looks_like_markdown = lower.starts_with("**")
        || lower.starts_with("## ")
        || (ext != "py" && lower.starts_with("# "))
        || lower.starts_with("the triage results")
        || lower.starts_with("i will now")
        || lower.starts_with("thought:")
        || lower.starts_with("rationale:")
        || lower.starts_with("status: green")
        || lower.starts_with("status: red")
        || lower.starts_with("previous triage")
        || lower.starts_with("the previous tdd")
        || lower.starts_with("build & test command");
    if looks_like_markdown {
        return Err(
            "write_test_suite content must be source code only — no markdown/prose narrative"
                .to_string(),
        );
    }

    let looks_like_code = match ext {
        "ts" | "tsx" | "js" | "jsx" => {
            trimmed.contains("import ")
                || trimmed.contains("describe(")
                || trimmed.contains("it(")
                || trimmed.contains("test(")
                || trimmed.contains("export ")
        }
        "rs" => {
            trimmed.contains("#[test]")
                || trimmed.contains("fn ")
                || trimmed.contains("mod ")
                || trimmed.contains("use ")
        }
        "py" => {
            trimmed.contains("def test") || trimmed.contains("import ") || trimmed.contains("from ")
        }
        _ => true,
    };
    if !looks_like_code {
        return Err(format!(
            "write_test_suite content for `{path}` does not look like {ext} source (missing imports/tests)"
        ));
    }
    Ok(())
}

pub fn extract_test_content(_content: Option<&str>, arguments: &Value) -> Result<String, String> {
    // Require tool `content` — assistant Thought/message must not supply the suite body.
    if let Some(text) = arguments
        .get("content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Ok(strip_code_fences(text));
    }
    Err("write_test_suite requires non-empty tool argument 'content'".to_string())
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
        assert!(query.contains("cache_manager_tests.rs"));
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
    fn extract_test_content_prefers_tool_argument_over_thought() {
        let content = extract_test_content(
            Some("Thought: I will write the suite now."),
            &json!({"content": "#[test]\nfn t() {}"}),
        )
        .expect("content");
        assert_eq!(content, "#[test]\nfn t() {}");
    }

    #[test]
    fn extract_test_content_rejects_assistant_message_fallback() {
        let err = extract_test_content(Some("```rust\n#[test]\nfn t() {}\n```"), &json!({}))
            .expect_err("require content arg");
        assert!(err.contains("content"), "{err}");
    }

    #[test]
    fn extract_test_content_unescapes_json_single_quotes() {
        let content = extract_test_content(
            None,
            &json!({"content": "std::env::set_var(\\'FOO\\', \"bar\");"}),
        )
        .expect("content");
        assert_eq!(content, "std::env::set_var(\"FOO\", \"bar\");");
    }

    #[test]
    fn extract_test_content_falls_back_to_tool_argument() {
        let content =
            extract_test_content(None, &json!({"content": "#[test] fn t() {}"})).expect("content");
        assert_eq!(content, "#[test] fn t() {}");
    }

    #[test]
    fn validate_test_content_rejects_thought_prefixed() {
        let err = validate_test_content(
            "Thought: rationale\nimport { describe } from 'vitest'\ndescribe('x', () => {})",
            "foo.test.ts",
        )
        .expect_err("thought");
        assert!(err.contains("source code only"), "{err}");
    }

    #[test]
    fn validate_test_content_rejects_markdown_prose() {
        let err = validate_test_content(
            "## PHASE_4_BUILDER Report\nimport { describe } from 'vitest'",
            "foo.test.ts",
        )
        .expect_err("prose");
        assert!(err.contains("source code only"), "{err}");
    }

    #[test]
    fn validate_test_content_accepts_python_hash_comment() {
        validate_test_content(
            "# helpers for the module under test\nimport pytest\n\ndef test_x():\n    assert True\n",
            "tests/test_x.py",
        )
        .expect("ok");
    }

    #[test]
    fn validate_test_content_rejects_oversized() {
        let huge = format!("import x from 'y'\n{}", "a".repeat(MAX_TEST_CONTENT_CHARS));
        let err = validate_test_content(&huge, "foo.test.ts").expect_err("size");
        assert!(err.contains("max"), "{err}");
    }

    #[test]
    fn validate_test_content_accepts_vitest() {
        validate_test_content(
            "import { describe, it } from 'vitest'\ndescribe('x', () => { it('y', () => {}) })",
            "foo.test.ts",
        )
        .expect("ok");
    }

    #[test]
    fn parse_write_test_suite_arguments_extracts_fields() {
        let (path, phase) = parse_write_test_suite_arguments(&json!({
            "path": "tests/example.rs",
            "tdd_phase": "red",
        }))
        .expect("args");

        assert_eq!(path, "tests/example.rs");
        assert_eq!(phase, "red");
    }

    #[test]
    fn parse_write_test_suite_arguments_rejects_unknown_tdd_phase() {
        let err = parse_write_test_suite_arguments(&json!({
            "path": "tests/example.rs",
            "tdd_phase": "blue",
        }))
        .expect_err("invalid phase");

        assert!(err.contains("tdd_phase"));
    }
}
