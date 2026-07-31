//! Forced step tools: cheap model fills fields; Rust builds Blueprint JSON.
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::planner::constraints::CoordinatorConstraints;
use crate::cache::resolve_workspace_path;
use crate::llm::{required_str, LlmTool, ToolDefinition};

use super::validate::{
    is_comment_sketch, is_kebab_case, path_line_from_goal, validate_blueprint,
    validate_blueprint_coordinator, HUNK_REPLACE_END, HUNK_SEARCH_START, HUNK_SEPARATOR,
    SURGICAL_MAX_NEW_LINES,
};

/// ponytail: create_file capped so plans stay diffs — large new modules still use tiny stubs + patch wiring.
pub(crate) const CREATE_FILE_MAX_LINES: usize = 15;

#[derive(Debug, Clone, Default)]
pub struct BlueprintDraft {
    begun: bool,
    task_id: String,
    architecture_summary: String,
    steps: Vec<DraftStep>,
}

#[derive(Debug, Clone)]
struct DraftStep {
    action: &'static str,
    target_file: String,
    goal: String,
    patch_content: String,
}

impl BlueprintDraft {
    pub fn begin(&mut self, task_id: String, architecture_summary: String) -> Result<(), String> {
        if !is_kebab_case(&task_id) {
            return Err(format!(
                "task_id must be kebab-case (lowercase, hyphen-separated), got: {task_id:?}"
            ));
        }
        if architecture_summary.trim().is_empty() {
            return Err("architecture_summary must not be empty".into());
        }
        self.begun = true;
        self.task_id = task_id;
        self.architecture_summary = architecture_summary;
        self.steps.clear();
        Ok(())
    }

    pub fn add_patch(
        &mut self,
        target_file: String,
        goal: String,
        search: String,
        replace: String,
    ) -> Result<String, String> {
        self.require_begun()?;
        if path_line_from_goal(&goal).is_none() {
            return Err("goal must cite path:line (e.g. src/foo.rs:42)".into());
        }
        let search = search.replace("\r\n", "\n");
        let replace = replace.replace("\r\n", "\n");
        if search.trim().is_empty() {
            return Err("search must be non-empty verbatim file text".into());
        }
        if count_nonempty_lines(&search) < 2 {
            return Err(
                "search must have ≥2 non-empty lines for uniqueness — add surrounding context"
                    .into(),
            );
        }
        if replace == search {
            return Err("replace identical to search — no-op edit".into());
        }
        let search_lines = count_nonempty_lines(&search);
        let replace_lines = count_nonempty_lines(&replace);
        if replace_lines > search_lines + SURGICAL_MAX_NEW_LINES {
            let extra = replace_lines.saturating_sub(search_lines);
            return Err(format!(
                "replace adds {extra} non-empty lines over {search_lines}-line search (max +{SURGICAL_MAX_NEW_LINES})"
            ));
        }
        if is_comment_sketch(&replace) || replace.contains("...") {
            return Err("replace contains ellipsis/placeholder — paste real code".into());
        }
        let abs = resolve_workspace_path(&target_file);
        let body = std::fs::read_to_string(&abs)
            .map_err(|err| format!("cannot read {target_file} to ground search: {err}"))?;
        let file = body.replace("\r\n", "\n");
        if !file.contains(&search) {
            let preview: String = search.chars().take(60).collect();
            return Err(format!(
                "search not found in {target_file} — copy verbatim from read_file/extract_search_anchor; got: {preview:?}"
            ));
        }
        let patch_content = wrap_hunk(&search, &replace);
        self.steps.push(DraftStep {
            action: "patch_file",
            target_file: target_file.clone(),
            goal,
            patch_content,
        });
        Ok(format!(
            "queued step {}: patch_file {target_file} ({} steps total)",
            self.steps.len(),
            self.steps.len()
        ))
    }

    pub fn add_create(
        &mut self,
        target_file: String,
        goal: String,
        contents: String,
    ) -> Result<String, String> {
        self.require_begun()?;
        if path_line_from_goal(&goal).is_none() {
            return Err("goal must cite path:line".into());
        }
        let lines = contents.lines().count();
        if lines == 0 || contents.trim().is_empty() {
            return Err("contents must be non-empty".into());
        }
        if lines > CREATE_FILE_MAX_LINES {
            return Err(format!(
                "create_file contents has {lines} lines (max {CREATE_FILE_MAX_LINES}) — use blueprint_add_patch on an existing file instead of dumping a full file"
            ));
        }
        if is_comment_sketch(&contents) || contents.contains("...") {
            return Err("contents contains ellipsis/placeholder".into());
        }
        let abs = resolve_workspace_path(&target_file);
        if abs.is_file() {
            return Err(format!(
                "{target_file} already exists — use blueprint_add_patch, not create"
            ));
        }
        self.steps.push(DraftStep {
            action: "create_file",
            target_file: target_file.clone(),
            goal,
            patch_content: contents,
        });
        Ok(format!(
            "queued step {}: create_file {target_file} ({} steps total)",
            self.steps.len(),
            self.steps.len()
        ))
    }

