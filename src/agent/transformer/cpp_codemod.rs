use std::path::Path;

use super::field_migration::{
    infer_source_module, instruction_contains_field_migration, ModuleIdStyle,
};

pub fn try_cpp_call_codemod(
    snippet: &str,
    instruction: &str,
    file_path: &Path,
) -> Option<String> {
    if !instruction_contains_field_migration(instruction) {
        return None;
    }

    let source_module = infer_source_module(file_path, instruction, ModuleIdStyle::Snake)?;
    let mut has_source_module =
        snippet.contains("source_module =") || snippet.contains("source_module=");
    let mut out = Vec::new();
    let mut changed = false;

    for line in snippet.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(".tags =") || trimmed.starts_with(".tags=") {
            changed = true;
            continue;
        }

        let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        let field_indent = format!("{indent}    ");
        let mut rewritten = line.to_string();

        if trimmed.starts_with(".headline =") || trimmed.starts_with(".headline=") {
            rewritten = rewritten.replacen(".headline", ".subject", 1);
            changed = true;
        } else if trimmed.starts_with(".message =") || trimmed.starts_with(".message=") {
            rewritten = rewritten.replacen(".message", ".summary", 1);
            changed = true;
        } else if trimmed.starts_with(".source_module =") || trimmed.starts_with(".source_module=") {
            rewritten = format!("{field_indent}.source_module = \"{source_module}\",");
            changed = true;
            has_source_module = true;
        }

        if !has_source_module
            && (trimmed.starts_with(".meta =") || trimmed.contains("SortMeta{"))
        {
            out.push(line.to_string());
            continue;
        }

        out.push(rewritten);

        if !has_source_module
            && (trimmed.starts_with(".correlation_id =") || trimmed.starts_with(".correlation_id="))
        {
            out.insert(
                out.len() - 1,
                format!("{field_indent}.source_module = \"{source_module}\","),
            );
            has_source_module = true;
            changed = true;
        }
    }

    changed.then(|| out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn renames_designated_initializers() {
        let snippet = "    log_sort_event(SortEvent{\n        .headline = SortHeadline{\n            .component = \"bubble\",\n            .message = \"sorted\",\n        },\n        .meta = SortMeta{\n            .tags = \"demo\",\n            .correlation_id = std::nullopt,\n        },\n    });";
        let out = try_cpp_call_codemod(
            snippet,
            "rename headline to subject, message to summary, remove tags, add source_module",
            &PathBuf::from("scripts/sort_demo_cpp/src/bubble_sort.cpp"),
        )
        .expect("codemod");

        assert!(out.contains(".subject ="));
        assert!(out.contains(".summary ="));
        assert!(!out.contains(".headline ="));
        assert!(!out.contains(".tags ="));
        assert!(out.contains(".source_module = \"sort_demo_cpp.bubble_sort\""));
    }
}
