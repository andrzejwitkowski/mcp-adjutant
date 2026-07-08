//! Tests for the repository documentation added/changed in this PR:
//! - `.cursor/skills/mcp-adjutant-delegation/SKILL.md`
//! - `README.md`
//!
//! These are plain filesystem/string assertions (no markdown/YAML parser is a
//! dependency of this crate), mirroring the style of `tests/config_storage.rs`.
//! They guard against broken links, malformed frontmatter, and the docs
//! drifting out of sync with the actual MCP tool names exported by the crate.

use std::path::PathBuf;

use mcp_adjutant::{
    EVALUATE_AGENT_PERFORMANCE_TOOL_NAME, GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
    QUERY_JOB_STATUS_TOOL_NAME, SCOUT_CONTEXT_TOOL_NAME, VERIFY_AND_TRIAGE_TOOL_NAME,
};

const ALL_TOOL_NAMES: [&str; 5] = [
    SCOUT_CONTEXT_TOOL_NAME,
    VERIFY_AND_TRIAGE_TOOL_NAME,
    GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME,
    EVALUATE_AGENT_PERFORMANCE_TOOL_NAME,
    QUERY_JOB_STATUS_TOOL_NAME,
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn skill_md_path() -> PathBuf {
    repo_root().join(".cursor/skills/mcp-adjutant-delegation/SKILL.md")
}

fn read_skill_md() -> String {
    std::fs::read_to_string(skill_md_path()).expect("read SKILL.md")
}

fn read_readme() -> String {
    std::fs::read_to_string(repo_root().join("README.md")).expect("read README.md")
}

/// Extract the YAML frontmatter block delimited by `---` lines at the top of
/// a markdown file. Returns the raw frontmatter body (without the `---`
/// delimiters) and the remaining body content.
fn split_frontmatter(content: &str) -> (String, String) {
    let mut lines = content.lines();
    let first = lines.next().expect("file has at least one line");
    assert_eq!(
        first.trim(),
        "---",
        "expected file to start with a `---` frontmatter delimiter"
    );

    let mut frontmatter_lines = Vec::new();
    let mut body_lines = Vec::new();
    let mut in_frontmatter = true;

    for line in lines {
        if in_frontmatter && line.trim() == "---" {
            in_frontmatter = false;
            continue;
        }
        if in_frontmatter {
            frontmatter_lines.push(line);
        } else {
            body_lines.push(line);
        }
    }

    assert!(
        !in_frontmatter,
        "frontmatter was never closed with a second `---` delimiter"
    );

    (frontmatter_lines.join("\n"), body_lines.join("\n"))
}

// ---------------------------------------------------------------------------
// SKILL.md
// ---------------------------------------------------------------------------

#[test]
fn skill_md_exists_at_expected_path() {
    let path = skill_md_path();
    assert!(
        path.is_file(),
        "SKILL.md must exist at {}",
        path.display()
    );
}

#[test]
fn skill_md_has_well_formed_frontmatter_block() {
    let content = read_skill_md();
    let (frontmatter, body) = split_frontmatter(&content);

    assert!(
        !frontmatter.trim().is_empty(),
        "frontmatter block must not be empty"
    );
    assert!(
        !body.trim().is_empty(),
        "body content must follow the frontmatter block"
    );
}

#[test]
fn skill_md_frontmatter_declares_required_fields() {
    let content = read_skill_md();
    let (frontmatter, _) = split_frontmatter(&content);

    assert!(
        frontmatter.contains("name: mcp-adjutant-delegation"),
        "frontmatter must declare the skill name"
    );
    assert!(
        frontmatter.contains("description:"),
        "frontmatter must declare a description"
    );
    assert!(
        frontmatter.contains("metadata:"),
        "frontmatter must declare a metadata block"
    );
}

#[test]
fn skill_md_description_mentions_activation_triggers() {
    let content = read_skill_md();
    let (frontmatter, _) = split_frontmatter(&content);

    let description_line = frontmatter
        .lines()
        .find(|line| line.trim_start().starts_with("description:"))
        .expect("description field present");

    for keyword in ["mcp-adjutant", "delegate", "low, medium, or hard"] {
        assert!(
            description_line.contains(keyword),
            "description should mention `{keyword}`, got: {description_line}"
        );
    }
}

#[test]
fn skill_md_metadata_declares_all_three_delegation_levels() {
    let content = read_skill_md();
    let (frontmatter, _) = split_frontmatter(&content);

    assert!(
        frontmatter.contains("delegation-levels: low, medium, hard"),
        "metadata.delegation-levels must list low, medium, hard"
    );
    assert!(
        frontmatter.contains("default-delegation-level: medium"),
        "metadata.default-delegation-level must be medium"
    );
}

#[test]
fn skill_md_documents_every_mcp_tool_by_actual_tool_name() {
    let content = read_skill_md();

    for tool_name in ALL_TOOL_NAMES {
        assert!(
            content.contains(tool_name),
            "SKILL.md must reference tool `{tool_name}`, but it was not found"
        );
    }
}

#[test]
fn skill_md_defines_a_section_for_every_delegation_level() {
    let content = read_skill_md();

    for level_heading in ["## LOW", "## MEDIUM", "## HARD"] {
        assert!(
            content.contains(level_heading),
            "expected a heading starting with `{level_heading}`"
        );
    }
}

#[test]
fn skill_md_describes_the_async_job_polling_protocol() {
    let content = read_skill_md();

    assert!(content.contains("## Async job protocol"));
    assert!(content.contains("request_uuid"));
    assert!(content.contains("terminal=true"));
    assert!(
        content.contains(QUERY_JOB_STATUS_TOOL_NAME),
        "async protocol section should reference query_job_status"
    );
}

#[test]
fn skill_md_json_argument_examples_are_valid_json() {
    let content = read_skill_md();
    let json_blocks = extract_fenced_code_blocks(&content, "json");

    assert!(
        !json_blocks.is_empty(),
        "expected at least one ```json fenced block in SKILL.md"
    );

    for block in json_blocks {
        assert!(
            is_syntactically_plausible_json(&block),
            "expected valid-looking JSON object, got:\n{block}"
        );
    }
}

#[test]
fn skill_md_contains_a_mermaid_decision_flowchart() {
    let content = read_skill_md();
    let mermaid_blocks = extract_fenced_code_blocks(&content, "mermaid");

    assert_eq!(
        mermaid_blocks.len(),
        1,
        "expected exactly one ```mermaid fenced block in SKILL.md"
    );
    let flowchart = &mermaid_blocks[0];
    assert!(flowchart.starts_with("flowchart TD"));
    // Every decision node referenced in edges should also be defined.
    for node in ["A[", "B{", "C[", "D{", "E{", "F[", "G[", "H{", "I[", "K{", "L["] {
        assert!(
            flowchart.contains(node),
            "expected flowchart node `{node}` to be defined"
        );
    }
}

#[test]
fn skill_md_has_balanced_code_fences() {
    let content = read_skill_md();
    let fence_count = content.matches("```").count();
    assert_eq!(
        fence_count % 2,
        0,
        "code fences (```) must be balanced (open/close pairs)"
    );
}

#[test]
fn skill_md_has_balanced_markdown_tables() {
    // Every `| --- | --- |`-style separator row must have the same number of
    // pipe-delimited columns as its header row directly above it.
    let content = read_skill_md();
    let lines: Vec<&str> = content.lines().collect();

    let mut checked_tables = 0;
    for i in 1..lines.len() {
        let line = lines[i].trim();
        if is_table_separator_row(line) {
            let header = lines[i - 1].trim();
            let header_cols = count_pipe_columns(header);
            let sep_cols = count_pipe_columns(line);
            assert_eq!(
                header_cols, sep_cols,
                "table header `{header}` and separator `{line}` must have the same column count"
            );
            checked_tables += 1;
        }
    }

    assert!(
        checked_tables >= 4,
        "expected at least 4 markdown tables in SKILL.md, found {checked_tables}"
    );
}

// ---------------------------------------------------------------------------
// README.md
// ---------------------------------------------------------------------------

#[test]
fn readme_links_to_skill_md_with_a_resolvable_relative_path() {
    let readme = read_readme();
    let link_target = ".cursor/skills/mcp-adjutant-delegation/SKILL.md";

    assert!(
        readme.contains(link_target),
        "README.md must link to `{link_target}`"
    );

    let resolved = repo_root().join(link_target);
    assert!(
        resolved.is_file(),
        "README.md link target `{link_target}` must exist on disk at {}",
        resolved.display()
    );
}

#[test]
fn readme_describes_all_five_mcp_tools_by_actual_tool_name() {
    let readme = read_readme();

    for tool_name in ALL_TOOL_NAMES {
        assert!(
            readme.contains(tool_name),
            "README.md must document tool `{tool_name}`"
        );
    }
}

#[test]
fn readme_mentions_all_three_delegation_levels() {
    let readme = read_readme();

    for level in ["**low**", "**medium**", "**hard**"] {
        assert!(
            readme.contains(level),
            "README.md should describe the `{level}` delegation level"
        );
    }
    assert!(readme.contains("MCP_ADJUTANT_DELEGATION_LEVEL"));
}

#[test]
fn readme_json_snippets_are_syntactically_plausible() {
    let readme = read_readme();
    let json_blocks = extract_fenced_code_blocks(&readme, "json");

    assert!(
        json_blocks.len() >= 3,
        "expected multiple ```json examples in README.md, found {}",
        json_blocks.len()
    );

    for block in json_blocks {
        assert!(
            is_syntactically_plausible_json(&block),
            "expected valid-looking JSON object, got:\n{block}"
        );
    }
}

#[test]
fn readme_links_to_agents_and_license_files_that_exist() {
    let readme = read_readme();

    assert!(readme.contains("[AGENTS.md](AGENTS.md)"));
    assert!(readme.contains("[LICENSE](LICENSE)"));

    assert!(repo_root().join("AGENTS.md").is_file());
    assert!(repo_root().join("LICENSE").is_file());
}

#[test]
fn readme_has_balanced_code_fences() {
    let readme = read_readme();
    let fence_count = readme.matches("```").count();
    assert_eq!(
        fence_count % 2,
        0,
        "code fences (```) in README.md must be balanced (open/close pairs)"
    );
}

#[test]
fn readme_references_env_vars_consistently_with_skill_md() {
    // The env var used to configure delegation level is only introduced in
    // this PR (in SKILL.md); make sure README.md uses the exact same name.
    let readme = read_readme();
    let skill = read_skill_md();

    assert!(skill.contains("MCP_ADJUTANT_DELEGATION_LEVEL"));
    assert!(readme.contains("MCP_ADJUTANT_DELEGATION_LEVEL"));
}

#[test]
fn readme_documents_five_tools_table_matching_skill_md_table() {
    let readme = read_readme();
    let skill = read_skill_md();

    // Both docs should present a "Tool" column table; verify the same set of
    // backtick-quoted tool identifiers appears in each.
    for tool_name in ALL_TOOL_NAMES {
        let backticked = format!("`{tool_name}`");
        assert!(
            readme.contains(&backticked),
            "README.md should reference {backticked}"
        );
        assert!(
            skill.contains(&backticked),
            "SKILL.md should reference {backticked}"
        );
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Extract the contents of every fenced code block tagged with `lang`
/// (e.g. ```json ... ```).
fn extract_fenced_code_blocks(content: &str, lang: &str) -> Vec<String> {
    let fence_open = format!("```{lang}");
    let mut blocks = Vec::new();
    let mut remaining = content;

    while let Some(start) = remaining.find(&fence_open) {
        let after_open = &remaining[start + fence_open.len()..];
        let end = after_open
            .find("```")
            .expect("fenced code block must be closed");
        blocks.push(after_open[..end].trim().to_string());
        remaining = &after_open[end + 3..];
    }

    blocks
}

/// A lightweight structural check for JSON-looking text without pulling in a
/// JSON parsing dependency: verifies balanced braces/brackets and that the
/// block starts/ends with an object or array delimiter.
fn is_syntactically_plausible_json(block: &str) -> bool {
    let trimmed = block.trim();
    let starts_ok = trimmed.starts_with('{') || trimmed.starts_with('[');
    let ends_ok = trimmed.ends_with('}') || trimmed.ends_with(']');

    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for ch in trimmed.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return false;
        }
    }

    starts_ok && ends_ok && depth == 0 && !in_string
}

fn count_pipe_columns(line: &str) -> usize {
    line.trim().trim_matches('|').split('|').count()
}

fn is_table_separator_row(line: &str) -> bool {
    if !line.starts_with('|') {
        return false;
    }
    line.chars()
        .all(|c| c == '|' || c == '-' || c == ' ' || c == ':')
        && line.contains('-')
}

// ---------------------------------------------------------------------------
// Regression tests for the helper functions themselves
// ---------------------------------------------------------------------------

#[test]
fn helper_extract_fenced_code_blocks_returns_each_block_verbatim() {
    let sample = "before\n```json\n{\"a\": 1}\n```\nmiddle\n```json\n[1, 2, 3]\n```\nafter";
    let blocks = extract_fenced_code_blocks(sample, "json");
    assert_eq!(blocks, vec!["{\"a\": 1}".to_string(), "[1, 2, 3]".to_string()]);
}

#[test]
fn helper_is_syntactically_plausible_json_rejects_unbalanced_braces() {
    assert!(!is_syntactically_plausible_json("{ \"a\": 1"));
    assert!(!is_syntactically_plausible_json("\"a\": 1 }"));
    assert!(is_syntactically_plausible_json("{ \"a\": [1, 2, {\"b\": \"}\"}] }"));
}

#[test]
fn helper_split_frontmatter_separates_header_from_body() {
    let sample = "---\nname: x\n---\n\n# Heading\nbody";
    let (frontmatter, body) = split_frontmatter(sample);
    assert_eq!(frontmatter, "name: x");
    assert!(body.contains("# Heading"));
}

#[test]
#[should_panic(expected = "frontmatter was never closed")]
fn helper_split_frontmatter_panics_when_unclosed() {
    let sample = "---\nname: x\nno closing delimiter";
    split_frontmatter(sample);
}