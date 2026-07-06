use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLanguage {
    Rust,
    TypeScript,
    Tsx,
    Python,
    Kotlin,
    Java,
    Sql,
    C,
    Cpp,
    Unknown,
}

impl SourceLanguage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Python => "python",
            Self::Kotlin => "kotlin",
            Self::Java => "java",
            Self::Sql => "sql",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileLanguageReport {
    pub path: String,
    pub language: SourceLanguage,
    pub method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectLanguageReport {
    pub root: String,
    pub markers: Vec<String>,
    pub file_counts: Vec<(SourceLanguage, u32)>,
    pub primary: Option<SourceLanguage>,
}

pub fn language_from_extension(ext: &str) -> Option<SourceLanguage> {
    match ext.to_ascii_lowercase().as_str() {
        "rs" => Some(SourceLanguage::Rust),
        "ts" => Some(SourceLanguage::TypeScript),
        "tsx" => Some(SourceLanguage::Tsx),
        "py" | "pyw" | "pyi" => Some(SourceLanguage::Python),
        "kt" | "kts" => Some(SourceLanguage::Kotlin),
        "java" => Some(SourceLanguage::Java),
        "sql" => Some(SourceLanguage::Sql),
        "c" => Some(SourceLanguage::C),
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" | "hh" => Some(SourceLanguage::Cpp),
        "h" => Some(SourceLanguage::C),
        _ => None,
    }
}

pub fn detect_file_language(path: &Path) -> Result<FileLanguageReport, String> {
    if !path.exists() {
        return Err(format!("path does not exist: {}", path.display()));
    }
    if path.is_dir() {
        return Err(format!(
            "expected a file path, got directory: {}",
            path.display()
        ));
    }

    if let Some(ext) = path.extension().and_then(|value| value.to_str()) {
        if let Some(mut language) = language_from_extension(ext) {
            let method = if ext.eq_ignore_ascii_case("h") {
                language = refine_header_language(path)?;
                "extension+content".to_string()
            } else {
                format!("extension:.{ext}")
            };

            return Ok(FileLanguageReport {
                path: path.display().to_string(),
                language,
                method,
            });
        }
    }

    let language = detect_language_from_content(path)?;
    Ok(FileLanguageReport {
        path: path.display().to_string(),
        language,
        method: "content".to_string(),
    })
}

pub fn detect_project_languages(root: &Path) -> Result<ProjectLanguageReport, String> {
    if !root.exists() {
        return Err(format!("path does not exist: {}", root.display()));
    }
    if !root.is_dir() {
        return Err(format!(
            "expected a directory path, got file: {}",
            root.display()
        ));
    }

    let markers = detect_project_markers(root);
    let file_counts = count_language_extensions(root, 6);
    let primary = file_counts
        .first()
        .map(|(language, _)| *language)
        .or_else(|| markers.first().map(|marker| marker_language(marker)));

    Ok(ProjectLanguageReport {
        root: root.display().to_string(),
        markers,
        file_counts,
        primary,
    })
}

fn refine_header_language(path: &Path) -> Result<SourceLanguage, String> {
    let sample = read_prefix(path, 8_192)?;
    if is_cpp_source(&sample) {
        Ok(SourceLanguage::Cpp)
    } else {
        Ok(SourceLanguage::C)
    }
}

fn detect_language_from_content(path: &Path) -> Result<SourceLanguage, String> {
    let sample = read_prefix(path, 8_192)?;

    if let Some(rest) = sample.strip_prefix("#!") {
        let shebang = rest.lines().next().unwrap_or_default().to_ascii_lowercase();
        if shebang.contains("python") {
            return Ok(SourceLanguage::Python);
        }
        if shebang.contains("kotlin") {
            return Ok(SourceLanguage::Kotlin);
        }
    }

    if sample.contains("fun main(") || sample.contains("fun main ") {
        return Ok(SourceLanguage::Kotlin);
    }

    if sample.contains("public class ") || sample.contains("public static void main") {
        return Ok(SourceLanguage::Java);
    }

    if sample.contains("def ") && sample.contains(':') {
        return Ok(SourceLanguage::Python);
    }

    if sample.contains("SELECT ") || sample.contains("CREATE TABLE ") {
        return Ok(SourceLanguage::Sql);
    }

    if is_cpp_source(&sample) {
        return Ok(SourceLanguage::Cpp);
    }

    if sample.contains("#include ") {
        return Ok(SourceLanguage::C);
    }

    if sample.contains("fn main(") || sample.contains("use std::") {
        return Ok(SourceLanguage::Rust);
    }

    Ok(SourceLanguage::Unknown)
}

fn is_cpp_source(sample: &str) -> bool {
    sample.contains("namespace ")
        || sample.contains("template<")
        || sample.contains("class ")
        || sample.contains("std::")
}

fn read_prefix(path: &Path, max_bytes: usize) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let prefix = &bytes[..bytes.len().min(max_bytes)];
    Ok(String::from_utf8_lossy(prefix).into_owned())
}

