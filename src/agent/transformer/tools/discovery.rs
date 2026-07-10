use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::targets::{RefactorTarget, TargetLineRange};
use crate::cache::{mcp_workspace_root, resolve_workspace_path};
use crate::tools::{
    detect_file_language, run_ripgrep_files, AstUsageFinder, LineRange, SourceLanguage,
};

pub fn find_refactor_targets(type_name: &str) -> Result<Vec<RefactorTarget>, String> {
    let pattern = format!(r"\b{}\b", regex_escape(type_name));
    let candidate_files = run_ripgrep_files(&pattern, &mcp_workspace_root())?;

    let mut by_file: BTreeMap<PathBuf, RefactorTarget> = BTreeMap::new();

    for file in candidate_files {
        let path = resolve_workspace_path(&file);
        if !is_production_src_file(&path)
            || is_skipped_refactor_file(&path)
            || !path.exists()
            || is_type_definition_file(&path, type_name)
        {
            continue;
        }

        let mut ranges =
            AstUsageFinder::find_construction_sites_in_file(&path, type_name).unwrap_or_default();
        let call_ranges = AstUsageFinder::find_call_expression_ranges_in_file(&path, type_name)
            .unwrap_or_default();
        for range in call_ranges {
            if !ranges
                .iter()
                .any(|existing| existing.start == range.start && existing.end == range.end)
            {
                ranges.push(range);
            }
        }

        let call_lines = AstUsageFinder::find_calls_in_file(&path, type_name).unwrap_or_default();

        for line in call_lines {
            if !ranges
                .iter()
                .any(|range| line >= range.start && line <= range.end)
            {
                ranges.push(LineRange {
                    start: line,
                    end: line,
                });
            }
        }

        if ranges.is_empty()
            && detect_file_language(&path)
                .map(|report| report.language == SourceLanguage::Unknown)
                .unwrap_or(true)
        {
            if let Ok(content) = std::fs::read_to_string(&path) {
                let lines: Vec<&str> = content.lines().collect();
                for (idx, line) in lines.iter().enumerate() {
                    if line_has_identifier(line, type_name) {
                        let range = if line.contains('{') {
                            expand_brace_range(&lines, idx)
                        } else {
                            LineRange {
                                start: idx + 1,
                                end: idx + 1,
                            }
                        };
                        ranges.push(range);
                    }
                }
            }
        }

        if ranges.is_empty() {
            continue;
        }

        drop_subset_ranges(&mut ranges);

        let entry = by_file
            .entry(path.clone())
            .or_insert_with(|| RefactorTarget {
                file_path: path,
                lines: Vec::new(),
                ranges: Vec::new(),
            });

        for range in ranges {
            let target_range = TargetLineRange {
                start: range.start,
                end: range.end,
            };
            if !entry.ranges.contains(&target_range) {
                entry.ranges.push(target_range);
            }
        }
    }

    Ok(by_file.into_values().collect())
}

/// ponytail: Python call+construction scans can emit both line 11 and 11-22; keep the superset only
fn drop_subset_ranges(ranges: &mut Vec<LineRange>) {
    let snapshot = ranges.clone();
    ranges.retain(|small| {
        !snapshot.iter().any(|big| {
            big.start <= small.start
                && big.end >= small.end
                && (big.start < small.start || big.end > small.end)
        })
    });
}

fn line_has_identifier(line: &str, ident: &str) -> bool {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|token| !token.is_empty())
        .any(|token| token == ident)
}

fn expand_brace_range(lines: &[&str], start_idx: usize) -> LineRange {
    let mut depth = 0i32;
    let mut started = false;
    for (idx, line) in lines.iter().enumerate().skip(start_idx) {
        for ch in line.chars() {
            if ch == '{' {
                depth += 1;
                started = true;
            } else if ch == '}' {
                depth -= 1;
            }
        }
        if started && depth <= 0 {
            return LineRange {
                start: start_idx + 1,
                end: idx + 1,
            };
        }
    }
    LineRange {
        start: start_idx + 1,
        end: lines.len(),
    }
}

fn is_production_src_file(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if normalized.contains("/tests/") || normalized.contains("/fixtures/") {
        return false;
    }
    if normalized.ends_with(".d.ts") {
        return false;
    }
    if normalized.ends_with(".rs") {
        if !normalized.contains("/src/main/java/") && !normalized.contains("/src/main/kotlin/") {
            if normalized.contains("/src/") || normalized.starts_with("src/") {
                return true;
            }
        }
    }
    if (normalized.contains("/frontend/src/") || normalized.starts_with("frontend/src/"))
        && (normalized.ends_with(".ts") || normalized.ends_with(".tsx"))
    {
        return true;
    }
    if (normalized.contains("/src/main/java/") && normalized.ends_with(".java"))
        || (normalized.contains("/src/main/kotlin/") && normalized.ends_with(".kt"))
    {
        return true;
    }
    if normalized.contains("/scripts/") || normalized.starts_with("scripts/") {
        return path.extension().is_some_and(|ext| {
            const EXTS: &[&str] = &[
                "py", "pyw", "java", "kt", "kts", "cpp", "cc", "cxx", "c", "h", "hpp", "zig",
            ];
            EXTS.iter().any(|want| ext.eq_ignore_ascii_case(want))
        });
    }
    false
}

