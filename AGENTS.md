# mcp-adjutant

An advanced Model Context Protocol (MCP) server intended to act as an operational
adjutant for premium LLMs, offloading codebase scouting, context pruning, test
generation, and compiler triage to cost-effective models.

## Cursor Cloud specific instructions

### Repository layout

- **Rust backend** — `src/`, `Cargo.toml`, MCP tools and agents
- **React config UI** — `frontend/` (Vite + React + TypeScript)
  - Config module: `frontend/src/modules/config-ui/`
  - Build output: `frontend/dist/` (served by the MCP binary)
- **MCP binary** — `cargo run --bin mcp-adjutant` starts stdio MCP + HTTP config UI

### Toolchain (preinstalled)

- Rust: `cargo`, `rustc` (1.83.0), `clippy`, `rustfmt`
- Node.js 20+ for the frontend (install deps with `npm ci` in `frontend/`)

### Standard commands

**Backend**

- Build (dev): `cargo build --bin mcp-adjutant`
- Run MCP + config UI: `cargo run --bin mcp-adjutant`
- Test: `cargo test --all-targets`
- Lint: `cargo clippy --all-targets` and `cargo fmt --check`

**Frontend**

```bash
cd frontend
npm ci
npm run lint
npm run build
npm run test
```

**Full local check (matches CI)**

```bash
bash scripts/download-embedding-fixtures.sh
cd frontend && npm ci && npm run lint && npm run build && cd ..
CXX=g++ cargo fmt -- --check
CXX=g++ cargo clippy --all-targets -- -D warnings
CXX=g++ cargo test --all-targets
```

Native build tools (`build-essential`, `g++`, `fd-find`, `ripgrep`) are required for tests that invoke `fd`/`rg` or link C++ (tree-sitter, tokenizers).

### Agent delegation (Cursor)

This repo runs **hard** adjutant delegation by default — see [`.cursor/skills/mcp-adjutant-delegation/SKILL.md`](.cursor/skills/mcp-adjutant-delegation/SKILL.md). Premium agents must route scouting, triage, test generation, web research, log analysis, and refactors through MCP tools before native Grep/Read/WebSearch/manual builds — **including Plan mode and other planning phases** (`scout_context` / `web_fetch` before drafting plans). `MCP_ADJUTANT_REQUIRE_BUILDER=true` in [`.cursor/mcp.json`](.cursor/mcp.json) — builder required for every changed **logic-bearing source file** (any language; see skill builder ledger), not hand-written tests.

**Log files:** always call `analyze_log` with `log_path` before Grep/Read/fastcontext on log content. `log_path` accepts local files, `https://…` log URLs, and `gh-run:<run_id>` (GitHub Actions failed-job logs via `gh` CLI — use after `gh run view <id> --log-failed` in babysitter/CI loops).

**PR babysitting:** invoke skill [`.cursor/skills/adjutant-babysitter/`](.cursor/skills/adjutant-babysitter/SKILL.md) or MCP tool `babysit_pr` (looped BabysitterAgent, 20 turns). Requires `gh` CLI + checkout on PR head branch.
