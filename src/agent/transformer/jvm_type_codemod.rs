use std::fs;
use std::path::{Path, PathBuf};

use super::field_migration::instruction_contains_field_migration;
use crate::cache::resolve_workspace_path;
use crate::tools::edit_file_range;

pub fn find_jvm_log_type_files(scope: &Path) -> Vec<PathBuf> {
    let root = resolve_workspace_path(scope);
    let mut out = Vec::new();
    collect_jvm_log_files(&root, &mut out);
    out
}

fn collect_jvm_log_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jvm_log_files(&path, out);
            continue;
        }
        if is_jvm_log_type_file(&path) {
            out.push(path);
        }
    }
}

fn is_jvm_log_type_file(path: &Path) -> bool {
    let ext = path.extension().and_then(|v| v.to_str()).unwrap_or_default();
    if ext != "java" && ext != "kt" {
        return false;
    }
    let name = path.file_name().and_then(|v| v.to_str()).unwrap_or_default();
    name.ends_with("Log.java") || name.ends_with("Log.kt")
}

pub fn try_jvm_type_codemod(content: &str, instruction: &str) -> Option<String> {
    if !instruction_contains_field_migration(instruction) {
        return None;
    }

    let has_subject = builder_method_exists(content, "subject");
    let has_summary = builder_method_exists(content, "summary");
    let has_source_module = builder_method_exists(content, "sourceModule");

    let mut changed = false;
    let mut out = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if has_subject && is_builder_method(trimmed, "headline") {
            changed = true;
            continue;
        }
        if has_summary && is_builder_method(trimmed, "message") {
            changed = true;
            continue;
        }
        if has_source_module && is_builder_method(trimmed, "tags") {
            changed = true;
            continue;
        }

        let mut rewritten = line.to_string();
        if is_builder_method(trimmed, "headline") {
            rewritten = rewritten.replace("headline(", "subject(");
            changed = true;
        } else if is_builder_method(trimmed, "message") {
            rewritten = rewritten.replace("message(", "summary(");
            changed = true;
        } else if is_builder_method(trimmed, "tags") {
            rewritten = rewritten.replace("tags(", "sourceModule(");
            changed = true;
        }
        out.push(rewritten);
    }

    changed.then(|| out.join("\n"))
}

fn builder_method_exists(content: &str, name: &str) -> bool {
    content.contains(&format!("Builder {name}(")) || content.contains(&format!("fun {name}("))
}

fn is_builder_method(trimmed: &str, name: &str) -> bool {
    trimmed.contains(&format!("Builder {name}(")) || trimmed.contains(&format!("fun {name}("))
}

pub fn apply_jvm_type_codemod(path: &Path, instruction: &str) -> Result<bool, String> {
    let content = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let Some(new_content) = try_jvm_type_codemod(&content, instruction) else {
        return Ok(false);
    };
    if new_content == content {
        return Ok(false);
    }
    let line_count = content.lines().count().max(1);
    edit_file_range(path, 1, line_count, &new_content)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AST_LOG: &str = r#"package astdemo;

public final class AstLog {
    public static final class AstEvent {
        public static final class Builder {
            public Builder headline(AstHeadline headline) { return this; }
            public Builder subject(AstHeadline headline) { return this; }
        }
    }
    public static final class AstHeadline {
        public static final class Builder {
            public Builder message(String message) { return this; }
            public Builder summary(String message) { return this; }
        }
    }
    public static final class AstMeta {
        public static final class Builder {
            public Builder tags(String tags) { return this; }
            public Builder sourceModule(String tags) { return this; }
        }
    }
}
"#;

    #[test]
    fn drops_legacy_builder_aliases_when_new_names_exist() {
        let out = try_jvm_type_codemod(
            AST_LOG,
            "rename headline to subject, message to summary, remove tags, add sourceModule",
        )
        .expect("codemod");

        assert!(!out.contains("headline("));
        assert!(!out.contains("message("));
        assert!(!out.contains("tags("));
        assert!(out.contains("subject("));
        assert!(out.contains("summary("));
        assert!(out.contains("sourceModule("));
    }
}
