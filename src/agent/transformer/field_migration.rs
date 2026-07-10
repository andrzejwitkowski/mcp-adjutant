use std::path::{Component, Path, PathBuf};

use crate::cache::{mcp_workspace_root, resolve_workspace_path};

pub(crate) fn instruction_contains_field_migration(instruction: &str) -> bool {
    let lower = instruction.to_lowercase();
    lower.contains("headline")
        || lower.contains("subject")
        || lower.contains("source_module")
        || lower.contains("sourcemodule")
        || lower.contains("tags")
        || lower.contains("message")
        || lower.contains("summary")
}

pub(crate) enum ModuleIdStyle {
    /// `pkg.sub.module` — Python, C, C++, Zig
    Snake,
    /// `crate::mod::file` — Rust `source_module` strings
    RustPath,
    /// `com.example.Foo` — Java / Kotlin
    JavaPackage,
    /// `config-ui/ConfigApp` — TS/TSX under `src/`
    TsSlash,
}

pub(crate) fn infer_source_module(
    file_path: &Path,
    instruction: &str,
    style: ModuleIdStyle,
) -> Option<String> {
    if let Some(module) = explicit_module_from_instruction(instruction) {
        return Some(module);
    }

    let rel = workspace_relative(file_path);
    let parts = path_components(&rel);
    match style {
        ModuleIdStyle::Snake => infer_snake_module(&parts, &rel),
        ModuleIdStyle::RustPath => infer_rust_module(&parts, &rel),
        ModuleIdStyle::JavaPackage => infer_java_module(&parts, &rel),
        ModuleIdStyle::TsSlash => infer_ts_module(&parts, &rel),
    }
}

fn workspace_relative(path: &Path) -> PathBuf {
    let abs = resolve_workspace_path(path);
    abs.strip_prefix(mcp_workspace_root())
        .map(Path::to_path_buf)
        .unwrap_or(abs)
}

fn explicit_module_from_instruction(instruction: &str) -> Option<String> {
    for line in instruction.lines() {
        let lower = line.to_lowercase();
        if !(lower.contains("source_module")
            || lower.contains("sourcemodule")
            || lower.contains("module"))
        {
            continue;
        }
        if let Some((_, module)) = line.split_once('→').or_else(|| line.split_once("->")) {
            let module = module.trim().trim_matches(['"', '\'']);
            if !module.is_empty() && (module.contains('.') || module.contains('/')) {
                return Some(module.to_string());
            }
        }
    }
    None
}

fn path_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|part| match part {
            Component::Normal(name) => name.to_str().map(str::to_owned),
            _ => None,
        })
        .collect()
}

fn infer_rust_module(parts: &[String], file_path: &Path) -> Option<String> {
    let stem = file_path.file_stem()?.to_str()?;
    if let Some(idx) = parts.iter().position(|part| part == "src") {
        let mut parents: Vec<String> = parts[idx + 1..parts.len().saturating_sub(1)].to_vec();
        if parents.is_empty() && idx > 0 {
            parents.push(parts[idx - 1].clone());
        }
        return Some(join_sep(&parents, stem, "::"));
    }
    Some(stem.to_string())
}

fn infer_snake_module(parts: &[String], file_path: &Path) -> Option<String> {
    let stem = file_path.file_stem()?.to_str()?;
    if let Some(idx) = parts.iter().position(|part| part == "src") {
        let mut parents: Vec<String> = parts[idx + 1..parts.len().saturating_sub(1)].to_vec();
        if parents.is_empty() && idx > 0 {
            parents.push(parts[idx - 1].clone());
        }
        return Some(join_sep(&parents, stem, "."));
    }

    if let Some(idx) = parts.iter().position(|part| part == "scripts") {
        let parents = &parts[idx + 1..parts.len().saturating_sub(1)];
        return Some(join_sep(parents, stem, "."));
    }

    const SKIP: &[&str] = &["scripts", "test", "tests", "benches", "examples"];
    let mut start = 0usize;
    while start < parts.len().saturating_sub(1) && SKIP.contains(&parts[start].as_str()) {
        start += 1;
    }
    let parents = &parts[start..parts.len().saturating_sub(1)];
    Some(join_sep(parents, stem, "."))
}

