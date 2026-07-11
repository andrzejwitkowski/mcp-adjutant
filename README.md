# mcp-adjutant

An advanced [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) server that acts as an operational adjutant for premium LLMs. It offloads codebase scouting, context pruning, test generation, and compiler triage to cost-effective models — reducing context-window inflation and token spend in **Cursor**, **OpenCode**, **Codex**, and other MCP-capable clients.

## What it does

mcp-adjutant exposes seven MCP tools. Six run long-lived agent jobs asynchronously; the seventh polls job status.

| Tool | Purpose |
| --- | --- |
| `scout_context` | Autonomous code scouting — returns condensed markdown context for a query |
| `verify_and_triage` | Compile/type-check changed code and auto-fix trivial issues |
| `generate_tests_and_scaffolding` | Generate unit, integration, or factory tests for a source file |
| `web_fetch` | Fetch and condense authoritative web content for a search phrase |
| `execute_global_refactor` | Propagate rename/signature changes across files (scout + codemod + triage) |
| `evaluate_agent_performance` | QA another agent's output against the original task |
| `query_job_status` | Poll async jobs by `request_uuid` until `terminal=true` |

Heavy tools return immediately with a `request_uuid`. Your client (or you) must call `query_job_status` with that UUID until the job finishes. Do not guess timeouts — keep polling until `terminal=true`.

## Agent skill (delegation levels)

Bundled Cursor/agent skill: [`.cursor/skills/mcp-adjutant-delegation/SKILL.md`](.cursor/skills/mcp-adjutant-delegation/SKILL.md)

Instructs premium agents when and how to delegate work to mcp-adjutant sub-agents. Three usage levels:

| Level | Behavior |
| --- | --- |
| **low** | Delegate only when clearly cost-effective (broad scout, mechanical triage, boilerplate tests) |
| **medium** (default) | Start selective; use `evaluate_agent_performance` to adapt — delegate more when scores are high, self-serve when low |
| **hard** | MCP-first: scout, builder, triage, web_fetch, refactor as applicable; evaluate every sub-agent result |

Set the level via user instruction, or `MCP_ADJUTANT_DELEGATION_LEVEL=low|medium|hard`.

The skill requires **iterative refinement**: if a sub-agent result is weak, retry with better prompts built from the prior attempt and its critique — do not give up after one try. Same tool can be called multiple times to polish one task.

Copy `.cursor/skills/` into your project (or install globally under `~/.cursor/skills/`) so Cursor auto-discovers the skill once mcp-adjutant is connected.

## Prerequisites

| Requirement | Notes |
| --- | --- |
| **Rust 1.83+** | `cargo`, `rustc`, `rustfmt`, `clippy` |
| **Node.js 20+** | Required to build the config UI (`npm ci` in `frontend/`) |
| **Native build tools** | `build-essential`, `g++` (tree-sitter, tokenizers), `curl` |
| **Search utilities** | `fd-find` (`fd`), `ripgrep` (`rg`) — used by scout/triage agents |
| **LLM API access** | DeepSeek by default; OpenRouter, OpenAI, or any OpenAI-compatible endpoint also supported |

## Install from source

Clone the repository and build the release binary plus the config UI:

```bash
git clone https://github.com/andrzejwitkowski/mcp-adjutant.git
cd mcp-adjutant

# Embedding model used for semantic code search (~130 MB download)
bash scripts/download-embedding-fixtures.sh

# Config UI (served at http://127.0.0.1:3000 by default)
cd frontend && npm ci && npm run build && cd ..

# MCP server binary
cargo build --release --bin mcp-adjutant
```

The release binary is at:

```text
/path/to/mcp-adjutant/target/release/mcp-adjutant
```

> **Important:** Paths to embedding fixtures and the built config UI are resolved at **compile time** from the repository root. Keep the full checkout intact — do not copy only the binary elsewhere. If you relocate the repo, rebuild with `cargo build --release --bin mcp-adjutant`.

Verify the server starts (Ctrl+C to stop):

```bash
./target/release/mcp-adjutant
```

On startup it:

- Speaks MCP over **stdio** (for your IDE/CLI client)
- Serves the React **config UI** on `http://127.0.0.1:3000` (port from config)
- Writes logs to **stderr** only (stdout is reserved for MCP)

## Configure LLM providers

Before agents can run, add API keys for the phases you use.

### Option A — Web UI (recommended)

1. Start `mcp-adjutant` (or let your MCP client start it).
2. Open **http://127.0.0.1:3000** in a browser.
3. For each agent phase (Scout, Builder, Triage, Evaluator, …), pick a provider and enter the API key, base URL, and model.