    pub fn add_tests(&mut self, target_file: String, goal: String) -> Result<String, String> {
        self.require_begun()?;
        if path_line_from_goal(&goal).is_none() {
            return Err(
                "goal must cite path:line (e.g. tests/foo_test.rs:1 or src/foo.rs:10)".into(),
            );
        }
        self.steps.push(DraftStep {
            action: "generate_tests",
            target_file: target_file.clone(),
            goal,
            patch_content: String::new(),
        });
        Ok(format!(
            "queued step {}: generate_tests {target_file} ({} steps total)",
            self.steps.len(),
            self.steps.len()
        ))
    }

    pub fn finalize(&self, coordinator: &CoordinatorConstraints) -> Result<String, String> {
        if !self.begun {
            return Err("call blueprint_begin first".into());
        }
        if self.steps.is_empty() {
            return Err(
                "pipeline empty — call blueprint_add_patch / add_create / add_tests".into(),
            );
        }
        let pipeline: Vec<Value> = self
            .steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                json!({
                    "step": i + 1,
                    "agent": "BuilderAgent",
                    "action": step.action,
                    "target_file": step.target_file,
                    "goal": step.goal,
                    "patch_content": step.patch_content,
                })
            })
            .collect();
        let blueprint = json!({
            "task_id": self.task_id,
            "architecture_summary": self.architecture_summary,
            "pipeline": pipeline,
        });
        let raw = serde_json::to_string(&blueprint)
            .map_err(|err| format!("serialize blueprint: {err}"))?;
        let validated =
            validate_blueprint(&raw).map_err(|err| format!("Blueprint rejected: {err}"))?;
        validate_blueprint_coordinator(&validated, coordinator)
            .map_err(|err| format!("Blueprint rejected: {err}"))?;
        serde_json::to_string_pretty(&validated)
            .map_err(|err| format!("re-serialize blueprint: {err}"))
    }

    fn require_begun(&self) -> Result<(), String> {
        if self.begun {
            Ok(())
        } else {
            Err("call blueprint_begin first".into())
        }
    }
}

pub(crate) fn wrap_hunk(search: &str, replace: &str) -> String {
    let search = search.trim_end_matches('\n');
    let replace = replace.trim_end_matches('\n');
    format!("{HUNK_SEARCH_START}\n{search}\n{HUNK_SEPARATOR}\n{replace}\n{HUNK_REPLACE_END}")
}

fn count_nonempty_lines(text: &str) -> usize {
    text.lines().filter(|line| !line.trim().is_empty()).count()
}

fn shared_draft() -> Arc<Mutex<BlueprintDraft>> {
    Arc::new(Mutex::new(BlueprintDraft::default()))
}

struct DraftTool {
    definition: ToolDefinition,
    draft: Arc<Mutex<BlueprintDraft>>,
    coordinator: CoordinatorConstraints,
    kind: DraftToolKind,
}

#[derive(Clone, Copy)]
enum DraftToolKind {
    Begin,
    AddPatch,
    AddCreate,
    AddTests,
    Finalize,
}

impl DraftTool {
    fn begin(draft: Arc<Mutex<BlueprintDraft>>, coordinator: CoordinatorConstraints) -> Self {
        Self {
            definition: ToolDefinition::new(
                "blueprint_begin",
                "Start a new Blueprint draft. Call once before add_* tools.",
            )
            .string_param("task_id", "kebab-case id (must contain a hyphen)", true)
            .string_param(
                "architecture_summary",
                "Brief approach citing path:line claims from scout",
                true,
            ),
            draft,
            coordinator,
            kind: DraftToolKind::Begin,
        }
    }

