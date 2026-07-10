use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::cache::resolve_workspace_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetLineRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefactorTarget {
    pub file_path: PathBuf,
    pub lines: Vec<usize>,
    pub ranges: Vec<TargetLineRange>,
}

pub fn parse_method_name(arguments: &Value) -> Result<String, String> {
    required_str(arguments, "method_name")
}

pub fn parse_refactor_targets_json(raw: &str) -> Result<Vec<RefactorTarget>, String> {
    let items: Vec<Value> = serde_json::from_str(raw)
        .map_err(|err| format!("refactor_targets_json must be a JSON array: {err}"))?;

    items.into_iter().map(parse_refactor_target_item).collect()
}

pub fn parse_apply_structural_codemod_arguments(
    arguments: &Value,
) -> Result<(String, Vec<RefactorTarget>), String> {
    let rule = required_str(arguments, "transformation_rule")?;
    let raw = required_str(arguments, "refactor_targets_json")?;
    let targets = parse_refactor_targets_json(&raw)?;
    if targets.is_empty() {
        return Err("refactor_targets_json must contain at least one target".to_string());
    }
    Ok((rule, targets))
}

pub fn filter_targets_by_scope(targets: Vec<RefactorTarget>, scope: &Path) -> Vec<RefactorTarget> {
    targets
        .into_iter()
        .filter(|target| path_under_scope(&target.file_path, scope))
        .collect()
}

pub fn path_under_scope(file: &Path, scope: &Path) -> bool {
    let file = file.to_string_lossy().replace('\\', "/");
    let scope = scope
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    file == scope || file.starts_with(&format!("{scope}/"))
}

pub fn format_refactor_targets_block(targets: &[RefactorTarget]) -> String {
    let payload = serde_json::to_string(targets).unwrap_or_else(|_| "[]".to_string());
    format!("```refactor_targets\n{payload}\n```")
}

pub(crate) fn required_str(arguments: &Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("tool argument '{key}' must be a string"))
}

fn parse_refactor_target_item(item: Value) -> Result<RefactorTarget, String> {
    let file_path = item
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| "each target must have file_path (string)".to_string())?;

    let mut lines = Vec::new();
    if let Some(lines_value) = item.get("lines").and_then(Value::as_array) {
        for line in lines_value {
            lines.push(parse_line_number(line)?);
        }
    }

    let mut ranges = Vec::new();
    if let Some(ranges_value) = item.get("ranges").and_then(Value::as_array) {
        for range in ranges_value {
            ranges.push(parse_target_range(range)?);
        }
    }

    if lines.is_empty() && ranges.is_empty() {
        return Err("each target must list lines and/or ranges".to_string());
    }

    Ok(RefactorTarget {
        file_path: resolve_workspace_path(file_path),
        lines,
        ranges,
    })
}

fn parse_line_number(line: &Value) -> Result<usize, String> {
    let line_no = line
        .as_u64()
        .ok_or_else(|| "lines must contain only integers".to_string())?;
    if line_no == 0 {
        return Err("line numbers must be >= 1".to_string());
    }
    Ok(line_no as usize)
}

fn parse_target_range(range: &Value) -> Result<TargetLineRange, String> {
    let start = range
        .get("start")
        .ok_or_else(|| "each range must have start".to_string())
        .and_then(parse_line_number)?;
    let end = range
        .get("end")
        .ok_or_else(|| "each range must have end".to_string())
        .and_then(parse_line_number)?;
    if end < start {
        return Err(format!("range end {end} must be >= start {start}"));
    }
    Ok(TargetLineRange { start, end })
}

impl serde::Serialize for RefactorTarget {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("RefactorTarget", 3)?;
        state.serialize_field("file_path", &self.file_path.to_string_lossy().to_string())?;
        if !self.lines.is_empty() {
            state.serialize_field("lines", &self.lines)?;
        }
        if !self.ranges.is_empty() {
            state.serialize_field("ranges", &self.ranges)?;
        }
        state.end()
    }
}

impl serde::Serialize for TargetLineRange {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("TargetLineRange", 2)?;
        state.serialize_field("start", &self.start)?;
        state.serialize_field("end", &self.end)?;
        state.end()
    }
}
