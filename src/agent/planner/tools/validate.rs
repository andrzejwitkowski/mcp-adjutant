use std::path::Path;

use serde_json::Value;

use crate::agent::planner::args::PlanKind;
use crate::agent::planner::constraints::CoordinatorConstraints;
use crate::cache::resolve_workspace_path;

/// ponytail: SEARCH/REPLACE hunk delimiters — markers the planner must emit in `patch_content`.
const HUNK_SEARCH_START: &str = "<<<<<<< SEARCH";
const HUNK_SEPARATOR: &str = "=======";
const HUNK_REPLACE_END: &str = ">>>>>>> REPLACE";

/// ponytail: REPLACE may add at most this many non-empty lines over SEARCH — guards against logic dumps.
const SURGICAL_MAX_NEW_LINES: usize = 15;

/// Pipeline agents — TriageAgent is never a blueprint step (triage runs downstream).
const ALLOWED_AGENTS: [&str; 2] = ["TranspilerAgent", "BuilderAgent"];

/// The allowed `action` values in a pipeline step.
const ALLOWED_ACTIONS: [&str; 4] = ["patch_file", "create_file", "sync_types", "generate_tests"];

/// First `{`…`}` span in `text` (ponytail: naive brace slice for LLM prose recovery).
pub fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

/// Validates that `raw` is a JSON object matching the Blueprint schema.
pub fn validate_blueprint(raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    let blueprint: Value =
        serde_json::from_str(trimmed).map_err(|err| format!("not valid JSON: {err}"))?;

    let obj = blueprint
        .as_object()
        .ok_or_else(|| "top-level value must be a JSON object".to_string())?;

    let task_id = obj
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing or non-string 'task_id'".to_string())?;
    if !is_kebab_case(task_id) {
        return Err(format!(
            "'task_id' must be kebab-case (lowercase, hyphen-separated), got: {task_id:?}"
        ));
    }

    let summary = obj
        .get("architecture_summary")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing or non-string 'architecture_summary'".to_string())?;
    if summary.trim().is_empty() {
        return Err("'architecture_summary' must not be empty".to_string());
    }

    let pipeline = obj
        .get("pipeline")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing or non-array 'pipeline'".to_string())?;
    if pipeline.is_empty() {
        return Err("'pipeline' must contain at least one step".to_string());
    }

    for (idx, step) in pipeline.iter().enumerate() {
        validate_step(idx, step)?;
    }

    validate_blueprint_completeness(&blueprint)?;
    validate_patch_hunks(pipeline)?;

    Ok(blueprint)
}

/// Coordinator plan_kind / expectation gates (skipped when no coordinator fields were set).
pub fn validate_blueprint_coordinator(
    blueprint: &Value,
    constraints: &CoordinatorConstraints,
) -> Result<(), String> {
    if constraints.is_default() {
        return Ok(());
    }

    let pipeline = blueprint
        .get("pipeline")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing pipeline".to_string())?;

    if let Some(kind) = constraints.plan_kind {
        validate_plan_kind_gates(kind, pipeline, constraints)?;
    }

    Ok(())
}