    fn add_patch(draft: Arc<Mutex<BlueprintDraft>>, coordinator: CoordinatorConstraints) -> Self {
        Self {
            definition: ToolDefinition::new(
                "blueprint_add_patch",
                "Queue a patch_file step. Pass raw search/replace text only — Rust wraps SEARCH/REPLACE markers.",
            )
            .string_param("target_file", "Repo-relative existing file", true)
            .string_param("goal", "Directive with path:line citation", true)
            .string_param(
                "search",
                "Verbatim ≥2-line SEARCH body (no <<<<<<< markers)",
                true,
            )
            .string_param(
                "replace",
                "REPLACE body (no markers); ≤15 extra non-empty lines over search",
                true,
            ),
            draft,
            coordinator,
            kind: DraftToolKind::AddPatch,
        }
    }

    fn add_create(draft: Arc<Mutex<BlueprintDraft>>, coordinator: CoordinatorConstraints) -> Self {
        Self {
            definition: ToolDefinition::new(
                "blueprint_add_create",
                "Queue a tiny create_file step (≤15 lines). Prefer blueprint_add_patch for existing files.",
            )
            .string_param("target_file", "Repo-relative new file path", true)
            .string_param("goal", "Directive with path:line citation", true)
            .string_param("contents", "Full new-file body (max 15 lines)", true),
            draft,
            coordinator,
            kind: DraftToolKind::AddCreate,
        }
    }

    fn add_tests(draft: Arc<Mutex<BlueprintDraft>>, coordinator: CoordinatorConstraints) -> Self {
        Self {
            definition: ToolDefinition::new(
                "blueprint_add_tests",
                "Queue generate_tests (usually final step). Builder writes the tests.",
            )
            .string_param("target_file", "Intended test file path", true)
            .string_param("goal", "Must cite path:line for source or test file", true),
            draft,
            coordinator,
            kind: DraftToolKind::AddTests,
        }
    }

    fn finalize(draft: Arc<Mutex<BlueprintDraft>>, coordinator: CoordinatorConstraints) -> Self {
        Self {
            definition: ToolDefinition::new(
                "blueprint_finalize",
                "Terminal: Rust assembles/validates Blueprint JSON from the draft.",
            ),
            draft,
            coordinator,
            kind: DraftToolKind::Finalize,
        }
    }

    fn lock_draft(&self) -> Result<std::sync::MutexGuard<'_, BlueprintDraft>, String> {
        self.draft
            .lock()
            .map_err(|_| "blueprint draft lock poisoned".into())
    }
}

impl LlmTool for DraftTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke(&self, arguments: &Value) -> Result<String, String> {
        match self.kind {
            DraftToolKind::Begin => {
                let task_id = required_str(arguments, "task_id")?;
                let summary = required_str(arguments, "architecture_summary")?;
                let mut draft = self.lock_draft()?;
                draft.begin(task_id.clone(), summary)?;
                Ok(format!("draft begun task_id={task_id}"))
            }
            DraftToolKind::AddPatch => {
                let mut draft = self.lock_draft()?;
                draft.add_patch(
                    required_str(arguments, "target_file")?,
                    required_str(arguments, "goal")?,
                    required_str(arguments, "search")?,
                    required_str(arguments, "replace")?,
                )
            }
            DraftToolKind::AddCreate => {
                let mut draft = self.lock_draft()?;
                draft.add_create(
                    required_str(arguments, "target_file")?,
                    required_str(arguments, "goal")?,
                    required_str(arguments, "contents")?,
                )
            }
            DraftToolKind::AddTests => {
                let mut draft = self.lock_draft()?;
                draft.add_tests(
                    required_str(arguments, "target_file")?,
                    required_str(arguments, "goal")?,
                )
            }
            DraftToolKind::Finalize => {
                let draft = self.lock_draft()?;
                draft.finalize(&self.coordinator)
            }
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self.kind, DraftToolKind::Finalize)
    }
}