fn infer_java_module(parts: &[String], file_path: &Path) -> Option<String> {
    let idx = parts
        .iter()
        .position(|part| part == "java" || part == "kotlin")?;
    let package = &parts[idx + 1..parts.len().saturating_sub(1)];
    let class_name = file_path.file_stem()?.to_str()?;
    Some(join_sep(package, class_name, "."))
}

fn infer_ts_module(parts: &[String], file_path: &Path) -> Option<String> {
    let idx = parts.iter().position(|part| part == "src")?;
    let mut segments: Vec<&str> = parts[idx + 1..parts.len().saturating_sub(1)]
        .iter()
        .map(String::as_str)
        .collect();
    if segments.first() == Some(&"modules") {
        segments.remove(0);
    }
    let stem = file_path.file_stem()?.to_str()?;
    Some(if segments.is_empty() {
        stem.to_string()
    } else {
        format!("{}/{}", segments.join("/"), stem)
    })
}

fn join_sep(parents: &[String], leaf: &str, sep: &str) -> String {
    if parents.is_empty() {
        leaf.to_string()
    } else {
        format!("{}{sep}{leaf}", parents.join(sep))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn rust_from_src_tree() {
        assert_eq!(
            infer_source_module(
                &PathBuf::from("src/agent/orchestrator.rs"),
                "add source_module",
                ModuleIdStyle::RustPath
            )
            .as_deref(),
            Some("agent::orchestrator")
        );
        assert_eq!(
            infer_source_module(
                &PathBuf::from("src/mcp/handlers.rs"),
                "add source_module",
                ModuleIdStyle::RustPath
            )
            .as_deref(),
            Some("mcp::handlers")
        );
        assert_eq!(
            infer_source_module(
                &PathBuf::from("src/jobs.rs"),
                "add source_module",
                ModuleIdStyle::RustPath
            )
            .as_deref(),
            Some("jobs")
        );
    }

    #[test]
    fn snake_from_src_tree() {
        let path = PathBuf::from("src/agent/transformer/foo.py");
        assert_eq!(
            infer_source_module(&path, "add source_module", ModuleIdStyle::Snake).as_deref(),
            Some("agent.transformer.foo")
        );
    }

    #[test]
    fn snake_from_lza_e2e_scripts() {
        let path = PathBuf::from("scripts/lza_e2e_py/block_lza.py");
        assert_eq!(
            infer_source_module(&path, "add source_module", ModuleIdStyle::Snake).as_deref(),
            Some("lza_e2e_py.block_lza")
        );
    }

    #[test]
    fn snake_from_absolute_lza_scripts_path() {
        let root = mcp_workspace_root();
        let path = root.join("scripts/lza_e2e_py/block_lza.py");
        assert_eq!(
            infer_source_module(&path, "add source_module", ModuleIdStyle::Snake).as_deref(),
            Some("lza_e2e_py.block_lza")
        );
    }

    #[test]
    fn snake_skips_scripts_prefix() {
        let path = PathBuf::from("scripts/sort_demo/bubble_sort.py");
        assert_eq!(
            infer_source_module(&path, "add source_module", ModuleIdStyle::Snake).as_deref(),
            Some("sort_demo.bubble_sort")
        );
    }

    #[test]
    fn java_from_maven_tree() {
        let path = PathBuf::from("src/main/java/com/acme/app/Handler.java");
        assert_eq!(
            infer_source_module(&path, "add sourceModule", ModuleIdStyle::JavaPackage).as_deref(),
            Some("com.acme.app.Handler")
        );
    }

    #[test]
    fn ts_from_frontend_src() {
        let path = PathBuf::from("frontend/src/modules/config-ui/ConfigApp.tsx");
        assert_eq!(
            infer_source_module(&path, "add sourceModule", ModuleIdStyle::TsSlash).as_deref(),
            Some("config-ui/ConfigApp")
        );
    }

    #[test]
    fn instruction_arrow_overrides_path() {
        let path = PathBuf::from("src/foo.py");
        assert_eq!(
            infer_source_module(
                &path,
                "add source_module\nsource_module -> my.module",
                ModuleIdStyle::Snake
            )
            .as_deref(),
            Some("my.module")
        );
    }
}