fn validate_plan_kind_gates(
    kind: PlanKind,
    pipeline: &[Value],
    constraints: &CoordinatorConstraints,
) -> Result<(), String> {
    match kind {
        PlanKind::SyncTypes => {
            for (idx, step) in pipeline.iter().enumerate() {
                let agent = step
                    .get("agent")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let action = step
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if agent != "TranspilerAgent" || action != "sync_types" {
                    return Err(format!(
                        "plan_kind sync_types: pipeline[{idx}] must be TranspilerAgent + sync_types only"
                    ));
                }
            }
        }
        PlanKind::Bugfix => {
            if pipeline.len() > 3 {
                return Err(format!(
                    "plan_kind bugfix: pipeline has {} steps (max 3)",
                    pipeline.len()
                ));
            }
            for (idx, step) in pipeline.iter().enumerate() {
                if step.get("action").and_then(Value::as_str) == Some("create_file") {
                    return Err(format!(
                        "plan_kind bugfix: pipeline[{idx}] must not use create_file"
                    ));
                }
            }
        }
        PlanKind::Refactor => {
            for (idx, step) in pipeline.iter().enumerate() {
                let action = step
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let target = step
                    .get("target_file")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if action == "create_file" && is_new_source_module(target) {
                    return Err(format!(
                        "plan_kind refactor: pipeline[{idx}] must not create_file a new source module"
                    ));
                }
            }
        }
        PlanKind::Feature => {
            if constraints.surgical_patches {
                let code_steps = pipeline
                    .iter()
                    .filter(|step| {
                        matches!(
                            step.get("action").and_then(Value::as_str),
                            Some("patch_file" | "create_file")
                        )
                    })
                    .count();
                if code_steps < 2 {
                    return Err(
                        "plan_kind feature with surgical expectation: need at least 2 patch_file/create_file steps"
                            .to_string(),
                    );
                }
            }
        }
    }
    Ok(())
}

/// Always-on: every `patch_file` step must use grounded SEARCH/REPLACE hunks.
pub fn validate_patch_hunks(pipeline: &[Value]) -> Result<(), String> {
    for (idx, step) in pipeline.iter().enumerate() {
        if step.get("action").and_then(Value::as_str) != Some("patch_file") {
            continue;
        }
        let target = step
            .get("target_file")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("pipeline[{idx}]: missing target_file"))?;
        let patch = step
            .get("patch_content")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let hunks = parse_hunks(patch).map_err(|err| format!("pipeline[{idx}]: {err}"))?;
        let abs = resolve_workspace_path(target);
        let file_body = std::fs::read_to_string(&abs)
            .map_err(|err| format!("pipeline[{idx}]: cannot read {target} for hunk gate: {err}"))?;

        validate_hunk_grounding(idx, target, &hunks, &file_body, step)?;
        validate_hunk_minimality(idx, target, &hunks)?;
    }
    Ok(())
}

/// A single SEARCH/REPLACE hunk parsed from `patch_content`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hunk {
    pub search: String,
    pub replace: String,
}

/// ponytail: line-based hunk parser — no AST, markers must sit at line start.
pub(crate) fn parse_hunks(patch: &str) -> Result<Vec<Hunk>, String> {
    let mut hunks = Vec::new();
    let mut lines = patch.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim_start() == HUNK_SEARCH_START || line == HUNK_SEARCH_START {
            let mut search = String::new();
            let mut sep_seen = false;
            for sline in lines.by_ref() {
                if sline == HUNK_SEPARATOR {
                    sep_seen = true;
                    break;
                }
                search.push_str(sline);
                search.push('\n');
            }
            if !sep_seen {
                return Err("SEARCH block missing '=======' separator".to_string());
            }
            let mut replace = String::new();
            let mut end_seen = false;
            for rline in lines.by_ref() {
                if rline == HUNK_REPLACE_END {
                    end_seen = true;
                    break;
                }
                replace.push_str(rline);
                replace.push('\n');
            }
            if !end_seen {
                return Err("REPLACE block missing '>>>>>>> REPLACE' terminator".to_string());
            }
            hunks.push(Hunk { search, replace });
        } else if line.trim().is_empty() {
            continue;
        } else if line == HUNK_SEPARATOR || line == HUNK_REPLACE_END {
            return Err(format!(
                "stray marker outside a SEARCH/REPLACE block: {line}"
            ));
        } else {
            return Err(format!(
                "patch_file content outside a SEARCH/REPLACE hunk: {line:?} — wrap all edits in <<<<<<< SEARCH / ======= / >>>>>>> REPLACE hunks"
            ));
        }
    }
    if hunks.is_empty() {
        return Err(
            "patch_file content has no SEARCH/REPLACE hunks — wrap edits in <<<<<<< SEARCH / ======= / >>>>>>> REPLACE"
                .to_string(),
        );
    }
    Ok(hunks)
}

