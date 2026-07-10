use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodemodChange {
    pub line: usize,
    pub kind: &'static str,
    pub detail: String,
}

const MAX_SNIPPET_LINES: usize = 15;
const MAX_SNIPPET_CHARS: usize = 2_000;

pub fn summarize_snippet_diff(before: &str, after: &str, start_line: usize) -> Vec<CodemodChange> {
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    let mut changes = Vec::new();
    let max = before_lines.len().max(after_lines.len());

    for i in 0..max {
        let line_no = start_line + i;
        let b = before_lines.get(i).copied().unwrap_or("");
        let a = after_lines.get(i).copied().unwrap_or("");

        if b == a {
            continue;
        }

        if b.is_empty() {
            push_change(&mut changes, line_no, "add", detect_added_field(a));
            continue;
        }

        if a.is_empty() {
            push_change(&mut changes, line_no, "remove", detect_removed_field(b));
            continue;
        }

        if let Some(detail) = detect_rename(b, a) {
            push_change(&mut changes, line_no, "rename", Some(detail));
        } else if let Some(removed) = detect_removed_field(b) {
            push_change(&mut changes, line_no, "remove", Some(removed));
            push_change(&mut changes, line_no, "add", detect_added_field(a));
        } else if b != a {
            push_change(&mut changes, line_no, "rewrite", Some(truncate_line(b)));
        }
    }

    changes
}

pub fn format_change_report(
    path: &Path,
    start: usize,
    end: usize,
    changes: &[CodemodChange],
    after: &str,
) -> String {
    let mut out = format!("## Codemod: {} (lines {}-{})\n", path.display(), start, end);
    if changes.is_empty() {
        out.push_str("- (no line-level field diff detected)\n");
    } else {
        for change in changes {
            out.push_str(&format!(
                "- L{} {}: {}\n",
                change.line, change.kind, change.detail
            ));
        }
    }
    out.push_str("After:\n```\n");
    out.push_str(&truncate_snippet(after));
    out.push_str("\n```\n");
    out
}

pub fn verification_passed(report: &str) -> bool {
    report.is_empty() || !report.lines().any(|line| line.starts_with('✗'))
}

pub fn verify_field_migration(paths: &[PathBuf], instruction: &str) -> String {
    let rules = migration_rules(instruction, paths);
    if rules.is_empty() {
        return String::new();
    }

    let mut out = String::from("## Refactor verification\n");
    let total = paths.len();

    for rule in &rules {
        let mut hits = 0usize;
        for path in paths {
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            let present = content_matches_rule(&content, rule);
            if rule.must_be_present == present {
                hits += 1;
            }
        }
        let mark = if hits == total && total > 0 {
            "✓"
        } else {
            "✗"
        };
        out.push_str(&format!("{mark} {} {hits}/{total} files\n", rule.label));
    }

    out
}

struct MigrationRule {
    label: &'static str,
    pattern: &'static str,
    must_be_present: bool,
}

fn uses_java_field_style(paths: &[PathBuf]) -> bool {
    !paths.is_empty()
        && paths.iter().all(|path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("java") | Some("kt")
            )
        })
}

fn migration_rules(instruction: &str, paths: &[PathBuf]) -> Vec<MigrationRule> {
    let lower = instruction.to_lowercase();
    let mut rules = Vec::new();
    let mut ctx = MigrationRuleCtx {
        lower: &lower,
        rules: &mut rules,
        java_style: uses_java_field_style(paths),
    };

    ctx.mention(
        &["subject", "headline"],
        "subject",
        "subject",
        "subject:/subject= present in call sites",
        ".subject( present in call sites",
        true,
    );
    ctx.mention(
        &["summary", "message"],
        "summary",
        "summary",
        "summary:/summary= present in call sites",
        ".summary( present in call sites",
        true,
    );
    ctx.mention(
        &["source_module", "sourcemodule"],
        "source_module",
        "sourceModule",
        "source_module present in call sites",
        "sourceModule: present in call sites",
        true,
    );
    ctx.mention(
        &["tags"],
        "tags",
        "tags",
        "tags= absent in call sites",
        ".tags( absent in call sites",
        false,
    );

    rules
}

struct MigrationRuleCtx<'a> {
    lower: &'a str,
    rules: &'a mut Vec<MigrationRule>,
    java_style: bool,
}

impl MigrationRuleCtx<'_> {
    fn mention(
        &mut self,
        triggers: &[&str],
        pattern: &'static str,
        java_pattern: &'static str,
        label: &'static str,
        java_label: &'static str,
        must_be_present: bool,
    ) {
        if !triggers.iter().any(|trigger| self.lower.contains(trigger)) {
            return;
        }
        self.rules.push(MigrationRule {
            label: if self.java_style { java_label } else { label },
            pattern: if self.java_style {
                java_pattern
            } else {
                pattern
            },
            must_be_present,
        });
    }
}

