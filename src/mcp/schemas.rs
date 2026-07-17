use serde_json::{json, Value};

use crate::cache::workspace_root_schema_property;
use crate::jobs::{query_job_status_schema, request_uuid_schema_property};

pub const SCOUT_CONTEXT_TOOL_NAME: &str = "scout_context";
pub const VERIFY_AND_TRIAGE_TOOL_NAME: &str = "verify_and_triage";
pub const GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME: &str = "generate_tests_and_scaffolding";
pub const EXECUTE_GLOBAL_REFACTOR_TOOL_NAME: &str = "execute_global_refactor";
pub const EVALUATE_AGENT_PERFORMANCE_TOOL_NAME: &str = "evaluate_agent_performance";
pub const WEB_FETCH_TOOL_NAME: &str = "web_fetch";
pub const ANALYZE_LOG_TOOL_NAME: &str = "analyze_log";
pub const BABYSIT_PR_TOOL_NAME: &str = "babysit_pr";
pub const TRANSPILE_TYPES_TOOL_NAME: &str = "transpile_types";
pub const PLAN_BLUEPRINT_TOOL_NAME: &str = "plan_blueprint";

pub fn scout_context_schema() -> Value {
    json!({
        "name": SCOUT_CONTEXT_TOOL_NAME,
        "description": "Runs autonomous code scouting and returns condensed markdown context. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Question or scouting goal for the repository."
                },
                "force_refresh": {
                    "type": "boolean",
                    "description": "When true, bypass the semantic cache lookup and scout fresh. Successful runs are still stored in the cache."
                },
                "workspace_root": workspace_root_schema_property(),
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["query", "workspace_root", "request_uuid"]
        }
    })
}

pub fn verify_and_triage_schema() -> Value {
    json!({
        "name": VERIFY_AND_TRIAGE_TOOL_NAME,
        "description": "Runs compile/type error analysis and automatically fixes trivial issues. ALWAYS call after code changes before committing. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "target_paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Paths to check. If empty, the agent uses git status."
                },
                "workspace_root": workspace_root_schema_property(),
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["workspace_root", "request_uuid"]
        }
    })
}

pub fn generate_tests_and_scaffolding_schema() -> Value {
    json!({
        "name": GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
        "description": "Generates unit/integration tests and factories. Automatically verifies compilation via triage. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "source_file_path": { "type": "string" },
                "test_type": {
                    "type": "string",
                    "enum": ["unit", "integration", "factory"]
                },
                "workspace_root": workspace_root_schema_property(),
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["source_file_path", "test_type", "workspace_root", "request_uuid"]
        }
    })
}

pub fn execute_global_refactor_schema() -> Value {
    json!({
        "name": EXECUTE_GLOBAL_REFACTOR_TOOL_NAME,
        "description": "Call when changing a method signature, struct name, or propagating a type change across many files. Scout finds call sites; Triage verifies compilation. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "method_name": {
                    "type": "string",
                    "description": "Method or struct whose signature/call sites change."
                },
                "refactor_instruction": {
                    "type": "string",
                    "description": "What must change at each call site?"
                },
                "scope_path": {
                    "type": "string",
                    "description": "Optional directory scope; only files under this path are gathered, codemodded, verified, and triaged."
                },
                "workspace_root": workspace_root_schema_property(),
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["method_name", "refactor_instruction", "workspace_root", "request_uuid"]
        }
    })
}

pub fn evaluate_agent_performance_schema() -> Value {
    json!({
        "name": EVALUATE_AGENT_PERFORMANCE_TOOL_NAME,
        "description": "Evaluate the quality of a report or code produced by another agent (e.g. Scout or Builder). Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "target_agent": {
                    "type": "string",
                    "description": "Name of the agent you are evaluating (e.g. Phase_1_Scout, Phase_4_Builder, Phase_5_Triage, BabysitterAgent, PlannerAgent)."
                },
                "original_task": {
                    "type": "string",
                    "description": "What exactly did you expect from this agent?"
                },
                "received_output": {
                    "type": "string",
                    "description": "Paste the FULL query_job_status.result string verbatim — do not paraphrase into a one-line status. If longer than 8k chars, keep the last 8k (prefer from [TRIAGE RESULT] / Observation / evidence sections through the end)."
                },
                "workspace_root": workspace_root_schema_property(),
                "project_path": {
                    "type": "string",
                    "description": "Legacy alias for workspace_root."
                },
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["target_agent", "original_task", "received_output", "workspace_root", "request_uuid"]
        }
    })
}

pub fn web_fetch_schema() -> Value {
    json!({
        "name": WEB_FETCH_TOOL_NAME,
        "description": "Fetches the latest authoritative web content for a search phrase as compacted markdown. Works for any topic - documentation, news, specs, comparisons, code examples, or any web research. The agent searches via Brave Search API, fetches top result pages, and returns a condensed report. Results are cached semantically. Requires brave_api_key in web_fetcher config. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "search_phrase": {
                    "type": "string",
                    "description": "Topic or search phrase to research on the web."
                },
                "force_refresh": {
                    "type": "boolean",
                    "description": "When true, bypass the semantic cache lookup and fetch fresh web content. Successful runs are still stored in the cache."
                },
                "workspace_root": workspace_root_schema_property(),
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["search_phrase", "workspace_root", "request_uuid"]
        }
    })
}

