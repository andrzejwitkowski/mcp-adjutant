use std::path::PathBuf;

use serde_json::Value;

use crate::cache::resolve_workspace_path;
use crate::mutation_journal::{assert_path_under_root, journaled_write};

use super::validate::{parse_hunks, Hunk};

/// Apply SEARCH/REPLACE hunks left-to-right. Each SEARCH must occur exactly once.
pub fn apply_hunks_to_body(body: &str, hunks: &[Hunk]) -> Result<String, String> {
    let mut out = body.replace("\r\n", "\n");
    for (i, hunk) in hunks.iter().enumerate() {
        let search = hunk.search.replace("\r\n", "\n");
        let replace = hunk.replace.replace("\r\n", "\n");
        if search.trim().is_empty() {
            return Err(format!("hunk[{i}]: empty SEARCH"));
        }
        let n = count_nonoverlapping(&out, &search);
        if n == 0 {
            let preview: String = search.chars().take(60).collect();
            return Err(format!("hunk[{i}]: SEARCH not found: {preview:?}"));
        }
        if n > 1 {
            return Err(format!(
                "hunk[{i}]: SEARCH matches {n} times (ambiguous) — narrow the anchor"
            ));
        }
        out = out.replacen(&search, &replace, 1);
    }
    Ok(out)
}

fn count_nonoverlapping(haystack: &str, needle: &str) -> usize {
    let mut count = 0;
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        count += 1;
        start += pos + needle.len();
    }
    count
}

