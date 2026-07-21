use std::path::{Path, PathBuf};

use crate::tools::{detect_file_language, SourceLanguage};

pub struct BuilderPromptParts {
    pub workflow: String,
    pub exemplar: String,
}

pub fn builder_task_parts(
    source_path: &Path,
    test_type: &str,
    source_file_path: &str,
    project_root: &Path,
) -> BuilderPromptParts {
    let language = detect_file_language(source_path)
        .map(|report| report.language)
        .unwrap_or(SourceLanguage::Unknown);

    match language {
        SourceLanguage::Rust => rust_parts(test_type, source_file_path, project_root),
        SourceLanguage::Tsx | SourceLanguage::TypeScript => {
            ts_parts(test_type, source_file_path, project_root)
        }
        SourceLanguage::Python => python_parts(test_type, source_file_path),
        _ => generic_parts(test_type, source_file_path, language),
    }
}

fn rust_parts(test_type: &str, source_file_path: &str, project_root: &Path) -> BuilderPromptParts {
    let stem = Path::new(source_file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module");
    let test_path = format!("tests/{stem}_integration_test.rs");
    BuilderPromptParts {
        workflow: format!(
            "Generate a `{test_type}` test for file: {source_file_path}\n\n\
             Source language: rust\n\
             Write ONE test function first to a **new** file `{test_path}` — never overwrite `tests/cache_manager_tests.rs`.\n\
             Workflow: write_test_suite(tdd_phase=red) then write_test_suite(tdd_phase=green). Job succeeds only when GREEN passes.\n\
             Use `mod common;` and helpers from `tests/common/mod.rs` — do not add new dev-dependencies.\n\
             Direct SQLite checks use `project_root.join(\".adjutant/cache.db\")` — never `cache.sqlite`.\n\
             Integration test crates cannot use `crate::` — import via `mcp_adjutant::...`.\n\
             Verify with `cargo test --test {stem}_integration_test`."
        ),
        exemplar: rust_integration_exemplar(project_root),
    }
}

fn ts_parts(test_type: &str, source_file_path: &str, project_root: &Path) -> BuilderPromptParts {
    let path = Path::new(source_file_path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("module");
    let parent = path.parent().unwrap_or(Path::new("frontend/src"));
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| *e == "ts" || *e == "tsx")
        .unwrap_or("tsx");
    let test_path = format!(
        "{}/{}.test.{ext}",
        parent.display().to_string().replace('\\', "/"),
        stem
    );
    let exemplar_path = project_root.join("frontend/src/modules/config-ui/AgentPhaseCard.test.tsx");
    let exemplar = std::fs::read_to_string(&exemplar_path)
        .map(|body| format!("Golden vitest pattern (copy structure):\n```tsx\n{body}\n```"))
        .unwrap_or_else(|_| {
            "Write vitest + @testing-library/react tests; run `npm test` from frontend/.".into()
        });
    BuilderPromptParts {
        workflow: format!(
            "Generate a `{test_type}` test for file: {source_file_path}\n\n\
             Source language: typescript/tsx — do NOT write Rust tests.\n\
             Write the test to `{test_path}` (co-located vitest + @testing-library/react).\n\
             Workflow: write_test_suite(tdd_phase=red) then write_test_suite(tdd_phase=green). Job succeeds only when GREEN passes.\n\
             Import the component under test from a relative path. Use `npm test` from the `frontend/` workspace."
        ),
        exemplar,
    }
}

fn python_parts(test_type: &str, source_file_path: &str) -> BuilderPromptParts {
    let stem = Path::new(source_file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module");
    BuilderPromptParts {
        workflow: format!(
            "Generate a `{test_type}` test for file: {source_file_path}\n\n\
             Source language: python\n\
             Write to `tests/test_{stem}.py`. Workflow: write_test_suite(red) then write_test_suite(green).\n\
             Verify with `pytest tests/test_{stem}.py`."
        ),
        exemplar: String::new(),
    }
}

fn generic_parts(
    test_type: &str,
    source_file_path: &str,
    language: SourceLanguage,
) -> BuilderPromptParts {
    BuilderPromptParts {
        workflow: format!(
            "Generate a `{test_type}` test for file: {source_file_path}\n\n\
             Source language: {}\n\
             Detect idiomatic test layout (detect_language, read_file) and write to a **new** test file matching that stack.\n\
             Workflow: write_test_suite(tdd_phase=red) then write_test_suite(tdd_phase=green). Job succeeds only when GREEN passes.",
            language.as_str()
        ),
        exemplar: String::new(),
    }
}

fn rust_integration_exemplar(project_root: &Path) -> String {
    let path = project_root.join("tests/cache_manager_tests.rs");
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let excerpt = if lines.len() > 47 {
        lines[6..47].join("\n")
    } else {
        content
    };
    format!(
        "Golden integration-test pattern (copy this setup — do not use tempfile):\n```rust\n{excerpt}\n```"
    )
}

pub fn validate_test_path_for_source(test_path: &str, source_path: &Path) -> Result<(), String> {
    let lang = detect_file_language(source_path)?.language;
    let ext = Path::new(test_path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let ok = match lang {
        SourceLanguage::Rust => ext == "rs",
        SourceLanguage::Tsx => ext == "tsx",
        SourceLanguage::TypeScript => ext == "ts" || ext == "tsx",
        SourceLanguage::Python => ext == "py",
        SourceLanguage::Java => ext == "java",
        SourceLanguage::Kotlin => ext == "kt" || ext == "kts",
        SourceLanguage::C | SourceLanguage::Cpp => ext == "c" || ext == "cc" || ext == "cpp",
        SourceLanguage::Sql | SourceLanguage::Unknown => true,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "test path `{test_path}` extension does not match source language `{}`",
            lang.as_str()
        ))
    }
}

pub fn source_file_from_builder_prompt(prompt: &str) -> Option<PathBuf> {
    prompt
        .lines()
        .find(|line| line.contains("for file:"))
        .and_then(|line| line.split("for file:").nth(1))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ts_parts_use_co_located_test_path() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let parts = ts_parts(
            "unit",
            "frontend/src/modules/config-ui/AgentPhaseCard.tsx",
            &root,
        );
        assert!(parts.workflow.contains("AgentPhaseCard.test.tsx"));
        assert!(parts.workflow.contains("do NOT write Rust"));
        assert!(parts.exemplar.contains("vitest"));
    }
}
