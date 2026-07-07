# mcp-adjutant

An advanced Model Context Protocol (MCP) server acting as an operational adjutant for premium LLMs. It autonomously offloads codebase scouting, context pruning, test generation, and compiler triage to cost-effective models, eliminating context window inflation and cutting token costs in Cursor, open code etc.

## Run

Build the config UI, then start the MCP server binary:

```bash
cd frontend && npm ci && npm run build
cargo build --release --bin mcp-adjutant
./target/release/mcp-adjutant
```

- MCP tools speak over **stdio** (for Cursor / Claude Code).
- The React config UI is served on `http://127.0.0.1:3000` (port from `AdjutantConfig.server_port`).

Environment:

- `MCP_ADJUTANT_CONFIG` — path to `config.json` (default: `~/.config/mcp-adjutant/config.json`)
- `MCP_ADJUTANT_STATIC_DIR` — path to built frontend (default: `frontend/dist`)

## Config UI module

`frontend/src/modules/config-ui/` — separate React module with `LlmClientCatalog`, listing all supported OpenAI-compatible clients (DeepSeek, OpenRouter, OpenAI, Custom) one after another per agent phase (Scout, Triage, Builder).