fn content_matches_rule(content: &str, rule: &MigrationRule) -> bool {
    match rule.pattern {
        "subject" => {
            content.contains("subject:")
                || content.contains("subject=")
                || content.contains(".subject(")
                || content.contains(".subject =")
        }
        "summary" => {
            content.contains("summary:")
                || content.contains("summary=")
                || content.contains(".summary(")
                || content.contains(".summary =")
        }
        "source_module" => {
            content.contains("source_module=")
                || content.contains("source_module:")
                || content.contains(".source_module =")
        }
        "sourceModule" => content.contains("sourceModule:") || content.contains(".sourceModule("),
        "tags" => {
            content.contains("tags=")
                || content.contains("tags:")
                || content.contains(".tags(")
                || content.contains(".tags =")
        }
        _ => false,
    }
}

fn push_change(
    changes: &mut Vec<CodemodChange>,
    line: usize,
    kind: &'static str,
    detail: Option<String>,
) {
    if let Some(detail) = detail {
        changes.push(CodemodChange { line, kind, detail });
    }
}

fn detect_rename(before: &str, after: &str) -> Option<String> {
    const PAIRS: &[(&str, &str)] = &[
        ("headline=", "subject="),
        ("headline:", "subject:"),
        ("message=", "summary="),
        ("message:", "summary:"),
        ("headline =", "subject ="),
        ("message =", "summary ="),
        (".headline(", ".subject("),
        (".message(", ".summary("),
        (".headline =", ".subject ="),
        (".message =", ".summary ="),
    ];
    for (old, new) in PAIRS {
        if before.contains(old) && after.contains(new) {
            return Some(format!("{old}→{new}"));
        }
    }
    None
}

fn detect_removed_field(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with("tags=") || trimmed.starts_with("tags =") {
        return Some("tags=".to_string());
    }
    if trimmed.starts_with("tags:") || trimmed.starts_with("tags :") {
        return Some("tags:".to_string());
    }
    if trimmed.starts_with(".tags(") {
        return Some(".tags(".to_string());
    }
    if trimmed.starts_with(".tags =") || trimmed.starts_with(".tags=") {
        return Some(".tags =".to_string());
    }
    None
}

fn detect_added_field(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with("source_module=") || trimmed.starts_with("source_module =") {
        return Some(trimmed.to_string());
    }
    if trimmed.starts_with("source_module:") || trimmed.starts_with("source_module :") {
        return Some(truncate_line(trimmed));
    }
    if trimmed.starts_with("sourceModule:") || trimmed.starts_with("sourceModule :") {
        return Some(truncate_line(trimmed));
    }
    if trimmed.starts_with(".sourceModule(") {
        return Some(truncate_line(trimmed));
    }
    if trimmed.starts_with(".source_module =") || trimmed.starts_with(".source_module=") {
        return Some(truncate_line(trimmed));
    }
    None
}

fn truncate_line(line: &str) -> String {
    truncate_chars(line, 120)
}

fn truncate_snippet(snippet: &str) -> String {
    let lines: Vec<&str> = snippet.lines().take(MAX_SNIPPET_LINES).collect();
    truncate_chars(&lines.join("\n"), MAX_SNIPPET_CHARS)
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_python_field_renames() {
        let before = "    log_sort_event(\n        SortEvent(\n            headline=SortHeadline(\n                component=\"bubble\",\n                message=f\"sorted\",\n            ),\n            meta=SortMeta(\n                tags=\"sort_demo.bubble_sort\",\n                correlation_id=None,\n            ),\n        )\n    )";
        let after = "    log_sort_event(\n        SortEvent(\n            subject=SortHeadline(\n                component=\"bubble\",\n                summary=f\"sorted\",\n            ),\n            meta=SortMeta(\n                source_module=\"sort_demo.bubble_sort\",\n                correlation_id=None,\n            ),\n        )\n    )";
        let changes = summarize_snippet_diff(before, after, 11);
        assert!(changes
            .iter()
            .any(|c| c.kind == "rename" && c.detail.contains("headline")));
        assert!(changes
            .iter()
            .any(|c| c.kind == "rename" && c.detail.contains("message")));
        assert!(changes
            .iter()
            .any(|c| c.kind == "add" && c.detail.contains("source_module")));
    }

    #[test]
    fn verification_passed_rejects_failed_checks() {
        let report = "## Refactor verification\n✓ subject 4/4 files\n✗ tags 3/4 files\n";
        assert!(!verification_passed(report));
        assert!(verification_passed(
            "## Refactor verification\n✓ subject 4/4 files\n"
        ));
        assert!(verification_passed(""));
    }

    #[test]
    fn verification_counts_expected_fields() {
        let path = PathBuf::from("scripts/sort_demo/bubble_sort.py");
        if !path.exists() {
            return;
        }
        let report = verify_field_migration(
            &[path],
            "rename headline to subject, message to summary, remove tags, add source_module",
        );
        assert!(report.contains("Refactor verification"));
        assert!(report.contains("subject"));
    }
}
