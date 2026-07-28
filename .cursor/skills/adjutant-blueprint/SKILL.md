---
name: adjutant-blueprint
description: >-
  Plan and apply feature/bugfix/refactor work via plan_blueprint then execute_blueprint.
  Use when implementing multi-file changes that should be surgical SEARCH/REPLACE pipelines,
  or when the user invokes /adjutant-blueprint.
disable-model-invocation: true
---

# adjutant-blueprint

## Role

You are the **coordinator** (premium agent). PlannerAgent emits a grounded Blueprint JSON; `execute_blueprint` applies it deterministically (patch/create → triage → Builder generate_tests). **Do not** hand-apply SEARCH/REPLACE hunks from a validated blueprint when `execute_blueprint` is available.

## When to use (first tools)

| Trigger | MCP tool (required) | Never substitute with |
| --- | --- | --- |
| Multi-step feature / bugfix / refactor needing a grounded patch pipeline | `plan_blueprint` then `execute_blueprint` | Premium Write/StrReplace of planned hunks; inventing patches without plan |
| Layout unknown before planning | `scout_context` **before** `plan_blueprint` | Grep chains, Task explore |
| `plan_kind: sync_types` | stop after plan; use [adjutant-transpiler](../adjutant-transpiler/SKILL.md) | `execute_blueprint` (rejects sync_types) |

## Mandatory pipeline (hard / medium)

1. **`scout_context`** — when layout or call sites are unknown (>1 file).
2. **`plan_blueprint`** — set `plan_kind` (`feature` \| `bugfix` \| `refactor`) and `expectation` (e.g. surgical patches only).
3. Poll `query_job_status` until `terminal=true` → strip `[ADJUTANT AUTO-EVAL APPENDIX…]` → confirm valid Blueprint JSON.
4. **`evaluate_agent_performance`** (`PlannerAgent`) — score ≥ 7 or retry plan (hard: polish until ≥ 7).
5. **`execute_blueprint`** — pass the JSON string as `blueprint`.
6. Poll until `terminal=true` → **`evaluate_agent_performance`** (`BlueprintExecutor`) — score ≥ 7.
7. Premium only fixes gaps triage/builder missed — **forbidden**: re-implementing successful `patch_file` / `create_file` steps by hand.

**Supervisor duty (premium):** after any `accepted` / `running` response, keep calling `query_job_status` in this turn until `terminal=true`. Sleep between polls is fine.

**FORBIDDEN:** end the turn with “planner/executor still running asynchronously” (or similar) while a blueprint `request_uuid` is non-terminal — that is fire-and-forget. Do not ask the user to wait; poll yourself.

## Args

**plan_blueprint**

```json
{
  "feature_request": "…",
  "plan_kind": "feature",
  "expectation": "surgical patches only — SEARCH/REPLACE wiring; create_file for new modules",
  "workspace_root": "/absolute/path",
  "request_uuid": "<uuid>"
}
```

**execute_blueprint**

```json
{
  "blueprint": "{ … Blueprint JSON … }",
  "workspace_root": "/absolute/path",
  "request_uuid": "<uuid>"
}
```

## Notes

- `generate_tests` goals must cite a non-test `path:line` for the source under test (executor resolves Builder from that citation).
- Failures roll back journaled writes.
- Always evaluate Planner + BlueprintExecutor outputs (medium/hard).