fn is_skipped_refactor_file(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.contains("/tests/")
        || normalized.contains("/tests/fixtures/")
        || normalized.contains("/fixtures/")
        || normalized.contains("/frontend/dist/")
        || normalized.contains("/node_modules/")
}

fn is_type_definition_file(path: &Path, type_name: &str) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    content.contains(&format!("struct {type_name}"))
        || content.contains(&format!("enum {type_name}"))
        || content.contains(&format!("class {type_name}"))
        || content.contains(&format!("interface {type_name}"))
        || content.contains(&format!("export interface {type_name}"))
        || content.contains(&format!("function {type_name}"))
        || content.contains(&format!("export function {type_name}"))
        || content.contains(&format!("def {type_name}"))
        || content.contains(&format!("void {type_name}("))
        || content.contains(&format!("static void {type_name}("))
        || content.contains(&format!("fun {type_name}("))
        || content.contains(&format!("pub fn {type_name}("))
}

fn regex_escape(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch.to_string()
            } else {
                format!("\\{ch}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::transformer::tools::{filter_targets_by_scope, RefactorTarget};
    use crate::cache::resolve_workspace_path;

    #[test]
    fn is_production_src_file_accepts_known_layouts() {
        assert!(is_production_src_file(&PathBuf::from("src/jobs.rs")));
        assert!(is_production_src_file(&PathBuf::from(
            "frontend/src/modules/foo/Bar.tsx"
        )));
        assert!(is_production_src_file(&PathBuf::from(
            "scripts/demo_pkg/foo.py"
        )));
        assert!(is_production_src_file(&PathBuf::from(
            "scripts/demo_java/src/main/java/demo/Foo.java"
        )));
        assert!(!is_production_src_file(&PathBuf::from("src/tests/foo.rs")));
        assert!(!is_production_src_file(&PathBuf::from(
            "scripts/demo/readme.txt"
        )));
    }

    #[test]
    fn filter_targets_by_scope_keeps_only_scoped_paths() {
        let scope = resolve_workspace_path("scripts/pkg_a");
        let targets = vec![
            RefactorTarget {
                file_path: resolve_workspace_path("scripts/pkg_a/foo.py"),
                lines: vec![1],
                ranges: Vec::new(),
            },
            RefactorTarget {
                file_path: resolve_workspace_path("scripts/pkg_b/foo.py"),
                lines: vec![2],
                ranges: Vec::new(),
            },
        ];
        let scoped = filter_targets_by_scope(targets, &scope);
        assert_eq!(scoped.len(), 1);
        assert!(scoped[0].file_path.to_string_lossy().contains("pkg_a"));
    }

    #[test]
    fn find_refactor_targets_skips_tests_and_non_src() {
        let targets = find_refactor_targets("LogEvent").expect("scan");
        for target in &targets {
            let path = target.file_path.to_string_lossy().replace('\\', "/");
            assert!(
                !path.contains("/tests/"),
                "tests/ should be skipped: {path}"
            );
            assert!(
                !path.contains("/fixtures/"),
                "fixtures/ should be skipped: {path}"
            );
            assert!(path.contains("/src/"), "expected src/ path: {path}");
            assert!(target.file_path.exists(), "target must exist: {path}");
        }
    }

    #[test]
    fn find_refactor_targets_discovers_emit_ui_notify_in_frontend() {
        let targets = find_refactor_targets("emitUiNotify").expect("scan");
        let paths: Vec<_> = targets
            .iter()
            .map(|target| target.file_path.to_string_lossy().replace('\\', "/"))
            .collect();

        assert!(
            paths.iter().any(|path| path.contains("frontend/src/")),
            "expected frontend call sites in {paths:?}"
        );
        assert!(
            !paths.iter().any(|path| path.contains("uiLog.ts")),
            "definition file should be skipped: {paths:?}"
        );
        for target in &targets {
            assert!(
                !target.ranges.is_empty() || !target.lines.is_empty(),
                "expected range targets in {}",
                target.file_path.display()
            );
        }
    }

    #[test]
    fn expand_brace_range_covers_zig_call_site() {
        let lines = vec![
            "    sort_log.log_sort_event(sort_log.SortEvent{",
            "        .headline = sort_log.SortHeadline{",
            "            .component = \"bubble\",",
            "        },",
            "    });",
        ];
        let range = expand_brace_range(&lines, 0);
        assert_eq!(range.start, 1);
        assert_eq!(range.end, 5);
    }

    #[test]
    fn find_refactor_targets_skips_log_event_definition_file() {
        let targets = find_refactor_targets("LogEvent").expect("scan");
        assert!(
            !targets
                .iter()
                .any(|target| target.file_path.ends_with("log.rs")),
            "definition file should be skipped"
        );
    }
}
