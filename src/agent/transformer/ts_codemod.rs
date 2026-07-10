use std::path::Path;

use super::field_migration::{
    infer_source_module, instruction_contains_field_migration, ModuleIdStyle,
};

/// ponytail: line-based TS/TSX object literal codemod for field-rename instructions
pub fn try_ts_object_literal_codemod(
    snippet: &str,
    instruction: &str,
    file_path: &Path,
) -> Option<String> {
    if !instruction_contains_field_migration(instruction) {
        return None;
    }

    let source_module = infer_source_module(file_path, instruction, ModuleIdStyle::TsSlash)?;
    let mut has_source_module = snippet.contains("sourceModule:");
    let mut out = Vec::new();
    let mut changed = false;

    for line in snippet.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("tags:") || trimmed.starts_with("tags :") {
            changed = true;
            continue;
        }

        let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        let field_indent = format!("{indent}  ");
        let mut rewritten = line.to_string();

        if trimmed.starts_with("headline:") || trimmed.starts_with("headline :") {
            rewritten = format!(
                "{indent}subject:{}",
                trimmed.trim_start_matches("headline").trim_start_matches(':')
            );
            changed = true;
        } else if trimmed.starts_with("message:") || trimmed.starts_with("message :") {
            rewritten = format!(
                "{indent}summary:{}",
                trimmed.trim_start_matches("message").trim_start_matches(':')
            );
            changed = true;
        } else if trimmed.starts_with("sourceModule:") || trimmed.starts_with("sourceModule :") {
            rewritten = format!("{field_indent}sourceModule: '{source_module}',");
            changed = true;
            has_source_module = true;
        }

        if !has_source_module
            && (trimmed.starts_with("meta:") || trimmed.starts_with("meta :"))
            && trimmed.contains('{')
        {
            out.push(line.to_string());
            out.push(format!("{field_indent}sourceModule: '{source_module}',"));
            has_source_module = true;
            changed = true;
            continue;
        }

        out.push(rewritten);

        if !has_source_module
            && (trimmed.starts_with("correlationId:")
                || trimmed.starts_with("correlationId :")
                || trimmed.starts_with("correlationId,"))
        {
            out.insert(
                out.len() - 1,
                format!("{field_indent}sourceModule: '{source_module}',"),
            );
            has_source_module = true;
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
    fn renames_ui_notify_object_fields() {
        let snippet = "      emitUiNotify({\n        headline: {\n          component: 'config',\n          message: 'saved',\n        },\n        meta: {\n          tags: ['ui'],\n          correlationId: null,\n        },\n      })";
        let out = try_ts_object_literal_codemod(
            snippet,
            "headline->subject, message->summary, remove tags, add sourceModule",
            &PathBuf::from("frontend/src/modules/config-ui/ConfigApp.tsx"),
        )
        .expect("codemod");

        assert!(out.contains("subject:"));
        assert!(out.contains("summary:"));
        assert!(!out.contains("headline:"));
        assert!(!out.contains("message:"));
        assert!(!out.contains("tags:"));
        assert!(out.contains("sourceModule: 'config-ui/ConfigApp'"));
    }
}