fn detect_project_markers(root: &Path) -> Vec<String> {
    const MARKERS: &[(&str, &[&str])] = &[
        ("rust", &["Cargo.toml", "Cargo.lock"]),
        (
            "typescript",
            &["package.json", "tsconfig.json", "pnpm-lock.yaml"],
        ),
        (
            "python",
            &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile"],
        ),
        ("kotlin", &["build.gradle.kts", "settings.gradle.kts"]),
        ("java", &["pom.xml", "build.gradle", "settings.gradle"]),
        ("sql", &["migrations", "schema.sql"]),
        ("cpp", &["CMakeLists.txt", "Makefile", "meson.build"]),
        ("c", &["configure.ac", "configure.in"]),
    ];

    let mut found = Vec::new();
    for (label, files) in MARKERS {
        for file in *files {
            let candidate = root.join(file);
            if candidate.exists() {
                found.push(format!("{label}:{file}"));
            }
        }
    }
    found
}

fn marker_language(marker: &str) -> SourceLanguage {
    match marker.split(':').next().unwrap_or_default() {
        "rust" => SourceLanguage::Rust,
        "typescript" => SourceLanguage::TypeScript,
        "python" => SourceLanguage::Python,
        "kotlin" => SourceLanguage::Kotlin,
        "java" => SourceLanguage::Java,
        "sql" => SourceLanguage::Sql,
        "c" => SourceLanguage::C,
        "cpp" => SourceLanguage::Cpp,
        _ => SourceLanguage::Unknown,
    }
}

fn count_language_extensions(root: &Path, max_depth: usize) -> Vec<(SourceLanguage, u32)> {
    let mut counts: HashMap<SourceLanguage, u32> = HashMap::new();
    walk_files(root, max_depth, &mut |path| {
        if let Some(ext) = path.extension().and_then(|value| value.to_str()) {
            if let Some(language) = language_from_extension(ext) {
                *counts.entry(language).or_insert(0) += 1;
            }
        }
    });

    let mut ranked: Vec<_> = counts.into_iter().collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| left.0.as_str().cmp(right.0.as_str()))
    });
    ranked
}

fn walk_files(root: &Path, max_depth: usize, visit: &mut dyn FnMut(&Path)) {
    fn walk(current: &Path, depth: usize, max_depth: usize, visit: &mut dyn FnMut(&Path)) {
        if depth > max_depth {
            return;
        }

        let entries = match fs::read_dir(current) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if should_skip(&path) {
                continue;
            }

            if path.is_dir() {
                walk(&path, depth + 1, max_depth, visit);
            } else if path.is_file() {
                visit(&path);
            }
        }
    }

    walk(root, 0, max_depth, visit);
}

fn should_skip(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".git" | "target" | "node_modules" | "dist" | "build" | ".cargo"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/scout")
            .join(name)
    }

    #[test]
    fn detects_language_from_extension() {
        let report = detect_file_language(&fixture("sample.py")).expect("python file");
        assert_eq!(report.language, SourceLanguage::Python);
        assert!(report.method.contains("extension"));
    }

    #[test]
    fn detects_cpp_header_by_content() {
        let report = detect_file_language(&fixture("sample.hpp")).expect("cpp header");
        assert_eq!(report.language, SourceLanguage::Cpp);
    }

    #[test]
    fn detects_project_markers_in_fixture_dir() {
        let root = fixture("project");
        let report = detect_project_languages(&root).expect("project scan");
        assert!(report
            .markers
            .iter()
            .any(|marker| marker.contains("Cargo.toml")));
        assert_eq!(report.primary, Some(SourceLanguage::Rust));
    }

    #[test]
    fn maps_c_project_marker_to_c_language() {
        let report = detect_project_languages(&fixture("project-c")).expect("c project scan");
        assert!(report.markers.iter().any(|marker| marker.starts_with("c:")));
        assert_eq!(report.primary, Some(SourceLanguage::C));
    }
}