pub fn analyze_log_schema() -> Value {
    json!({
        "name": ANALYZE_LOG_TOOL_NAME,
        "description": "Reads a log file or remote log source and triages the first root cause (what failed and where). Supports local paths, https:// URLs, and gh-run:<run_id> for GitHub Actions. ALWAYS call first when investigating logs, crash output, CI logs, or searching for errors in log files. Built-in parsers run first; cheap LLM fallback when needed. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "log_path": {
                    "type": "string",
                    "description": "Local workspace or absolute file path, https:// log URL, or gh-run:<run_id> for GitHub Actions failed-job logs."
                },
                "workspace_root": workspace_root_schema_property(),
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["log_path", "workspace_root", "request_uuid"]
        }
    })
}

pub fn babysit_pr_schema() -> Value {
    json!({
        "name": BABYSIT_PR_TOOL_NAME,
        "description": "Runs the BabysitterAgent loop (max 20 turns) to drive a GitHub PR toward mergeable state: CI green and actionable reviews fixed. Requires `gh` CLI, authenticated `gh auth login`, and local checkout on the PR head branch. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "pr_number": {
                    "type": "integer",
                    "description": "GitHub pull request number in the current repository."
                },
                "workspace_root": workspace_root_schema_property(),
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["pr_number", "workspace_root", "request_uuid"]
        }
    })
}

pub fn transpile_types_schema() -> Value {
    json!({
        "name": TRANSPILE_TYPES_TOOL_NAME,
        "description": "Sync API types/DTOs across languages via TranspilerAgent. Returns immediately; fetch via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "source_paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Source-language files containing API types/DTOs."
                },
                "target_path": {
                    "type": "string",
                    "description": "Primary target-language output file (created or overwritten)."
                },
                "architecture_layout": {
                    "type": "string",
                    "description": "Coordinator wish: idiom mapping, file layout, symbol grouping, re-export strategy, validation libs, wire-format naming."
                },
                "preserve_paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files the agent must not overwrite."
                },
                "verify_workspace": {
                    "type": "string",
                    "description": "Directory to run verify_command in (default: parent of target_path or repo root)."
                },
                "verify_command": {
                    "type": "string",
                    "description": "Optional verification shell command (e.g. npm run typecheck, cargo check, mypy pkg). Triage auto-discovers when omitted."
                },
                "workspace_root": workspace_root_schema_property(),
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["source_paths", "target_path", "architecture_layout", "workspace_root", "request_uuid"]
        }
    })
}

pub fn plan_blueprint_schema() -> Value {
    json!({
        "name": PLAN_BLUEPRINT_TOOL_NAME,
        "description": "Runs the Lead Architect (PlannerAgent): analyzes a high-level feature request or bug report, scouts the repo with read-only tools, and emits a strict Blueprint JSON pipeline for downstream sub-agents (Triage/Transpiler/Builder). Optional coordinator fields plan_kind and expectation steer pipeline shape and patch style. The returned JSON is a prompt contract — the server validates shape but does not execute it. Returns immediately; fetch the result via query_job_status.",
        "input_schema": {
            "type": "object",
            "properties": {
                "feature_request": {
                    "type": "string",
                    "description": "High-level feature request or bug report to design a solution for."
                },
                "plan_kind": {
                    "type": "string",
                    "enum": ["feature", "bugfix", "refactor", "sync_types"],
                    "description": "Expected blueprint shape. Coordinator sets this so the planner picks the right pipeline template."
                },
                "expectation": {
                    "type": "string",
                    "description": "Free-form coordinator constraints: patch style (surgical vs create_file), min steps, files/deps policy, test expectations."
                },
                "workspace_root": workspace_root_schema_property(),
                "request_uuid": request_uuid_schema_property()["request_uuid"]
            },
            "required": ["feature_request", "workspace_root", "request_uuid"]
        }
    })
}

pub fn registered_mcp_tools() -> Vec<Value> {
    vec![
        scout_context_schema(),
        verify_and_triage_schema(),
        generate_tests_and_scaffolding_schema(),
        execute_global_refactor_schema(),
        evaluate_agent_performance_schema(),
        web_fetch_schema(),
        analyze_log_schema(),
        babysit_pr_schema(),
        transpile_types_schema(),
        plan_blueprint_schema(),
        query_job_status_schema(),
    ]
}

#[cfg(test)]
mod plan_blueprint_schema_tests {
    use super::plan_blueprint_schema;

    #[test]
    fn schema_includes_coordinator_fields() {
        let schema = plan_blueprint_schema();
        let props = schema["input_schema"]["properties"]
            .as_object()
            .expect("properties");
        assert!(props.contains_key("plan_kind"));
        assert!(props.contains_key("expectation"));
        let kinds = props["plan_kind"]["enum"]
            .as_array()
            .expect("plan_kind enum");
        let values: Vec<_> = kinds.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(values, vec!["feature", "bugfix", "refactor", "sync_types"]);
    }
}

#[cfg(test)]
mod workspace_root_schema_tests {
    use super::registered_mcp_tools;
    use crate::jobs::QUERY_JOB_STATUS_TOOL_NAME;

    #[test]
    fn repo_tools_include_workspace_root_except_query_status() {
        for tool in registered_mcp_tools() {
            let name = tool["name"].as_str().expect("name");
            let props = tool["input_schema"]["properties"]
                .as_object()
                .expect("properties");
            if name == QUERY_JOB_STATUS_TOOL_NAME {
                assert!(
                    !props.contains_key("workspace_root"),
                    "{name} should not take workspace_root"
                );
            } else {
                assert!(
                    props.contains_key("workspace_root"),
                    "{name} missing workspace_root"
                );
                let required = tool["input_schema"]["required"]
                    .as_array()
                    .expect("required");
                let req: Vec<_> = required.iter().filter_map(|v| v.as_str()).collect();
                assert!(
                    req.contains(&"workspace_root"),
                    "{name} must require workspace_root, got {req:?}"
                );
            }
        }
    }
}
