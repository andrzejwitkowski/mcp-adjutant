use std::path::Path;

use serde_json::Value;

use crate::llm::{LlmTool, LlmToolSet, ToolDefinition};
use crate::tools::{
    detect_file_language, detect_project_languages, read_file_range, run_ripgrep, AstUsageFinder,
};

pub fn scout_tool_set() -> LlmToolSet {
    LlmToolSet::new()
        .register(DetectLanguageTool::new())
        .register(RipgrepTool::new())
        .register(AstCallsTool::new())
        .register(ReadFileTool::new())
        .register(FinalizeTool::new())
}

struct DetectLanguageTool {
    definition: ToolDefinition,
}

impl DetectLanguageTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "detect_language",
                "Wykrywa język pliku lub projektu na podstawie rozszerzenia, markerów (Cargo.toml, package.json, ...) i heurystyk treści.",
            )
            .string_param("path", "Ścieżka do pliku lub katalogu projektu.", true)
            .enum_param(
                "scope",
                "file = pojedynczy plik, project = skan katalogu repo.",
                &["file", "project"],
                true,
            ),
        }
    }
}

impl LlmTool for DetectLanguageTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        let path = required_str(arguments, "path")?;
        let scope = required_str(arguments, "scope")?;
        let report = match scope.as_str() {
            "file" => serde_json::to_string(&detect_file_language(Path::new(&path))?),
            "project" => serde_json::to_string(&detect_project_languages(Path::new(&path))?),
            other => {
                return Err(format!(
                    "detect_language scope must be file|project, got: {other}"
                ))
            }
        }
        .map_err(|err| format!("failed to serialize language report: {err}"))?;
        Ok(report)
    }
}

struct RipgrepTool {
    definition: ToolDefinition,
}

impl RipgrepTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "ripgrep",
                "Szeroki zwiad tekstowy: uruchamia ripgrep z kontekstem linii.",
            )
            .string_param(
                "pattern",
                "Wzorzec wyszukiwania przekazywany do ripgrep.",
                true,
            ),
        }
    }
}

impl LlmTool for RipgrepTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        run_ripgrep(&required_str(arguments, "pattern")?)
    }
}

struct AstCallsTool {
    definition: ToolDefinition,
}

impl AstCallsTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "ast_calls",
                "Skalpel AST: zwraca numery linii fizycznych wywołań metody (bez komentarzy i stringów).",
            )
            .string_param(
                "file",
                "Ścieżka do pliku źródłowego (np. .rs, .py, .java, .kt, .sql, .c, .cpp).",
                true,
            )
            .string_param("method", "Nazwa wywoływanej metody/funkcji.", true),
        }
    }
}

impl LlmTool for AstCallsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        let file = required_str(arguments, "file")?;
        let method = required_str(arguments, "method")?;
        let lines = AstUsageFinder::find_calls_in_file(Path::new(&file), &method)?;
        if lines.is_empty() {
            Ok("No call sites found.".to_string())
        } else {
            Ok(format!("Call sites at lines: {lines:?}"))
        }
    }
}

struct ReadFileTool {
    definition: ToolDefinition,
}

impl ReadFileTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "read_file",
                "Czyta wycinek pliku po numerach linii (1-based, włącznie).",
            )
            .string_param("file", "Ścieżka do pliku.", true)
            .integer_param("start", "Pierwsza linia (>= 1).", true)
            .integer_param("end", "Ostatnia linia (>= start).", true),
        }
    }
}

impl LlmTool for ReadFileTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        let file = required_str(arguments, "file")?;
        let start = required_usize(arguments, "start")?;
        let end = required_usize(arguments, "end")?;
        read_file_range(Path::new(&file), start, end)
    }
}

struct FinalizeTool {
    definition: ToolDefinition,
}

impl FinalizeTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::new(
                "finalize",
                "Kończy zwiad i zwraca skondensowany raport markdown.",
            )
            .string_param("report", "Finalny skondensowany raport markdown.", true),
        }
    }
}

impl LlmTool for FinalizeTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        required_str(arguments, "report")
    }

    fn is_terminal(&self) -> bool {
        true
    }
}

fn required_str(arguments: &Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("tool argument '{key}' must be a string"))
}

fn required_usize(arguments: &Value, key: &str) -> Result<usize, String> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("tool argument '{key}' must be a positive integer"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scout_tool_set_registers_all_tools() {
        let tools = scout_tool_set();
        let names: Vec<_> = tools
            .definitions()
            .into_iter()
            .map(|tool| tool.name.clone())
            .collect();

        assert_eq!(
            names,
            vec![
                "detect_language".to_string(),
                "ripgrep".to_string(),
                "ast_calls".to_string(),
                "read_file".to_string(),
                "finalize".to_string(),
            ]
        );
    }
}