Supported providers: **DeepSeek**, **OpenRouter**, **OpenAI**, and **Custom** (any OpenAI-compatible API).

### Option B — Edit `config.json` directly

Config file location (created automatically on first run):

```text
~/.config/mcp-adjutant/config.json
```

Override with the `MCP_ADJUTANT_CONFIG` environment variable.

Example snippet (DeepSeek for scouting):

```json
{
  "phases": {
    "scout": {
      "provider": "deep_seek",
      "api_key": "sk-…",
      "base_url": "https://api.deepseek.com/v1",
      "model_name": "deepseek-chat",
      "max_tokens": 4096,
      "temperature": 0.3
    }
  },
  "server_port": 3000,
  "storage_path": "~/.config/mcp-adjutant/config.json"
}
```

## Environment variables

| Variable | Default | Description |
| --- | --- | --- |
| `MCP_ADJUTANT_CONFIG` | `~/.config/mcp-adjutant/config.json` | Path to the JSON config file |
| `MCP_ADJUTANT_STATIC_DIR` | `<repo>/frontend/dist` | Built config UI assets |
| `RUST_LOG` | `mcp_adjutant=info` | Rust tracing filter (logs go to stderr) |

---

## Connect your MCP client

Replace `/path/to/mcp-adjutant` with the absolute path to your clone (the directory that contains `Cargo.toml`).

All examples assume you already ran the [install steps](#install-from-source) and the binary exists at `target/release/mcp-adjutant`.

### Cursor

Cursor reads MCP servers from:

- **Global:** `~/.cursor/mcp.json` — available in every project
- **Project:** `.cursor/mcp.json` in the workspace root — shareable via git (omit secrets)

Add this to `mcp.json`:

```json
{
  "mcpServers": {
    "mcp-adjutant": {
      "type": "stdio",
      "command": "/path/to/mcp-adjutant/target/release/mcp-adjutant",
      "env": {
        "MCP_ADJUTANT_CONFIG": "/home/you/.config/mcp-adjutant/config.json"
      }
    }
  }
}
```

**Steps:**

1. Create or edit `~/.cursor/mcp.json` (global) or `.cursor/mcp.json` (project).
2. Paste the configuration above with your real paths.
3. Reload Cursor: **Command Palette → Developer: Reload Window**.
4. Open **Settings → Tools & MCP** and confirm `mcp-adjutant` is connected with seven tools.
5. Open **http://127.0.0.1:3000** to enter API keys if you have not already.

Use `${workspaceFolder}` in paths for project-scoped config:

```json
{
  "mcpServers": {
    "mcp-adjutant": {
      "type": "stdio",
      "command": "/path/to/mcp-adjutant/target/release/mcp-adjutant",
      "env": {
        "MCP_ADJUTANT_CONFIG": "${workspaceFolder}/.mcp-adjutant/config.json"
      }
    }
  }
}
```

### OpenCode

OpenCode defines MCP servers under the `mcp` key in `opencode.json`:

- **Global:** `~/.config/opencode/opencode.json`
- **Project:** `opencode.json` in the project root

Add a **local** (stdio) server:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "mcp-adjutant": {
      "type": "local",
      "command": [
        "/path/to/mcp-adjutant/target/release/mcp-adjutant"
      ],
      "enabled": true,
      "timeout": 30000,
      "environment": {
        "MCP_ADJUTANT_CONFIG": "/home/you/.config/mcp-adjutant/config.json"
      }
    }
  }
}
```

**Steps:**

1. Install [OpenCode](https://opencode.ai/) if you have not already.
2. Add the block above to your `opencode.json`.
3. Restart OpenCode or start a new session so it spawns the server.
4. Confirm tools appear when you reference `mcp-adjutant` in a prompt.
5. Configure API keys at **http://127.0.0.1:3000**.

For per-project LLM config, point `MCP_ADJUTANT_CONFIG` at a file inside the repo and set `cwd` to your project root:

```json
{
  "mcp": {
    "mcp-adjutant": {
      "type": "local",
      "command": ["/path/to/mcp-adjutant/target/release/mcp-adjutant"],
      "cwd": "/path/to/your/project",
      "environment": {
        "MCP_ADJUTANT_CONFIG": "/path/to/your/project/.mcp-adjutant/config.json"
      }
    }
  }
}
```

### Codex (OpenAI)

Codex stores MCP configuration in TOML:

- **Global:** `~/.codex/config.toml`
- **Project:** `.codex/config.toml` (trusted projects only)

#### CLI (quickest)

```bash
codex mcp add mcp-adjutant \
  --env MCP_ADJUTANT_CONFIG=/home/you/.config/mcp-adjutant/config.json \
  -- /path/to/mcp-adjutant/target/release/mcp-adjutant