/// Every SEARCH anchor must be a verbatim substring of the on-disk file.
fn validate_hunk_grounding(
    idx: usize,
    target: &str,
    hunks: &[Hunk],
    file_body: &str,
    step: &Value,
) -> Result<(), String> {
    for hunk in hunks {
        if hunk.search.trim().is_empty() {
            return Err(format!(
                "pipeline[{idx}]: empty SEARCH block in {target} — call read_file or extract_search_anchor on {target} and copy the anchor verbatim"
            ));
        }
        let file = file_body.replace("\r\n", "\n");
        let search = hunk.search.replace("\r\n", "\n");
        if !file.contains(&search) {
            let preview: String = hunk.search.chars().take(60).collect();
            let hint = grounding_fix_hint(step, target);
            return Err(format!(
                "pipeline[{idx}]: SEARCH block not found in {target} — anchor must be copied verbatim from read_file; got: {preview:?}.{hint}"
            ));
        }
    }
    Ok(())
}

fn grounding_fix_hint(step: &Value, target: &str) -> String {
    let goal = step.get("goal").and_then(Value::as_str).unwrap_or_default();
    if let Some(line) = goal_line_number(goal) {
        return format!(
            " Fix: call extract_search_anchor(file={target:?}, start={line}, end={line}) during scout and paste the returned SEARCH block."
        );
    }
    format!(" Fix: call extract_search_anchor on {target} with the line range from your goal.")
}

fn goal_line_number(goal: &str) -> Option<usize> {
    let idx = goal.find(':')?;
    let after = &goal[idx + 1..];
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok().filter(|n: &usize| *n >= 1)
}

/// REPLACE must differ from SEARCH and must not dump large new logic blocks.
fn validate_hunk_minimality(idx: usize, target: &str, hunks: &[Hunk]) -> Result<(), String> {
    for hunk in hunks {
        if hunk.replace == hunk.search {
            return Err(format!(
                "pipeline[{idx}]: REPLACE identical to SEARCH in {target} — no-op edit"
            ));
        }
        let search_lines = count_nonempty_lines(&hunk.search);
        let replace_lines = count_nonempty_lines(&hunk.replace);
        if replace_lines > search_lines + SURGICAL_MAX_NEW_LINES {
            let extra = replace_lines.saturating_sub(search_lines);
            return Err(format!(
                "pipeline[{idx}]: REPLACE adds {extra} non-empty lines over {search_lines}-line SEARCH in {target} (max +{SURGICAL_MAX_NEW_LINES}) — move the extra logic into a create_file step for a new module, keep patch_file as wiring-only hunks"
            ));
        }
        if search_lines >= 2
            && search_lines == replace_lines
            && count_preserved_search_lines(&hunk.search, &hunk.replace) == 0
        {
            return Err(format!(
                "pipeline[{idx}]: REPLACE rewrites every SEARCH line in {target} — keep at least one verbatim SEARCH line or use create_file for large changes"
            ));
        }
    }
    Ok(())
}

fn count_preserved_search_lines(search: &str, replace: &str) -> usize {
    search
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| replace.lines().any(|r| r.trim() == *line))
        .count()
}

fn count_nonempty_lines(text: &str) -> usize {
    text.lines().filter(|line| !line.trim().is_empty()).count()
}

/// Every `target_file` must match a path the planner read during scouting.
pub fn validate_blueprint_grounding(
    blueprint: &Value,
    touched: &[std::path::PathBuf],
) -> Result<(), String> {
    let pipeline = blueprint
        .get("pipeline")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing pipeline".to_string())?;

    if touched.is_empty() {
        return Err(
            "no files scouted — read_file every target_file before emit_blueprint".to_string(),
        );
    }

    for (idx, step) in pipeline.iter().enumerate() {
        let action = step
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if matches!(action, "create_file" | "generate_tests") {
            continue;
        }

        let target = step
            .get("target_file")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("pipeline[{idx}]: missing target_file"))?;

        if !touched.iter().any(|path| path_matches_target(path, target)) {
            return Err(format!(
                "pipeline[{idx}]: target_file {target:?} was not read — call read_file on it before emit_blueprint"
            ));
        }
    }

    Ok(())
}