/// Apply create_file/patch_file via journal. Caller must have validated the blueprint.
/// Skips generate_tests. Rejects sync_types. Returns absolute paths written.
pub fn apply_blueprint_file_steps(blueprint: &Value) -> Result<Vec<PathBuf>, String> {
    let pipeline = blueprint
        .get("pipeline")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing pipeline".to_string())?;

    reject_sync_types(pipeline)?;

    let root = crate::cache::mcp_workspace_root();
    let mut touched = Vec::new();

    for (idx, step) in pipeline.iter().enumerate() {
        let action = step
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if action == "generate_tests" {
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
        let abs = resolve_workspace_path(target);
        assert_path_under_root(&abs, &root)?;

        match action {
            "create_file" => {
                if abs.exists() {
                    return Err(format!(
                        "pipeline[{idx}]: create_file target already exists: {target}"
                    ));
                }
                journaled_write(&abs, patch.as_bytes())?;
            }
            "patch_file" => {
                let body = std::fs::read_to_string(&abs)
                    .map_err(|e| format!("pipeline[{idx}]: read {target}: {e}"))?;
                let hunks = parse_hunks(patch).map_err(|e| format!("pipeline[{idx}]: {e}"))?;
                let updated = apply_hunks_to_body(&body, &hunks)
                    .map_err(|e| format!("pipeline[{idx}]: {e}"))?;
                journaled_write(&abs, updated.as_bytes())?;
            }
            other => {
                return Err(format!(
                    "pipeline[{idx}]: unsupported action for apply: {other}"
                ));
            }
        }
        touched.push(abs);
    }
    Ok(touched)
}

fn reject_sync_types(pipeline: &[Value]) -> Result<(), String> {
    for (idx, step) in pipeline.iter().enumerate() {
        if step.get("action").and_then(Value::as_str) == Some("sync_types") {
            return Err(format!(
                "pipeline[{idx}]: sync_types is not executed by execute_blueprint — use transpile_types"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::with_thread_workspace_root;
    use crate::metrics::{with_job_context_async, JobContext};
    use crate::mutation_journal::{begin_job_journal, end_job_journal};
    use serde_json::json;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    // ponytail: serialize workspace env mutations across tests in this module
    static WORKSPACE_LOCK: Mutex<()> = Mutex::new(());

    fn temp_workspace(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("adjutant-apply-{label}-{nanos}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn apply_hunks_happy_path() {
        let body = "pub mod agent;\npub mod cache;\n";
        let hunks = parse_hunks(
            "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\npub mod limit;\n>>>>>>> REPLACE\n",
        )
        .unwrap();
        let out = apply_hunks_to_body(body, &hunks).unwrap();
        assert_eq!(out, "pub mod agent;\npub mod limit;\npub mod cache;\n");
    }

    #[test]
    fn apply_hunks_missing_search() {
        let hunks =
            parse_hunks("<<<<<<< SEARCH\nmissing;\n=======\nx;\n>>>>>>> REPLACE\n").unwrap();
        let err = apply_hunks_to_body("pub mod agent;\n", &hunks).unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn apply_hunks_ambiguous() {
        let body = "x\nx\n";
        let hunks = parse_hunks("<<<<<<< SEARCH\nx\n=======\ny\n>>>>>>> REPLACE\n").unwrap();
        let err = apply_hunks_to_body(body, &hunks).unwrap_err();
        assert!(err.contains("ambiguous"), "{err}");
    }

    #[test]
    fn create_file_refuse_if_exists() {
        let _g = WORKSPACE_LOCK.lock().unwrap();
        let dir = temp_workspace("refuse");
        let existing = dir.join("src/exists.rs");
        let lib = dir.join("src/lib.rs");
        std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
        std::fs::write(&existing, "old\n").unwrap();
        std::fs::write(&lib, "pub mod agent;\n").unwrap();

        with_thread_workspace_root(dir.clone(), || {
            let bp = json!({
                "task_id": "refuse-create",
                "architecture_summary": "Refuse.",
                "pipeline": [{
                    "step": 1,
                    "agent": "BuilderAgent",
                    "action": "create_file",
                    "target_file": "src/exists.rs",
                    "goal": "At exists.rs:1.",
                    "patch_content": "fn new() {}\n"
                }, {
                    "step": 2,
                    "agent": "BuilderAgent",
                    "action": "patch_file",
                    "target_file": "src/lib.rs",
                    "goal": "Wire at lib.rs:1.",
                    "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\npub mod exists;\n>>>>>>> REPLACE\n"
                }, {
                    "step": 3,
                    "agent": "BuilderAgent",
                    "action": "generate_tests",
                    "target_file": "tests/exists_test.rs",
                    "goal": "Smoke at src/exists.rs:1.",
                    "patch_content": ""
                }]
            });
            let err = apply_blueprint_file_steps(&bp).unwrap_err();
            assert!(err.contains("already exists"), "{err}");
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_patch_and_create_on_temp_workspace() {
        let _g = WORKSPACE_LOCK.lock().unwrap();
        let dir = temp_workspace("happy");
        let lib = dir.join("src/lib.rs");
        std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
        std::fs::write(&lib, "pub mod agent;\n").unwrap();

        with_thread_workspace_root(dir.clone(), || {
            let bp = json!({
                "task_id": "apply-mini",
                "architecture_summary": "Wire limit.",
                "pipeline": [{
                    "step": 1,
                    "agent": "BuilderAgent",
                    "action": "create_file",
                    "target_file": "src/limit.rs",
                    "goal": "New module; lib.rs:1 has no limit.",
                    "patch_content": "pub fn max() -> u32 { 60 }\n"
                }, {
                    "step": 2,
                    "agent": "BuilderAgent",
                    "action": "patch_file",
                    "target_file": "src/lib.rs",
                    "goal": "Declare limit at lib.rs:1.",
                    "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\npub mod limit;\n>>>>>>> REPLACE\n"
                }, {
                    "step": 3,
                    "agent": "BuilderAgent",
                    "action": "generate_tests",
                    "target_file": "tests/limit_test.rs",
                    "goal": "Test at src/limit.rs:1.",
                    "patch_content": ""
                }]
            });
            let paths = apply_blueprint_file_steps(&bp).expect("apply");
            assert_eq!(paths.len(), 2);
            assert_eq!(
                std::fs::read_to_string(dir.join("src/lib.rs")).unwrap(),
                "pub mod agent;\npub mod limit;\n"
            );
            assert_eq!(
                std::fs::read_to_string(dir.join("src/limit.rs")).unwrap(),
                "pub fn max() -> u32 { 60 }\n"
            );
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_types_rejected() {
        let bp = json!({
            "task_id": "sync-only",
            "architecture_summary": "Sync.",
            "pipeline": [{
                "step": 1,
                "agent": "TranspilerAgent",
                "action": "sync_types",
                "target_file": "frontend/src/types.ts",
                "goal": "Sync at src/domain.rs:1.",
                "patch_content": ""
            }]
        });
        let err = apply_blueprint_file_steps(&bp).unwrap_err();
        assert!(err.contains("sync_types"), "{err}");
    }

    #[tokio::test]
    async fn apply_under_journal_rolls_back_on_fail() {
        // JobContext.workspace_root drives mcp_workspace_root — no thread-local / WORKSPACE_LOCK
        let dir = temp_workspace("journal-rb");
        let lib = dir.join("src/lib.rs");
        std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
        std::fs::write(&lib, "pub mod agent;\n").unwrap();

        let job_id = format!(
            "apply-rb-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        with_job_context_async(
            JobContext {
                request_uuid: Some(job_id.clone()),
                mcp_tool: Some("execute_blueprint".into()),
                workspace_root: Some(dir.clone()),
            },
            || async {
                begin_job_journal(&job_id);
                let bp = json!({
                    "task_id": "journal-rb",
                    "architecture_summary": "Rollback.",
                    "pipeline": [{
                        "step": 1,
                        "agent": "BuilderAgent",
                        "action": "create_file",
                        "target_file": "src/limit.rs",
                        "goal": "New at src/limit.rs:1.",
                        "patch_content": "pub fn max() -> u32 { 60 }\n"
                    }, {
                        "step": 2,
                        "agent": "BuilderAgent",
                        "action": "patch_file",
                        "target_file": "src/lib.rs",
                        "goal": "Wire at src/lib.rs:1.",
                        "patch_content": "<<<<<<< SEARCH\npub mod agent;\n=======\npub mod agent;\npub mod limit;\n>>>>>>> REPLACE\n"
                    }]
                });
                apply_blueprint_file_steps(&bp).expect("apply");
                assert_eq!(
                    std::fs::read_to_string(&lib).unwrap(),
                    "pub mod agent;\npub mod limit;\n"
                );
                assert!(dir.join("src/limit.rs").is_file());
                end_job_journal(&job_id, false).expect("rollback");
                assert_eq!(std::fs::read_to_string(&lib).unwrap(), "pub mod agent;\n");
                assert!(!dir.join("src/limit.rs").exists());
            },
        )
        .await;

        let _ = std::fs::remove_dir_all(&dir);
    }
}
