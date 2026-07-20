---
name: adjutant-git-janitor
description: >-
  Prepare commit messages, PR titles/bodies, and changelog entries via mcp-adjutant
  GitJanitorAgent (prepare_git_copy). Also gate wrong/stale/default branches and create
  feature branches (create_git_branch). Use before any git commit, push, gh pr create,
  changelog writing, or when user invokes /adjutant-git-janitor.
---

# adjutant-git-janitor

## Role

Coordinate GitJanitorAgent through MCP. Do **not** invent commit/PR/changelog text when `prepare_git_copy` is available. Do **not** run `git checkout -b` via Shell when `create_git_branch` is available.

## Hard rules

1. **Before** drafting a commit message, running `git commit`, writing a PR title/body, `gh pr create`, pushing with new commits, or writing a changelog entry → call `prepare_git_copy` with `workspace_root` + `request_uuid` (+ optional `feature_context` / `expected_ticket` / `user_instructions`).
2. Poll `query_job_status` until `terminal=true`.
3. If `action_required` is `create_branch` or `commit_allowed` is `false` → **STOP**. Call `create_git_branch` with `suggested_branch_name` (or a better name). Poll. Then **ALWAYS** `evaluate_agent_performance` (`target_agent`: `GitJanitorAgent`). Re-run `prepare_git_copy` until `commit_allowed: true`.
4. Use returned `commit_message` / `pr_title` / `pr_body` / `changelog_entry`.
5. On pre-commit / commit-msg / commitlint failure → re-call `prepare_git_copy` with `mode=refine_from_hooks` and `hook_failure_output` (set `persist_conventions=true` when rules should stick).
6. **ALWAYS** after every `prepare_git_copy` and every `create_git_branch` → `evaluate_agent_performance` with `target_agent: GitJanitorAgent` and the **full** `query_job_status.result` verbatim. Score &lt; 7 → refine and retry. Evaluator uses a **create_git_branch** rubric for branch JSON (`branch`/`status`/`previous`) and a **prepare** rubric for commit/PR JSON — do not expect prepare fields from `create_git_branch`.

## Tool args (quick)

```json
{
  "workspace_root": "/absolute/project",
  "request_uuid": "<uuid>",
  "feature_context": "optional: what this change is",
  "expected_ticket": "optional: WATT-402",
  "persist_conventions": false,
  "mode": "generate"
}
```

```json
{
  "branch_name": "feat/WATT-402-slug",
  "workspace_root": "/absolute/project",
  "request_uuid": "<uuid>"
}
```

## Persistence

Default: propose `.adjutant.toml` only (`suggested_adjutant_toml`). Write disk only with `persist_conventions=true` or `mode=update_conventions`.