/// Feature-blueprint quality gates beyond per-step schema checks.
fn validate_blueprint_completeness(blueprint: &Value) -> Result<(), String> {
    let pipeline = blueprint
        .get("pipeline")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing pipeline".to_string())?;

    for (idx, step) in pipeline.iter().enumerate() {
        let action = step
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if action == "generate_tests" {
            continue;
        }
        let goal = step.get("goal").and_then(Value::as_str).unwrap_or_default();
        if !goal_has_line_citation(goal) {
            return Err(format!(
                "pipeline[{idx}]: goal must cite path:line evidence (e.g. config_server.rs:35)"
            ));
        }
    }

    let actions: Vec<&str> = pipeline
        .iter()
        .filter_map(|step| step.get("action").and_then(Value::as_str))
        .collect();

    let has_code_change = actions
        .iter()
        .any(|action| matches!(*action, "patch_file" | "create_file"));
    let has_tests = actions.contains(&"generate_tests");
    if has_code_change && !has_tests {
        return Err(
            "pipeline must include a generate_tests step when patch_file or create_file are present"
                .to_string(),
        );
    }

    let creates_new_module = pipeline.iter().any(|step| {
        step.get("action").and_then(Value::as_str) == Some("create_file")
            && step
                .get("target_file")
                .and_then(Value::as_str)
                .is_some_and(is_new_source_module)
    });
    let patches_module_entry = pipeline.iter().any(|step| {
        step.get("action").and_then(Value::as_str) == Some("patch_file")
            && step
                .get("target_file")
                .and_then(Value::as_str)
                .is_some_and(is_module_entry_file)
    });
    if creates_new_module && !patches_module_entry {
        return Err(
            "pipeline with create_file for a new source module must include patch_file on the package entry (lib.rs, mod.rs, __init__.py, index.ts, …)"
                .to_string(),
        );
    }

    Ok(())
}

fn goal_has_line_citation(goal: &str) -> bool {
    goal.match_indices(':').any(|(idx, _)| {
        let after = &goal[idx + 1..];
        if !after.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
            return false;
        }
        let before = &goal[..idx];
        let path = before
            .rsplit_once(|ch: char| ch.is_whitespace() || ch == '(' || ch == ';')
            .map(|(_, tail)| tail)
            .unwrap_or(before);
        path.contains('.')
    })
}

fn is_dependency_manifest(path: &str) -> bool {
    matches!(
        path.rsplit('/').next().unwrap_or(path),
        "Cargo.toml" | "package.json" | "go.mod" | "pyproject.toml" | "Gemfile" | "pom.xml"
    )
}

fn is_module_entry_file(path: &str) -> bool {
    matches!(
        path.rsplit('/').next().unwrap_or(path),
        "lib.rs" | "mod.rs" | "__init__.py" | "index.ts" | "index.tsx" | "index.js"
    )
}

/// ponytail: test output convention — tests live under these dirs across stacks.
fn is_test_output_path(path: &str) -> bool {
    let norm = path.replace('\\', "/");
    ["/tests/", "/test/", "/__tests__/", "/spec/", "/specs/"]
        .iter()
        .any(|seg| norm.contains(seg))
        || norm.starts_with("tests/")
        || norm.starts_with("test/")
}

fn is_new_source_module(path: &str) -> bool {
    !is_dependency_manifest(path)
        && path.contains('.')
        && !path.ends_with(".md")
        && !path.ends_with(".json")
        && !path.contains("/tests/")
        && !is_module_entry_file(path)
}

fn path_matches_target(touched: &Path, target: &str) -> bool {
    resolve_workspace_path(target) == *touched
}

