use std::path::Path;

use super::field_migration::{
    infer_source_module, instruction_contains_field_migration, ModuleIdStyle,
};

/// ponytail: line-based Rust struct literal codemod for known field-rename instructions
pub fn try_rust_struct_literal_codemod(
    snippet: &str,
    instruction: &str,
    file_path: &Path,
) -> Option<String> {
    if !instruction_contains_field_migration(instruction) {
        return None;
    }

    let source_module = infer_source_module(file_path, instruction, ModuleIdStyle::RustPath)?;
    let has_source_module = snippet.contains("source_module:");
    let mut out = Vec::new();
    let mut changed = false;

    for line in snippet.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("tags:") || trimmed.starts_with("tags :") {
            changed = true;
            continue;
        }

        let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        let mut rewritten = line.to_string();

        if trimmed.starts_with("headline:") || trimmed.starts_with("headline :") {
            rewritten = format!("{indent}subject:{}", trimmed.trim_start_matches("headline").trim_start_matches(':'));
            changed = true;
        } else if trimmed.starts_with("message:") || trimmed.starts_with("message :") {
            rewritten = format!("{indent}summary:{}", trimmed.trim_start_matches("message").trim_start_matches(':'));
            changed = true;
        } else if trimmed.starts_with("source_module:") || trimmed.starts_with("source_module :") {
            rewritten = format!("{indent}source_module: \"{source_module}\".into(),");
            changed = true;
        }

        out.push(rewritten);

        if !has_source_module
            && (trimmed.starts_with("correlation_id:") || trimmed.starts_with("correlation_id :"))
        {
            out.insert(
                out.len() - 1,
                format!("{indent}source_module: \"{source_module}\".into(),"),
            );
            changed = true;
        }
    }

    if !changed {
        return None;
    }

    Some(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn renames_log_event_literal_fields() {
        let snippet = "            &LogEvent {\n                headline: LogHeadline {\n                    component: \"jobs\".into(),\n                    message: format!(\"queued\"),\n                },\n                meta: LogMeta {\n                    tags: vec![\"async\".into()],\n                    correlation_id: Some(id),\n                },\n            },";
        let out = try_rust_struct_literal_codemod(
            snippet,
            "headline->subject, message->summary, remove tags, add source_module",
            &PathBuf::from("src/jobs.rs"),
        )
        .expect("codemod");

        assert!(out.contains("subject:"));
        assert!(out.contains("summary:"));
        assert!(!out.contains("headline:"));
        assert!(!out.contains("message:"));
        assert!(!out.contains("tags:"));
        assert!(out.contains("source_module: \"jobs\".into()"));
    }

    #[test]
    fn infers_rust_module_path_from_src_tree() {
        let snippet = "                meta: LogMeta {\n                    tags: vec![\"async\".into()],\n                    correlation_id: None,\n                },";
        let out = try_rust_struct_literal_codemod(
            snippet,
            "remove tags, add source_module",
            &PathBuf::from("src/agent/orchestrator.rs"),
        )
        .expect("codemod");
        assert!(out.contains("source_module: \"agent::orchestrator\".into()"));
    }

    #[test]
    fn returns_none_for_unrelated_instruction() {
        let snippet = "config.validate();";
        assert!(try_rust_struct_literal_codemod(
            snippet,
            "Add true as argument",
            &PathBuf::from("src/a.rs"),
        )
        .is_none());
    }
}
