use std::path::Path;

use super::field_migration::{
    infer_source_module, instruction_contains_field_migration, ModuleIdStyle,
};

pub fn try_java_call_codemod(
    snippet: &str,
    instruction: &str,
    file_path: &Path,
) -> Option<String> {
    if !instruction_contains_field_migration(instruction) {
        return None;
    }

    let source_module = infer_source_module(file_path, instruction, ModuleIdStyle::JavaPackage)?;
    let mut has_source_module = snippet.contains(".sourceModule(");
    let mut out = Vec::new();
    let mut changed = false;

    for line in snippet.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(".tags(") {
            changed = true;
            continue;
        }

        let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        let field_indent = format!("{indent}    ");
        let mut rewritten = line.to_string();

        if trimmed.starts_with(".headline(") {
            rewritten = rewritten.replacen(".headline(", ".subject(", 1);
            changed = true;
        } else if trimmed.starts_with(".message(") {
            rewritten = rewritten.replacen(".message(", ".summary(", 1);
            changed = true;
        } else if trimmed.starts_with(".sourceModule(") {
            rewritten = format!("{field_indent}.sourceModule(\"{source_module}\")");
            changed = true;
            has_source_module = true;
        }

        if !has_source_module
            && (trimmed.starts_with(".meta(") || trimmed.contains("SortMeta.builder()"))
        {
            out.push(line.to_string());
            continue;
        }

        out.push(rewritten);

        if !has_source_module && trimmed.starts_with(".correlationId(") {
            out.insert(
                out.len() - 1,
                format!("{field_indent}.sourceModule(\"{source_module}\")"),
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
    fn renames_sort_event_builder_fields() {
        let snippet = "        logSortEvent(\n            SortEvent.builder()\n                .headline(\n                    SortHeadline.builder()\n                        .component(\"bubble\")\n                        .message(\"sorted\")\n                        .build())\n                .meta(\n                    SortMeta.builder()\n                        .tags(\"demo\")\n                        .correlationId(null)\n                        .build())\n                .build());";
        let out = try_java_call_codemod(
            snippet,
            "rename headline to subject, message to summary, remove tags, add sourceModule",
            &PathBuf::from("scripts/lza_e2e_java/src/main/java/lzademo/BlockLza.java"),
        )
        .expect("codemod");

        assert!(out.contains(".subject("));
        assert!(out.contains(".summary("));
        assert!(!out.contains(".headline("));
        assert!(!out.contains(".message("));
        assert!(!out.contains(".tags("));
        assert!(out.contains(".sourceModule(\"lzademo.BlockLza\")"));
    }
}