fn validate_step(idx: usize, step: &Value) -> Result<(), String> {
    let obj = step
        .as_object()
        .ok_or_else(|| format!("pipeline[{idx}] must be a JSON object"))?;

    let step_num = obj
        .get("step")
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("pipeline[{idx}]: missing or non-integer 'step'"))?;
    if step_num < 1 {
        return Err(format!(
            "pipeline[{idx}]: 'step' must be >= 1, got {step_num}"
        ));
    }

    let agent = obj
        .get("agent")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("pipeline[{idx}]: missing or non-string 'agent'"))?;
    if !ALLOWED_AGENTS.contains(&agent) {
        return Err(format!(
            "pipeline[{idx}]: 'agent' must be one of {ALLOWED_AGENTS:?} (TriageAgent is not a blueprint step), got {agent:?}"
        ));
    }

    let action = obj
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("pipeline[{idx}]: missing or non-string 'action'"))?;
    if !ALLOWED_ACTIONS.contains(&action) {
        return Err(format!(
            "pipeline[{idx}]: 'action' must be one of {ALLOWED_ACTIONS:?}, got {action:?}"
        ));
    }

    validate_agent_action_routing(idx, agent, action)?;

    let target_file = obj
        .get("target_file")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("pipeline[{idx}]: missing or non-string 'target_file'"))?;
    if target_file.trim().is_empty() {
        return Err(format!("pipeline[{idx}]: 'target_file' must not be empty"));
    }

    if action == "generate_tests" && !is_test_output_path(target_file) {
        return Err(format!(
            "pipeline[{idx}]: generate_tests target_file must be under tests/, test/, __tests__/, or spec/ — got {target_file:?}"
        ));
    }

    if !matches!(action, "create_file" | "generate_tests") {
        let abs = resolve_workspace_path(target_file);
        if !abs.is_file() {
            return Err(format!(
                "pipeline[{idx}]: target_file not found: {target_file}"
            ));
        }
    }

    let goal = obj
        .get("goal")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("pipeline[{idx}]: missing or non-string 'goal'"))?;
    if goal.trim().is_empty() {
        return Err(format!("pipeline[{idx}]: 'goal' must not be empty"));
    }

    if !obj.contains_key("patch_content") {
        return Err(format!(
            "pipeline[{idx}]: missing 'patch_content' (use empty string for sync_types/generate_tests)"
        ));
    }
    if !matches!(obj.get("patch_content"), Some(Value::String(_))) {
        return Err(format!("pipeline[{idx}]: 'patch_content' must be a string"));
    }

    let patch = obj
        .get("patch_content")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if matches!(action, "sync_types" | "generate_tests") && !patch.is_empty() {
        return Err(format!(
            "pipeline[{idx}]: 'patch_content' must be empty for action '{action}'"
        ));
    }

    if matches!(action, "patch_file" | "create_file") {
        if patch.trim().is_empty() {
            return Err(format!(
                "pipeline[{idx}]: 'patch_content' required for action '{action}'"
            ));
        }
        validate_patch_body_quality(idx, action, patch)?;
    }

    Ok(())
}

/// Quality checks (placeholder / ellipsis / comment-sketch) on patch bodies.
///
/// For SEARCH/REPLACE hunks (`patch_file`), checks run on each REPLACE body
/// individually — the SEARCH anchor is a verbatim disk copy that may legitimately
/// contain `..` ranges or `//` comments, so scanning it causes false rejections.
/// For `create_file` or non-hunk patches, checks run on the whole string.
fn validate_patch_body_quality(idx: usize, action: &str, patch: &str) -> Result<(), String> {
    if action == "patch_file" {
        let hunks = parse_hunks(patch).map_err(|err| {
            format!("pipeline[{idx}]: {err} — patch_file must use SEARCH/REPLACE hunks")
        })?;
        for hunk in &hunks {
            check_body_quality(idx, &hunk.replace)?;
        }
        return Ok(());
    }
    check_body_quality(idx, patch)
}