```

Verify:

```bash
codex mcp list
```

Inside a Codex TUI session, run `/mcp` to inspect connected servers and tools.

#### Manual `config.toml`

```toml
[mcp_servers.mcp-adjutant]
command = "/path/to/mcp-adjutant/target/release/mcp-adjutant"
enabled = true
startup_timeout_sec = 30
tool_timeout_sec = 300

[mcp_servers.mcp-adjutant.env]
MCP_ADJUTANT_CONFIG = "/home/you/.config/mcp-adjutant/config.json"
```

**Steps:**

1. Install [Codex CLI](https://developers.openai.com/codex) (CLI and IDE extension share this config).
2. Add the server via `codex mcp add` or edit `~/.codex/config.toml`.
3. Start Codex and run `/mcp` — `mcp-adjutant` should list seven tools.
4. Set API keys in the config UI at **http://127.0.0.1:3000**.

Increase `tool_timeout_sec` if scout or builder jobs run longer than five minutes.

---

## Using the tools

### Async job pattern

Every heavy tool requires a caller-generated `request_uuid` (use any UUID v4 string). The tool returns immediately; poll until done:

```text
1. Call scout_context (or verify_and_triage, etc.) with request_uuid
2. Call query_job_status with the same request_uuid
3. Repeat step 2 until terminal=true
4. Read result from the status response
```

Example `scout_context` arguments:

```json
{
  "query": "Where is JWT authentication implemented?",
  "request_uuid": "550e8400-e29b-41d4-a716-446655440000"
}
```

Example `query_job_status` response fields:

| Field | Meaning |
| --- | --- |
| `terminal` | `true` when the job finished (success or failure) |
| `status` | `queued`, `running`, `completed`, `failed`, or `stalled` |
| `result` | Output text when `status=completed` |
| `error` | Error message when `status=failed` |
| `possibly_stalled` | Advisory only — keep polling if `terminal=false` |

### Tool reference

**`scout_context`** — Autonomous repository scouting.

```json
{ "query": "How does caching work?", "request_uuid": "<uuid>" }
```

**`verify_and_triage`** — Run after code changes, before committing.

```json
{ "target_paths": ["src/agent/builder.rs"], "request_uuid": "<uuid>" }
```

Omit `target_paths` (or pass `[]`) to let the agent use `git status`.

**`generate_tests_and_scaffolding`** — Generate tests for a file.

```json
{
  "source_file_path": "src/agent/scout.rs",
  "test_type": "unit",
  "request_uuid": "<uuid>"
}
```

`test_type` must be `unit`, `integration`, or `factory`.

**`evaluate_agent_performance`** — QA another agent's work.

```json
{
  "target_agent": "Phase_1_Scout",
  "original_task": "Find all API route handlers",
  "received_output": "…paste scout report…",
  "request_uuid": "<uuid>"
}
```

---

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| No tools in client | Confirm the `command` path is absolute and executable; reload the client |
| Server exits immediately | Run the binary manually in a terminal — check stderr for errors |
| `model.onnx` / embedding errors | Run `bash scripts/download-embedding-fixtures.sh` from the repo root |
| Config UI 404 | Rebuild frontend: `cd frontend && npm run build` |
| Jobs fail with API errors | Open http://127.0.0.1:3000 and verify API keys and models |
| Scout/triage cannot find `fd`/`rg` | Install `fd-find` and `ripgrep`; ensure they are on `PATH` |
| Codex handshake timeout | Raise `startup_timeout_sec` to `30` or higher in `config.toml` |

Validate JSON configs before reloading:

```bash
python3 -m json.tool ~/.cursor/mcp.json
```

Test the binary outside your IDE:

```bash
/path/to/mcp-adjutant/target/release/mcp-adjutant
# Should stay running with no stdout output; config UI at :3000
```

---

## Development

Repository layout:

| Path | Purpose |
| --- | --- |
| `src/` | Rust MCP server, agents, tools |
| `frontend/src/modules/config-ui/` | React config UI |
| `frontend/dist/` | Built UI (served by the binary) |
| `tests/fixtures/embedding/` | Local embedding model (downloaded) |

**Run in development:**

```bash
cd frontend && npm ci && npm run build && cd ..
cargo run --bin mcp-adjutant
```

**Full CI check:**

```bash
bash scripts/download-embedding-fixtures.sh
cd frontend && npm ci && npm run lint && npm run build && cd ..
CXX=g++ cargo fmt -- --check
CXX=g++ cargo clippy --all-targets -- -D warnings
CXX=g++ cargo test --all-targets
```

See [AGENTS.md](AGENTS.md) for Cursor Cloud agent instructions.

## License

See [LICENSE](LICENSE).
