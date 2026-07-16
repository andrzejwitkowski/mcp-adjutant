# Adjutant Backlog

---
### 🤖 AUDIT ENTRY: Per-request workspace_root
* **Date**: 2026-07-16
* **What I Did (Waste Analysis)**: Wired `workspace_root` through JobContext, `mcp_workspace_root()`, all MCP handlers/schemas, skill/docs, and `~/.cursor/mcp.json`. Reasoning waste: **Medium** — mechanical schema/handler plumbing dominated; core design was small.
* **The Delegation Gap**: Adding the same `workspace_root` schema property and `parse_workspace_root_arg` + `dispatch_async_job(..., workspace_root)` wiring across ~10 handlers is repetitive JSON/Rust glue a cheap model could do from a template after the first handler was done.
* **Proposed Sub-Agent / Skill**:
 * **Agent Name**: `McpSchemaParamPropagator`
 * **Required MCP Tools**: `[read_file, file_patch, grep]`
 * **Gemini Flash Lite System Prompt**:
 """
 You add one optional MCP tool argument across every registered tool schema and handler in mcp-adjutant.

 Inputs you will receive:
 - param_name (e.g. workspace_root)
 - schema property JSON fragment
 - parse helper function name already implemented
 - list of handler functions / schema functions to update

 Rules:
 1. Add the schema property via the shared helper call; do not duplicate descriptions.
 2. In each async handler: parse the arg once before dispatch_async_job; pass Option through dispatch.
 3. Never change query_job_status.
 4. Resolve relative paths inside the job future (after JobContext is scoped), not before dispatch.
 5. Keep evaluate's project_path as an alias if the param is workspace_root.
 6. Do not invent new abstractions; follow the first updated handler as the template.
 7. Run cargo check on touched files mentally: every dispatch_async_job call must have the new argument.
 """