fn check_body_quality(idx: usize, body: &str) -> Result<(), String> {
    if contains_placeholder(body) {
        return Err(format!(
            "pipeline[{idx}]: patch body contains a placeholder — write full production code"
        ));
    }
    if is_comment_sketch(body) {
        return Err(format!(
            "pipeline[{idx}]: patch body is a comment sketch — write paste-ready production code"
        ));
    }
    if body.lines().any(is_ellipsis_sketch_line) {
        return Err(format!(
            "pipeline[{idx}]: patch body contains ellipsis sketch ('...')"
        ));
    }
    Ok(())
}

fn is_ellipsis_sketch_line(line: &str) -> bool {
    let t = line.trim();
    if t == "..." || t.ends_with(" ...") || t.starts_with("... ") {
        return true;
    }
    t.contains("{ ... }") || t.contains("{...}")
}

fn validate_agent_action_routing(idx: usize, agent: &str, action: &str) -> Result<(), String> {
    let ok = match action {
        "patch_file" | "create_file" | "generate_tests" => agent == "BuilderAgent",
        "sync_types" => agent == "TranspilerAgent",
        _ => false,
    };
    if ok {
        return Ok(());
    }
    Err(format!(
        "pipeline[{idx}]: agent '{agent}' cannot perform action '{action}' — use BuilderAgent for code/tests, TranspilerAgent for sync_types"
    ))
}

/// ponytail: >50% comment lines = sketch, not production code.
pub(crate) fn is_comment_sketch(patch: &str) -> bool {
    let lines: Vec<_> = patch.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return true;
    }
    let comment_count = lines
        .iter()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("//") || t.starts_with("#") || t.starts_with("/*")
        })
        .count();
    (comment_count as f32 / lines.len() as f32) > 0.5
}

fn contains_placeholder(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    const MARKERS: [&str; 11] = [
        "implement logic here",
        "todo:",
        "// todo",
        "... implement ...",
        "<your code here>",
        "// in run()",
        "// ...",
        "etc.",
        "<insert",
        "your code",
        "tbd",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

pub(crate) fn is_kebab_case(id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && id.contains('-')
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn is_ellipsis_sketch_line_allows_spread_syntax() {
        assert!(!is_ellipsis_sketch_line("return { ...obj };"));
        assert!(is_ellipsis_sketch_line("fn run() { ... }"));
        assert!(is_ellipsis_sketch_line("..."));
    }

    #[test]
    fn path_matches_target_requires_exact_workspace_path() {
        let touched = resolve_workspace_path("src/lib.rs");
        assert!(path_matches_target(&touched, "src/lib.rs"));
        assert!(!path_matches_target(&touched, "lib.rs"));
    }

    #[test]
    fn hunk_grounding_normalizes_crlf() {
        let file_body = "fn foo() {\r\n    let x = 1;\r\n}\r\n";
        let hunk = Hunk {
            search: "    let x = 1;\n".to_string(),
            replace: "    let x = 2;\n".to_string(),
        };
        let step = json!({});
        validate_hunk_grounding(0, "f.rs", &[hunk], file_body, &step).expect("crlf normalize");
    }

    #[test]
    fn hunk_minimality_rejects_wholesale_equal_size_rewrite() {
        let hunks = vec![Hunk {
            search: "line one\nline two\n".to_string(),
            replace: "alpha\nbeta\n".to_string(),
        }];
        let err = validate_hunk_minimality(0, "f.rs", &hunks).unwrap_err();
        assert!(err.contains("rewrites every SEARCH line"), "{err}");
    }

    #[test]
    fn hunk_minimality_allows_single_line_rewrite() {
        let hunks = vec![Hunk {
            search: "axum = \"0.7\"\n".to_string(),
            replace: "axum = { version = \"0.7\", features = [\"macros\"] }\n".to_string(),
        }];
        validate_hunk_minimality(0, "Cargo.toml", &hunks).expect("single-line ok");
    }
}
