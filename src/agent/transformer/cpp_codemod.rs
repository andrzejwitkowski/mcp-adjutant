use std::path::Path;

use super::field_migration::{
    infer_source_module, instruction_contains_field_migration, ModuleIdStyle,
};

pub fn try_cpp_call_codemod(snippet: &str, instruction: &str, file_path: &Path) -> Option<String> {
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

        if trimmed.contains(".headline =") || trimmed.contains(".headline=") {
            rewritten = rewritten
                .replace(".headline =", ".subject =")
                .replace(".headline=", ".subject=");
            changed = true;
        }
        if trimmed.contains(".message =") || trimmed.contains(".message=") {
            rewritten = rewritten
                .replace(".message =", ".summary =")
                .replace(".message=", ".summary=");
            changed = true;
        }
        if trimmed.contains(".source_module =") || trimmed.contains(".source_module=") {
            rewritten = format!("{field_indent}.source_module = \"{source_module}\",");
            changed = true;
            has_source_module = true;
        }
        if trimmed.contains(".tags =") || trimmed.contains(".tags=") {
            rewritten = strip_inline_tags_assign(&rewritten);
            changed = true;
        }

        if !has_source_module
            && (trimmed.starts_with(".meta =") || trimmed.starts_with(".meta="))
            && rewritten.contains("correlation_id")
        {
            if let Some(idx) = rewritten.find(".correlation_id") {
                rewritten = format!(
                    "{}{}.source_module = \"{source_module}\", {}",
                    &rewritten[..idx],
                    field_indent,
                    &rewritten[idx..]
                );
                has_source_module = true;
                changed = true;
            }
        }

        if !has_source_module
            && (trimmed.starts_with(".meta =") || trimmed.starts_with(".meta="))
            && !trimmed.contains("correlation_id")
        {
            out.push(rewritten);
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

fn strip_inline_tags_assign(line: &str) -> String {
    let Some(start) = line.find(".tags =").or_else(|| line.find(".tags=")) else {
        return line.to_string();
    };
    let rest = &line[start..];
    let comma = rest.find(',').map(|i| start + i + 1).unwrap_or(start);
    let mut out = format!("{}{}", &line[..start], &line[comma..]);
    while out.contains("  ") {
        out = out.replace("  ", " ");
    }
    out.replace(" { ,", " {").replace("{ ,", "{")
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
            &PathBuf::from("scripts/demo_cpp/src/widget.cpp"),
        )
        .expect("codemod");

        assert!(out.contains(".subject ="));
        assert!(out.contains(".summary ="));
        assert!(!out.contains(".headline ="));
        assert!(!out.contains(".tags ="));
        assert!(out.contains(".source_module = \"demo_cpp.widget\""));
    }

    #[test]
    fn renames_c_compound_literal_inline_fields() {
        let snippet = "    log_event(&(Event){\n        .headline = { .component = \"block\", .message = \"done\" },\n        .meta = { .tags = \"demo_c\", .correlation_id = NULL },\n    });";
        let out = try_cpp_call_codemod(
            snippet,
            "rename headline to subject, message to summary, remove tags, add source_module",
            &PathBuf::from("scripts/demo_c/src/widget.c"),
        )
        .expect("codemod");

        assert!(out.contains(".subject ="));
        assert!(out.contains(".summary ="));
        assert!(!out.contains(".headline ="));
        assert!(!out.contains(".message ="));
        assert!(!out.contains(".tags ="));
        assert!(out.contains(".source_module = \"demo_c.widget\""));
    }
}