/// Emit tool set: read_file + forced draft tools (no free-form JSON string).
pub fn draft_emit_tools(coordinator: CoordinatorConstraints) -> crate::llm::LlmToolSet {
    let draft = shared_draft();
    crate::llm::LlmToolSet::new()
        .register(crate::agent::read_only_tools::ReadFileTool::new())
        .register(DraftTool::begin(Arc::clone(&draft), coordinator.clone()))
        .register(DraftTool::add_patch(
            Arc::clone(&draft),
            coordinator.clone(),
        ))
        .register(DraftTool::add_create(
            Arc::clone(&draft),
            coordinator.clone(),
        ))
        .register(DraftTool::add_tests(
            Arc::clone(&draft),
            coordinator.clone(),
        ))
        .register(DraftTool::finalize(draft, coordinator))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_dir(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("planner-emit-{label}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn wrap_hunk_inserts_rust_markers() {
        let h = wrap_hunk("a\nb\n", "a\nc\n");
        assert!(h.starts_with(HUNK_SEARCH_START));
        assert!(h.contains(HUNK_SEPARATOR));
        assert!(h.ends_with(HUNK_REPLACE_END));
        assert!(!h.contains("<<<<<<< SEARCH\n<<<<<<<"));
    }

    #[test]
    fn begin_rejects_bad_task_id() {
        let mut d = BlueprintDraft::default();
        let err = d
            .begin("bad".into(), "summary with path".into())
            .unwrap_err();
        assert!(err.contains("kebab"), "{err}");
    }

    #[test]
    fn add_create_rejects_oversize() {
        let mut d = BlueprintDraft::default();
        d.begin(
            "fix-emit-tools".into(),
            "cap create at validate.rs:1".into(),
        )
        .unwrap();
        let big = (0..20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let err = d
            .add_create(
                "src/new_mod.rs".into(),
                "new mod at src/new_mod.rs:1".into(),
                big,
            )
            .unwrap_err();
        assert!(err.contains("max"), "{err}");
    }

    #[test]
    fn add_patch_rejects_one_line_search() {
        let dir = tmp_dir("one-line");
        let file = dir.join("f.rs");
        fs::write(&file, "fn a() {}\nfn b() {}\n").unwrap();
        let mut d = BlueprintDraft::default();
        d.begin("fix-one-line".into(), "f.rs:1".into()).unwrap();
        let err = d
            .add_patch(
                file.display().to_string(),
                "f.rs:1 tweak".into(),
                "fn a() {}".into(),
                "fn a() { 1 }".into(),
            )
            .unwrap_err();
        assert!(err.contains("≥2"), "{err}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn finalize_without_begin_fails() {
        let d = BlueprintDraft::default();
        let err = d.finalize(&CoordinatorConstraints::none()).unwrap_err();
        assert!(err.contains("blueprint_begin"), "{err}");
    }

    #[test]
    fn finalize_builds_valid_patch_pipeline() {
        let dir = tmp_dir("finalize");
        let file = dir.join("lib.rs");
        fs::write(&file, "fn hello() {\n    println!(\"hi\");\n}\n").unwrap();
        let mut d = BlueprintDraft::default();
        d.begin("add-bye".into(), "patch hello at lib.rs:1".into())
            .unwrap();
        d.add_patch(
            file.display().to_string(),
            "lib.rs:1 greet".into(),
            "fn hello() {\n    println!(\"hi\");\n}".into(),
            "fn hello() {\n    println!(\"bye\");\n}".into(),
        )
        .unwrap();
        d.add_tests("tests/t.rs".into(), "tests/t.rs:1 cover hello".into())
            .unwrap();
        let json = d.finalize(&CoordinatorConstraints::none()).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["pipeline"][0]["action"], "patch_file");
        assert_eq!(v["pipeline"][0]["agent"], "BuilderAgent");
        assert!(v["pipeline"][0]["patch_content"]
            .as_str()
            .unwrap()
            .contains(HUNK_SEARCH_START));
        assert_eq!(v["pipeline"][1]["action"], "generate_tests");
        assert_eq!(v["pipeline"][1]["agent"], "BuilderAgent");
        validate_blueprint(&json).expect("valid");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn draft_tools_share_state_through_finalize() {
        let dir = tmp_dir("share");
        let file = dir.join("x.rs");
        fs::write(&file, "a\nb\nc\n").unwrap();
        let draft = shared_draft();
        let c = CoordinatorConstraints::none();
        DraftTool::begin(Arc::clone(&draft), c.clone())
            .invoke(&json!({"task_id":"share-draft","architecture_summary":"x.rs:1"}))
            .unwrap();
        DraftTool::add_patch(Arc::clone(&draft), c.clone())
            .invoke(&json!({
                "target_file": file.display().to_string(),
                "goal": "x.rs:1 swap",
                "search": "a\nb",
                "replace": "a\nz"
            }))
            .unwrap();
        DraftTool::add_tests(Arc::clone(&draft), c.clone())
            .invoke(&json!({"target_file":"tests/x_test.rs","goal":"tests/x_test.rs:1"}))
            .unwrap();
        let out = DraftTool::finalize(draft, c).invoke(&json!({})).unwrap();
        assert!(out.contains("\"task_id\""));
        assert!(out.contains("share-draft"));
        let _ = fs::remove_dir_all(dir);
    }
}
