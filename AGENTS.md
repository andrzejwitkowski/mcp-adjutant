# mcp-adjutant

An advanced Model Context Protocol (MCP) server intended to act as an operational
adjutant for premium LLMs, offloading codebase scouting, context pruning, test
generation, and compiler triage to cost-effective models.

## Cursor Cloud specific instructions

### Current repository state

- This repository is currently a **stub**: it contains only `README.md`, `LICENSE`,
  and `.gitignore`. There is no `Cargo.toml`, no `src/`, and no application code yet.
- Because there is no crate manifest, there is nothing to build, test, or run today.
  Any "run the app" request cannot be satisfied until Rust source and a `Cargo.toml`
  are added to the repo.

### Toolchain (preinstalled)

- The Rust toolchain is preinstalled and on `PATH`: `cargo`, `rustc` (1.83.0), plus the
  `clippy` and `rustfmt` components. No installation step is needed for these.
- `CARGO_HOME` is `/usr/local/cargo`.

### Standard commands (valid once a `Cargo.toml` exists)

- Build (dev): `cargo build`
- Run (dev): `cargo run`
- Test: `cargo test`
- Lint: `cargo clippy --all-targets` (and `cargo fmt --check` for formatting)

Until a `Cargo.toml` is added, these commands will report that no manifest was found —
that is expected, not an environment problem.
